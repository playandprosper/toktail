// SPDX-License-Identifier: Apache-2.0
//! Saved prompts and responses ("records").
//!
//! Off unless turned on in Settings. When on, each counted request writes two
//! files named for the moment it finished:
//!
//! ```text
//! <capture folder>/<host>/<YYYY>/<MM>/<DD>/YYYY-MM-DD-HH-mm-SS.prompt
//!                                          YYYY-MM-DD-HH-mm-SS.response
//! ```
//!
//! (a "-02", "-03"… suffix keeps requests from the same second apart). Each
//! file is the gzip of the original bytes, encrypted when encryption is on
//! (see crypto.rs). The CSV row's `record` column holds the path stem, so
//! every number in the CSV links to its files. With S3 on, files are mirrored
//! to `<prefix>records/<stem>.<kind>`; reads fall back to S3 when the local
//! copy is gone, so the portal works the same either way. Retention settings
//! delete old files locally and in S3 on a rolling window.

use crate::app::App;
use crate::s3sync;
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Local, NaiveDate};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Prompt,
    Response,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Prompt => "prompt",
            Kind::Response => "response",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "prompt" => Some(Kind::Prompt),
            "response" => Some(Kind::Response),
            _ => None,
        }
    }
    const ALL: [Kind; 2] = [Kind::Prompt, Kind::Response];
}

/// Your notes on a record: a title, tags, free text and a star.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Note {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub starred: bool,
}

struct Job {
    stem: String,
    prompt: Vec<u8>,
    response: Vec<u8>,
}

pub struct Records {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    /// Stems handed out in the current second, so names never collide.
    alloc: Mutex<(i64, HashSet<String>)>,
    /// Stems written locally but not yet in S3.
    pending: Mutex<Vec<String>>,
    /// Notes by stem (kept sealed in `notes.tkt`).
    notes: Mutex<BTreeMap<String, Note>>,
    pub errors: Mutex<Option<String>>,
}

impl Default for Records {
    fn default() -> Self {
        Records::new()
    }
}

impl Records {
    pub fn new() -> Records {
        Records {
            tx: Mutex::new(None),
            alloc: Mutex::new((0, HashSet::new())),
            pending: Mutex::new(Vec::new()),
            notes: Mutex::new(BTreeMap::new()),
            errors: Mutex::new(None),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    pub fn note(&self, stem: &str) -> Option<Note> {
        self.notes.lock().get(stem).cloned()
    }

    pub fn notes_snapshot(&self) -> BTreeMap<String, Note> {
        self.notes.lock().clone()
    }

    /// A unique path stem for a request that finished at `ts` on `host`.
    pub fn allocate(&self, root: &Path, host: &str, ts: DateTime<Local>) -> String {
        let sec = ts.timestamp();
        let base = format!(
            "{host}/{}/{}",
            ts.format("%Y/%m/%d"),
            ts.format("%Y-%m-%d-%H-%M-%S")
        );
        let mut a = self.alloc.lock();
        if a.0 != sec {
            *a = (sec, HashSet::new());
        }
        let mut n = 1;
        loop {
            let stem = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n:02}")
            };
            let taken = a.1.contains(&stem) || root.join(format!("{stem}.prompt")).exists();
            if !taken {
                a.1.insert(stem.clone());
                return stem;
            }
            n += 1;
        }
    }
}

fn pending_file(app: &App) -> PathBuf {
    app.paths.base.join("records-pending.txt")
}

fn notes_file(app: &App) -> PathBuf {
    app.paths.base.join("notes.tkt")
}

pub fn root(app: &App) -> PathBuf {
    let c = app.config.read();
    c.capture_dir(&app.paths.base)
}

pub fn local_path(app: &App, stem: &str, kind: Kind) -> PathBuf {
    root(app).join(format!("{stem}.{}", kind.ext()))
}

pub fn s3_key(app: &App, stem: &str, kind: Kind) -> String {
    let prefix = app.config.read().s3.prefix.clone();
    s3sync::object_key(&prefix, &format!("records/{stem}.{}", kind.ext()))
}

fn s3_on(app: &App) -> bool {
    let c = app.config.read();
    c.s3.enabled && c.capture.s3_upload
}

/// Stems may only contain what `allocate` produces: no "..", no leading "/".
pub fn valid_stem(stem: &str) -> bool {
    !stem.is_empty()
        && !stem.starts_with('/')
        && !stem.contains("..")
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/' || c == '_')
}

/// Starts the writer thread and loads the upload queue and notes.
pub fn start(app: Arc<App>) {
    let (tx, rx) = mpsc::channel::<Job>();
    *app.records.tx.lock() = Some(tx);
    if let Ok(text) = std::fs::read_to_string(pending_file(&app)) {
        let mut seen = HashSet::new();
        let list: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && seen.insert(l.to_string()))
            .map(String::from)
            .collect();
        *app.records.pending.lock() = list;
    }
    load_notes(&app);
    let writer = app.clone();
    std::thread::Builder::new()
        .name("toktail-records".into())
        .spawn(move || {
            for job in rx {
                if let Err(e) = write_job(&writer, &job) {
                    log::warn!("couldn't save the prompt/response for {}: {e:#}", job.stem);
                    *writer.records.errors.lock() = Some(format!("{e:#}"));
                }
            }
        })
        .ok();
    let r = app.clone();
    app.rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            if let Err(e) = retention(&r).await {
                log::warn!("retention: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// Called by the proxy: returns the stem to put in the CSV row, or "" when
/// capture is off or encryption isn't ready (nothing unencrypted is written
/// when encryption is on but locked).
pub fn capture(
    app: &App,
    host: &str,
    ts: DateTime<Local>,
    prompt: Vec<u8>,
    response: Vec<u8>,
) -> String {
    if !app.config.read().capture.enabled {
        return String::new();
    }
    if let Some(p) = app.crypto.problem() {
        *app.records.errors.lock() = Some(p);
        return String::new();
    }
    let root = root(app);
    let stem = app.records.allocate(&root, host, ts);
    if let Some(tx) = app.records.tx.lock().as_ref() {
        if tx
            .send(Job {
                stem: stem.clone(),
                prompt,
                response,
            })
            .is_ok()
        {
            return stem;
        }
    }
    String::new()
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_data()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn write_job(app: &App, job: &Job) -> Result<()> {
    for (kind, bytes) in [(Kind::Prompt, &job.prompt), (Kind::Response, &job.response)] {
        let sealed = app.crypto.seal(bytes)?;
        write_atomic(&local_path(app, &job.stem, kind), &sealed)?;
    }
    if s3_on(app) {
        queue_upload(app, &job.stem);
    }
    *app.records.errors.lock() = None;
    Ok(())
}

fn queue_upload(app: &App, stem: &str) {
    app.records.pending.lock().push(stem.to_string());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pending_file(app))
    {
        let _ = writeln!(f, "{stem}");
    }
}

/// Queues already-saved local files for upload (after S3 is turned on).
pub fn queue_existing(app: &App) -> usize {
    let stems: Vec<String> = app.ledger.with_data(|d| {
        d.rows()
            .filter_map(|r| r.record.as_deref().map(String::from))
            .collect()
    });
    let pending: HashSet<String> = app.records.pending.lock().iter().cloned().collect();
    let mut n = 0;
    for stem in stems {
        if !pending.contains(&stem) && local_path(app, &stem, Kind::Prompt).exists() {
            queue_upload(app, &stem);
            n += 1;
        }
    }
    n
}

fn rewrite_pending(app: &App) {
    let list = app.records.pending.lock().clone();
    let tmp = pending_file(app).with_extension("tmp");
    if std::fs::write(
        &tmp,
        list.join("\n") + if list.is_empty() { "" } else { "\n" },
    )
    .is_ok()
    {
        let _ = std::fs::rename(&tmp, pending_file(app));
    }
}

/// Uploads queued records to S3. Returns how many records went up.
pub async fn upload_pending(app: &Arc<App>) -> Result<usize> {
    if !s3_on(app) {
        return Ok(0);
    }
    let batch: Vec<String> = app
        .records
        .pending
        .lock()
        .iter()
        .take(500)
        .cloned()
        .collect();
    let mut done = HashSet::new();
    for stem in &batch {
        for kind in Kind::ALL {
            let path = local_path(app, stem, kind);
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    s3sync::put_object(
                        app,
                        &s3_key(app, stem, kind),
                        &bytes,
                        "application/octet-stream",
                    )
                    .await?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        done.insert(stem.clone());
    }
    if !done.is_empty() {
        app.records.pending.lock().retain(|s| !done.contains(s));
        rewrite_pending(app);
    }
    if app.records.pending.lock().is_empty() && !done.is_empty() {
        // Notes travel with the records.
        if let Ok(bytes) = tokio::fs::read(notes_file(app)).await {
            let prefix = app.config.read().s3.prefix.clone();
            s3sync::put_object(
                app,
                &s3sync::object_key(&prefix, "records/notes.tkt"),
                &bytes,
                "application/octet-stream",
            )
            .await?;
        }
    }
    Ok(done.len())
}

/// Where a record's files are right now.
#[derive(Debug, Serialize)]
pub struct Availability {
    pub local: bool,
    pub s3: bool,
}

pub async fn availability(app: &Arc<App>, stem: &str) -> Availability {
    let local = local_path(app, stem, Kind::Prompt).exists();
    let pending = app.records.pending.lock().iter().any(|s| s == stem);
    let s3 = app.config.read().s3.enabled && !pending && !local_only(app);
    Availability { local, s3 }
}

fn local_only(app: &App) -> bool {
    !app.config.read().capture.s3_upload
}

/// The decrypted, decompressed content of one file, from disk or from S3.
pub async fn read(app: &Arc<App>, stem: &str, kind: Kind) -> Result<Vec<u8>> {
    if !valid_stem(stem) {
        bail!("bad record name");
    }
    let path = local_path(app, stem, kind);
    let raw = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) if app.config.read().s3.enabled => s3sync::get_object(app, &s3_key(app, stem, kind))
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "This record's {} isn't on this Mac or in S3 (it may have expired).",
                    kind.ext()
                )
            })?,
        Err(_) => bail!("This record's {} isn't saved on this Mac.", kind.ext()),
    };
    let a = app.clone();
    tokio::task::spawn_blocking(move || a.crypto.open_file(&raw)).await?
}

/// Deletes a record's files here and in S3, and forgets their keys.
pub async fn delete_files(app: &Arc<App>, stem: &str) -> Result<()> {
    if !valid_stem(stem) {
        bail!("bad record name");
    }
    let s3 = app.config.read().s3.enabled;
    for kind in Kind::ALL {
        let path = local_path(app, stem, kind);
        let head = match tokio::fs::read(&path).await {
            Ok(b) => Some(b),
            Err(_) if s3 => s3sync::get_object(app, &s3_key(app, stem, kind))
                .await
                .ok()
                .flatten(),
            Err(_) => None,
        };
        if let Some(h) = head {
            let a = app.clone();
            let _ = tokio::task::spawn_blocking(move || a.crypto.forget(&h)).await;
        }
        let _ = tokio::fs::remove_file(&path).await;
        if s3 {
            s3sync::delete_object(app, &s3_key(app, stem, kind)).await?;
        }
    }
    app.records.pending.lock().retain(|s| s != stem);
    rewrite_pending(app);
    if app.records.notes.lock().remove(stem).is_some() {
        save_notes(app)?;
    }
    Ok(())
}

pub fn set_note(app: &App, stem: &str, note: Note) -> Result<()> {
    {
        let mut n = app.records.notes.lock();
        if note.title.is_empty() && note.tags.is_empty() && note.notes.is_empty() && !note.starred {
            n.remove(stem);
        } else {
            n.insert(stem.to_string(), note);
        }
    }
    save_notes(app)
}

fn save_notes(app: &App) -> Result<()> {
    let json = serde_json::to_vec(&*app.records.notes.lock())?;
    let sealed = app.crypto.seal(&json)?;
    write_atomic(&notes_file(app), &sealed)
}

fn load_notes(app: &App) {
    if let Ok(raw) = std::fs::read(notes_file(app)) {
        match app
            .crypto
            .open_file(&raw)
            .and_then(|b| Ok(serde_json::from_slice::<BTreeMap<String, Note>>(&b)?))
        {
            Ok(n) => *app.records.notes.lock() = n,
            Err(e) => log::warn!("couldn't read your record notes: {e:#}"),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RetentionState {
    /// Dates before these were already cleaned.
    local_done_before: Option<NaiveDate>,
    s3_done_before: Option<NaiveDate>,
}

/// Rolling-window retention: deletes files older than the configured number
/// of days, locally (only once uploaded, when S3 is on) and in S3.
pub async fn retention(app: &Arc<App>) -> Result<()> {
    let (local_days, s3_days, s3_enabled) = {
        let c = app.config.read();
        (
            c.capture.local_retention_days,
            c.capture.s3_retention_days,
            c.s3.enabled,
        )
    };
    if local_days == 0 && (s3_days == 0 || !s3_enabled) {
        return Ok(());
    }
    let state_path = app.paths.base.join("records-retention.json");
    let mut st: RetentionState = std::fs::read(&state_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let today = Local::now().date_naive();
    let by_date: Vec<(NaiveDate, String)> = app.ledger.with_data(|d| {
        d.rows()
            .filter_map(|r| r.record.as_deref().map(|s| (r.date, s.to_string())))
            .collect()
    });
    let pending: HashSet<String> = app.records.pending.lock().iter().cloned().collect();
    if local_days > 0 {
        let cutoff = today - chrono::Days::new(local_days as u64);
        let mut n = 0;
        for (date, stem) in &by_date {
            if *date < cutoff
                && st.local_done_before.is_none_or(|d| *date >= d)
                && !(s3_enabled && pending.contains(stem))
            {
                for kind in Kind::ALL {
                    if tokio::fs::remove_file(local_path(app, stem, kind))
                        .await
                        .is_ok()
                    {
                        n += 1;
                    }
                }
            }
        }
        if n > 0 {
            log::info!("retention: removed {n} local files older than {local_days} days");
        }
        if !s3_enabled || pending.is_empty() {
            st.local_done_before = Some(cutoff);
        }
    }
    if s3_days > 0 && s3_enabled {
        let cutoff = today - chrono::Days::new(s3_days as u64);
        let mut n = 0;
        for (date, stem) in &by_date {
            if *date < cutoff && st.s3_done_before.is_none_or(|d| *date >= d) {
                for kind in Kind::ALL {
                    s3sync::delete_object(app, &s3_key(app, stem, kind)).await?;
                    n += 1;
                }
            }
        }
        if n > 0 {
            log::info!("retention: removed {n} files from S3 older than {s3_days} days");
        }
        st.s3_done_before = Some(cutoff);
    }
    std::fs::write(&state_path, serde_json::to_vec_pretty(&st)?)?;
    Ok(())
}

/// What a record's prompt looked like, for listing and the chat view:
/// returns the messages (role, text) from an OpenAI-style request body.
pub fn summarize_prompt(body: &[u8]) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let mut msgs = Vec::new();
    let text_of = |c: &serde_json::Value| -> String {
        match c {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|p| {
                    p.get("text")
                        .and_then(|t| t.as_str())
                        .map(String::from)
                        .or_else(|| p.as_str().map(String::from))
                })
                .collect::<Vec<_>>()
                .join("\n"),
            other if other.is_null() => String::new(),
            other => other.to_string(),
        }
    };
    if let Some(s) = v.get("system").or_else(|| v.get("instructions")) {
        msgs.push(serde_json::json!({ "role": "system", "text": text_of(s) }));
    }
    if let Some(arr) = v.get("messages").and_then(|m| m.as_array()) {
        for m in arr {
            msgs.push(serde_json::json!({ "role": m.get("role").and_then(|r| r.as_str()).unwrap_or("user"), "text": text_of(m.get("content").unwrap_or(&serde_json::Value::Null)) }));
        }
    }
    if let Some(p) = v.get("prompt").or_else(|| v.get("input")) {
        msgs.push(serde_json::json!({ "role": "user", "text": text_of(p) }));
    }
    let mut params = serde_json::Map::new();
    if let Some(o) = v.as_object() {
        for (k, val) in o {
            if !["messages", "prompt", "input", "system", "instructions"].contains(&k.as_str()) {
                params.insert(k.clone(), val.clone());
            }
        }
    }
    serde_json::json!({ "messages": msgs, "params": params })
}

/// The response's text, from a non-streamed body or Toktail's stream summary.
pub fn summarize_response(body: &[u8]) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    if v.get("object").and_then(|o| o.as_str()) == Some("toktail.stream") {
        return v;
    }
    let mut text = String::new();
    let mut reasoning = String::new();
    if let Some(ch) = v.get("choices").and_then(|c| c.as_array()) {
        for c in ch {
            if let Some(m) = c.get("message") {
                text.push_str(m.get("content").and_then(|t| t.as_str()).unwrap_or(""));
                reasoning.push_str(
                    m.get("reasoning_content")
                        .or_else(|| m.get("reasoning"))
                        .and_then(|t| t.as_str())
                        .unwrap_or(""),
                );
            } else if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
        }
    }
    if text.is_empty() {
        if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
            for p in arr {
                text.push_str(p.get("text").and_then(|t| t.as_str()).unwrap_or(""));
            }
        }
    }
    if text.is_empty() && !v.is_null() && v.get("data").is_some() {
        text = "(embedding vectors)".into();
    }
    if text.is_empty() && v.is_null() {
        text = String::from_utf8_lossy(body).to_string();
    }
    serde_json::json!({ "object": "toktail.response", "model": v.get("model"), "content": text, "reasoning": reasoning, "usage": v.get("usage") })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn stems_are_unique_and_safe() {
        let r = Records::new();
        let root = std::env::temp_dir().join(format!("tl-rec-{}", uuid::Uuid::new_v4()));
        let ts = Local.with_ymd_and_hms(2026, 10, 11, 14, 3, 22).unwrap();
        let a = r.allocate(&root, "studio", ts);
        let b = r.allocate(&root, "studio", ts);
        assert_eq!(a, "studio/2026/10/11/2026-10-11-14-03-22");
        assert_eq!(b, "studio/2026/10/11/2026-10-11-14-03-22-02");
        assert!(valid_stem(&a));
        assert!(!valid_stem("../etc/passwd"));
        assert!(!valid_stem("/abs"));
    }

    #[test]
    fn summaries() {
        let p = summarize_prompt(br#"{"model":"m","temperature":0.2,"messages":[{"role":"system","content":"be nice"},{"role":"user","content":[{"type":"text","text":"hi"}]}]}"#);
        assert_eq!(p["messages"][1]["text"], "hi");
        assert_eq!(p["params"]["temperature"], 0.2);
        let r = summarize_response(br#"{"model":"m","choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":1}}"#);
        assert_eq!(r["content"], "hello");
    }
}
