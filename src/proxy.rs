// SPDX-License-Identifier: Apache-2.0
//! Transparent reverse proxies in front of your model servers, one per
//! watched host.
//!
//! Each host gets its own local port. Clients talk to that port; Toktail
//! forwards every request to that host's server unchanged and, for inference
//! endpoints, reads the token usage on its way back. The CSV gets counts,
//! timings, the model name, the host, a one-way id of the credential, and a
//! SHA-256 fingerprint of the prompt, never the prompt itself. Only when
//! "Save prompts and responses" is on are the bodies kept, in separate gzip
//! files (encrypted when encryption is on; see records.rs).
//!
//! Hosts come in two kinds. An **LM Studio** host is a local server whose
//! API token Toktail may hold and add for you. A **public API** host
//! (Anthropic, OpenAI, another provider) is reached with the agent's own key,
//! which passes through untouched, or with a Toktail gateway key that stands
//! in for a provider key kept in the Keychain (see tokens.rs). Either way the
//! key exists only for the life of the request.
//!
//! With the prompt cache on (cache.rs), an identical request is answered
//! from disk and never reaches the server.

use crate::app::App;
use crate::config::HostConfig;
use crate::ledger::{Meta, Record};
use crate::tokens::{self, Credential};
use crate::usage::{self, classify, EndpointKind, SseTracker, Usage};
use crate::{cache, commentary};
use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bytes::Bytes;
use chrono::Local;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const MAX_REQUEST_BYTES: usize = 512 * 1024 * 1024;
/// Responses bigger than this aren't kept in memory for saving.
const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

struct Ctx {
    app: Arc<App>,
    client: reqwest::Client,
    host: String,
}

/// What we know about a counted request before the server answers.
struct Tracked {
    endpoint: String,
    request_model: Option<String>,
    fingerprint: String,
    streamed: bool,
    strip_injected_usage: bool,
    started: Instant,
    /// The request exactly as the client sent it, when saving is on.
    original: Option<Bytes>,
    /// Who sent it, what it's about, and the newest turn (for Pip).
    detected: commentary::Detected,
    turn: String,
    /// What's new in the prompt, for Pip's summary (summaries.rs).
    new_part: String,
    token_id: String,
    client: String,
    /// Set when the answer should be stored in the prompt cache.
    cache_key: Option<String>,
    /// The host it runs on (changes when you reroute it).
    host: String,
    /// What happened to it in Toktail, for the live feed.
    note: String,
}

/// How the credential is forwarded.
#[derive(Clone)]
enum Forward {
    /// Whatever the client sent goes through unchanged.
    KeepClient,
    /// The client's credential (if any) is dropped; this one is sent instead.
    Replace { header: &'static str, value: String },
    /// The client's credential is dropped and nothing is sent (a reroute to a
    /// host with no stored token: the agent's key never follows a request).
    Strip,
}

/// Where a host's requests go: its server address, normalized.
fn upstream_of(app: &App, host: &str) -> Option<String> {
    let c = app.config.read();
    let raw = c.host(host)?.upstream.clone();
    Some(normalize_upstream(&raw).unwrap_or(raw))
}

/// The address to reach a proxy listening on `listen` from this machine.
pub fn local_url(listen: &str) -> String {
    let port = listen.rsplit(':').next().unwrap_or("1234");
    format!("http://127.0.0.1:{port}")
}

/// Accepts "studio.local:1234", "http://10.0.0.5:1234/v1/", "https://lm.example.com"
/// and returns "scheme://host[:port][/path]" without a trailing slash or /v1.
pub fn normalize_upstream(raw: &str) -> Result<String, String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return Err("Enter the server's address, like http://127.0.0.1:1235".into());
    }
    if !s.contains("://") {
        s = format!("http://{s}");
    }
    let url = reqwest::Url::parse(&s).map_err(|e| format!("That isn't a valid address: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("The address must start with http:// or https://".into());
    }
    let host = url
        .host_str()
        .ok_or("The address needs a host name or IP")?;
    let mut out = format!(
        "{}://{}",
        url.scheme(),
        if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        }
    );
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/v1").unwrap_or(path);
    out.push_str(path);
    Ok(out)
}

/// True when `upstream` points back at Toktail's own proxy port on this machine.
pub fn loops_back(upstream: &str, listen_port: u16) -> bool {
    let Ok(url) = reqwest::Url::parse(upstream) else {
        return false;
    };
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1")
    );
    local && url.port_or_known_default() == Some(listen_port)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("http client")
}

/// Guards that keep a request counted as in flight until it ends.
struct InFlight {
    _all: crate::live::ActiveGuard,
    host: crate::live::ActiveGuard,
    flight: crate::live::FlightGuard,
}

struct Running {
    listen: String,
    stop: Arc<Notify>,
    done: tokio::task::JoinHandle<()>,
}

/// What a host kind is called in messages.
pub fn kind_name(h: &HostConfig) -> &'static str {
    match h.kind.as_str() {
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "other" => "the API",
        _ => "LM Studio",
    }
}

/// Keeps one listener per enabled host, matching the host list whenever
/// Settings change (`proxy_restart`).
pub async fn run(app: Arc<App>) {
    let client = client();
    tokio::spawn(health_loop(app.clone(), client.clone()));
    let mut running: HashMap<String, Running> = HashMap::new();
    loop {
        let hosts: Vec<HostConfig> = app.config.read().hosts.clone();
        let wanted: HashMap<String, HostConfig> = hosts
            .into_iter()
            .filter(|h| h.enabled)
            .map(|h| (h.id.clone(), h))
            .collect();
        // Stop listeners for removed, disabled or moved hosts, and dead ones.
        let ids: Vec<String> = running.keys().cloned().collect();
        for id in ids {
            let keep = wanted
                .get(&id)
                .is_some_and(|h| h.listen == running[&id].listen)
                && !running[&id].done.is_finished();
            if !keep {
                let r = running.remove(&id).unwrap();
                r.stop.notify_one();
                let _ = tokio::time::timeout(Duration::from_secs(5), r.done).await;
                if !wanted.contains_key(&id) {
                    app.status.write().hosts.remove(&id);
                }
            }
        }
        {
            let mut st = app.status.write();
            st.hosts.retain(|id, _| wanted.contains_key(id));
        }
        for (id, h) in &wanted {
            if running.contains_key(id) {
                continue;
            }
            match tokio::net::TcpListener::bind(&h.listen).await {
                Ok(listener) => {
                    let up = upstream_of(&app, id).unwrap_or_default();
                    log::info!("[{id}] proxy listening on {}, forwarding to {up}", h.listen);
                    if h.is_public() && h.allows_lan() {
                        log::info!("[{id}] a public API host on the network: only gateway keys and the agents' own keys are accepted from other machines");
                    }
                    app.set_proxy_status(id, true, format!("Counting on {} → {up}", h.listen));
                    let ctx = Arc::new(Ctx {
                        app: app.clone(),
                        client: client.clone(),
                        host: id.clone(),
                    });
                    let router = Router::new().fallback(handle).with_state(ctx);
                    let stop = Arc::new(Notify::new());
                    let s2 = stop.clone();
                    let a2 = app.clone();
                    let id2 = id.clone();
                    let done = tokio::spawn(async move {
                        let served = axum::serve(
                            listener,
                            router.into_make_service_with_connect_info::<SocketAddr>(),
                        )
                        .with_graceful_shutdown(async move { s2.notified().await })
                        .await;
                        if let Err(e) = served {
                            log::error!("[{id2}] proxy stopped: {e}");
                            a2.set_proxy_status(&id2, false, format!("Proxy stopped: {e}"));
                        }
                    });
                    running.insert(
                        id.clone(),
                        Running {
                            listen: h.listen.clone(),
                            stop,
                            done,
                        },
                    );
                }
                Err(e) => {
                    let msg = format!(
                        "Can't listen on {}: {e}. Is another app using that port?",
                        h.listen
                    );
                    let changed = app
                        .status
                        .read()
                        .hosts
                        .get(id)
                        .map(|s| s.proxy_msg != msg)
                        .unwrap_or(true);
                    if changed {
                        log::warn!("[{id}] {msg}");
                    }
                    app.set_proxy_status(id, false, msg);
                }
            }
        }
        // Retry failed ports every 10 s, or at once after a Settings change.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
            _ = app.proxy_restart.notified() => {}
        }
    }
}

/// Result of asking a server for its models.
pub struct Probe {
    pub ok: bool,
    pub message: String,
    pub models: Vec<String>,
}

/// Asks `upstream` for `/v1/models`, with the token if given. For a public
/// API a 401 or 403 means the server is there and the agents' own keys will
/// be used, so it counts as reachable.
pub async fn probe(
    client: &reqwest::Client,
    upstream: &str,
    token: Option<&str>,
    kind: &str,
) -> Probe {
    let public = kind != "lmstudio";
    let mut req = client
        .get(format!("{upstream}/v1/models"))
        .timeout(Duration::from_secs(6));
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = if kind == "anthropic" {
            req.header("x-api-key", t)
        } else {
            req.bearer_auth(t)
        };
    }
    if kind == "anthropic" {
        req = req.header("anthropic-version", "2023-06-01");
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let models: Vec<String> = r
                .json_value()
                .await
                .ok()
                .and_then(|v| v.get("data").and_then(Value::as_array).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            Probe {
                ok: true,
                message: format!(
                    "{} answered with {} model{}",
                    if public { "The API" } else { "LM Studio" },
                    models.len(),
                    if models.len() == 1 { "" } else { "s" }
                ),
                models,
            }
        }
        Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => Probe {
            ok: public,
            message: if public {
                if token.is_some_and(|t| !t.is_empty()) {
                    "Reachable, but the stored key was refused".into()
                } else {
                    "Reachable. The agents' own keys (or a gateway key) are used".into()
                }
            } else if token.is_some_and(|t| !t.is_empty()) {
                "LM Studio refused the API token".into()
            } else {
                "LM Studio wants an API token".into()
            },
            models: vec![],
        },
        Ok(r) => Probe {
            ok: false,
            message: format!("The server answered {}", r.status()),
            models: vec![],
        },
        Err(e) => Probe {
            ok: false,
            message: if e.is_timeout() {
                format!("No answer from {upstream}")
            } else {
                format!("Can't reach {upstream}")
            },
            models: vec![],
        },
    }
}

/// Checks every host's server every 30 s (or right after Settings change).
async fn health_loop(app: Arc<App>, client: reqwest::Client) {
    loop {
        let hosts: Vec<HostConfig> = app
            .config
            .read()
            .hosts
            .iter()
            .filter(|h| h.enabled)
            .cloned()
            .collect();
        let checks = hosts.into_iter().map(|h| {
            let app = app.clone();
            let client = client.clone();
            async move {
                let up = normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone());
                let token = app.host_token(&h.id);
                let p = probe(&client, &up, token.as_deref(), &h.kind).await;
                let mut s = app.status.write();
                let hs = s.hosts.entry(h.id.clone()).or_default();
                if hs.upstream.ok != p.ok || !hs.upstream.checked {
                    log::info!("[{}] {} at {up}: {}", h.id, kind_name(&h), p.message);
                }
                hs.upstream = crate::app::UpstreamStatus {
                    checked: true,
                    ok: p.ok,
                    message: p.message,
                    models: p.models,
                    checked_at: Some(Local::now()),
                };
            }
        });
        futures_util::future::join_all(checks).await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            _ = app.check_upstream.notified() => {}
        }
    }
}

trait JsonValue {
    async fn json_value(self) -> Result<Value, reqwest::Error>;
}
impl JsonValue for reqwest::Response {
    async fn json_value(self) -> Result<Value, reqwest::Error> {
        let b = self.bytes().await?;
        Ok(serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// Headers Toktail reads and never forwards.
fn is_toktail_header(name: &HeaderName) -> bool {
    name.as_str().starts_with("x-toktail-")
}

/// Headers that carry a credential.
fn is_credential_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization" | "x-api-key" | "x-goog-api-key"
    )
}

fn forward_request_headers(src: &HeaderMap, tracked: bool, auth: &Forward) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in src {
        if is_hop_by_hop(k)
            || k == header::HOST
            || k == header::CONTENT_LENGTH
            || is_toktail_header(k)
        {
            continue;
        }
        // With a replacement credential, whatever the client sent is dropped
        // (OpenAI SDKs insist on sending one, often a placeholder).
        if matches!(auth, Forward::Replace { .. } | Forward::Strip) && is_credential_header(k) {
            continue;
        }
        // Ask for an uncompressed body on counted endpoints so it can be read.
        if tracked && k == header::ACCEPT_ENCODING {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    if let Forward::Replace { header: h, value } = auth {
        let v = if *h == "authorization" {
            format!("Bearer {value}")
        } else {
            value.clone()
        };
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(h.as_bytes()),
            v.parse::<HeaderValue>(),
        ) {
            out.insert(name, val);
        }
    }
    out
}

fn forward_response_headers(src: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in src {
        if is_hop_by_hop(k) || k == header::CONTENT_LENGTH {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

fn bad_gateway(what: &str, upstream: &str, e: impl std::fmt::Display) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": format!("Toktail could not reach {what} at {upstream}: {e}"),
            "type": "toktail_upstream_unreachable"
        }
    });
    (StatusCode::BAD_GATEWAY, axum::Json(body)).into_response()
}

fn refuse(status: StatusCode, kind: &str, message: &str) -> Response {
    let body = serde_json::json!({ "error": { "message": message, "type": kind } });
    let mut r = (status, axum::Json(body)).into_response();
    r.headers_mut()
        .insert("x-toktail-cache", HeaderValue::from_static("off"));
    r
}

/// Decides what credential goes out, and what the CSV calls it. Returns the
/// forwarding plan and the token id, or a response that ends the request.
fn authorize(
    ctx: &Ctx,
    h: &HostConfig,
    headers: &HeaderMap,
    peer: SocketAddr,
) -> Result<(Forward, String), Box<Response>> {
    let cred = tokens::client_credential(headers);
    let stored = ctx.app.host_token(&ctx.host);
    let loopback = peer.ip().is_loopback();
    if !h.is_public() {
        // LM Studio: a stored token replaces whatever the client sent.
        let id = cred.as_ref().map(Credential::id).unwrap_or_default();
        return Ok(match stored {
            Some(t) => (
                Forward::Replace {
                    header: "authorization",
                    value: t,
                },
                id,
            ),
            None => (Forward::KeepClient, id),
        });
    }
    match cred {
        Some(c) if c.is_gateway() => {
            let Some(key) = tokens::match_gateway_key(h, &c) else {
                return Err(Box::new(refuse(
                    StatusCode::UNAUTHORIZED,
                    "toktail_unknown_gateway_key",
                    "This Toktail gateway key isn't valid for this host. Issue one in Settings → Hosts.",
                )));
            };
            if !loopback && !key.lan {
                return Err(Box::new(refuse(
                    StatusCode::FORBIDDEN,
                    "toktail_gateway_key_local_only",
                    "This gateway key only works from the computer Toktail runs on.",
                )));
            }
            let Some(t) = stored else {
                return Err(Box::new(refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "toktail_no_provider_key",
                    &format!(
                        "No {} key is stored for this host. Add it in Settings → Hosts.",
                        kind_name(h)
                    ),
                )));
            };
            Ok((
                Forward::Replace {
                    header: h.key_header(),
                    value: t,
                },
                key.id.clone(),
            ))
        }
        Some(c) => {
            if !loopback {
                note_lan_key(ctx, h, peer);
            }
            Ok((Forward::KeepClient, c.id()))
        }
        None => {
            if !loopback {
                return Err(Box::new(refuse(
                    StatusCode::UNAUTHORIZED,
                    "toktail_gateway_key_required",
                    "Requests from other machines need a Toktail gateway key or the provider's own key. Toktail never adds its stored key for them.",
                )));
            }
            match stored.filter(|_| h.inject_key) {
                Some(t) => Ok((
                    Forward::Replace {
                        header: h.key_header(),
                        value: t,
                    },
                    "stored".into(),
                )),
                None => Ok((Forward::KeepClient, String::new())),
            }
        }
    }
}

/// A provider key arrived from another machine over plain HTTP. Said once
/// per host in the log and on the host's status line.
fn note_lan_key(ctx: &Ctx, h: &HostConfig, peer: SocketAddr) {
    let mut s = ctx.app.status.write();
    let hs = s.hosts.entry(h.id.clone()).or_default();
    if hs.lan_key_from.is_none() {
        log::warn!(
            "[{}] an API key for {} arrived from {} over plain HTTP. Hand that machine a gateway key instead (Settings → Hosts).",
            h.id,
            kind_name(h),
            peer.ip()
        );
    }
    hs.lan_key_from = Some(peer.ip().to_string());
}

async fn handle(
    State(ctx): State<Arc<Ctx>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let started = Instant::now();
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let path_q = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| path.clone());
    let kind = if parts.method == Method::POST {
        classify(&path)
    } else {
        None
    };

    let mut body_bytes = match to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, format!("request body: {e}")).into_response()
        }
    };

    let Some(h) = ctx.app.config.read().host(&ctx.host).cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "This host was removed from Toktail.",
        )
            .into_response();
    };
    let up = normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone());
    if let Some(port) = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.rsplit(':').next())
        .and_then(|p| p.parse::<u16>().ok())
    {
        if loops_back(&up, port) {
            let body = serde_json::json!({"error": {"message": "Toktail's upstream for this host points back at Toktail itself. Change the server address in Settings.", "type": "toktail_loop"}});
            return (StatusCode::LOOP_DETECTED, axum::Json(body)).into_response();
        }
    }
    if ctx
        .app
        .shutting_down
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return (StatusCode::SERVICE_UNAVAILABLE, "Toktail is shutting down").into_response();
    }
    let (auth, token_id) = match authorize(&ctx, &h, &parts.headers, peer) {
        Ok(x) => x,
        Err(r) => return *r,
    };
    let toktail_client = parts
        .headers
        .get("x-toktail-client")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(24)
        .collect::<String>();

    // The prompt cache: an identical request from the same credential.
    let cache_on = ctx.app.config.read().cache.enabled;
    let cache_mode = cache::mode_from_headers(&parts.headers);
    let cache_key = if cache_on
        && kind.is_some()
        && toktail_client != commentary::CLIENT
        && toktail_client != crate::summaries::CLIENT
    {
        Some(cache::key(&ctx.host, &path, &token_id, &body_bytes))
    } else {
        None
    };
    if let (Some(key), cache::Mode::Use) = (&cache_key, cache_mode) {
        let a = ctx.app.clone();
        let k = key.clone();
        if let Ok(Some(entry)) = tokio::task::spawn_blocking(move || cache::get(&a, &k)).await {
            return serve_cached(
                &ctx,
                &h,
                entry,
                &path,
                &parts.headers,
                &body_bytes,
                &token_id,
                &toktail_client,
                started,
            );
        }
        cache::note_miss(&ctx.app);
    }

    let saving = ctx.app.config.read().capture.enabled;
    // Identical requests (an agent's retry, say) share this key, so one can
    // answer the other. Computed from the body exactly as the agent sent it.
    // An agent resending the same input (a retry after a timeout) shares this
    // key with the request still running, so one answer serves both.
    let flight_key = (kind.is_some() && toktail_client != commentary::CLIENT)
        .then(|| crate::flights::same_input_key(&ctx.host, &path, &token_id, &body_bytes))
        .flatten();
    let prompt_bytes = body_bytes.len() as u64;
    let tracked = kind.and_then(|kind| {
        prepare(
            kind,
            &path,
            &parts.headers,
            &mut body_bytes,
            started,
            saving,
            token_id.clone(),
            toktail_client.clone(),
            match cache_mode {
                cache::Mode::Bypass => None,
                _ => cache_key.clone(),
            },
        )
    });
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);

    let Some(mut t) = tracked else {
        // Not an inference call: stream it straight through.
        let resp = ctx
            .client
            .request(method, format!("{up}{path_q}"))
            .headers(forward_request_headers(&parts.headers, false, &auth))
            .body(body_bytes)
            .send()
            .await;
        return match resp {
            Ok(resp) => {
                let status =
                    StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
                let headers = forward_response_headers(resp.headers());
                let mut r = Response::new(Body::from_stream(resp.bytes_stream()));
                *r.status_mut() = status;
                *r.headers_mut() = headers;
                r
            }
            Err(e) => bad_gateway(kind_name(&h), &up, e),
        };
    };
    t.host = ctx.host.clone();

    // Counts this request as in flight (for Pip and the live feed) until the
    // guards are dropped, however the request ends.
    let guards = InFlight {
        _all: ctx.app.live.begin(),
        host: ctx.app.live_for(&ctx.host).begin(),
        flight: ctx.app.activity.begin(
            &ctx.host,
            t.request_model.as_deref(),
            &t.endpoint,
            t.streamed,
        ),
    };
    guards.flight.describe(
        flight_key.clone(),
        &t.detected.agent,
        &t.detected.session,
        &t.detected.project,
        prompt_bytes,
    );
    let cache_header = match (cache_on, cache_mode) {
        (false, _) => "off",
        (true, cache::Mode::Bypass) => "bypass",
        (true, cache::Mode::Refresh) => "refresh",
        (true, cache::Mode::Use) => "miss",
    };
    let grace = ctx.app.config.read().flights.abandoned_grace_secs;
    // The request runs in its own task, so it can be steered from the portal
    // and can outlive an agent that hangs up (a retry may still want it).
    let (reply, mut answer) = tokio::sync::mpsc::channel::<Response>(1);
    let run = Run {
        ctx: ctx.clone(),
        t,
        out: Outgoing {
            method,
            path_q,
            headers: parts.headers.clone(),
            body: body_bytes,
        },
        to: Target {
            host: ctx.host.clone(),
            upstream: up.clone(),
            forward: auth,
            model: None,
        },
        guards,
        reply: Some(reply),
        cache_header,
        key: flight_key,
        gone_at: None,
        grace,
        settled: false,
        rerouted_from: None,
    };
    tokio::spawn(fly(run));
    match answer.recv().await {
        Some(r) => r,
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Toktail lost track of this request.",
        )
            .into_response(),
    }
}

// ------------------------------------------------------------------ steering

/// What's needed to send (or resend) a request.
struct Outgoing {
    method: reqwest::Method,
    path_q: String,
    headers: HeaderMap,
    /// The body after `prepare` (with usage asked for in streams).
    body: Bytes,
}

/// Where a request goes and how it's authorized there.
#[derive(Clone)]
struct Target {
    host: String,
    upstream: String,
    forward: Forward,
    /// Replaces the request's model (after a reroute).
    model: Option<String>,
}

type BodyTx = tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>;

/// One counted request, from the moment it arrives until its row is written.
struct Run {
    ctx: Arc<Ctx>,
    t: Tracked,
    out: Outgoing,
    to: Target,
    guards: InFlight,
    /// Where the response goes, until it has been sent.
    reply: Option<tokio::sync::mpsc::Sender<Response>>,
    cache_header: &'static str,
    key: Option<String>,
    /// When the agent hung up.
    gone_at: Option<Instant>,
    /// How long to keep going for a retry after the agent hangs up.
    grace: u64,
    /// Its row is written (it was answered by another request).
    settled: bool,
    /// The host it was sent to before you rerouted it.
    rerouted_from: Option<String>,
}

enum Steer<T> {
    Got(T),
    /// Rerouted: send again, to the new target.
    Resend,
    /// Finished (cancelled, joined, stopped): nothing left to do.
    Done,
}

/// A host to reroute to, authorized with its own stored key, never the agent's.
fn retarget(app: &App, host: &str, model: &str) -> Result<Target, String> {
    let h = app
        .config
        .read()
        .host(host)
        .filter(|h| h.enabled)
        .cloned()
        .ok_or_else(|| format!("{host} isn't an enabled host."))?;
    let forward = match app.host_token(&h.id) {
        Some(t) => Forward::Replace {
            header: h.key_header(),
            value: t,
        },
        None if h.is_public() => {
            return Err(format!(
                "No {} key is stored for {}.",
                kind_name(&h),
                h.name
            ))
        }
        None => Forward::Strip,
    };
    Ok(Target {
        upstream: normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone()),
        host: h.id,
        forward,
        model: Some(model.to_string()).filter(|m| !m.is_empty()),
    })
}

/// The body with its model swapped (other fields untouched).
fn with_model(body: &Bytes, model: &str) -> Bytes {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(mut m)) => {
            m.insert("model".into(), Value::String(model.to_string()));
            Bytes::from(serde_json::to_vec(&Value::Object(m)).unwrap_or_default())
        }
        _ => body.clone(),
    }
}

/// An error the agent can read, in the shape of the API it called.
fn error_event(endpoint: &str, kind: &str, message: &str) -> Bytes {
    let s = if endpoint.contains("/messages") {
        format!(
            "event: error\ndata: {}\n\n",
            serde_json::json!({"type": "error", "error": {"type": kind, "message": message}})
        )
    } else {
        format!(
            "data: {}\n\n",
            serde_json::json!({"error": {"type": kind, "message": message, "code": 499}})
        )
    };
    Bytes::from(s)
}

fn cancelled_response(message: &str) -> Response {
    let body = serde_json::json!({"error": {"message": message, "type": "toktail_cancelled", "code": 499}});
    let mut r = (
        StatusCode::from_u16(499).unwrap_or(StatusCode::BAD_REQUEST),
        axum::Json(body),
    )
        .into_response();
    r.headers_mut()
        .insert("x-toktail-cancelled", HeaderValue::from_static("1"));
    r
}

fn host_name(app: &App, id: &str) -> String {
    app.config
        .read()
        .host(id)
        .map(|h| h.name.clone())
        .unwrap_or_else(|| id.to_string())
}

impl Run {
    fn request(&self) -> reqwest::RequestBuilder {
        let body = match &self.to.model {
            Some(m) => with_model(&self.out.body, m),
            None => self.out.body.clone(),
        };
        self.ctx
            .client
            .request(
                self.out.method.clone(),
                format!("{}{}", self.to.upstream, self.out.path_q),
            )
            .headers(forward_request_headers(
                &self.out.headers,
                true,
                &self.to.forward,
            ))
            .body(body)
    }

    fn tracker(&self) -> SseTracker {
        // Saving keeps the whole answer; otherwise only enough of it to tell
        // a refusal from an answer (the character count stays exact).
        let cap = if self.t.original.is_some() {
            MAX_CAPTURE_BYTES / 2
        } else {
            16 * 1024
        };
        SseTracker::new(self.t.strip_injected_usage).collect_text(cap)
    }

    fn respond(&mut self, r: Response) {
        if let Some(tx) = self.reply.take() {
            let _ = tx.try_send(r);
        }
    }

    fn reroute(&mut self, to: Target) {
        let from = host_name(&self.ctx.app, &self.t.host);
        self.guards
            .flight
            .rerouted(&from, &to.host, to.model.as_deref());
        self.guards.host = self.ctx.app.live_for(&to.host).begin();
        if let Some(m) = &to.model {
            self.t.request_model = Some(m.clone());
        }
        self.t.note = format!("rerouted from {from}");
        self.rerouted_from
            .get_or_insert_with(|| self.t.host.clone());
        // Not cached: the cache key names the host the agent asked for.
        self.t.cache_key = None;
        self.t.host = to.host.clone();
        log::info!(
            "flight #{}: rerouted from {from} to {}{}",
            self.guards.flight.id(),
            self.t.host,
            to.model
                .as_deref()
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        );
        self.to = to;
    }

    fn agent_hung_up(&mut self) {
        if self.gone_at.is_none() {
            self.gone_at = Some(Instant::now());
            self.guards.flight.agent_gone(self.grace);
        }
    }

    /// The agent left long enough ago, and no retry joined.
    fn abandoned(&self) -> bool {
        self.gone_at
            .is_some_and(|g| g.elapsed().as_secs() >= self.grace)
            && self.guards.flight.joiners() == 0
    }

    /// Ends a request that never reached the agent's answer: a row, the
    /// feed, and its joiners told.
    fn end_unanswered(mut self, status: u16, note: &str, reply: Response) {
        self.respond(reply);
        self.t.note = note.to_string();
        let duration = self.t.started.elapsed();
        record(
            &self.ctx,
            self.t,
            None,
            status,
            Usage::default(),
            0,
            None,
            duration,
            None,
        );
        self.guards
            .flight
            .control()
            .tee
            .finish(crate::flights::Summary::default());
    }

    /// Waits for `fut` while listening to the portal and to the agent.
    async fn steer<T>(
        &mut self,
        cmds: &mut tokio::sync::watch::Receiver<crate::flights::Cmd>,
        fut: impl std::future::Future<Output = T>,
    ) -> Steer<T> {
        use crate::flights::Cmd;
        tokio::pin!(fut);
        let watch = self.reply.clone();
        let gone = async move {
            match watch {
                Some(t) => t.closed().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(gone);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                v = &mut fut => return Steer::Got(v),
                ch = cmds.changed() => {
                    if ch.is_err() {
                        // The control outlives the flight; this can't happen.
                        continue;
                    }
                    let cmd = cmds.borrow_and_update().clone();
                    match cmd {
                        Cmd::Cancel => return Steer::Done,
                        Cmd::Reroute { host, model } => match retarget(&self.ctx.app, &host, &model) {
                            Ok(to) => { self.reroute(to); return Steer::Resend; }
                            Err(e) => log::warn!("flight #{}: can't reroute: {e}", self.guards.flight.id()),
                        },
                        Cmd::Join(id) => {
                            if let Some(leader) = self.ctx.app.activity.control_of(id) {
                                match self.wait_for_join(id, leader, cmds).await {
                                    JoinEnd::Done => return Steer::Done,
                                    JoinEnd::Rerouted => return Steer::Resend,
                                    JoinEnd::Fallback => {}
                                }
                            }
                        }
                        Cmd::None => {}
                    }
                }
                _ = &mut gone, if self.gone_at.is_none() => self.agent_hung_up(),
                _ = tick.tick(), if self.gone_at.is_some() => {
                    if self.abandoned() {
                        return Steer::Done;
                    }
                }
            }
        }
    }
}

enum JoinEnd {
    /// Answered with the other request's answer.
    Done,
    /// The other request failed before answering: carry on alone.
    Fallback,
    /// Rerouted while waiting.
    Rerouted,
}

/// Runs one counted request to the end.
async fn fly(mut r: Run) {
    let mut cmds = r.guards.flight.control().commands();
    // An identical request already running answers this one.
    let join_on = r.ctx.app.config.read().flights.join_duplicates;
    if let (true, Some(key)) = (join_on, r.key.clone()) {
        if let Some((id, leader)) = r.ctx.app.activity.leader_for(&key, r.guards.flight.id()) {
            match r.wait_for_join(id, leader, &mut cmds).await {
                JoinEnd::Done => return,
                JoinEnd::Fallback | JoinEnd::Rerouted => {}
            }
        }
    }
    loop {
        // ---- the answer's head
        let send = r.request().send();
        let resp = match r.steer(&mut cmds, send).await {
            Steer::Got(v) => v,
            Steer::Resend => continue,
            Steer::Done => return finish_steered(r, &cmds),
        };
        let resp = match resp {
            Ok(resp) => resp,
            Err(e) => {
                let to = r.to.upstream.clone();
                let what = r
                    .ctx
                    .app
                    .config
                    .read()
                    .host(&r.t.host)
                    .map(kind_name)
                    .unwrap_or("the server");
                let reply = bad_gateway(what, &to, &e);
                return r.end_unanswered(502, "", reply);
            }
        };
        let status =
            StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let mut headers = forward_response_headers(resp.headers());
        headers.insert("x-toktail-cache", HeaderValue::from_static(r.cache_header));
        // Tells the agent's side where the answer really came from.
        if let Some(from) = &r.rerouted_from {
            if let Ok(v) = HeaderValue::from_str(&format!("{from} -> {}", r.t.host)) {
                headers.insert("x-toktail-rerouted", v);
            }
        }
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        if content_type.contains("text/event-stream") {
            return stream(r, cmds, status, headers, resp).await;
        }
        // ---- a whole JSON answer (or an error body)
        let bytes = match r.steer(&mut cmds, resp.bytes()).await {
            Steer::Got(Ok(b)) => b,
            Steer::Got(Err(e)) => {
                let to = r.to.upstream.clone();
                let reply = bad_gateway("the server", &to, &e);
                return r.end_unanswered(502, "", reply);
            }
            Steer::Resend => continue,
            Steer::Done => return finish_steered(r, &cmds),
        };
        let mut u = Usage::default();
        let mut model = None;
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            usage::scan_usage(&v, &mut u);
            model = usage::response_model(&v);
        }
        if let Some(m) = model.as_deref() {
            r.guards.flight.set_model(m);
        }
        let control = r.guards.flight.control();
        control.tee.set_head(status.as_u16(), &content_type);
        control.tee.push(&bytes);
        let mut reply = Response::new(Body::from(bytes.clone()));
        *reply.status_mut() = status;
        *reply.headers_mut() = headers;
        r.respond(reply);
        let gone = r.gone_at.is_some() && r.guards.flight.joiners() == 0;
        let duration = r.t.started.elapsed();
        let kept = bytes.clone();
        let rec = record_and_cache(
            &r.ctx,
            r.t,
            model,
            if gone { 499 } else { status.as_u16() },
            u,
            0,
            None,
            duration,
            Some(bytes.clone()),
            CacheBody {
                content_type,
                streamed: false,
                bytes,
            },
        );
        control.tee.finish(summary_of(
            rec.as_ref(),
            status.as_u16() == 200,
            Some(&kept),
        ));
        return;
    }
}

/// How a request the portal (or a hung-up agent) stopped ends.
fn finish_steered(r: Run, cmds: &tokio::sync::watch::Receiver<crate::flights::Cmd>) {
    let id = r.guards.flight.id();
    if r.settled {
        return;
    }
    if matches!(*cmds.borrow(), crate::flights::Cmd::Cancel) {
        log::info!("flight #{id}: cancelled from the portal");
        r.end_unanswered(
            499,
            "cancelled in Toktail",
            cancelled_response("Cancelled in Toktail."),
        );
    } else if r.gone_at.is_some() {
        log::info!("flight #{id}: the agent hung up and no retry came; stopped");
        r.end_unanswered(
            499,
            "agent hung up; stopped",
            cancelled_response("The agent hung up."),
        );
    }
    // Otherwise it was answered by another request (joined), already recorded.
}

/// How an answer ended, for the requests it answers. `answered`: the model
/// finished a good answer, even if this request's own agent had hung up
/// (its row says 499, but a retry can still use the answer).
fn summary_of(
    rec: Option<&Record>,
    answered: bool,
    captured: Option<&[u8]>,
) -> crate::flights::Summary {
    match rec {
        Some(r) => {
            let (outcome, signal, chars) = if r.status == 200 || !answered {
                (r.outcome.clone(), r.signal.clone(), r.response_chars)
            } else {
                crate::intelligence::classify(200, captured.unwrap_or(&[]))
            };
            crate::flights::Summary {
                ok: answered,
                host: r.host.clone(),
                model: r.model.clone(),
                input: r.input_tokens,
                output: r.output_tokens,
                outcome,
                signal,
                response_chars: chars,
            }
        }
        None => crate::flights::Summary::default(),
    }
}

/// Sends `b` to the agent and keeps it for joiners and the cache.
async fn emit(
    b: Vec<u8>,
    tx: &BodyTx,
    tee: &crate::flights::Tee,
    cache: &mut Option<Vec<u8>>,
    agent_gone: &mut bool,
) {
    if b.is_empty() {
        return;
    }
    tee.push(&b);
    if let Some(s) = cache.as_mut() {
        if s.len() + b.len() <= cache::MAX_ENTRY_BYTES {
            s.extend_from_slice(&b);
        } else {
            *cache = None;
        }
    }
    if !*agent_gone && tx.send(Ok(Bytes::from(b))).await.is_err() {
        *agent_gone = true;
    }
}

/// A streamed answer: counted as it passes, steerable until its first token.
async fn stream(
    mut r: Run,
    mut cmds: tokio::sync::watch::Receiver<crate::flights::Cmd>,
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
) {
    use crate::flights::Cmd;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    let control = r.guards.flight.control();
    control.tee.set_head(status.as_u16(), &content_type);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut reply = Response::new(Body::from_stream(body));
    *reply.status_mut() = status;
    *reply.headers_mut() = headers;
    let was_gone = r.gone_at.is_some();
    r.respond(reply);
    let mut agent_gone = was_gone;

    let mut upstream = resp.bytes_stream();
    let mut tracker = r.tracker();
    let mut code = status.as_u16();
    // Nothing reaches the agent until the first token: until then the
    // request can still be rerouted or joined without the agent noticing.
    let mut held: Vec<u8> = Vec::new();
    let mut released = false;
    let mut cache_copy: Option<Vec<u8>> = r.t.cache_key.is_some().then(Vec::new);
    let mut upstream_failed = false;
    let mut stopped: Option<&'static str> = None;
    let mut named = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            chunk = upstream.next() => match chunk {
                None => break,
                Some(Err(e)) => {
                    log::warn!("upstream stream ended with an error: {e}");
                    upstream_failed = true;
                    break;
                }
                Some(Ok(c)) => {
                    let before = tracker.deltas;
                    let out = tracker.push(&c);
                    if tracker.deltas > before {
                        r.ctx.app.live.add(tracker.deltas - before, 0);
                        r.ctx.app.live_for(&r.t.host).add(tracker.deltas - before, 0);
                    }
                    r.guards.flight.progress(tracker.deltas, tracker.first_token_at);
                    if !named {
                        if let Some(m) = tracker.model.as_deref() {
                            r.guards.flight.set_model(m);
                            named = true;
                        }
                    }
                    if released {
                        emit(out, &tx, &control.tee, &mut cache_copy, &mut agent_gone).await;
                    } else {
                        held.extend_from_slice(&out);
                        if tracker.deltas > 0 || tracker.tool_call || held.len() > 64 * 1024 {
                            released = true;
                            r.guards.flight.released();
                            emit(std::mem::take(&mut held), &tx, &control.tee, &mut cache_copy, &mut agent_gone).await;
                        }
                    }
                    if agent_gone {
                        r.agent_hung_up();
                    }
                    // The last event went by: done, even if the server
                    // leaves the connection open.
                    if tracker.ended {
                        break;
                    }
                }
            },
            ch = cmds.changed() => {
                if ch.is_err() {
                    continue;
                }
                let cmd = cmds.borrow_and_update().clone();
                match cmd {
                    Cmd::Cancel => {
                        stopped = Some("cancelled in Toktail");
                        break;
                    }
                    Cmd::Reroute { host, model } if !released => match retarget(&r.ctx.app, &host, &model) {
                        Ok(to) => {
                            r.reroute(to);
                            match r.request().send().await {
                                Ok(nr) if nr.status().is_success()
                                    && nr.headers().get(header::CONTENT_TYPE)
                                        .and_then(|v| v.to_str().ok())
                                        .is_some_and(|c| c.contains("text/event-stream")) =>
                                {
                                    upstream = nr.bytes_stream();
                                    tracker = r.tracker();
                                    held.clear();
                                    named = false;
                                    r.guards.flight.progress(0, None);
                                }
                                Ok(nr) => {
                                    code = nr.status().as_u16();
                                    let body = nr.bytes().await.unwrap_or_default();
                                    let msg = format!(
                                        "Rerouted to {} and it answered HTTP {code}: {}",
                                        host_name(&r.ctx.app, &r.t.host),
                                        String::from_utf8_lossy(&body).chars().take(400).collect::<String>()
                                    );
                                    emit(error_event(&r.t.endpoint, "toktail_reroute_failed", &msg).to_vec(), &tx, &control.tee, &mut None::<Vec<u8>>, &mut agent_gone).await;
                                    stopped = Some("rerouted, but the new host refused it");
                                    break;
                                }
                                Err(e) => {
                                    let msg = format!("Rerouted to {}, which couldn't be reached: {e}", host_name(&r.ctx.app, &r.t.host));
                                    emit(error_event(&r.t.endpoint, "toktail_reroute_failed", &msg).to_vec(), &tx, &control.tee, &mut None::<Vec<u8>>, &mut agent_gone).await;
                                    code = 502;
                                    stopped = Some("rerouted, but the new host couldn't be reached");
                                    break;
                                }
                            }
                        }
                        Err(e) => log::warn!("flight #{}: can't reroute: {e}", r.guards.flight.id()),
                    },
                    Cmd::Join(id) if !released => {
                        if let Some(leader) = r.ctx.app.activity.control_of(id) {
                            drop(upstream);
                            r.guards.flight.joined(id);
                            relay(&mut r, id, leader, tx, cmds).await;
                            return;
                        }
                    }
                    _ => {}
                }
            },
            _ = tx.closed(), if !agent_gone => {
                agent_gone = true;
                r.agent_hung_up();
            },
            _ = tick.tick(), if r.gone_at.is_some() => {
                if r.abandoned() {
                    stopped = Some("agent hung up; stopped");
                    break;
                }
            },
        }
    }
    drop(upstream);
    if let Some(note) = stopped {
        if note == "cancelled in Toktail" {
            emit(
                error_event(&r.t.endpoint, "toktail_cancelled", "Cancelled in Toktail.").to_vec(),
                &tx,
                &crate::flights::Tee::default(),
                &mut None::<Vec<u8>>,
                &mut agent_gone,
            )
            .await;
            code = 499;
        } else if note.starts_with("agent hung up") {
            code = 499;
        }
        r.t.note = if r.t.note.is_empty() {
            note.to_string()
        } else {
            format!("{}; {note}", r.t.note)
        };
        log::info!("flight #{}: {note}", r.guards.flight.id());
    } else {
        // Answers with no token at all (an error, say) are sent at the end.
        if !released {
            emit(
                std::mem::take(&mut held),
                &tx,
                &control.tee,
                &mut cache_copy,
                &mut agent_gone,
            )
            .await;
        }
        let rest = tracker.finish();
        emit(rest, &tx, &control.tee, &mut cache_copy, &mut agent_gone).await;
        if upstream_failed {
            code = 502;
        } else if agent_gone && r.guards.flight.joiners() == 0 {
            code = 499;
        }
    }
    drop(tx);
    let ttft = tracker
        .first_token_at
        .map(|f| f.duration_since(r.t.started));
    let duration = r.t.started.elapsed();
    let captured = tracker.text.take().map(|(content, reasoning)| {
        let v = serde_json::json!({
            "object": "toktail.stream",
            "model": tracker.model,
            "content": content,
            "reasoning": reasoning,
            "usage": { "input_tokens": tracker.usage.input, "output_tokens": tracker.usage.output },
            "status": code,
            "refusal": tracker.refusal,
            "tool_call": tracker.tool_call,
            "content_chars": tracker.content_chars,
        });
        Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
    });
    let (model, usage, deltas) = (tracker.model.clone(), tracker.usage, tracker.deltas);
    let answered = stopped.is_none() && !upstream_failed && status.as_u16() == 200;
    let kept = captured.clone();
    let rec = match cache_copy.filter(|_| code == 200 && stopped.is_none()) {
        Some(bytes) => record_and_cache(
            &r.ctx,
            r.t,
            model,
            code,
            usage,
            deltas,
            ttft,
            duration,
            captured,
            CacheBody {
                content_type,
                streamed: true,
                bytes: Bytes::from(bytes),
            },
        ),
        None => record(
            &r.ctx, r.t, model, code, usage, deltas, ttft, duration, captured,
        ),
    };
    control
        .tee
        .finish(summary_of(rec.as_ref(), answered, kept.as_deref()));
    drop(r.guards);
}

impl Run {
    /// Answers this request with request `id`'s answer, from its first byte.
    /// Waits for that answer to start; if it fails first, carries on alone.
    async fn wait_for_join(
        &mut self,
        id: u64,
        leader: Arc<crate::flights::FlightControl>,
        cmds: &mut tokio::sync::watch::Receiver<crate::flights::Cmd>,
    ) -> JoinEnd {
        use crate::flights::Cmd;
        self.guards.flight.joined(id);
        let finished = self.ctx.app.activity.is_orphan(id);
        log::info!(
            "flight #{}: {} sent the same input as #{id} again; {}",
            self.guards.flight.id(),
            if self.t.detected.agent.is_empty() {
                "the agent"
            } else {
                &self.t.detected.agent
            },
            if finished {
                "#{id} already finished after its agent hung up, and its answer goes to this one"
            } else {
                "it waits for #{id}'s answer instead of being read again"
            }
        );
        let mut version = leader.tee.subscribe();
        let watch = self.reply.clone();
        let gone = async move {
            match watch {
                Some(t) => t.closed().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(gone);
        let head = loop {
            let rd = leader.tee.read(0);
            if let Some(h) = rd.head {
                break h;
            }
            if rd.done.is_some() || rd.overflow {
                self.guards.flight.unjoin();
                return JoinEnd::Fallback;
            }
            tokio::select! {
                ch = version.changed() => if ch.is_err() {
                    self.guards.flight.unjoin();
                    return JoinEnd::Fallback;
                },
                ch = cmds.changed() => {
                    if ch.is_err() { continue; }
                    let cmd = cmds.borrow_and_update().clone();
                    match cmd {
                        Cmd::Cancel => {
                            self.guards.flight.unjoin();
                            self.settle(499, "cancelled in Toktail", cancelled_response("Cancelled in Toktail."));
                            return JoinEnd::Done;
                        }
                        Cmd::Reroute { host, model } => {
                            if let Ok(to) = retarget(&self.ctx.app, &host, &model) {
                                self.guards.flight.unjoin();
                                self.reroute(to);
                                return JoinEnd::Rerouted;
                            }
                        }
                        _ => {}
                    }
                },
                _ = &mut gone => {
                    // Nobody left to answer.
                    self.guards.flight.unjoin();
                    self.settle(499, "agent hung up", cancelled_response("The agent hung up."));
                    return JoinEnd::Done;
                }
            }
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
        let body = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        let mut reply = Response::new(Body::from_stream(body));
        *reply.status_mut() = StatusCode::from_u16(head.status).unwrap_or(StatusCode::OK);
        if let Ok(ct) = head.content_type.parse() {
            reply.headers_mut().insert(header::CONTENT_TYPE, ct);
        }
        reply.headers_mut().insert(
            "x-toktail-joined",
            HeaderValue::from_str(&id.to_string()).unwrap_or(HeaderValue::from_static("1")),
        );
        self.respond(reply);
        relay(self, id, leader, tx, cmds.clone()).await;
        JoinEnd::Done
    }

    /// Writes this request's row without an answer of its own.
    fn settle(&mut self, status: u16, note: &str, reply: Response) {
        self.respond(reply);
        let mut t = std::mem::replace(&mut self.t, empty_tracked());
        t.note = note.to_string();
        let duration = t.started.elapsed();
        record(
            &self.ctx,
            t,
            None,
            status,
            Usage::default(),
            0,
            None,
            duration,
            None,
        );
        self.settled = true;
    }
}

/// A placeholder after a request's details moved elsewhere.
fn empty_tracked() -> Tracked {
    Tracked {
        endpoint: String::new(),
        request_model: None,
        fingerprint: String::new(),
        streamed: false,
        strip_injected_usage: false,
        started: Instant::now(),
        original: None,
        detected: commentary::Detected::default(),
        turn: String::new(),
        new_part: String::new(),
        token_id: String::new(),
        client: String::new(),
        cache_key: None,
        host: String::new(),
        note: String::new(),
    }
}

/// Streams request `id`'s answer (from its first byte) to this request's
/// agent, then writes this request's row: tokens the agent received, which
/// no model ran for it.
async fn relay(
    r: &mut Run,
    id: u64,
    leader: Arc<crate::flights::FlightControl>,
    tx: BodyTx,
    mut cmds: tokio::sync::watch::Receiver<crate::flights::Cmd>,
) {
    use crate::flights::Cmd;
    let mut version = leader.tee.subscribe();
    let mut next = 0;
    let mut status = 200;
    let mut summary = None;
    let mut cancelled = false;
    loop {
        let rd = leader.tee.read(next);
        if let Some(h) = &rd.head {
            status = h.status;
        }
        next += rd.chunks.len();
        let mut gone = false;
        for c in rd.chunks {
            if tx.send(Ok(c)).await.is_err() {
                gone = true;
                break;
            }
        }
        if gone {
            status = 499;
            break;
        }
        if let Some(d) = rd.done {
            if !d.ok && d.model.is_empty() {
                // It stopped without an answer (cancelled, hung up, failed).
                let _ = tx
                    .send(Ok(error_event(
                        &r.t.endpoint,
                        "toktail_joined_failed",
                        &format!(
                            "The request #{id} this one was waiting on stopped without finishing."
                        ),
                    )))
                    .await;
                status = 502;
            }
            summary = Some(d);
            break;
        }
        if rd.overflow {
            status = 502;
            break;
        }
        tokio::select! {
            ch = version.changed() => if ch.is_err() { status = 502; break; },
            ch = cmds.changed() => {
                if ch.is_ok() && matches!(*cmds.borrow_and_update(), Cmd::Cancel) {
                    let _ = tx.send(Ok(error_event(&r.t.endpoint, "toktail_cancelled", "Cancelled in Toktail."))).await;
                    cancelled = true;
                    status = 499;
                    break;
                }
            },
            _ = tx.closed() => { status = 499; break; },
        }
    }
    drop(tx);
    let s = summary.unwrap_or_default();
    record_joined(
        &r.ctx,
        &mut r.t,
        &s,
        status,
        if cancelled {
            "cancelled in Toktail".into()
        } else if status == 499 {
            format!("hung up while waiting for #{id}'s answer")
        } else {
            format!("answered by #{id}")
        },
    );
    r.settled = true;
}

/// The row for a request another one answered: the agent got those tokens,
/// no model ran for this one (counted like a cache hit).
fn record_joined(
    ctx: &Ctx,
    t: &mut Tracked,
    s: &crate::flights::Summary,
    status: u16,
    note: String,
) {
    let ok = status == 200 && s.ok;
    let rec = Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: Local::now(),
        model: if s.model.is_empty() {
            t.request_model.clone().unwrap_or_else(|| "unknown".into())
        } else {
            s.model.clone()
        },
        endpoint: t.endpoint.clone(),
        streamed: t.streamed,
        status,
        input_tokens: if ok { s.input } else { 0 },
        output_tokens: if ok { s.output } else { 0 },
        duration_ms: t.started.elapsed().as_millis() as u64,
        ttft_ms: None,
        usage_source: "joined",
        prompt_sha256: t.fingerprint.clone(),
        host: if s.host.is_empty() {
            t.host.clone()
        } else {
            s.host.clone()
        },
        record: String::new(),
        outcome: if ok {
            s.outcome.clone()
        } else if status == 499 {
            "cancelled".into()
        } else {
            "error".into()
        },
        signal: if ok { s.signal.clone() } else { String::new() },
        response_chars: if ok { s.response_chars } else { 0 },
        prompt_blob: String::new(),
        prompt_key: String::new(),
        response_key: String::new(),
        reference_key: String::new(),
        meta: Meta {
            token_id: t.token_id.clone(),
            cache_read: 0,
            cache_write: 0,
            cache_hit: true,
            session_id: t.detected.session.clone(),
            chain_node: String::new(),
            prefix_tokens: 0,
            agent: t.detected.agent.clone(),
            client: t.client.clone(),
        },
    };
    ctx.app.activity.finish(crate::live::Finished {
        seq: 0,
        request_id: rec.request_id.clone(),
        host: rec.host.clone(),
        model: rec.model.clone(),
        endpoint: rec.endpoint.clone(),
        status: rec.status,
        input: rec.input_tokens,
        output: rec.output_tokens,
        duration_ms: rec.duration_ms,
        ttft_ms: None,
        tokens_per_sec: None,
        outcome: rec.outcome.clone(),
        signal: rec.signal.clone(),
        finished_at: rec.timestamp.to_rfc3339(),
        cache_hit: true,
        agent: rec.meta.agent.clone(),
        note,
    });
    crate::records::enqueue(&ctx.app, rec, None, None);
}

/// Answers from the prompt cache: the client gets the bytes it got the first
/// time, the CSV gets a row marked as a hit, and no model runs.
#[allow(clippy::too_many_arguments)]
fn serve_cached(
    ctx: &Ctx,
    h: &HostConfig,
    entry: cache::Entry,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
    token_id: &str,
    client: &str,
    started: Instant,
) -> Response {
    let head = entry.head;
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let detected = commentary::detect(headers, &v);
    let now = Local::now();
    let rec = Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: now,
        model: head.model.clone(),
        endpoint: path.to_string(),
        streamed: head.streamed,
        status: 200,
        input_tokens: head.input,
        output_tokens: head.output,
        duration_ms: started.elapsed().as_millis() as u64,
        ttft_ms: None,
        usage_source: "cached",
        prompt_sha256: usage::prompt_fingerprint(&v).unwrap_or_default(),
        host: ctx.host.clone(),
        record: String::new(),
        outcome: head.outcome.clone(),
        signal: head.signal.clone(),
        response_chars: head.response_chars,
        prompt_blob: String::new(),
        prompt_key: String::new(),
        response_key: String::new(),
        reference_key: String::new(),
        meta: Meta {
            token_id: token_id.to_string(),
            cache_read: head.cache_read,
            cache_write: head.cache_write,
            cache_hit: true,
            session_id: detected.session.clone(),
            chain_node: String::new(),
            prefix_tokens: 0,
            agent: detected.agent.clone(),
            client: client.to_string(),
        },
    };
    ctx.app.activity.finish(crate::live::Finished {
        seq: 0,
        request_id: rec.request_id.clone(),
        host: rec.host.clone(),
        model: rec.model.clone(),
        endpoint: rec.endpoint.clone(),
        status: 200,
        input: rec.input_tokens,
        output: rec.output_tokens,
        duration_ms: rec.duration_ms,
        ttft_ms: None,
        tokens_per_sec: None,
        outcome: rec.outcome.clone(),
        signal: rec.signal.clone(),
        finished_at: rec.timestamp.to_rfc3339(),
        cache_hit: true,
        agent: rec.meta.agent.clone(),
        note: String::new(),
    });
    // A hit is not saved as a record (the original already is).
    crate::records::enqueue(&ctx.app, rec, None, None);
    let _ = h;
    let mut r = Response::new(Body::from(entry.body));
    *r.status_mut() = StatusCode::OK;
    if let Ok(ct) = head.content_type.parse() {
        r.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    r.headers_mut()
        .insert("x-toktail-cache", HeaderValue::from_static("hit"));
    r
}

/// Reads model/stream/prompt from the request and, for streamed OpenAI-style
/// calls, asks the server to include usage in the stream.
#[allow(clippy::too_many_arguments)]
fn prepare(
    kind: EndpointKind,
    path: &str,
    headers: &HeaderMap,
    body: &mut Bytes,
    started: Instant,
    saving: bool,
    token_id: String,
    client: String,
    cache_key: Option<String>,
) -> Option<Tracked> {
    let original = saving.then(|| body.clone());
    let Ok(mut v) = serde_json::from_slice::<Value>(body) else {
        // Not JSON: still count the call, without a fingerprint.
        return Some(Tracked {
            endpoint: path.to_string(),
            request_model: None,
            fingerprint: String::new(),
            streamed: false,
            strip_injected_usage: false,
            started,
            original,
            detected: commentary::detect(headers, &Value::Null),
            turn: String::new(),
            new_part: String::new(),
            token_id,
            client,
            cache_key,
            host: String::new(),
            note: String::new(),
        });
    };
    let fingerprint = usage::prompt_fingerprint(&v).unwrap_or_default();
    let request_model = v.get("model").and_then(Value::as_str).map(str::to_string);
    let streamed = v.get("stream").and_then(Value::as_bool) == Some(true);
    let detected = commentary::detect(headers, &v);
    let turn = commentary::last_user_turn(&v);
    let new_part = if client.is_empty() {
        crate::summaries::new_part(&v)
    } else {
        String::new()
    };
    let mut strip = false;
    if streamed && kind.supports_include_usage() {
        let asked = v
            .pointer("/stream_options/include_usage")
            .and_then(Value::as_bool)
            == Some(true);
        if !asked {
            if let Some(obj) = v.as_object_mut() {
                let so = obj
                    .entry("stream_options")
                    .or_insert_with(|| Value::Object(Default::default()));
                if !so.is_object() {
                    *so = Value::Object(Default::default());
                }
                so["include_usage"] = Value::Bool(true);
                if let Ok(b) = serde_json::to_vec(&v) {
                    *body = Bytes::from(b);
                    strip = true;
                }
            }
        }
    }
    Some(Tracked {
        endpoint: path.to_string(),
        request_model,
        fingerprint,
        streamed,
        strip_injected_usage: strip,
        started,
        original,
        detected,
        turn,
        new_part,
        token_id,
        client,
        cache_key,
        host: String::new(),
        note: String::new(),
    })
}

/// The answer as the client received it, for the prompt cache.
struct CacheBody {
    content_type: String,
    streamed: bool,
    bytes: Bytes,
}

/// Records the request, then stores the answer in the cache when it qualifies.
#[allow(clippy::too_many_arguments)]
fn record_and_cache(
    ctx: &Ctx,
    t: Tracked,
    response_model: Option<String>,
    status: u16,
    u: Usage,
    deltas: u64,
    ttft: Option<Duration>,
    duration: Duration,
    response: Option<Bytes>,
    cache_body: CacheBody,
) -> Option<Record> {
    let key = t.cache_key.clone();
    let endpoint = t.endpoint.clone();
    let rec = record(
        ctx,
        t,
        response_model,
        status,
        u,
        deltas,
        ttft,
        duration,
        response,
    );
    let copy = rec.clone();
    if let (Some(key), Some(rec)) = (key, rec) {
        if rec.status == 200 && rec.outcome == "complete" {
            let head = cache::Head {
                status: 200,
                content_type: cache_body.content_type,
                streamed: cache_body.streamed,
                model: rec.model,
                input: rec.input_tokens,
                output: rec.output_tokens,
                cache_read: rec.meta.cache_read,
                cache_write: rec.meta.cache_write,
                usage_source: rec.usage_source.to_string(),
                outcome: rec.outcome,
                signal: rec.signal,
                response_chars: rec.response_chars,
                request_id: rec.request_id,
                created: Local::now().to_rfc3339(),
                endpoint,
            };
            let app = ctx.app.clone();
            ctx.app.rt.spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = cache::put(&app, &key, &head, &cache_body.bytes) {
                        log::debug!("cache: not stored: {e:#}");
                    }
                })
                .await;
            });
        }
    }
    copy
}

/// Writes the row, updates the live meters and feed, hands the bodies to
/// records, and offers the exchange to Pip. Returns a copy of the record.
#[allow(clippy::too_many_arguments)]
fn record(
    ctx: &Ctx,
    t: Tracked,
    response_model: Option<String>,
    status: u16,
    u: Usage,
    deltas: u64,
    ttft: Option<Duration>,
    duration: Duration,
    response: Option<Bytes>,
) -> Option<Record> {
    let app = &ctx.app;
    let (input, output, source) = if u.is_reported() {
        (u.input.unwrap_or(0), u.output.unwrap_or(0), "reported")
    } else if deltas > 0 {
        (0, deltas, "estimated")
    } else {
        (0, 0, "none")
    };
    // Live meter: streamed deltas were counted as they passed; add the rest
    // (the reported total may differ from the delta count) and the prompt.
    app.live.add(output.saturating_sub(deltas), input);
    app.live_for(&t.host)
        .add(output.saturating_sub(deltas), input);
    let now = Local::now();
    let (outcome, signal, response_chars) =
        crate::intelligence::classify(status, response.as_deref().unwrap_or(&[]));
    // Saved bodies are scrubbed of anything key-shaped first.
    let prompt = t
        .original
        .filter(|p| p.len() <= MAX_CAPTURE_BYTES)
        .map(|p| tokens::scrub_bytes(&p));
    let captured = response
        .as_ref()
        .filter(|r| r.len() <= MAX_CAPTURE_BYTES)
        .map(|r| tokens::scrub_bytes(r));
    let model = response_model
        .or(t.request_model)
        .unwrap_or_else(|| "unknown".into());
    let rec = Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: now,
        model: model.clone(),
        endpoint: t.endpoint,
        streamed: t.streamed,
        status,
        input_tokens: input,
        output_tokens: output,
        duration_ms: duration.as_millis() as u64,
        ttft_ms: ttft.map(|d| d.as_millis() as u64),
        usage_source: source,
        prompt_sha256: t.fingerprint,
        host: t.host.clone(),
        record: String::new(),
        outcome,
        signal,
        response_chars,
        prompt_blob: String::new(),
        prompt_key: String::new(),
        response_key: String::new(),
        reference_key: String::new(),
        meta: Meta {
            token_id: t.token_id,
            cache_read: u.cache_read,
            cache_write: u.cache_write,
            cache_hit: false,
            session_id: t.detected.session.clone(),
            chain_node: String::new(),
            prefix_tokens: 0,
            agent: t.detected.agent.clone(),
            client: t.client.clone(),
        },
    };
    let gen = rec
        .duration_ms
        .saturating_sub(rec.ttft_ms.unwrap_or(0))
        .max(if rec.ttft_ms.is_some() { 1 } else { 0 });
    app.activity.finish(crate::live::Finished {
        seq: 0,
        request_id: rec.request_id.clone(),
        host: rec.host.clone(),
        model: rec.model.clone(),
        endpoint: rec.endpoint.clone(),
        status: rec.status,
        input: rec.input_tokens,
        output: rec.output_tokens,
        duration_ms: rec.duration_ms,
        ttft_ms: rec.ttft_ms,
        tokens_per_sec: (rec.output_tokens > 0 && gen > 0)
            .then(|| rec.output_tokens as f64 * 1000.0 / gen as f64),
        outcome: rec.outcome.clone(),
        signal: rec.signal.clone(),
        finished_at: rec.timestamp.to_rfc3339(),
        cache_hit: false,
        agent: rec.meta.agent.clone(),
        note: t.note.clone(),
    });
    // Pip's own requests (chat, commentary) are counted but never commented on.
    if t.client.is_empty() && status == 200 && rec.outcome == "complete" {
        if let Some(h) = app.config.read().host(&t.host).cloned() {
            let answer = response
                .as_deref()
                .map(|r| {
                    crate::records::summarize_response(r)["content"]
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default();
            crate::summaries::offer(
                app,
                &rec.request_id,
                &h,
                &model,
                &t.detected.agent,
                &t.detected.project,
                &t.new_part,
                &answer,
            );
            if let Some(c) =
                commentary::candidate(&rec.request_id, &h, &model, &t.detected, &t.turn, &answer)
            {
                commentary::offer(app, c, false);
            }
        }
    }
    // Toktail's own requests aren't saved as records.
    let (prompt, captured) =
        if t.client == commentary::CLIENT || t.client == crate::summaries::CLIENT {
            (None, None)
        } else {
            (prompt, captured)
        };
    let copy = rec.clone();
    crate::records::enqueue(app, rec, prompt, captured);
    Some(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_addresses() {
        assert_eq!(
            normalize_upstream("studio.local:1234").unwrap(),
            "http://studio.local:1234"
        );
        assert_eq!(
            normalize_upstream(" http://10.0.0.5:1234/v1/ ").unwrap(),
            "http://10.0.0.5:1234"
        );
        assert_eq!(
            normalize_upstream("https://lm.example.com").unwrap(),
            "https://lm.example.com"
        );
        assert_eq!(
            normalize_upstream("https://lm.example.com/lmstudio/v1").unwrap(),
            "https://lm.example.com/lmstudio"
        );
        assert!(normalize_upstream("").is_err());
        assert!(normalize_upstream("ftp://x").is_err());
    }

    #[test]
    fn refuses_to_forward_to_itself() {
        assert!(loops_back("http://127.0.0.1:1234", 1234));
        assert!(loops_back("http://localhost:4321", 4321));
        assert!(!loops_back("http://127.0.0.1:1235", 1234));
        assert!(!loops_back("http://studio.local:1234", 1234));
    }

    #[test]
    fn token_replaces_client_key() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer lm-studio".parse().unwrap());
        h.insert(header::HOST, "localhost:1234".parse().unwrap());
        h.insert("x-toktail-session", "s1".parse().unwrap());
        let out = forward_request_headers(
            &h,
            true,
            &Forward::Replace {
                header: "authorization",
                value: "secret".into(),
            },
        );
        assert_eq!(out.get(header::AUTHORIZATION).unwrap(), "Bearer secret");
        assert!(out.get(header::HOST).is_none());
        assert!(
            out.get("x-toktail-session").is_none(),
            "Toktail headers stay home"
        );
        let out = forward_request_headers(&h, true, &Forward::KeepClient);
        assert_eq!(out.get(header::AUTHORIZATION).unwrap(), "Bearer lm-studio");
        // Anthropic keys go in x-api-key; a client's Authorization is dropped.
        let out = forward_request_headers(
            &h,
            true,
            &Forward::Replace {
                header: "x-api-key",
                value: "ant".into(),
            },
        );
        assert_eq!(out.get("x-api-key").unwrap(), "ant");
        assert!(out.get(header::AUTHORIZATION).is_none());
    }

    fn ctx_with(app: Arc<App>, host: HostConfig) -> Arc<Ctx> {
        let id = host.id.clone();
        app.config.write().hosts = vec![host];
        Arc::new(Ctx {
            app,
            client: client(),
            host: id,
        })
    }

    #[tokio::test]
    async fn public_hosts_pass_keys_through_and_never_relay() {
        let app = crate::test_support::app();
        let (plain, entry) = tokens::issue_gateway_key("roc");
        let host = HostConfig {
            id: "claude".into(),
            kind: "anthropic".into(),
            upstream: "https://api.anthropic.com".into(),
            listen: "0.0.0.0:4000".into(),
            gateway_keys: vec![entry.clone()],
            ..Default::default()
        };
        let ctx = ctx_with(app.clone(), host.clone());
        let here: SocketAddr = "127.0.0.1:5555".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:5555".parse().unwrap();
        // 1. An agent's own key passes through unchanged, with a one-way id.
        let mut h = HeaderMap::new();
        h.insert(
            "x-api-key",
            "sk-ant-api03-agentkey0123456789".parse().unwrap(),
        );
        let (fwd, id) = authorize(&ctx, &host, &h, lan).unwrap();
        assert!(matches!(fwd, Forward::KeepClient));
        assert!(id.starts_with("tk_") && !id.contains("agentkey"));
        assert!(
            app.status.read().hosts["claude"].lan_key_from.is_some(),
            "a key from the LAN is noted"
        );
        // 2. No key from another machine: refused, even with a stored key.
        app.host_tokens
            .write()
            .insert("claude".into(), "sk-ant-stored-0123456789abcdef".into());
        let r = authorize(&ctx, &host, &HeaderMap::new(), lan)
            .err()
            .expect("refused");
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        // 3. No key from this machine: the stored key is added.
        let (fwd, id) = authorize(&ctx, &host, &HeaderMap::new(), here).unwrap();
        match fwd {
            Forward::Replace { header, value } => {
                assert_eq!(header, "x-api-key");
                assert!(value.starts_with("sk-ant-stored"));
            }
            _ => panic!("expected the stored key"),
        }
        assert_eq!(id, "stored");
        // 4. A gateway key from the LAN becomes the stored key; the id is the key's.
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            format!("Bearer {plain}").parse().unwrap(),
        );
        let (fwd, id) = authorize(&ctx, &host, &h, lan).unwrap();
        assert!(matches!(
            fwd,
            Forward::Replace {
                header: "x-api-key",
                ..
            }
        ));
        assert_eq!(id, entry.id);
        // 5. An unknown gateway key is refused.
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            "Bearer tkg_0000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            authorize(&ctx, &host, &h, here).err().unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        // 6. An LM Studio host keeps its old behaviour: the stored token replaces the client's.
        let lm = HostConfig {
            id: "studio".into(),
            ..Default::default()
        };
        let ctx = ctx_with(app.clone(), lm.clone());
        app.host_tokens
            .write()
            .insert("studio".into(), "lm-token".into());
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer placeholder".parse().unwrap());
        let (fwd, _) = authorize(&ctx, &lm, &h, lan).unwrap();
        assert!(
            matches!(fwd, Forward::Replace { header: "authorization", value } if value == "lm-token")
        );
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}

#[cfg(test)]
mod flight_tests {
    //! Steering requests in flight, end to end: a proxy on a real port, fake
    //! model servers that hold a request in "prefill" until told to answer.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake LM Studio. Requests for model "slow" wait for `open`.
    /// Requests a fake server saw: (model, authorization).
    type Seen = Arc<parking_lot::Mutex<Vec<(String, Option<String>)>>>;

    #[derive(Clone)]
    struct Fake {
        name: &'static str,
        received: Arc<AtomicUsize>,
        dropped: Arc<AtomicUsize>,
        open: tokio::sync::watch::Sender<bool>,
        seen: Seen,
        /// Send SSE headers at once (then stall), like a server that answers
        /// the head before prefill finishes.
        early_head: bool,
    }

    struct DropMark(Arc<AtomicUsize>, bool);
    impl Drop for DropMark {
        fn drop(&mut self) {
            if !self.1 {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    async fn fake_handler(State(f): State<Fake>, req: Request) -> Response {
        let auth = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = to_bytes(req.into_body(), 1 << 20).await.unwrap_or_default();
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let model = v["model"].as_str().unwrap_or("").to_string();
        let streamed = v["stream"] == true;
        f.received.fetch_add(1, Ordering::SeqCst);
        f.seen.lock().push((model.clone(), auth));
        let mut open = f.open.subscribe();
        let mut mark = DropMark(f.dropped.clone(), false);
        let wait = model == "slow";
        let name = f.name;
        if !streamed {
            if wait {
                let _ = open.wait_for(|o| *o).await;
            }
            mark.1 = true;
            let body = serde_json::json!({"model": model, "choices": [{"message": {"role": "assistant", "content": format!("answer from {name}")}}], "usage": {"prompt_tokens": 7, "completion_tokens": 3}});
            return axum::Json(body).into_response();
        }
        if wait && !f.early_head {
            let _ = open.wait_for(|o| *o).await;
        }
        let stream = async_stream(open, wait, model, name, mark);
        let mut r = Response::new(Body::from_stream(stream));
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        r
    }

    fn async_stream(
        open: tokio::sync::watch::Receiver<bool>,
        wait: bool,
        model: String,
        name: &'static str,
        mark: DropMark,
    ) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> {
        futures_util::stream::unfold((0u8, mark), move |(step, mut mark)| {
            let model = model.clone();
            let mut open = open.clone();
            async move {
                if step == 0 && wait {
                    let _ = open.wait_for(|o| *o).await;
                }
                let chunk = match step {
                    0 => format!(
                        "data: {}\n\n",
                        serde_json::json!({"model": model, "choices": [{"delta": {"content": format!("answer from {name}")}}]})
                    ),
                    1 => format!(
                        "data: {}\n\n",
                        serde_json::json!({"model": model, "choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 3}})
                    ),
                    2 => "data: [DONE]\n\n".to_string(),
                    _ if model == "lingers" => {
                        // A server that keeps the connection open after
                        // its last event.
                        std::future::pending::<()>().await;
                        return None;
                    }
                    _ => {
                        // Finished cleanly: not a drop.
                        #[allow(unused_assignments)]
                        {
                            mark.1 = true;
                        }
                        return None;
                    }
                };
                Some((Ok(Bytes::from(chunk)), (step + 1, mark)))
            }
        })
    }

    async fn start_fake(name: &'static str, early_head: bool) -> (Fake, String) {
        let f = Fake {
            name,
            received: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicUsize::new(0)),
            open: tokio::sync::watch::channel(false).0,
            seen: Arc::new(parking_lot::Mutex::new(Vec::new())),
            early_head,
        };
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let app = Router::new().fallback(fake_handler).with_state(f.clone());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (f, format!("http://{addr}"))
    }

    /// Toktail with hosts "studio" and "mini" in front of the two fakes.
    async fn start_proxy(a: &str, b: &str, grace: u64) -> (Arc<App>, String) {
        let app = crate::test_support::app();
        {
            let mut c = app.config.write();
            c.hosts = vec![
                HostConfig {
                    id: "studio".into(),
                    name: "Studio".into(),
                    upstream: a.into(),
                    listen: "127.0.0.1:0".into(),
                    ..Default::default()
                },
                HostConfig {
                    id: "mini".into(),
                    name: "Mini".into(),
                    upstream: b.into(),
                    listen: "127.0.0.1:0".into(),
                    ..Default::default()
                },
            ];
            c.flights.abandoned_grace_secs = grace;
        }
        let ctx = Arc::new(Ctx {
            app: app.clone(),
            client: client(),
            host: "studio".into(),
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let router = Router::new().fallback(handle).with_state(ctx);
        tokio::spawn(async move {
            axum::serve(
                l,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        (app, format!("http://{addr}"))
    }

    async fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..200 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn body(model: &str, stream: bool) -> String {
        serde_json::json!({"model": model, "stream": stream, "messages": [{"role": "user", "content": "refactor the parser"}]}).to_string()
    }

    fn flights(app: &App) -> Vec<crate::live::FlightView> {
        app.activity.snapshot(0, 50).0
    }
    fn finished(app: &App) -> Vec<crate::live::Finished> {
        app.activity.snapshot(0, 50).1
    }

    fn post(url: &str, b: String) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("{url}/v1/chat/completions"))
            .header("content-type", "application/json")
            .header("authorization", "Bearer sk-agent-secret-0123456789")
            .body(b)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_stops_the_model_and_tells_the_agent() {
        let (a, au) = start_fake("studio", false).await;
        let (_b, bu) = start_fake("mini", false).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let req = tokio::spawn(post(&url, body("slow", false)).send());
        until("the request is in flight", || flights(&app).len() == 1).await;
        until("the model server has it", || {
            a.received.load(Ordering::SeqCst) == 1
        })
        .await;
        let id = flights(&app)[0].id;
        assert!(flights(&app)[0].can_reroute);
        app.activity
            .command(id, crate::flights::Cmd::Cancel)
            .unwrap();
        let resp = req.await.unwrap().unwrap();
        assert_eq!(resp.status().as_u16(), 499);
        let v: Value = resp.json_value().await.unwrap();
        assert_eq!(v["error"]["type"], "toktail_cancelled");
        until("the model server sees the connection close", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        until("the feed has it", || {
            finished(&app)
                .iter()
                .any(|f| f.note == "cancelled in Toktail" && f.status == 499)
        })
        .await;
        assert!(flights(&app).is_empty());
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reroute_sends_it_elsewhere_and_never_forwards_the_agents_key() {
        let (a, au) = start_fake("studio", false).await;
        let (b, bu) = start_fake("mini", false).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let req = tokio::spawn(post(&url, body("slow", false)).send());
        until("in flight", || flights(&app).len() == 1).await;
        until("studio has it", || a.received.load(Ordering::SeqCst) == 1).await;
        let id = flights(&app)[0].id;
        app.activity
            .command(
                id,
                crate::flights::Cmd::Reroute {
                    host: "mini".into(),
                    model: "fast".into(),
                },
            )
            .unwrap();
        let resp = req.await.unwrap().unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers()
                .get("x-toktail-rerouted")
                .and_then(|v| v.to_str().ok()),
            Some("studio -> mini"),
            "the agent's side can tell where the answer came from"
        );
        let v: Value = resp.json_value().await.unwrap();
        assert_eq!(
            v["choices"][0]["message"]["content"], "answer from mini",
            "the agent that sent it gets the rerouted answer"
        );
        assert_eq!(b.received.load(Ordering::SeqCst), 1);
        let seen = b.seen.lock().clone();
        assert_eq!(seen[0].0, "fast", "the model was swapped");
        assert_eq!(seen[0].1, None, "the agent's key stayed home");
        until("studio's connection closes", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        until("the feed has it", || {
            finished(&app)
                .iter()
                .any(|f| f.host == "mini" && f.note == "rerouted from Studio" && f.model == "fast")
        })
        .await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    /// A request like opencode's: a big prompt, with settings a retry may change.
    fn agent_body(prompt: &str, max_tokens: u32, temperature: f64) -> String {
        serde_json::json!({
            "model": "slow", "stream": true, "max_tokens": max_tokens, "temperature": temperature,
            "messages": [{"role": "system", "content": "You are opencode."}, {"role": "user", "content": prompt}],
        })
        .to_string()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_answer_ends_at_its_last_event_even_if_the_server_stays_open() {
        let (_a, au) = start_fake("studio", false).await;
        let (_b, bu) = start_fake("mini", false).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let res = tokio::time::timeout(
            Duration::from_secs(5),
            reqwest::Client::new()
                .post(format!("{url}/v1/chat/completions"))
                .header("content-type", "application/json")
                .body(body("lingers", true))
                .send(),
        )
        .await
        .expect("headers in time")
        .unwrap();
        let text = tokio::time::timeout(Duration::from_secs(5), res.text())
            .await
            .expect("the agent's stream ends after [DONE]")
            .unwrap();
        assert!(text.contains("answer from studio"));
        assert!(text.trim_end().ends_with("data: [DONE]"));
        until("the request leaves the in-flight list", || {
            app.activity.snapshot(0, 10).0.is_empty()
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stack_of_retries_is_read_once_and_the_latest_gets_the_answer() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 60).await;
        let big = "x".repeat(200_000);
        // The original, then three retries that each time out and give up,
        // tweaking max_tokens and temperature as SDK retries do.
        let u = url.clone();
        let b0 = agent_body(&big, 800, 0.2);
        let orig = tokio::spawn(async move {
            drop(post(&u, b0).send().await.unwrap());
        });
        orig.await.unwrap();
        until("the original is in flight", || flights(&app).len() == 1).await;
        for n in 1..=2u32 {
            let u = url.clone();
            let b = agent_body(&big, 800 + n * 100, 0.2 + n as f64 / 10.0);
            tokio::spawn(async move {
                drop(post(&u, b).send().await.unwrap());
            })
            .await
            .unwrap();
        }
        let u = url.clone();
        let latest = tokio::spawn(async move {
            post(&u, agent_body(&big, 4096, 0.7))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("every resend has attached to the original", || {
            let st = app.activity.stacks();
            st.len() == 1
                && st[0]
                    .groups
                    .first()
                    .is_some_and(|g| g.attempts == 4 && !g.answered_by_original.is_empty())
        })
        .await;
        let st = app.activity.stacks();
        let g = &st[0].groups[0];
        assert_eq!(
            st[0].line.len(),
            1,
            "only the original is waiting to be read"
        );
        assert!(g.original_gone, "its agent gave up");
        assert_eq!(
            g.attempts, 4,
            "the original and three resends, counted even after two gave up"
        );
        assert!(g.prompt_tokens_est > 50_000, "{}", g.prompt_tokens_est);
        let orig_view = flights(&app)
            .into_iter()
            .find(|f| f.id == g.original)
            .unwrap();
        assert_eq!(
            orig_view.stops_in_secs, None,
            "kept running: a retry wants it"
        );
        assert_eq!(
            a.received.load(Ordering::SeqCst),
            1,
            "LM Studio got the prompt once"
        );
        a.open.send(true).unwrap();
        let text = latest.await.unwrap();
        assert!(text.contains("answer from studio"), "{text}");
        assert_eq!(a.received.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_answer_that_finishes_after_the_hang_up_waits_for_the_late_retry() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 60).await;
        let u = url.clone();
        tokio::spawn(async move {
            drop(post(&u, body("slow", true)).send().await.unwrap());
        })
        .await
        .unwrap();
        until("Toktail notices the agent left", || {
            flights(&app).first().is_some_and(|f| f.agent_gone)
        })
        .await;
        // The model finishes while the agent is backing off before its retry.
        a.open.send(true).unwrap();
        until("the original is done", || flights(&app).is_empty()).await;
        let text = post(&url, body("slow", true))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(text.contains("answer from studio"), "{text}");
        assert!(text.trim_end().ends_with("data: [DONE]"));
        assert_eq!(
            a.received.load(Ordering::SeqCst),
            1,
            "the 700K prompt wasn't read twice"
        );
        until("the retry's row", || {
            finished(&app)
                .iter()
                .any(|f| f.cache_hit && f.note.starts_with("answered by #"))
        })
        .await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn different_prompts_queue_and_the_portal_sees_the_line() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let mut reqs = Vec::new();
        for p in ["subagent one", "subagent two", "subagent three"] {
            let u = url.clone();
            let b = agent_body(p, 800, 0.2);
            reqs.push(tokio::spawn(async move {
                post(&u, b).send().await.unwrap().text().await.unwrap()
            }));
            until("in flight", || flights(&app).len() == reqs.len()).await;
        }
        let fl = flights(&app);
        let mut pos: Vec<(u64, Option<u32>, u32)> = fl
            .iter()
            .map(|f| (f.id, f.queue_pos, f.queue_len))
            .collect();
        pos.sort();
        assert_eq!(
            pos.iter().map(|p| p.1).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3)]
        );
        assert!(pos.iter().all(|p| p.2 == 3));
        let st = app.activity.stacks();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].line.len(), 3);
        assert!(
            st[0].groups.is_empty(),
            "different prompts: nothing to share"
        );
        a.open.send(true).unwrap();
        for r in reqs {
            assert!(r.await.unwrap().contains("answer from studio"));
        }
        assert_eq!(a.received.load(Ordering::SeqCst), 3);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn copies_already_running_merge_into_the_oldest() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        app.config.write().flights.join_duplicates = false;
        let mut reqs = Vec::new();
        for n in 0..3u32 {
            let u = url.clone();
            let b = agent_body("the same big prompt", 800 + n, 0.2);
            reqs.push(tokio::spawn(async move {
                post(&u, b).send().await.unwrap().text().await.unwrap()
            }));
            until("in flight", || flights(&app).len() == reqs.len()).await;
        }
        until("all three reached LM Studio", || {
            a.received.load(Ordering::SeqCst) == 3
        })
        .await;
        let st = app.activity.stacks();
        assert_eq!(st[0].groups.len(), 1);
        assert_eq!(st[0].groups[0].running_separately.len(), 2);
        let first = st[0].groups[0].original;
        assert_eq!(app.activity.collapse(first).unwrap(), 2);
        until("the two copies stop being read", || {
            a.dropped.load(Ordering::SeqCst) == 2
        })
        .await;
        a.open.send(true).unwrap();
        let texts: Vec<String> = futures_util::future::join_all(reqs)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert!(texts
            .iter()
            .all(|t| t == &texts[0] && t.contains("answer from studio")));
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_reroute_must_pick_a_model_the_target_has() {
        let (a, au) = start_fake("studio", false).await;
        let (_b, bu) = start_fake("mini", false).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let req = tokio::spawn(post(&url, body("slow", false)).send());
        until("in flight", || flights(&app).len() == 1).await;
        let id = flights(&app)[0].id;
        // The laptop has small models only.
        app.status
            .write()
            .hosts
            .entry("mini".into())
            .or_default()
            .upstream
            .models = vec!["qwen3-8b".into(), "gemma-3-4b".into()];
        let e = crate::flights::check_target(&app, id, "mini", "").unwrap_err();
        assert!(
            e.contains("doesn't have slow") && e.contains("qwen3-8b"),
            "{e}"
        );
        let e = crate::flights::check_target(&app, id, "mini", "llama-70b").unwrap_err();
        assert!(e.contains("doesn't have llama-70b"), "{e}");
        assert_eq!(
            crate::flights::check_target(&app, id, "mini", "gemma-3-4b").unwrap(),
            "gemma-3-4b"
        );
        // Same model is fine when the target has it.
        app.status
            .write()
            .hosts
            .get_mut("mini")
            .unwrap()
            .upstream
            .models
            .push("slow".into());
        assert_eq!(
            crate::flights::check_target(&app, id, "mini", "").unwrap(),
            "slow"
        );
        // A host that lists nothing takes the name as given.
        app.status
            .write()
            .hosts
            .get_mut("mini")
            .unwrap()
            .upstream
            .models
            .clear();
        assert_eq!(
            crate::flights::check_target(&app, id, "mini", "anything").unwrap(),
            "anything"
        );
        a.open.send(true).unwrap();
        let _ = req.await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stream_can_be_rerouted_until_its_first_token() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let req = tokio::spawn(async move {
            post(&url, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("in flight", || flights(&app).len() == 1).await;
        // The head has arrived; nothing has gone back to the agent yet.
        until("studio has it", || a.received.load(Ordering::SeqCst) == 1).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let id = flights(&app)[0].id;
        assert!(flights(&app)[0].can_reroute);
        app.activity
            .command(
                id,
                crate::flights::Cmd::Reroute {
                    host: "mini".into(),
                    model: "fast".into(),
                },
            )
            .unwrap();
        let text = req.await.unwrap();
        assert!(text.contains("answer from mini"), "{text}");
        assert!(!text.contains("answer from studio"));
        assert!(text.trim_end().ends_with("data: [DONE]"));
        until("studio's stream is dropped", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_identical_retry_joins_instead_of_running_twice() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        let u1 = url.clone();
        let first = tokio::spawn(async move {
            post(&u1, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("the first is in flight", || flights(&app).len() == 1).await;
        let u2 = url.clone();
        let retry = tokio::spawn(async move {
            post(&u2, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("the retry joins", || {
            flights(&app).iter().any(|f| f.joined_to.is_some())
        })
        .await;
        let fl = flights(&app);
        let leader = fl.iter().find(|f| f.joined_to.is_none()).unwrap();
        assert_eq!(leader.joiners, 1);
        a.open.send(true).unwrap();
        let (t1, t2) = (first.await.unwrap(), retry.await.unwrap());
        assert_eq!(t1, t2, "the retry gets the same answer, byte for byte");
        assert!(t1.contains("answer from studio"));
        assert_eq!(
            a.received.load(Ordering::SeqCst),
            1,
            "the model did the work once"
        );
        until("both rows", || finished(&app).len() == 2).await;
        let joined = finished(&app).into_iter().find(|f| f.cache_hit).unwrap();
        assert!(joined.note.starts_with("answered by #"), "{}", joined.note);
        assert_eq!(joined.output, 3);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_hung_up_request_waits_for_its_retry_then_answers_it() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 30).await;
        let u1 = url.clone();
        // The agent times out: its connection goes away mid-prefill.
        let first = tokio::spawn(async move {
            let r = post(&u1, body("slow", true)).send().await.unwrap();
            drop(r);
        });
        first.await.unwrap();
        until("Toktail notices the agent left", || {
            flights(&app).first().is_some_and(|f| f.agent_gone)
        })
        .await;
        assert!(flights(&app)[0].stops_in_secs.is_some_and(|s| s > 20));
        // ...and retries the same request.
        let retry = tokio::spawn(async move {
            post(&url, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("the retry joins the original", || {
            flights(&app).iter().any(|f| f.joined_to.is_some())
        })
        .await;
        a.open.send(true).unwrap();
        let text = retry.await.unwrap();
        assert!(text.contains("answer from studio"), "{text}");
        assert_eq!(
            a.received.load(Ordering::SeqCst),
            1,
            "the prefill wasn't thrown away"
        );
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_hung_up_request_nobody_wants_is_stopped_after_the_grace() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 1).await;
        let r = post(&url, body("slow", true)).send().await.unwrap();
        drop(r);
        until("stopped, and the model server's connection closed", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        until("recorded", || {
            finished(&app)
                .iter()
                .any(|f| f.note == "agent hung up; stopped")
        })
        .await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_hang_up_is_noticed_before_the_server_answers_at_all() {
        // No head yet: the server holds everything until prefill is done.
        let (a, au) = start_fake("studio", false).await;
        let (_b, bu) = start_fake("mini", false).await;
        let (app, url) = start_proxy(&au, &bu, 1).await;
        let req = tokio::spawn(post(&url, body("slow", true)).send());
        until("in flight", || flights(&app).len() == 1).await;
        req.abort();
        until("Toktail notices the agent left", || {
            flights(&app).first().is_none_or(|f| f.agent_gone)
        })
        .await;
        until("and stops it after the grace", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_join_and_bulk_rules() {
        let (a, au) = start_fake("studio", true).await;
        let (_b, bu) = start_fake("mini", true).await;
        let (app, url) = start_proxy(&au, &bu, 0).await;
        app.config.write().flights.join_duplicates = false;
        let u1 = url.clone();
        let first = tokio::spawn(async move {
            post(&u1, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("first", || flights(&app).len() == 1).await;
        let u2 = url.clone();
        let dup = tokio::spawn(async move {
            post(&u2, body("slow", true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        until("both running", || {
            flights(&app).len() == 2 && a.received.load(Ordering::SeqCst) == 2
        })
        .await;
        let fl = flights(&app);
        let later = fl.iter().max_by_key(|f| f.id).unwrap();
        let earlier = fl.iter().min_by_key(|f| f.id).unwrap();
        assert_eq!(
            later.duplicate_of,
            Some(earlier.id),
            "the portal can see it's a retry"
        );
        // A different request can't share an answer.
        let u3 = url.clone();
        let other = tokio::spawn(async move {
            post(&u3, body("fast", false))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        assert!(other.await.unwrap().contains("answer from studio"));
        app.activity
            .command(later.id, crate::flights::Cmd::Join(earlier.id))
            .unwrap();
        assert!(app
            .activity
            .command(earlier.id, crate::flights::Cmd::Join(earlier.id))
            .is_err());
        until("studio's second stream is dropped", || {
            a.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
        a.open.send(true).unwrap();
        let (t1, t2) = (first.await.unwrap(), dup.await.unwrap());
        assert_eq!(t1, t2);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
