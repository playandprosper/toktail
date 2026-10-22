// SPDX-License-Identifier: Apache-2.0
//! Requests in flight you can steer.
//!
//! Coding agents pile up: a long prompt is still being read when the agent
//! times out and sends it again, and subagents queue behind each other on
//! one machine. From the live feed you can:
//!
//! - **Cancel** a request. Toktail drops its connection to the model server
//!   (which stops the work there) and tells the agent it was cancelled, with
//!   HTTP 499, which agents don't retry.
//! - **Reroute** a request to another host and model, while nothing has been
//!   streamed back yet. Toktail sends the same request there and returns that
//!   answer to the agent as if it came from where it was sent. The agent's
//!   own key is never forwarded to another host: the target's stored key is
//!   used, or none.
//! - **Join** a request to an identical one already running: it gets that
//!   request's answer, streamed from the start, and nothing new is sent to a
//!   model. Identical retries join on their own (Settings → In flight).
//!
//! When an agent hangs up on a request nobody else is waiting for, Toktail
//! keeps it running for a grace period so the agent's retry can join it, then
//! stops it.
//!
//! The answer a request streams to its agent is kept in its [`Tee`] while it
//! runs, so a request that joins late still gets every byte from the start.

use crate::app::App;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::watch;

/// The most answer bytes kept for joiners; past this, nobody can join.
pub const MAX_TEE_BYTES: usize = 64 * 1024 * 1024;

/// What you asked a request to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    None,
    Cancel,
    Reroute { host: String, model: String },
    Join(u64),
}

/// The start of an answer: what joiners send before the bytes.
#[derive(Debug, Clone)]
pub struct Head {
    pub status: u16,
    pub content_type: String,
}

/// How an answer ended, for the joiners' CSV rows.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub ok: bool,
    pub host: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub outcome: String,
    pub signal: String,
    pub response_chars: u64,
}

#[derive(Default)]
struct TeeState {
    head: Option<Head>,
    chunks: Vec<Bytes>,
    bytes: usize,
    overflow: bool,
    done: Option<Summary>,
}

/// The answer as the agent receives it, kept so identical requests can be
/// answered with it.
pub struct Tee {
    state: Mutex<TeeState>,
    version: watch::Sender<u64>,
}

impl Default for Tee {
    fn default() -> Self {
        Tee {
            state: Mutex::new(TeeState::default()),
            version: watch::channel(0).0,
        }
    }
}

impl Tee {
    fn bump(&self) {
        self.version.send_modify(|v| *v += 1);
    }
    pub fn set_head(&self, status: u16, content_type: &str) {
        self.state.lock().head = Some(Head {
            status,
            content_type: content_type.to_string(),
        });
        self.bump();
    }
    pub fn push(&self, b: &[u8]) {
        if b.is_empty() {
            return;
        }
        {
            let mut s = self.state.lock();
            if s.overflow {
                return;
            }
            if s.bytes + b.len() > MAX_TEE_BYTES {
                s.overflow = true;
                s.chunks.clear();
            } else {
                s.bytes += b.len();
                s.chunks.push(Bytes::copy_from_slice(b));
            }
        }
        self.bump();
    }
    /// Ends the answer (first call wins).
    pub fn finish(&self, summary: Summary) {
        {
            let mut s = self.state.lock();
            if s.done.is_some() {
                return;
            }
            s.done = Some(summary);
        }
        self.bump();
    }
    /// Someone can still join: not overflowed, not failed.
    pub fn joinable(&self) -> bool {
        let s = self.state.lock();
        !s.overflow && s.done.as_ref().is_none_or(|d| d.ok)
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }
    /// Chunks from `from` on, and the head and ending if known.
    pub fn read(&self, from: usize) -> TeeRead {
        let s = self.state.lock();
        TeeRead {
            head: s.head.clone(),
            chunks: s
                .chunks
                .get(from..)
                .map(<[Bytes]>::to_vec)
                .unwrap_or_default(),
            done: s.done.clone(),
            overflow: s.overflow,
        }
    }
}

pub struct TeeRead {
    pub head: Option<Head>,
    pub chunks: Vec<Bytes>,
    pub done: Option<Summary>,
    pub overflow: bool,
}

/// One request's steering wheel: commands in, its answer out.
pub struct FlightControl {
    pub tee: Tee,
    cmd: watch::Sender<Cmd>,
}

impl Default for FlightControl {
    fn default() -> Self {
        FlightControl {
            tee: Tee::default(),
            cmd: watch::channel(Cmd::None).0,
        }
    }
}

impl FlightControl {
    pub fn commands(&self) -> watch::Receiver<Cmd> {
        self.cmd.subscribe()
    }
    pub fn send(&self, c: Cmd) {
        let _ = self.cmd.send(c);
    }
}

// ------------------------------------------------------------------ same input

/// Fields that change what the answer can be, besides the prompt itself.
const ANSWER_FIELDS: [&str; 6] = [
    "tools",
    "tool_choice",
    "functions",
    "function_call",
    "response_format",
    "text",
];

/// Two requests with this key ask the same thing: same host, endpoint and
/// credential, same model, same prompt (system, messages, input), same
/// tools and output format, both streamed or both not. Sampling settings
/// (temperature, max_tokens, seed, stream_options…) are left out, so an
/// agent's retry that tweaks them still matches. `None` for bodies with no
/// prompt.
pub fn same_input_key(host: &str, endpoint: &str, token_id: &str, body: &[u8]) -> Option<String> {
    use sha2::{Digest, Sha256};
    let v: Value = serde_json::from_slice(body).ok()?;
    let obj = v.as_object()?;
    let mut picked = serde_json::Map::new();
    for k in crate::usage::PROMPT_FIELDS {
        if let Some(x) = obj.get(k) {
            picked.insert(k.to_string(), x.clone());
        }
    }
    if picked.is_empty() {
        return None;
    }
    for k in ANSWER_FIELDS {
        if let Some(x) = obj.get(k) {
            picked.insert(k.to_string(), x.clone());
        }
    }
    picked.insert(
        "model".into(),
        obj.get("model").cloned().unwrap_or(Value::Null),
    );
    picked.insert(
        "stream".into(),
        Value::Bool(obj.get("stream").and_then(Value::as_bool) == Some(true)),
    );
    let mut canon = String::new();
    crate::usage::write_canonical(&Value::Object(picked), &mut canon);
    let mut h = Sha256::new();
    for part in [host, endpoint, token_id] {
        h.update(part.as_bytes());
        h.update(b"\0");
    }
    h.update(canon.as_bytes());
    Some(hex::encode(h.finalize()))
}

/// About how many tokens a request body's prompt is (for "~712K to read").
pub fn tokens_estimate(bytes: u64) -> u64 {
    // JSON overhead and escapes make ~3.6 bytes a token a fair middle.
    (bytes as f64 / 3.6).round() as u64
}

// ------------------------------------------------------------------ targets

/// Whether a host of `kind` speaks the API at `endpoint`.
pub fn compatible(kind: &str, endpoint: &str) -> bool {
    if endpoint.contains("/messages") {
        matches!(kind, "anthropic" | "lmstudio")
    } else if endpoint.contains("/chat/completions") {
        true
    } else {
        kind != "anthropic"
    }
}

/// Hosts a request at `endpoint` can be rerouted to, with their models.
pub fn targets(app: &App, endpoint: &str) -> Vec<Value> {
    let cfg = app.config.read().clone();
    let st = app.status.read().clone();
    cfg.hosts
        .iter()
        .filter(|h| h.enabled)
        .map(|h| {
            let hs = st.hosts.get(&h.id);
            let key = app.host_token(&h.id).is_some();
            let ok = compatible(&h.kind, endpoint);
            let why = if !ok {
                format!("{} doesn't speak {endpoint}", h.name)
            } else if h.is_public() && !key {
                format!(
                    "No {} key is stored for {}",
                    crate::proxy::kind_name(h),
                    h.name
                )
            } else if hs.is_some_and(|s| s.upstream.checked && !s.upstream.ok) {
                format!("{} isn't answering", h.name)
            } else {
                String::new()
            };
            json!({
                "id": h.id,
                "name": h.name,
                "kind": h.kind,
                "public": h.is_public(),
                "available": ok && (!h.is_public() || key),
                "reason": why,
                "models": hs.map(|s| s.upstream.models.clone()).unwrap_or_default(),
            })
        })
        .collect()
}

// ------------------------------------------------------------------ API

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/flights/targets", get(get_targets))
        .route("/api/flights/bulk", post(bulk))
        .route("/api/flights/{id}/cancel", post(cancel))
        .route("/api/flights/{id}/reroute", post(reroute))
        .route("/api/flights/{id}/join", post(join))
        .route("/api/flights/{id}/collapse", post(collapse))
}

#[derive(Deserialize)]
struct TargetsQ {
    endpoint: Option<String>,
}

async fn get_targets(State(app): State<Arc<App>>, Query(q): Query<TargetsQ>) -> Json<Value> {
    let ep = q.endpoint.unwrap_or_else(|| "/v1/chat/completions".into());
    refresh_models(&app).await;
    Json(json!({ "hosts": targets(&app, &ep) }))
}

/// Asks every enabled host which models it has right now (the reroute
/// dialog offers only those). A host that doesn't answer in time keeps the
/// list from its last health check.
pub async fn refresh_models(app: &Arc<App>) {
    let hosts: Vec<crate::config::HostConfig> = app
        .config
        .read()
        .hosts
        .iter()
        .filter(|h| h.enabled)
        .cloned()
        .collect();
    let client = app.chat_client.clone();
    let asks = hosts.into_iter().map(|h| {
        let app = app.clone();
        let client = client.clone();
        async move {
            let up = crate::proxy::normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone());
            let token = app.host_token(&h.id);
            let p = tokio::time::timeout(
                std::time::Duration::from_millis(2500),
                crate::proxy::probe(&client, &up, token.as_deref(), &h.kind),
            )
            .await;
            if let Ok(p) = p {
                if p.ok && !p.models.is_empty() {
                    let mut st = app.status.write();
                    let hs = st.hosts.entry(h.id.clone()).or_default();
                    hs.upstream.models = p.models;
                }
            }
        }
    });
    futures_util::future::join_all(asks).await;
}

fn answer(r: Result<String, String>) -> Response {
    match r {
        Ok(m) => Json(json!({ "ok": true, "message": m })).into_response(),
        Err(m) => (
            axum::http::StatusCode::CONFLICT,
            Json(json!({ "ok": false, "message": m })),
        )
            .into_response(),
    }
}

async fn cancel(State(app): State<Arc<App>>, Path(id): Path<u64>) -> Response {
    answer(app.activity.command(id, Cmd::Cancel))
}

#[derive(Deserialize)]
struct RerouteIn {
    host: String,
    #[serde(default)]
    model: String,
}

async fn reroute(
    State(app): State<Arc<App>>,
    Path(id): Path<u64>,
    Json(b): Json<RerouteIn>,
) -> Response {
    answer(check_target(&app, id, &b.host, &b.model).and_then(|model| {
        app.activity.command(
            id,
            Cmd::Reroute {
                host: b.host,
                model,
            },
        )
    }))
}

#[derive(Deserialize)]
struct JoinIn {
    to: u64,
}

async fn join(State(app): State<Arc<App>>, Path(id): Path<u64>, Json(b): Json<JoinIn>) -> Response {
    answer(app.activity.command(id, Cmd::Join(b.to)))
}

/// Answers every running copy of request `id`'s input with `id`'s answer.
async fn collapse(State(app): State<Arc<App>>, Path(id): Path<u64>) -> Response {
    answer(match app.activity.collapse(id) {
        Ok(0) => Err("No other copy of its input is still waiting.".into()),
        Ok(n) => Ok(format!(
            "{n} cop{} of #{id}'s input will get its answer instead of being read again.",
            if n == 1 { "y" } else { "ies" }
        )),
        Err(e) => Err(e),
    })
}

#[derive(Deserialize)]
struct BulkIn {
    ids: Vec<u64>,
    action: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    model: String,
}

async fn bulk(State(app): State<Arc<App>>, Json(b): Json<BulkIn>) -> Json<Value> {
    let mut done = 0;
    let mut errors = Vec::new();
    for id in b.ids.iter().take(500) {
        let r = match b.action.as_str() {
            "cancel" => app.activity.command(*id, Cmd::Cancel),
            "reroute" => check_target(&app, *id, &b.host, &b.model).and_then(|model| {
                app.activity.command(
                    *id,
                    Cmd::Reroute {
                        host: b.host.clone(),
                        model,
                    },
                )
            }),
            _ => Err("Unknown action.".into()),
        };
        match r {
            Ok(_) => done += 1,
            Err(e) => errors.push(format!("#{id}: {e}")),
        }
    }
    Json(json!({ "ok": errors.is_empty(), "done": done, "errors": errors }))
}

/// The target exists, is on, speaks this request's API, has a key if it
/// needs one, and has the model. Returns the model the request will use
/// there: `model`, or (when empty) the one it already asked for.
pub(crate) fn check_target(app: &App, id: u64, host: &str, model: &str) -> Result<String, String> {
    let endpoint = app
        .activity
        .endpoint_of(id)
        .ok_or("That request already finished.")?;
    let t = targets(app, &endpoint);
    let Some(h) = t.iter().find(|x| x["id"] == host) else {
        return Err("No such host, or it's turned off.".into());
    };
    if h["available"] != true {
        return Err(h["reason"].as_str().unwrap_or("Not available").to_string());
    }
    let model = match model.trim() {
        "" => app.activity.model_of(id).unwrap_or_default(),
        m => m.to_string(),
    };
    if model.is_empty() {
        return Err("Pick a model for it to run on.".into());
    }
    // Not every machine has every model: only one the target has will do.
    let models: Vec<&str> = h["models"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !models.is_empty() && !models.contains(&model.as_str()) {
        let name = h["name"].as_str().unwrap_or(host);
        let some = models
            .iter()
            .take(6)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{name} doesn't have {model}. Pick one of its models: {some}{}",
            if models.len() > 6 { "…" } else { "" }
        ));
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_match_by_input_not_by_bytes() {
        let k = |b: &str| same_input_key("studio", "/v1/chat/completions", "tk_1", b.as_bytes());
        let base = k(
            r#"{"model":"m","stream":true,"messages":[{"role":"user","content":"700K of code"}],"temperature":0.2,"max_tokens":800}"#,
        );
        assert!(base.is_some());
        // A retry that tweaks sampling settings, or asks for usage, is the same input.
        assert_eq!(
            base,
            k(
                r#"{"max_tokens":4096,"stream_options":{"include_usage":true},"messages":[{"content":"700K of code","role":"user"}],"stream":true,"model":"m","temperature":0.7,"seed":3}"#
            )
        );
        // A different prompt, model, tool set, or stream mode is not.
        assert_ne!(
            base,
            k(
                r#"{"model":"m","stream":true,"messages":[{"role":"user","content":"700K of code!"}]}"#
            )
        );
        assert_ne!(
            base,
            k(
                r#"{"model":"other","stream":true,"messages":[{"role":"user","content":"700K of code"}]}"#
            )
        );
        assert_ne!(
            base,
            k(
                r#"{"model":"m","stream":false,"messages":[{"role":"user","content":"700K of code"}]}"#
            )
        );
        assert_ne!(
            base,
            k(
                r#"{"model":"m","stream":true,"tools":[{"type":"function"}],"messages":[{"role":"user","content":"700K of code"}]}"#
            )
        );
        // ...nor from another key or host.
        assert_ne!(base, same_input_key("studio", "/v1/chat/completions", "tk_2", br#"{"model":"m","stream":true,"messages":[{"role":"user","content":"700K of code"}]}"#));
        assert_ne!(base, same_input_key("mini", "/v1/chat/completions", "tk_1", br#"{"model":"m","stream":true,"messages":[{"role":"user","content":"700K of code"}]}"#));
        assert!(
            k(r#"{"model":"m"}"#).is_none(),
            "no prompt, nothing to match"
        );
        assert_eq!(tokens_estimate(2_563_200), 712_000);
    }

    #[test]
    fn api_shapes_decide_targets() {
        assert!(compatible("lmstudio", "/v1/chat/completions"));
        assert!(compatible("anthropic", "/v1/messages"));
        assert!(compatible("lmstudio", "/v1/messages"));
        assert!(!compatible("openai", "/v1/messages"));
        assert!(!compatible("anthropic", "/v1/responses"));
        assert!(compatible("other", "/v1/completions"));
    }

    #[tokio::test]
    async fn the_tee_replays_from_the_start_and_wakes_joiners() {
        let tee = Tee::default();
        let mut rx = tee.subscribe();
        tee.set_head(200, "text/event-stream");
        tee.push(b"data: 1\n\n");
        let r = tee.read(0);
        assert_eq!(r.head.unwrap().status, 200);
        assert_eq!(r.chunks.len(), 1);
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();
        let waiter = tokio::spawn(async move { rx.changed().await.is_ok() });
        tee.push(b"data: 2\n\n");
        assert!(waiter.await.unwrap(), "a push wakes a waiting joiner");
        assert_eq!(tee.read(1).chunks, vec![Bytes::from_static(b"data: 2\n\n")]);
        tee.finish(Summary {
            ok: true,
            ..Default::default()
        });
        tee.finish(Summary::default());
        assert!(tee.read(2).done.unwrap().ok, "the first ending wins");
        assert!(tee.joinable());
        let big = Tee::default();
        big.push(&vec![0u8; MAX_TEE_BYTES + 1]);
        assert!(!big.joinable());
    }
}
