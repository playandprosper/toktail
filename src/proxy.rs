//! Transparent reverse proxy in front of LM Studio.
//!
//! Clients keep talking to port 1234; TokenLedger forwards every request to
//! LM Studio unchanged and, for inference endpoints, reads the token usage
//! from the response on its way back. Only counts, timings, the model name
//! and a SHA-256 fingerprint of the prompt are recorded. Request and response
//! bodies are never written to disk or to the log.

use crate::app::App;
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
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_REQUEST_BYTES: usize = 512 * 1024 * 1024;

struct Ctx {
    app: Arc<App>,
    client: reqwest::Client,
    upstream: String,
}

/// What we know about a counted request before LM Studio answers.
struct Tracked {
    endpoint: String,
    request_model: Option<String>,
    fingerprint: String,
    streamed: bool,
    strip_injected_usage: bool,
    started: Instant,
}

pub async fn run(app: Arc<App>) {
    let (listen, upstream) = {
        let c = app.config.read();
        (
            c.proxy.listen.clone(),
            c.proxy.upstream.trim_end_matches('/').to_string(),
        )
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("http client");
    let ctx = Arc::new(Ctx {
        app: app.clone(),
        client,
        upstream: upstream.clone(),
    });
    let router = Router::new().fallback(handle).with_state(ctx);
    let mut warned = false;
    loop {
        match tokio::net::TcpListener::bind(&listen).await {
            Ok(listener) => {
                log::info!("proxy listening on {listen}, forwarding to {upstream}");
                app.set_proxy_status(true, format!("Proxy {listen} → {upstream}"));
                warned = false;
                if let Err(e) = axum::serve(listener, router.clone()).await {
                    log::error!("proxy stopped: {e}");
                    app.set_proxy_status(false, format!("Proxy stopped: {e}"));
                }
            }
            Err(e) => {
                if !warned {
                    log::warn!(
                        "cannot bind proxy to {listen}: {e} (is LM Studio still on that port?)"
                    );
                    warned = true;
                }
                app.set_proxy_status(
                    false,
                    format!("Proxy can't use {listen} ({e}). Move LM Studio to {upstream}."),
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
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

fn forward_request_headers(src: &HeaderMap, tracked: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in src {
        if is_hop_by_hop(k) || k == header::HOST || k == header::CONTENT_LENGTH {
            continue;
        }
        // Ask for an uncompressed body on counted endpoints so it can be read.
        if tracked && k == header::ACCEPT_ENCODING {
            continue;
        }
        out.append(k.clone(), v.clone());
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
            "message": format!("TokenLedger could not reach LM Studio at {upstream}: {e}"),
            "type": "tokenledger_upstream_unreachable"
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

    let tracked = kind.and_then(|kind| prepare(kind, &path, &mut body_bytes, started));

    let url = format!("{}{}", ctx.upstream, path_q);
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let upstream_resp = ctx
        .client
        .request(method, &url)
        .headers(forward_request_headers(&parts.headers, tracked.is_some()))
        .body(body_bytes)
        .send()
        .await;
    let resp = match upstream_resp {
        Ok(r) => r,
        Err(e) => return bad_gateway(&ctx.upstream, e),
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
        stream_and_count(ctx, t, status, headers, resp)
    } else {
        // Non-streamed JSON (or an error body): read it whole, count, return it.
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return bad_gateway(&ctx.upstream, e),
        };
        let mut u = Usage::default();
        let mut model = None;
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            usage::scan_usage(&v, &mut u);
            model = usage::response_model(&v);
        }
        let duration = t.started.elapsed();
        record(&ctx.app, &t, model, status.as_u16(), u, 0, None, duration);
        let mut r = Response::new(Body::from(bytes));
        *r.status_mut() = status;
        *r.headers_mut() = headers;
        r
    }
}

/// Reads model/stream/prompt from the request and, for streamed OpenAI-style
/// calls, asks LM Studio to include usage in the stream.
fn prepare(kind: EndpointKind, path: &str, body: &mut Bytes, started: Instant) -> Option<Tracked> {
    let Ok(mut v) = serde_json::from_slice::<Value>(body) else {
        // Not JSON: still count the call, without a fingerprint.
        return Some(Tracked {
            endpoint: path.to_string(),
            request_model: None,
            fingerprint: String::new(),
            streamed: false,
            strip_injected_usage: false,
            started,
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
    })
}

fn stream_and_count(
    ctx: Arc<Ctx>,
    t: Tracked,
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    tokio::spawn(async move {
        let mut tracker = SseTracker::new(t.strip_injected_usage);
        let mut upstream = resp.bytes_stream();
        let mut client_gone = false;
        let mut upstream_failed = false;
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(c) => {
                    let out = tracker.push(&c);
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
        record(
            &ctx.app,
            &t,
            tracker.model.clone(),
            code,
            tracker.usage,
            tracker.deltas,
            ttft,
            duration,
        );
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
    app: &Arc<App>,
    t: &Tracked,
    response_model: Option<String>,
    status: u16,
    u: Usage,
    deltas: u64,
    ttft: Option<Duration>,
    duration: Duration,
) {
    let (input, output, source) = if u.is_reported() {
        (u.input.unwrap_or(0), u.output.unwrap_or(0), "reported")
    } else if deltas > 0 {
        (0, deltas, "estimated")
    } else {
        (0, 0, "none")
    };
    let rec = Record {
        request_id: uuid::Uuid::new_v4().to_string(),
        timestamp: Local::now(),
        model: response_model
            .or_else(|| t.request_model.clone())
            .unwrap_or_else(|| "unknown".into()),
        endpoint: t.endpoint.clone(),
        streamed: t.streamed,
        status,
        input_tokens: input,
        output_tokens: output,
        duration_ms: duration.as_millis() as u64,
        ttft_ms: ttft.map(|d| d.as_millis() as u64),
        usage_source: source,
        prompt_sha256: t.fingerprint.clone(),
    };
    let ledger = app.ledger.clone();
    app.rt.spawn_blocking(move || {
        if let Err(e) = ledger.append(&rec) {
            log::error!("could not write CSV row: {e:#}");
        }
    });
}
