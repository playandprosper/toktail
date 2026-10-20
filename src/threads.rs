// SPDX-License-Identifier: Apache-2.0
//! Chat threads: the conversations from Prompts → New chat, kept apart from
//! the proxy's prompt history.
//!
//! Each thread is one sealed file, `<base>/threads/<id>.tkt`, holding its
//! messages and settings (encrypted like saved records when encryption is
//! on; gzip otherwise). A small plain index, `threads/index.json`, carries
//! only titles, times and counts so the sidebar lists without decrypting.
//!
//! The model calls a thread makes go through Toktail's own proxy like any
//! other request, tagged with the thread id as their session, so their
//! prompts are deduplicated into one chain per thread (chain.rs).

use crate::app::App;
use anyhow::{bail, Context, Result};
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub const MAX_THREADS: usize = 2000;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub system: String,
    #[serde(default = "yes")]
    pub context: bool,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// As the portal keeps them: role, content, and whatever it attached.
    #[serde(default)]
    pub messages: Vec<Value>,
}

fn yes() -> bool {
    true
}

/// What the sidebar shows without opening the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub pinned: bool,
    pub created: String,
    pub updated: String,
    pub messages: usize,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub model: String,
}

pub fn dir(app: &App) -> PathBuf {
    app.paths.base.join("threads")
}

fn index_path(app: &App) -> PathBuf {
    dir(app).join("index.json")
}

fn thread_path(app: &App, id: &str) -> PathBuf {
    dir(app).join(format!("{id}.tkt"))
}

pub fn valid_id(id: &str) -> bool {
    id.len() >= 4
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn new_id() -> String {
    format!("t_{}", hex::encode(crate::crypto::random_bytes::<8>()))
}

pub fn load_index(app: &App) -> Vec<Entry> {
    std::fs::read_to_string(index_path(app))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_index(app: &App, entries: &[Entry]) -> Result<()> {
    std::fs::create_dir_all(dir(app))?;
    let p = index_path(app);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

fn preview_of(t: &Thread) -> String {
    t.messages
        .iter()
        .rev()
        .find_map(|m| m.get("content").and_then(Value::as_str))
        .unwrap_or("")
        .chars()
        .take(90)
        .collect::<String>()
        .replace('\n', " ")
}

fn title_of(t: &Thread) -> String {
    if !t.title.trim().is_empty() {
        return t.title.trim().chars().take(80).collect();
    }
    t.messages
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|m| m.get("content").and_then(Value::as_str))
        .map(|s| {
            s.trim()
                .chars()
                .take(60)
                .collect::<String>()
                .replace('\n', " ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "New chat".into())
}

fn entry_of(t: &Thread) -> Entry {
    Entry {
        id: t.id.clone(),
        title: title_of(t),
        pinned: t.pinned,
        created: t.created.clone(),
        updated: t.updated.clone(),
        messages: t.messages.len(),
        preview: preview_of(t),
        model: t.model.clone(),
    }
}

pub fn load(app: &App, id: &str) -> Result<Thread> {
    if !valid_id(id) {
        bail!("bad thread id");
    }
    let raw = std::fs::read(thread_path(app, id)).context("that chat is gone")?;
    let plain = app.crypto.open_file(&raw)?;
    Ok(serde_json::from_slice(&plain)?)
}

/// Writes a thread and refreshes its index entry. The caller fills in
/// everything but the times.
pub fn save(app: &App, mut t: Thread) -> Result<Entry> {
    if !valid_id(&t.id) {
        bail!("bad thread id");
    }
    let now = Local::now().to_rfc3339();
    if t.created.is_empty() {
        t.created = now.clone();
    }
    t.updated = now;
    let plain = serde_json::to_vec(&t)?;
    if plain.len() > MAX_BYTES {
        bail!("This chat is over 4 MB. Start a new one.");
    }
    let sealed = app.crypto.seal(&plain)?;
    std::fs::create_dir_all(dir(app))?;
    let p = thread_path(app, &t.id);
    let tmp = p.with_extension("tkt.tmp");
    std::fs::write(&tmp, &sealed)?;
    let old = std::fs::read(&p).ok();
    std::fs::rename(&tmp, &p)?;
    if let Some(old) = old {
        app.crypto.forget(&old);
    }
    let entry = entry_of(&t);
    let mut index = load_index(app);
    index.retain(|e| e.id != t.id);
    index.push(entry.clone());
    sort(&mut index);
    save_index(app, &index)?;
    Ok(entry)
}

pub fn remove(app: &App, id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("bad thread id");
    }
    let p = thread_path(app, id);
    match std::fs::read(&p) {
        Ok(raw) => {
            std::fs::remove_file(&p)?;
            app.crypto.forget(&raw);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut index = load_index(app);
    index.retain(|e| e.id != id);
    save_index(app, &index)?;
    Ok(())
}

fn sort(index: &mut [Entry]) {
    index.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated.cmp(&a.updated)));
}

/// Rebuilds the index from the files (after a restore, or if it's damaged).
pub fn rebuild_index(app: &App) -> Result<usize> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir(app)) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "tkt") {
                if let Some(id) = p.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(t) = load(app, id) {
                        out.push(entry_of(&t));
                    }
                }
            }
        }
    }
    sort(&mut out);
    save_index(app, &out)?;
    Ok(out.len())
}

// ------------------------------------------------------------------ API

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/threads", get(list).post(upsert))
        .route("/api/threads/rebuild", post(rebuild))
        .route("/api/threads/{id}", get(get_one).delete(delete_one))
        .route("/api/threads/{id}/meta", post(set_meta))
}

#[derive(Debug, Deserialize)]
struct ListQ {
    q: Option<String>,
}

/// The sidebar. With `q`, titles and previews match without decrypting;
/// message bodies are searched too, opening up to 200 threads.
async fn list(State(app): State<Arc<App>>, Query(q): Query<ListQ>) -> Json<Value> {
    let a = app.clone();
    let needle = q.q.unwrap_or_default().trim().to_lowercase();
    let entries = tokio::task::spawn_blocking(move || {
        let mut index = load_index(&a);
        if index.is_empty() {
            if let Ok(n) = rebuild_index(&a) {
                if n > 0 {
                    index = load_index(&a);
                }
            }
        }
        if needle.is_empty() {
            return index;
        }
        let mut hits: Vec<Entry> = Vec::new();
        for e in index.into_iter().take(200) {
            let shallow = e.title.to_lowercase().contains(&needle)
                || e.preview.to_lowercase().contains(&needle);
            let deep = !shallow
                && load(&a, &e.id).is_ok_and(|t| {
                    t.messages.iter().any(|m| {
                        m.get("content")
                            .and_then(Value::as_str)
                            .is_some_and(|c| c.to_lowercase().contains(&needle))
                    })
                });
            if shallow || deep {
                hits.push(e);
            }
        }
        hits
    })
    .await
    .unwrap_or_default();
    Json(json!({ "threads": entries, "max": MAX_THREADS }))
}

async fn get_one(State(app): State<Arc<App>>, AxPath(id): AxPath<String>) -> Response {
    let a = app.clone();
    match tokio::task::spawn_blocking(move || load(&a, &id)).await {
        Ok(Ok(t)) => Json(t).into_response(),
        Ok(Err(e)) => (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Creates or replaces a thread (the portal sends the whole thread after
/// each turn).
async fn upsert(State(app): State<Arc<App>>, Json(mut t): Json<Thread>) -> Response {
    if t.id.is_empty() {
        t.id = new_id();
    }
    if !valid_id(&t.id) {
        return (StatusCode::BAD_REQUEST, "bad thread id").into_response();
    }
    if t.messages.len() > 400 {
        return (
            StatusCode::BAD_REQUEST,
            "This chat has too many messages. Start a new one.",
        )
            .into_response();
    }
    let a = app.clone();
    let r = tokio::task::spawn_blocking(move || {
        let index = load_index(&a);
        if index.len() >= MAX_THREADS && !index.iter().any(|e| e.id == t.id) {
            bail!("Toktail keeps up to {MAX_THREADS} chats. Delete some first.");
        }
        // Keep what the portal doesn't send back (created, pinned).
        if let Ok(old) = load(&a, &t.id) {
            if t.created.is_empty() {
                t.created = old.created;
            }
            t.pinned = t.pinned || old.pinned;
        }
        save(&a, t)
    })
    .await;
    match r {
        Ok(Ok(entry)) => Json(entry).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct MetaIn {
    title: Option<String>,
    pinned: Option<bool>,
}

async fn set_meta(
    State(app): State<Arc<App>>,
    AxPath(id): AxPath<String>,
    Json(m): Json<MetaIn>,
) -> Response {
    let a = app.clone();
    let r = tokio::task::spawn_blocking(move || {
        let mut t = load(&a, &id)?;
        if let Some(title) = m.title {
            t.title = title.trim().chars().take(80).collect();
        }
        if let Some(p) = m.pinned {
            t.pinned = p;
        }
        save(&a, t)
    })
    .await;
    match r {
        Ok(Ok(entry)) => Json(entry).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete_one(State(app): State<Arc<App>>, AxPath(id): AxPath<String>) -> Response {
    let a = app.clone();
    match tokio::task::spawn_blocking(move || remove(&a, &id)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn rebuild(State(app): State<Arc<App>>) -> Response {
    let a = app.clone();
    match tokio::task::spawn_blocking(move || rebuild_index(&a)).await {
        Ok(Ok(n)) => Json(json!({ "ok": true, "threads": n })).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[tokio::test]
    async fn threads_are_sealed_indexed_and_searchable() {
        let app = test_support::app();
        app.crypto
            .fixture_per_file(app.paths.base.join("keystore.tkt"));
        let t = Thread {
            id: new_id(),
            host: "studio".into(),
            model: "qwen".into(),
            mode: "direct".into(),
            messages: vec![
                json!({"role":"user","content":"Explain borrowing in Rust"}),
                json!({"role":"assistant","content":"Borrowing lets you..."}),
            ],
            ..Default::default()
        };
        let id = t.id.clone();
        let e = save(&app, t).unwrap();
        assert_eq!(e.title, "Explain borrowing in Rust");
        assert_eq!(e.messages, 2);
        // On disk the file is sealed: no plaintext.
        let raw = std::fs::read(thread_path(&app, &id)).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"borrowing"));
        let back = load(&app, &id).unwrap();
        assert_eq!(back.messages[1]["content"], "Borrowing lets you...");
        assert!(!back.created.is_empty());
        // Rename and pin.
        let mut t2 = back.clone();
        t2.title = "Rust notes".into();
        t2.pinned = true;
        let e2 = save(&app, t2).unwrap();
        assert!(e2.pinned && e2.title == "Rust notes");
        // A second thread sorts under the pinned one; rebuilding keeps both.
        save(
            &app,
            Thread {
                id: new_id(),
                messages: vec![json!({"role":"user","content":"hello"})],
                ..Default::default()
            },
        )
        .unwrap();
        let idx = load_index(&app);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].id, id);
        std::fs::remove_file(index_path(&app)).unwrap();
        assert_eq!(rebuild_index(&app).unwrap(), 2);
        remove(&app, &id).unwrap();
        assert!(load(&app, &id).is_err());
        assert_eq!(load_index(&app).len(), 1);
        assert!(!valid_id("../x") && !valid_id("a b") && valid_id("t_0123456789abcdef"));
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
