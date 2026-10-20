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
    token_id: String,
    client: String,
    /// Set when the answer should be stored in the prompt cache.
    cache_key: Option<String>,
}

/// How the credential is forwarded.
enum Forward {
    /// Whatever the client sent goes through unchanged.
    KeepClient,
    /// The client's credential (if any) is dropped; this one is sent instead.
    Replace { header: &'static str, value: String },
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
    _host: crate::live::ActiveGuard,
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
        if matches!(auth, Forward::Replace { .. }) && is_credential_header(k) {
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
    let cache_key = if cache_on && kind.is_some() && toktail_client != commentary::CLIENT {
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
    // Counts this request as in flight (for Pip and the live feed) until the
    // guards are dropped, however the request ends.
    let in_flight = tracked.as_ref().map(|t| InFlight {
        _all: ctx.app.live.begin(),
        _host: ctx.app.live_for(&ctx.host).begin(),
        flight: ctx.app.activity.begin(
            &ctx.host,
            t.request_model.as_deref(),
            &t.endpoint,
            t.streamed,
        ),
    });

    let url = format!("{up}{path_q}");
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let upstream_resp = ctx
        .client
        .request(method, &url)
        .headers(forward_request_headers(
            &parts.headers,
            tracked.is_some(),
            &auth,
        ))
        .body(body_bytes)
        .send()
        .await;
    drop(auth);
    let resp = match upstream_resp {
        Ok(r) => r,
        Err(e) => {
            // Count the failure, so a host that stops answering shows up as
            // errors on the dashboard instead of silence.
            if let Some(t) = tracked {
                let duration = t.started.elapsed();
                record(
                    &ctx,
                    t,
                    None,
                    502,
                    Usage::default(),
                    0,
                    None,
                    duration,
                    None,
                );
            }
            drop(in_flight);
            return bad_gateway(kind_name(&h), &up, e);
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = forward_response_headers(resp.headers());
    let is_sse = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/event-stream"));

    let Some(t) = tracked else {
        // Not an inference call: stream it straight through.
        let mut r = Response::new(Body::from_stream(resp.bytes_stream()));
        *r.status_mut() = status;
        *r.headers_mut() = headers;
        return r;
    };
    headers.insert(
        "x-toktail-cache",
        HeaderValue::from_static(match (cache_on, cache_mode) {
            (false, _) => "off",
            (true, cache::Mode::Bypass) => "bypass",
            (true, cache::Mode::Refresh) => "refresh",
            (true, cache::Mode::Use) => "miss",
        }),
    );

    if is_sse {
        stream_and_count(ctx, t, status, headers, resp, in_flight)
    } else {
        // Non-streamed JSON (or an error body): read it whole, count, return it.
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return bad_gateway(kind_name(&h), &up, e),
        };
        let mut u = Usage::default();
        let mut model = None;
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            usage::scan_usage(&v, &mut u);
            model = usage::response_model(&v);
        }
        let duration = t.started.elapsed();
        if let (Some(f), Some(m)) = (in_flight.as_ref(), model.as_deref()) {
            f.flight.set_model(m);
        }
        let captured = Some(bytes.clone());
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        record_and_cache(
            &ctx,
            t,
            model,
            status.as_u16(),
            u,
            0,
            None,
            duration,
            captured,
            CacheBody {
                content_type,
                streamed: false,
                bytes: bytes.clone(),
            },
        );
        drop(in_flight);
        let mut r = Response::new(Body::from(bytes));
        *r.status_mut() = status;
        *r.headers_mut() = headers;
        r
    }
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
            token_id,
            client,
            cache_key,
        });
    };
    let fingerprint = usage::prompt_fingerprint(&v).unwrap_or_default();
    let request_model = v.get("model").and_then(Value::as_str).map(str::to_string);
    let streamed = v.get("stream").and_then(Value::as_bool) == Some(true);
    let detected = commentary::detect(headers, &v);
    let turn = commentary::last_user_turn(&v);
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
        token_id,
        client,
        cache_key,
    })
}

fn stream_and_count(
    ctx: Arc<Ctx>,
    t: Tracked,
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
    in_flight: Option<InFlight>,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_string();
    tokio::spawn(async move {
        // Saving keeps the whole answer; otherwise only enough of it to tell
        // a refusal from an answer (the character count stays exact).
        let cap = if t.original.is_some() {
            MAX_CAPTURE_BYTES / 2
        } else {
            16 * 1024
        };
        let mut tracker = SseTracker::new(t.strip_injected_usage).collect_text(cap);
        let mut named = false;
        let host_live = ctx.app.live_for(&ctx.host);
        let mut upstream = resp.bytes_stream();
        let mut client_gone = false;
        let mut upstream_failed = false;
        // What the client receives, kept for the cache while it fits.
        let mut sent: Option<Vec<u8>> = t.cache_key.is_some().then(Vec::new);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(c) => {
                    let before = tracker.deltas;
                    let out = tracker.push(&c);
                    if tracker.deltas > before {
                        ctx.app.live.add(tracker.deltas - before, 0);
                        host_live.add(tracker.deltas - before, 0);
                    }
                    if let Some(f) = in_flight.as_ref() {
                        f.flight.progress(tracker.deltas, tracker.first_token_at);
                        if !named {
                            if let Some(m) = tracker.model.as_deref() {
                                f.flight.set_model(m);
                                named = true;
                            }
                        }
                    }
                    if !out.is_empty() {
                        if let Some(s) = sent.as_mut() {
                            if s.len() + out.len() <= cache::MAX_ENTRY_BYTES {
                                s.extend_from_slice(&out);
                            } else {
                                sent = None;
                            }
                        }
                        if tx.send(Ok(Bytes::from(out))).await.is_err() {
                            client_gone = true;
                            break;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("upstream stream ended with an error: {e}");
                    upstream_failed = true;
                    break;
                }
            }
        }
        if !client_gone {
            let rest = tracker.finish();
            if !rest.is_empty() {
                if let Some(s) = sent.as_mut() {
                    s.extend_from_slice(&rest);
                }
                let _ = tx.send(Ok(Bytes::from(rest))).await;
            }
        }
        drop(tx);
        let code = if client_gone {
            499
        } else if upstream_failed {
            502
        } else {
            status.as_u16()
        };
        let ttft = tracker.first_token_at.map(|f| f.duration_since(t.started));
        let duration = t.started.elapsed();
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
        let cache_body = sent.filter(|_| code == 200).map(|bytes| CacheBody {
            content_type: content_type.clone(),
            streamed: true,
            bytes: Bytes::from(bytes),
        });
        match cache_body {
            Some(cb) => record_and_cache(
                &ctx, t, model, code, usage, deltas, ttft, duration, captured, cb,
            ),
            None => {
                record(
                    &ctx, t, model, code, usage, deltas, ttft, duration, captured,
                );
            }
        }
        drop(in_flight);
    });
    let body_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut r = Response::new(Body::from_stream(body_stream));
    *r.status_mut() = status;
    *r.headers_mut() = headers;
    r
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
) {
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
    app.live_for(&ctx.host)
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
        host: ctx.host.clone(),
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
    });
    // Pip's own requests (chat, commentary) are counted but never commented on.
    if t.client.is_empty() && status == 200 && rec.outcome == "complete" {
        if let Some(h) = app.config.read().host(&ctx.host).cloned() {
            let answer = response
                .as_deref()
                .map(|r| {
                    crate::records::summarize_response(r)["content"]
                        .as_str()
                        .unwrap_or("")
                        .to_string()
                })
                .unwrap_or_default();
            if let Some(c) =
                commentary::candidate(&rec.request_id, &h, &model, &t.detected, &t.turn, &answer)
            {
                commentary::offer(app, c, false);
            }
        }
    }
    // Toktail's own requests aren't saved as records.
    let (prompt, captured) = if t.client == commentary::CLIENT {
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
