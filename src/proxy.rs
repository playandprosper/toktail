// SPDX-License-Identifier: Apache-2.0
//! Transparent reverse proxies in front of LM Studio, one per watched host.
//!
//! Each host gets its own local port. Clients talk to that port; Toktail
//! forwards every request to that host's LM Studio unchanged and, for
//! inference endpoints, reads the token usage on its way back. The CSV gets
//! counts, timings, the model name, the host and a SHA-256 fingerprint of
//! the prompt, never the prompt itself. Only when "Save prompts and
//! responses" is on are the bodies kept, in separate gzip files (encrypted
//! when encryption is on; see records.rs).

use crate::app::App;
use crate::config::HostConfig;
use crate::ledger::Record;
use crate::usage::{self, classify, EndpointKind, SseTracker, Usage};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bytes::Bytes;
use chrono::Local;
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
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

/// What we know about a counted request before LM Studio answers.
struct Tracked {
    endpoint: String,
    request_model: Option<String>,
    fingerprint: String,
    streamed: bool,
    strip_injected_usage: bool,
    started: Instant,
    /// The request exactly as the client sent it, when saving is on.
    original: Option<Bytes>,
}

/// Where a host's requests go: its LM Studio address, normalized.
fn upstream_of(app: &App, host: &str) -> Option<String> {
    let c = app.config.read();
    let raw = c.host(host)?.upstream.clone();
    Some(normalize_upstream(&raw).unwrap_or(raw))
}

/// The address to reach a proxy listening on `listen` from this Mac.
pub fn local_url(listen: &str) -> String {
    let port = listen.rsplit(':').next().unwrap_or("1234");
    format!("http://127.0.0.1:{port}")
}

/// Accepts "studio.local:1234", "http://10.0.0.5:1234/v1/", "https://lm.example.com"
/// and returns "scheme://host[:port][/path]" without a trailing slash or /v1.
pub fn normalize_upstream(raw: &str) -> Result<String, String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return Err("Enter LM Studio's address, like http://127.0.0.1:1235".into());
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

struct Running {
    listen: String,
    stop: Arc<Notify>,
    done: tokio::task::JoinHandle<()>,
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
                        let served = axum::serve(listener, router)
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

/// Result of asking LM Studio for its models.
pub struct Probe {
    pub ok: bool,
    pub message: String,
    pub models: Vec<String>,
}

/// Asks `upstream` for `/v1/models`, with the token if given.
pub async fn probe(client: &reqwest::Client, upstream: &str, token: Option<&str>) -> Probe {
    let mut req = client
        .get(format!("{upstream}/v1/models"))
        .timeout(Duration::from_secs(6));
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
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
                    "LM Studio answered with {} model{}",
                    models.len(),
                    if models.len() == 1 { "" } else { "s" }
                ),
                models,
            }
        }
        Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => Probe {
            ok: false,
            message: if token.is_some_and(|t| !t.is_empty()) {
                "LM Studio refused the API token".into()
            } else {
                "LM Studio wants an API token".into()
            },
            models: vec![],
        },
        Ok(r) => Probe {
            ok: false,
            message: format!("LM Studio answered {}", r.status()),
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

/// Checks every host's LM Studio every 30 s (or right after Settings change).
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
                let p = probe(&client, &up, token.as_deref()).await;
                let mut s = app.status.write();
                let hs = s.hosts.entry(h.id.clone()).or_default();
                if hs.upstream.ok != p.ok || !hs.upstream.checked {
                    log::info!("[{}] LM Studio at {up}: {}", h.id, p.message);
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

fn forward_request_headers(src: &HeaderMap, tracked: bool, token: Option<&str>) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in src {
        if is_hop_by_hop(k) || k == header::HOST || k == header::CONTENT_LENGTH {
            continue;
        }
        // With a configured token, it replaces whatever key the client sent
        // (OpenAI SDKs insist on sending one, often a placeholder).
        if token.is_some() && k == header::AUTHORIZATION {
            continue;
        }
        // Ask for an uncompressed body on counted endpoints so it can be read.
        if tracked && k == header::ACCEPT_ENCODING {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    if let Some(t) = token {
        if let Ok(v) = format!("Bearer {t}").parse() {
            out.insert(header::AUTHORIZATION, v);
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

fn bad_gateway(upstream: &str, e: impl std::fmt::Display) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": format!("Toktail could not reach LM Studio at {upstream}: {e}"),
            "type": "toktail_upstream_unreachable"
        }
    });
    (StatusCode::BAD_GATEWAY, axum::Json(body)).into_response()
}

async fn handle(State(ctx): State<Arc<Ctx>>, req: Request) -> Response {
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

    let Some(up) = upstream_of(&ctx.app, &ctx.host) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "This host was removed from Toktail.",
        )
            .into_response();
    };
    if let Some(port) = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.rsplit(':').next())
        .and_then(|p| p.parse::<u16>().ok())
    {
        if loops_back(&up, port) {
            let body = serde_json::json!({"error": {"message": "Toktail's upstream for this host points back at Toktail itself. Change the LM Studio address in Settings.", "type": "toktail_loop"}});
            return (StatusCode::LOOP_DETECTED, axum::Json(body)).into_response();
        }
    }
    let saving = ctx.app.config.read().capture.enabled;
    let tracked = kind.and_then(|kind| prepare(kind, &path, &mut body_bytes, started, saving));
    // Counts this request as in flight (for Pip) until the guards are dropped.
    let in_flight = tracked
        .as_ref()
        .map(|_| (ctx.app.live.begin(), ctx.app.live_for(&ctx.host).begin()));

    let token = ctx.app.host_token(&ctx.host);
    let url = format!("{up}{path_q}");
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let upstream_resp = ctx
        .client
        .request(method, &url)
        .headers(forward_request_headers(
            &parts.headers,
            tracked.is_some(),
            token.as_deref(),
        ))
        .body(body_bytes)
        .send()
        .await;
    let resp = match upstream_resp {
        Ok(r) => r,
        Err(e) => return bad_gateway(&up, e),
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = forward_response_headers(resp.headers());
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

    if is_sse {
        stream_and_count(ctx, t, status, headers, resp, in_flight)
    } else {
        // Non-streamed JSON (or an error body): read it whole, count, return it.
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return bad_gateway(&up, e),
        };
        let mut u = Usage::default();
        let mut model = None;
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            usage::scan_usage(&v, &mut u);
            model = usage::response_model(&v);
        }
        let duration = t.started.elapsed();
        let captured = t.original.is_some().then(|| bytes.clone());
        record(
            &ctx,
            t,
            model,
            status.as_u16(),
            u,
            0,
            None,
            duration,
            captured,
        );
        drop(in_flight);
        let mut r = Response::new(Body::from(bytes));
        *r.status_mut() = status;
        *r.headers_mut() = headers;
        r
    }
}

/// Reads model/stream/prompt from the request and, for streamed OpenAI-style
/// calls, asks LM Studio to include usage in the stream.
fn prepare(
    kind: EndpointKind,
    path: &str,
    body: &mut Bytes,
    started: Instant,
    saving: bool,
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
        });
    };
    let fingerprint = usage::prompt_fingerprint(&v).unwrap_or_default();
    let request_model = v.get("model").and_then(Value::as_str).map(str::to_string);
    let streamed = v.get("stream").and_then(Value::as_bool) == Some(true);
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
    })
}

fn stream_and_count(
    ctx: Arc<Ctx>,
    t: Tracked,
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
    in_flight: Option<(crate::live::ActiveGuard, crate::live::ActiveGuard)>,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    tokio::spawn(async move {
        let mut tracker = SseTracker::new(t.strip_injected_usage);
        if t.original.is_some() {
            tracker = tracker.collect_text();
        }
        let host_live = ctx.app.live_for(&ctx.host);
        let mut upstream = resp.bytes_stream();
        let mut client_gone = false;
        let mut upstream_failed = false;
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(c) => {
                    let before = tracker.deltas;
                    let out = tracker.push(&c);
                    if tracker.deltas > before {
                        ctx.app.live.add(tracker.deltas - before, 0);
                        host_live.add(tracker.deltas - before, 0);
                    }
                    if !out.is_empty() && tx.send(Ok(Bytes::from(out))).await.is_err() {
                        client_gone = true;
                        break;
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
            });
            Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
        });
        let (model, usage, deltas) = (tracker.model.clone(), tracker.usage, tracker.deltas);
        record(
            &ctx, t, model, code, usage, deltas, ttft, duration, captured,
        );
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
) {
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
    let record = match (t.original, response) {
        (Some(p), Some(r)) if r.len() <= MAX_CAPTURE_BYTES => {
            crate::records::capture(app, &ctx.host, now, p.to_vec(), r.to_vec())
        }
        _ => String::new(),
    };
    let rec = Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: now,
        model: response_model
            .or(t.request_model)
            .unwrap_or_else(|| "unknown".into()),
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
        record,
    };
    let ledger = app.ledger.clone();
    app.rt.spawn_blocking(move || {
        if let Err(e) = ledger.append(&rec) {
            log::error!("could not write CSV row: {e:#}");
        }
    });
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
        let out = forward_request_headers(&h, true, Some("secret"));
        assert_eq!(out.get(header::AUTHORIZATION).unwrap(), "Bearer secret");
        assert!(out.get(header::HOST).is_none());
        let out = forward_request_headers(&h, true, None);
        assert_eq!(out.get(header::AUTHORIZATION).unwrap(), "Bearer lm-studio");
    }
}
