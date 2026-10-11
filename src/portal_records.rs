// SPDX-License-Identifier: Apache-2.0
//! The Prompts page: browse, read, annotate and delete saved prompts and
//! responses, and chat with a host through its counting proxy.
//!
//! Records are addressed by the CSV row's request id. Their files are read
//! from this Mac first and from S3 when the local copy is gone, so the page
//! works the same whichever side holds them.

use crate::app::App;
use crate::ledger::{self, percentile_rank, Row};
use crate::portal::Q;
use crate::portal_settings::bad_request;
use crate::records::{self, Kind, Note};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/records", get(list))
        .route("/api/records/delete", post(bulk_delete))
        .route("/api/records/{id}", get(detail).delete(delete_one))
        .route("/api/records/{id}/note", post(set_note))
        .route("/api/records/{id}/{kind}", get(content))
        .route("/api/chat", post(chat))
}

#[derive(Debug, Deserialize)]
struct ListQ {
    from: Option<String>,
    to: Option<String>,
    models: Option<String>,
    hosts: Option<String>,
    limit: Option<usize>,
    /// Free text: model, fingerprint, request id, title, tag or note.
    search: Option<String>,
    starred: Option<bool>,
    /// "saved" (default): only rows with saved files; "all": every request.
    scope: Option<String>,
    sort: Option<String>,
    offset: Option<usize>,
}

async fn list(State(app): State<Arc<App>>, Query(lq): Query<ListQ>) -> Response {
    let f = Q {
        from: lq.from.clone(),
        to: lq.to.clone(),
        models: lq.models.clone(),
        hosts: lq.hosts.clone(),
        limit: None,
    }
    .filter();
    let notes = app.records.notes_snapshot();
    let search = lq
        .search
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let only_saved = lq.scope.as_deref() != Some("all");
    let starred = lq.starred == Some(true);
    let limit = lq.limit.unwrap_or(50).min(500);
    let offset = lq.offset.unwrap_or(0);
    let sort = lq.sort.clone().unwrap_or_else(|| "newest".into());
    let keep = |r: &Row| {
        if only_saved && r.record.is_none() {
            return false;
        }
        let note = r.record.as_deref().and_then(|s| notes.get(s));
        if starred && !note.is_some_and(|n| n.starred) {
            return false;
        }
        if let Some(q) = &search {
            let hit = r.model.to_lowercase().contains(q.as_str())
                || r.host.to_lowercase().contains(q.as_str())
                || r.request_id.starts_with(q.as_str())
                || (q.len() >= 6 && r.fp_hex().starts_with(q.as_str()))
                || r.record.as_deref().is_some_and(|s| s.contains(q.as_str()))
                || note.is_some_and(|n| {
                    n.title.to_lowercase().contains(q.as_str())
                        || n.notes.to_lowercase().contains(q.as_str())
                        || n.tags.iter().any(|t| t.to_lowercase() == *q)
                });
            if !hit {
                return false;
            }
        }
        true
    };
    let (total, rows) = app
        .ledger
        .with_data(|d| ledger::page(d, &f, keep, &sort, offset, limit));
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let note = r.record.as_deref().and_then(|s| notes.get(s)).cloned();
            json!({ "row": r, "note": note })
        })
        .collect();
    Json(json!({ "total": total, "offset": offset, "rows": rows, "saving": app.config.read().capture.enabled })).into_response()
}

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, msg.to_string()).into_response()
}

/// One metric for a record, with where it ranks.
fn rank(
    values_all: &mut Vec<f64>,
    values_model: &mut Vec<f64>,
    v: Option<f64>,
    higher_is_better: bool,
) -> serde_json::Value {
    let Some(v) = v else {
        return serde_json::Value::Null;
    };
    let med = |xs: &mut Vec<f64>| -> Option<f64> {
        if xs.is_empty() {
            return None;
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        Some(xs[xs.len() / 2])
    };
    json!({
        "value": v,
        "pct_all": percentile_rank(values_all, v),
        "pct_model": percentile_rank(values_model, v),
        "n_all": values_all.len(),
        "n_model": values_model.len(),
        "median_all": med(values_all),
        "median_model": med(values_model),
        "higher_is_better": higher_is_better,
    })
}

async fn detail(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(row) = app.ledger.find(&id) else {
        return not_found("No request with that id.");
    };
    let a = app.clone();
    let r2 = row.clone();
    // Percentiles over the whole history; runs off the async threads.
    let perf = tokio::task::spawn_blocking(move || {
        a.ledger.with_data(|d| {
            let mut v: [(Vec<f64>, Vec<f64>); 5] = Default::default();
            let mut same_prompt = 0u64;
            for x in d.rows() {
                if x.status >= 400 {
                    continue;
                }
                let same = x.model == r2.model;
                let mut push = |i: usize, val: Option<f64>| {
                    if let Some(val) = val {
                        v[i].0.push(val);
                        if same {
                            v[i].1.push(val);
                        }
                    }
                };
                push(0, x.tokens_per_sec());
                push(1, x.ttft_ms.map(|t| t as f64));
                push(2, (x.output > 0).then_some(x.output as f64));
                push(3, (x.input > 0).then_some(x.input as f64));
                push(4, Some(x.duration_ms as f64));
                if x.fp.is_some() && x.fp == r2.fp {
                    same_prompt += 1;
                }
            }
            let [mut tps, mut ttft, mut out, mut inp, mut dur] = v;
            json!({
                "tokens_per_sec": rank(&mut tps.0, &mut tps.1, r2.tokens_per_sec(), true),
                "ttft_ms": rank(&mut ttft.0, &mut ttft.1, r2.ttft_ms.map(|t| t as f64), false),
                "output": rank(&mut out.0, &mut out.1, (r2.output > 0).then_some(r2.output as f64), true),
                "input": rank(&mut inp.0, &mut inp.1, (r2.input > 0).then_some(r2.input as f64), true),
                "duration_ms": rank(&mut dur.0, &mut dur.1, Some(r2.duration_ms as f64), false),
                "same_prompt_count": same_prompt,
            })
        })
    })
    .await
    .unwrap_or(serde_json::Value::Null);
    let stem = row.record.as_deref().map(String::from);
    let (note, avail) = match &stem {
        Some(s) => (
            app.records.note(s),
            Some(records::availability(&app, s).await),
        ),
        None => (None, None),
    };
    let host_name = app
        .config
        .read()
        .host(&row.host)
        .map(|h| h.name.clone())
        .unwrap_or_else(|| row.host.to_string());
    Json(json!({
        "row": ledger::RecentRow::from(&row),
        "host_name": host_name,
        "fingerprint": row.fp_hex(),
        "stem": stem,
        "files": stem.as_ref().map(|s| json!({
            "prompt": format!("{s}.prompt"),
            "response": format!("{s}.response"),
        })),
        "note": note,
        "availability": avail,
        "performance": perf,
        "encryption": app.config.read().encryption.mode,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ContentQ {
    /// Download the original bytes instead of the readable summary.
    raw: Option<bool>,
}

async fn content(
    State(app): State<Arc<App>>,
    Path((id, kind)): Path<(String, String)>,
    Query(cq): Query<ContentQ>,
) -> Response {
    let Some(kind) = Kind::parse(&kind) else {
        return not_found("Unknown file kind.");
    };
    let Some(row) = app.ledger.find(&id) else {
        return not_found("No request with that id.");
    };
    let Some(stem) = row.record.as_deref().map(String::from) else {
        return not_found("This request's prompt and response weren't saved.");
    };
    let bytes = match records::read(&app, &stem, kind).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    };
    if cq.raw == Some(true) {
        let name = format!(
            "{}.{}.json",
            stem.rsplit('/').next().unwrap_or("record"),
            kind.ext()
        );
        return (
            [
                (header::CONTENT_TYPE, "application/json".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            bytes,
        )
            .into_response();
    }
    let v = match kind {
        Kind::Prompt => records::summarize_prompt(&bytes),
        Kind::Response => records::summarize_response(&bytes),
    };
    Json(json!({ "kind": kind.ext(), "bytes": bytes.len(), "content": v })).into_response()
}

async fn set_note(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(mut note): Json<Note>,
) -> Response {
    let Some(stem) = app
        .ledger
        .find(&id)
        .and_then(|r| r.record.map(String::from))
    else {
        return not_found("This request has no saved files to annotate.");
    };
    note.title = note.title.trim().chars().take(200).collect();
    note.notes = note.notes.chars().take(20_000).collect();
    note.tags = note
        .tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && t.len() <= 40)
        .take(20)
        .collect();
    let a = app.clone();
    let n2 = note.clone();
    match tokio::task::spawn_blocking(move || records::set_note(&a, &stem, n2)).await {
        Ok(Ok(())) => {
            if app.config.read().s3.enabled {
                app.sync_now.notify_one();
            }
            Json(json!({ "note": note })).into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteQ {
    /// Also remove the request from the token history (the CSV row).
    #[serde(default)]
    history: bool,
}

#[derive(Debug, Deserialize)]
struct BulkIn {
    ids: Vec<String>,
    #[serde(default)]
    history: bool,
}

async fn delete_one(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQ>,
) -> Response {
    delete_many(&app, vec![id], q.history).await
}

async fn bulk_delete(State(app): State<Arc<App>>, Json(b): Json<BulkIn>) -> Response {
    if b.ids.len() > 10_000 {
        return bad_request("Delete at most 10,000 at a time.");
    }
    delete_many(&app, b.ids, b.history).await
}

/// Deletes saved files (here and in S3) and, if asked, the CSV rows.
async fn delete_many(app: &Arc<App>, ids: Vec<String>, history: bool) -> Response {
    let mut files = 0;
    let mut errors = Vec::new();
    for id in &ids {
        if let Some(stem) = app.ledger.find(id).and_then(|r| r.record.map(String::from)) {
            match records::delete_files(app, &stem).await {
                Ok(()) => files += 1,
                Err(e) => errors.push(format!("{id}: {e:#}")),
            }
        }
    }
    let mut rows = 0;
    if !history {
        // Keep the counts, but the row no longer points at files.
        let set: HashSet<String> = ids.iter().cloned().collect();
        let a = app.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || a.ledger.forget_records(&set)).await
        {
            errors.push(format!("{e:#}"));
        }
    }
    if history {
        let set: HashSet<String> = ids.into_iter().collect();
        let a = app.clone();
        match tokio::task::spawn_blocking(move || a.ledger.delete_rows(&set)).await {
            Ok(Ok(n)) => rows = n,
            Ok(Err(e)) => errors.push(format!("{e:#}")),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if app.config.read().s3.enabled {
        app.sync_now.notify_one();
    }
    Json(json!({ "deleted_files": files, "deleted_rows": rows, "errors": errors })).into_response()
}

#[derive(Debug, Deserialize)]
struct ChatIn {
    host: String,
    model: String,
    messages: Vec<serde_json::Value>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
}

/// Sends a chat through the host's own Toktail port, so it is counted (and
/// saved, when saving is on) exactly like any other app's request. The
/// stream is passed straight back to the page.
async fn chat(State(app): State<Arc<App>>, Json(c): Json<ChatIn>) -> Response {
    let Some(h) = app.config.read().host(&c.host).cloned() else {
        return bad_request("Pick a host.");
    };
    if !h.enabled {
        return bad_request(format!("{} is turned off in Settings.", h.name));
    }
    if c.model.trim().is_empty() {
        return bad_request("Pick a model.");
    }
    if c.messages.is_empty() || c.messages.len() > 500 {
        return bad_request("Nothing to send.");
    }
    let mut body = json!({ "model": c.model, "messages": c.messages, "stream": true });
    if let Some(t) = c
        .temperature
        .filter(|t| t.is_finite() && (0.0..=2.0).contains(t))
    {
        body["temperature"] = json!(t);
    }
    if let Some(m) = c.max_tokens.filter(|m| *m > 0) {
        body["max_tokens"] = json!(m);
    }
    let url = format!("{}/v1/chat/completions", crate::proxy::local_url(&h.listen));
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    let resp = client
        .post(&url)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Toktail-Client", "portal-chat")
        .body(serde_json::to_vec(&body).unwrap_or_default())
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = r
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/event-stream")
                .to_string();
            let mut out = Response::new(Body::from_stream(r.bytes_stream()));
            *out.status_mut() = status;
            out.headers_mut().insert(
                header::CONTENT_TYPE,
                ct.parse()
                    .unwrap_or(header::HeaderValue::from_static("text/event-stream")),
            );
            out
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Couldn't reach Toktail's port for {} ({url}): {e}", h.name),
        )
            .into_response(),
    }
}
