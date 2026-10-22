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
use crate::chain;
use crate::s3sync;
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Local, NaiveDate};
use notify::Watcher;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

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
    rec: crate::ledger::Record,
    prompt: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
    _permit: Option<OwnedSemaphorePermit>,
}

pub struct Records {
    pub mutation: Mutex<()>,
    scan: tokio::sync::Mutex<()>,
    blobs: tokio::sync::Mutex<()>,
    pub blob_cache: Mutex<std::collections::HashMap<String, String>>,
    cursor: AtomicUsize,
    jobs: AtomicUsize,
    budget: Arc<Semaphore>,
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    /// Stems handed out in the current second, so names never collide.
    alloc: Mutex<(i64, HashSet<String>)>,
    /// Stems written locally but not yet in S3.
    pending: Mutex<Vec<String>>,
    pending_io: Mutex<()>,
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
            mutation: Mutex::new(()),
            scan: tokio::sync::Mutex::new(()),
            blobs: tokio::sync::Mutex::new(()),
            blob_cache: Mutex::new(Default::default()),
            cursor: AtomicUsize::new(0),
            jobs: AtomicUsize::new(0),
            budget: Arc::new(Semaphore::new(128 * 1024 * 1024)),
            tx: Mutex::new(None),
            alloc: Mutex::new((0, HashSet::new())),
            pending: Mutex::new(Vec::new()),
            pending_io: Mutex::new(()),
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

fn mirror_available(app: &App) -> bool {
    app.config.read().s3.enabled
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

/// A bounded writer keeps compression, encryption and CSV work off proxy workers.
pub fn start(app: Arc<App>) {
    if app.config.read().capture.enabled {
        let _ = std::fs::create_dir_all(root(&app));
    }
    let (tx, mut rx) = mpsc::channel::<Job>(128);
    *app.records.tx.lock() = Some(tx);
    if let Ok(text) = std::fs::read_to_string(pending_file(&app)) {
        *app.records.pending.lock() = text
            .lines()
            .filter(|s| valid_stem(s))
            .map(String::from)
            .collect();
    }
    load_notes(&app);
    let writer = app.clone();
    std::thread::Builder::new()
        .name("toktail-records".into())
        .spawn(move || {
            while let Some(mut job) = rx.blocking_recv() {
                let _blobs = writer.rt.block_on(writer.records.blobs.lock());
                let _guard = writer.records.mutation.lock();
                if let Err(e) = write_job(&writer, &mut job) {
                    log::warn!("record save failed: {e:#}");
                    *writer.records.errors.lock() = Some(format!("{e:#}"));
                    job.rec.record.clear();
                }
                if let Err(e) = writer.ledger.append(&job.rec) {
                    log::error!("could not write CSV: {e:#}");
                }
                if !job.rec.record.is_empty() {
                    if let (Some(row), Some(p), Some(r)) = (
                        writer.ledger.find(&job.rec.request_id),
                        job.prompt.as_ref(),
                        job.response.as_ref(),
                    ) {
                        writer.retrieval.insert(row, p, r);
                    }
                }
                if !job.rec.record.is_empty() && s3_on(&writer) {
                    queue_upload(&writer, &job.rec.record);
                }
                writer.records.jobs.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .expect("start record writer");
    let a = app.clone();
    app.rt.spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            let a = a.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let _guard = a.records.mutation.lock();
                if let Err(e) = a.crypto.flush().and_then(|_| a.ledger.flush()) {
                    log::warn!("flushing records: {e:#}");
                }
            })
            .await;
        }
    });
    let a = app.clone();
    app.rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        migrate_chains(a).await;
    });
    let a = app.clone();
    app.rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut cycles = 0;
        loop {
            if let Err(e) = reconcile(&a, None).await {
                log::debug!("reconciliation deferred: {e:#}");
            }
            crate::retrieval::warm(&a).await;
            if cycles % 360 == 0 {
                let _ = collect_blobs(&a).await;
                if let Err(e) = retention(&a).await {
                    log::warn!("retention: {e:#}");
                }
            }
            cycles += 1;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
    let a = app.clone();
    app.rt.spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let Ok(mut watcher) =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(e) = event {
                    if matches!(e.kind, notify::EventKind::Remove(_)) {
                        let _ = tx.send(e.paths);
                    }
                }
            })
        else {
            return;
        };
        let mut watched=root(&a);
        let mut watching=watcher.watch(&watched,notify::RecursiveMode::Recursive).is_ok();
        let mut tick=tokio::time::interval(Duration::from_secs(10));
        loop {
            let mut paths=tokio::select! {
                p=rx.recv()=>{let Some(p)=p else {break;};p},
                _=tick.tick()=>{
                    let next=root(&a);
                    if next!=watched || !watching {
                        if watching {let _=watcher.unwatch(&watched);}
                        watched=next;
                        if a.config.read().capture.enabled {let _=std::fs::create_dir_all(&watched);}
                        watching=watcher.watch(&watched,notify::RecursiveMode::Recursive).is_ok();
                    }
                    continue;
                }
            };
            if paths.iter().any(|p|p==&watched){watching=false;}
            tokio::time::sleep(Duration::from_millis(300)).await;
            while let Ok(p) = rx.try_recv() {
                paths.extend(p);
            }
            let root = root(&a);
            a.records.blob_cache.lock().clear();
            let relative: Vec<String> = paths
                .iter()
                .filter_map(|p| p.strip_prefix(&root).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect();
            let targets = a.ledger.with_data(|d| {
                d.rows()
                    .filter_map(|r| r.record.as_deref())
                    .filter(|s| {
                        relative.iter().any(|p| {
                            p.is_empty() || *s == p.trim_end_matches(".prompt").trim_end_matches(".response")
                                || s.starts_with(&(p.clone() + "/"))
                        })
                    })
                    .map(String::from)
                    .collect()
            });
            let _ = reconcile(&a, Some(targets)).await;
        }
    });
}

pub fn enqueue(
    app: &Arc<App>,
    rec: crate::ledger::Record,
    prompt: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
) {
    let saving = app.config.read().capture.enabled && app.crypto.problem().is_none();
    let size = prompt
        .as_ref()
        .map_or(0, Vec::len)
        .saturating_add(response.as_ref().map_or(0, Vec::len));
    let permit = if saving && size <= 128 * 1024 * 1024 {
        app.records
            .budget
            .clone()
            .try_acquire_many_owned(size.max(1) as u32)
            .ok()
    } else {
        None
    };
    let capture = permit.is_some();
    let job = Job {
        rec,
        prompt: if capture { prompt } else { None },
        response: if capture { response } else { None },
        _permit: permit,
    };
    if saving && !capture {
        *app.records.errors.lock() =
            Some("Capture queue is full; token counts are retained.".into());
    }
    if let Some(tx) = app.records.tx.lock().as_ref().cloned() {
        app.records.jobs.fetch_add(1, Ordering::SeqCst);
        let a = app.clone();
        app.rt.spawn(async move {
            if tx.send(job).await.is_err() {
                a.records.jobs.fetch_sub(1, Ordering::SeqCst);
            }
        });
    }
}

pub async fn shutdown(app: &Arc<App>) {
    crate::summaries::flush(app);
    app.shutting_down.store(true, Ordering::SeqCst);
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while app.live.snapshot(1).active_requests > 0 && std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    while app.records.jobs.load(Ordering::SeqCst) > 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let a = app.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _guard = a.records.mutation.lock();
        let _ = a.crypto.flush();
        let _ = a.ledger.flush();
    })
    .await;
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

/// Prompts share their semantic content across models. Model/sampling/tool
/// parameters remain in a sealed request reference; downloads reconstruct all fields.
fn split_prompt(bytes: &[u8]) -> (Vec<u8>, Option<serde_json::Map<String, serde_json::Value>>) {
    if let Ok(serde_json::Value::Object(mut params)) = serde_json::from_slice(bytes) {
        let mut prompt = serde_json::Map::new();
        for key in crate::usage::PROMPT_FIELDS {
            if let Some(value) = params.remove(key) {
                prompt.insert(key.into(), value);
            }
        }
        if !prompt.is_empty() {
            return (serde_json::to_vec(&prompt).unwrap(), Some(params));
        }
    }
    (bytes.to_vec(), None)
}

fn blob_stem(hash: &str) -> String {
    format!("blobs/{}/{}", &hash[..2], hash)
}
fn blob_key(app: &App, hash: &str) -> String {
    s3_key(app, &blob_stem(hash), Kind::Prompt)
}

/// Reads and decrypts a blob by content hash from the local folder.
pub fn blob_reader(app: &App) -> impl Fn(&str) -> Result<Option<Vec<u8>>> + '_ {
    move |hash: &str| {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("bad blob hash");
        }
        match std::fs::read(local_path(app, &blob_stem(hash), Kind::Prompt)) {
            Ok(raw) => Ok(Some(app.crypto.open_file(&raw)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Seals and writes a blob, forgetting the key of the file it replaces.
pub fn blob_writer(app: &App) -> impl Fn(&str, &[u8]) -> Result<()> + '_ {
    blob_writer_keeping(app, false)
}

/// Like `blob_writer`; with `keep_old_key` the replaced file's key stays
/// (its copy under `.pre-v0.0.3/` must stay readable until "Free space").
pub fn blob_writer_keeping(
    app: &App,
    keep_old_key: bool,
) -> impl Fn(&str, &[u8]) -> Result<()> + '_ {
    move |hash: &str, plain: &[u8]| {
        let path = local_path(app, &blob_stem(hash), Kind::Prompt);
        let old = std::fs::read(&path).ok();
        let sealed = app.crypto.seal(plain)?;
        write_atomic(&path, &sealed)?;
        if let Some(old) = old.filter(|_| !keep_old_key) {
            app.crypto.forget(&old);
        }
        app.records
            .blob_cache
            .lock()
            .insert(hash.to_string(), crate::crypto::Crypto::key_id(&sealed));
        if s3_on(app) {
            queue_upload(app, &blob_stem(hash));
        }
        Ok(())
    }
}

/// The S3 key a blob would have, when a mirror is configured.
fn blob_s3_key(app: &App) -> impl Fn(&str) -> Option<String> + '_ {
    move |hash: &str| (!app.config.read().s3.bucket.trim().is_empty()).then(|| blob_key(app, hash))
}

fn write_job(app: &App, job: &mut Job) -> Result<()> {
    let (Some(prompt), Some(response)) = (&job.prompt, &job.response) else {
        return Ok(());
    };
    let (content, params) = split_prompt(prompt);
    let hash = hex::encode(Sha256::digest(&content));
    let path = local_path(app, &blob_stem(&hash), Kind::Prompt);
    let cached_key = app.records.blob_cache.lock().get(&hash).cloned();
    let mut chain_node = None;
    let key = if let Some(key) = cached_key {
        key
    } else {
        let previous = std::fs::read(&path).ok();
        let sealed = if let Some(old) = previous.filter(|raw| {
            app.crypto.open_file(raw).is_ok_and(|plain| {
                plain == content
                    || chain::unfold(&plain, &mut |r| blob_reader(app)(&r.sha256))
                        .is_ok_and(|u| u.intact() && u.bytes == content)
            })
        }) {
            old
        } else {
            // A new prompt: store only what's new over the prompt it grew from.
            let session_hint = Some(job.rec.meta.session_id.as_str()).filter(|s| !s.is_empty());
            let plan = app.chains.plan(
                &root(app),
                &content,
                &hash,
                session_hint,
                &job.rec.host,
                &job.rec.meta.agent,
                "",
                &blob_s3_key(app),
                &blob_reader(app),
            )?;
            chain_node = Some(plan.header.clone());
            let sealed = app.crypto.seal(&plan.folded)?;
            write_atomic(&path, &sealed)?;
            sealed
        };
        let key = crate::crypto::Crypto::key_id(&sealed);
        let mut cache = app.records.blob_cache.lock();
        if cache.len() >= 4096 {
            cache.clear();
        }
        cache.insert(hash.clone(), key.clone());
        key
    };
    job.rec.prompt_blob = hash.clone();
    job.rec.prompt_key = key;
    let stem = app
        .records
        .allocate(&root(app), &job.rec.host, job.rec.timestamp);
    // Chain facts for the CSV: the session, the node, and how much of the
    // prompt was a repeat (as tokens, pro rata).
    let located = match &chain_node {
        Some(h) => Some((h.session.clone(), h.node.clone(), h.prefix, content.len())),
        None => app
            .chains
            .locate(&root(app), &hash)
            .map(|(s, n)| (s.id, n.node, n.prefix, n.size)),
    };
    if let Some((session, node, prefix, size)) = located {
        app.chains.set_stem(&root(app), &session, &node, &stem);
        job.rec.meta.session_id = session;
        job.rec.meta.chain_node = node;
        if size > 0 {
            job.rec.meta.prefix_tokens =
                (job.rec.input_tokens as u128 * prefix as u128 / size as u128) as u64;
        }
    }
    let reference =
        serde_json::to_vec(&serde_json::json!({"toktail_ref":1, "hash":hash, "params":params}))?;
    for (kind, bytes) in [
        (Kind::Prompt, reference.as_slice()),
        (Kind::Response, response.as_slice()),
    ] {
        let sealed = app.crypto.seal(bytes)?;
        if kind == Kind::Prompt {
            job.rec.reference_key = crate::crypto::Crypto::key_id(&sealed);
        } else {
            job.rec.response_key = crate::crypto::Crypto::key_id(&sealed);
        }
        write_atomic(&local_path(app, &stem, kind), &sealed)?;
    }
    job.rec.record = stem;
    *app.records.errors.lock() = None;
    Ok(())
}

fn queue_upload(app: &App, stem: &str) {
    let _guard = app.records.pending_io.lock();
    if app.records.pending.lock().iter().any(|s| s == stem) {
        return;
    }
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
    let _guard = app.records.pending_io.lock();
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
        let hash = app.ledger.with_data(|d| {
            d.rows()
                .find(|r| r.record.as_deref() == Some(stem.as_str()))
                .map(|r| r.prompt_blob.clone())
                .unwrap_or_default()
        });
        if hash.len() == 64 {
            let bytes = tokio::fs::read(local_path(app, &blob_stem(&hash), Kind::Prompt)).await?;
            s3sync::put_object(
                app,
                &blob_key(app, &hash),
                &bytes,
                "application/octet-stream",
            )
            .await?;
        }
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
    let local = Kind::ALL.iter().all(|k| local_path(app, stem, *k).exists());
    let s3 = if mirror_available(app) {
        s3sync::exists_object(app, &s3_key(app, stem, Kind::Prompt))
            .await
            .ok()
            == Some(true)
            && s3sync::exists_object(app, &s3_key(app, stem, Kind::Response))
                .await
                .ok()
                == Some(true)
    } else {
        false
    };
    Availability { local, s3 }
}

async fn read_sealed(app: &Arc<App>, stem: &str, kind: Kind) -> Result<Vec<u8>> {
    let raw = match tokio::fs::read(local_path(app, stem, kind)).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && mirror_available(app) => {
            tokio::time::timeout(
                Duration::from_secs(8),
                s3sync::get_object(app, &s3_key(app, stem, kind)),
            )
            .await??
            .ok_or_else(|| anyhow!("record is missing on disk and in S3"))?
        }
        Err(e) => return Err(e.into()),
    };
    let a = app.clone();
    tokio::task::spawn_blocking(move || a.crypto.open_file(&raw)).await?
}

/// A prompt blob unfolded: each link is read here or from S3. A link that
/// is gone everywhere is marked in the result, not fatal.
pub async fn read_blob(app: &Arc<App>, hash: &str) -> Result<chain::Unfolded> {
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("bad prompt reference");
    }
    // Collect every layer first (async), then unfold (sync).
    let mut layers: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    let mut next = Some(hash.to_string());
    while let Some(h) = next.take() {
        if layers.len() > (chain::SNAPSHOT_EVERY * 2 + 2) as usize {
            bail!("this prompt chain is longer than Toktail allows");
        }
        let raw = match read_sealed(app, &blob_stem(&h), Kind::Prompt).await {
            Ok(b) => Some(b),
            Err(e) if is_not_found(&e) => None,
            Err(e) => return Err(e),
        };
        if let Some(raw) = &raw {
            if let chain::Parsed::Folded {
                reference: Some(r), ..
            } = chain::parse(raw)?
            {
                if !r.is_empty() {
                    next = Some(r.sha256.clone());
                }
            }
        }
        layers.push((h, raw));
    }
    let (first, rest) = layers.split_first().unwrap();
    let Some(top) = first.1.as_ref() else {
        bail!("record is missing on disk and in S3");
    };
    let lookup: std::collections::HashMap<&str, &Option<Vec<u8>>> =
        rest.iter().map(|(h, b)| (h.as_str(), b)).collect();
    let u = chain::unfold(top, &mut |r| {
        Ok(lookup.get(r.sha256.as_str()).and_then(|b| (*b).clone()))
    })?;
    if !u.intact() {
        for (h, b) in rest {
            if b.is_none() {
                app.chains.mark_missing(&root(app), h);
            }
        }
    }
    Ok(u)
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
        || e.to_string().contains("missing on disk and in S3")
}

/// Old raw captures and new shared request references are both readable.
pub async fn read(app: &Arc<App>, stem: &str, kind: Kind) -> Result<Vec<u8>> {
    if !valid_stem(stem) {
        bail!("bad record name");
    }
    let bytes = read_sealed(app, stem, kind).await?;
    if kind != Kind::Prompt {
        return Ok(bytes);
    }
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if v["toktail_ref"] == 1 {
            let hash = v["hash"].as_str().unwrap_or("");
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("bad prompt reference");
            }
            let blob = read_blob(app, hash).await?.bytes;
            if let Some(params) = v["params"].as_object() {
                let mut original: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_slice(&blob)?;
                original.extend(params.clone());
                return Ok(serde_json::to_vec(&original)?);
            }
            return Ok(blob);
        }
    }
    Ok(bytes)
}

/// Notices saved records whose files are gone everywhere (deleted in Finder,
/// expired by retention here and in S3) and turns them into count-only rows:
/// the CSV keeps every token count, only the pointer to the files and their
/// keys are dropped. Only confirmed absence counts; network errors,
/// permissions and a configured-but-disabled mirror defer the check.
pub async fn reconcile(app: &Arc<App>, targets: Option<HashSet<String>>) -> Result<usize> {
    let _scan = app.records.scan.lock().await;
    if std::fs::read_dir(root(app)).is_err() {
        return Ok(0);
    }
    let configured = !app.config.read().s3.bucket.trim().is_empty();
    if configured && !mirror_available(app) {
        return Ok(0);
    }
    let mut rows: Vec<_> = app
        .ledger
        .with_data(|d| d.rows().filter(|r| r.record.is_some()).cloned().collect());
    rows.sort_by(|a, b| a.request_id.cmp(&b.request_id));
    if let Some(t) = targets.as_ref() {
        rows.retain(|r| t.contains(r.record.as_deref().unwrap_or("")));
    } else if !rows.is_empty() {
        let start = app.records.cursor.fetch_add(256, Ordering::Relaxed) % rows.len();
        rows.rotate_left(start);
        rows.truncate(256);
        rows.retain(|r| r.ts_ms < Local::now().timestamp_millis() - 300_000);
    }
    let pending: HashSet<_> = app.records.pending.lock().iter().cloned().collect();
    let mut gone = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    for row in rows {
        if std::time::Instant::now() > deadline {
            break;
        }
        let stem = row.record.as_deref().unwrap();
        if pending.contains(stem) {
            continue;
        }
        let mut absent = true;
        for kind in Kind::ALL {
            match tokio::fs::metadata(local_path(app, stem, kind)).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    absent = false;
                    break;
                }
            }
        }
        if !absent {
            continue;
        }
        if mirror_available(app) {
            for kind in Kind::ALL {
                let missing = tokio::time::timeout(
                    Duration::from_secs(3),
                    s3sync::exists_object(app, &s3_key(app, stem, kind)),
                )
                .await;
                if !matches!(missing, Ok(Ok(false))) {
                    absent = false;
                    break;
                }
            }
        }
        if absent {
            gone.push(row);
        }
    }
    let ids: HashSet<_> = gone.iter().map(|r| r.request_id.clone()).collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let a = app.clone();
    let n = tokio::task::spawn_blocking(move || {
        let _guard = a.records.mutation.lock();
        let n = a.ledger.forget_records(&ids)?;
        for r in gone {
            a.retrieval.remove(&r.request_id);
            a.crypto.forget_id(&r.reference_key);
            a.crypto.forget_id(&r.response_key);
            if let Some(stem) = r.record.as_deref() {
                a.records.notes.lock().remove(stem);
            }
        }
        save_notes(&a)?;
        Ok::<usize, anyhow::Error>(n)
    })
    .await??;
    app.sync_now.notify_one();
    Ok(n)
}

pub async fn collect_blobs(app: &Arc<App>) -> Result<()> {
    let _blobs = app.records.blobs.lock().await;
    let budget = std::time::Instant::now() + Duration::from_secs(2);
    if !app.config.read().s3.bucket.trim().is_empty() && !s3_on(app) {
        return Ok(());
    }
    let used: HashSet<_> = app.ledger.with_data(|d| {
        d.rows()
            .filter(|r| r.record.is_some())
            .map(|r| r.prompt_blob.clone())
            .collect()
    });
    let dirs = match std::fs::read_dir(root(app).join("blobs")) {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };
    for dir in dirs.flatten() {
        if let Ok(files) = std::fs::read_dir(dir.path()) {
            for entry in files.flatten() {
                if std::time::Instant::now() > budget {
                    return Ok(());
                }
                let path = entry.path();
                let hash = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if hash.len() != 64 || used.contains(&hash) {
                    continue;
                }
                if s3_on(app) {
                    tokio::time::timeout(
                        Duration::from_secs(2),
                        s3sync::delete_object(app, &blob_key(app, &hash)),
                    )
                    .await??;
                }
                let a = app.clone();
                tokio::task::spawn_blocking(move || {
                    let _guard = a.records.mutation.lock();
                    // Recheck after concurrent capture: a shared key must outlive all references.
                    if a.ledger.with_data(|d| {
                        d.rows()
                            .any(|r| r.record.is_some() && r.prompt_blob == hash)
                    }) {
                        return;
                    }
                    // Children of this prompt are refolded first, so erasing
                    // a link never breaks the rest of the chain.
                    if let Err(e) = a.chains.detach(
                        &root(&a),
                        &hash,
                        &blob_s3_key(&a),
                        &blob_reader(&a),
                        &blob_writer(&a),
                    ) {
                        log::warn!("couldn't rebalance the chain around {hash}: {e:#}; keeping it");
                        return;
                    }
                    if let Ok(raw) = std::fs::read(&path) {
                        if std::fs::remove_file(&path).is_ok() {
                            a.records.blob_cache.lock().remove(&hash);
                            a.crypto.forget(&raw);
                        }
                    }
                })
                .await?;
            }
        }
    }
    Ok(())
}

/// Deletes a record's files here and in S3, and forgets their keys.
pub async fn delete_files(app: &Arc<App>, stem: &str) -> Result<()> {
    if !valid_stem(stem) {
        bail!("bad record name");
    }
    if !app.config.read().s3.bucket.trim().is_empty() && !mirror_available(app) {
        bail!("The configured S3 mirror is disabled. Enable it to delete both copies.");
    }
    let keys = app.ledger.with_data(|d| {
        d.rows()
            .find(|r| r.record.as_deref() == Some(stem))
            .map(|r| (r.reference_key.clone(), r.response_key.clone()))
    });
    let mut heads = Vec::new();
    // Finish remote deletion before destroying any local file or encryption key.
    for kind in Kind::ALL {
        let raw = tokio::fs::read(local_path(app, stem, kind)).await.ok();
        if mirror_available(app) {
            s3sync::delete_object(app, &s3_key(app, stem, kind)).await?;
        }
        heads.push((kind, raw));
    }
    let a = app.clone();
    let stem = stem.to_string();
    tokio::task::spawn_blocking(move || {
        let _guard = a.records.mutation.lock();
        for (kind, raw) in heads {
            match std::fs::remove_file(local_path(&a, &stem, kind)) {
                Ok(()) => {
                    if let Some(raw) = raw {
                        a.crypto.forget(&raw);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some((reference, response)) = keys {
            a.crypto.forget_id(&reference);
            a.crypto.forget_id(&response);
        }
        a.records.pending.lock().retain(|s| s != &stem);
        rewrite_pending(&a);
        if a.records.notes.lock().remove(&stem).is_some() {
            save_notes(&a)?;
        }
        Ok(())
    })
    .await?
}

/// Erases prompt blobs no record refers to any more, rebalancing their
/// chains first. Called right after records are deleted so the person sees
/// the deletion only once the chain is patched and verified.
pub async fn release_blobs(app: &Arc<App>, hashes: Vec<String>) -> Result<usize> {
    let _blobs = app.records.blobs.lock().await;
    let mut released = 0;
    for hash in hashes {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let still_used = app.ledger.with_data(|d| {
            d.rows()
                .any(|r| r.record.is_some() && r.prompt_blob == hash)
        });
        if still_used {
            continue;
        }
        if s3_on(app) {
            tokio::time::timeout(
                Duration::from_secs(5),
                s3sync::delete_object(app, &blob_key(app, &hash)),
            )
            .await??;
        }
        let a = app.clone();
        let h = hash.clone();
        let ok = tokio::task::spawn_blocking(move || -> Result<bool> {
            let _guard = a.records.mutation.lock();
            let n = a.chains.detach(
                &root(&a),
                &h,
                &blob_s3_key(&a),
                &blob_reader(&a),
                &blob_writer(&a),
            )?;
            if n > 0 {
                log::info!(
                    "chain: {n} prompt(s) refolded around erased link {}",
                    &h[..12]
                );
            }
            let path = local_path(&a, &blob_stem(&h), Kind::Prompt);
            match std::fs::read(&path) {
                Ok(raw) => {
                    std::fs::remove_file(&path)?;
                    a.records.blob_cache.lock().remove(&h);
                    a.crypto.forget(&raw);
                    Ok(true)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e.into()),
            }
        })
        .await??;
        if ok {
            released += 1;
        }
    }
    Ok(released)
}

/// Where originals go while a v1 (unfolded) blob is rewritten as a chain link.
pub fn pre_v2_dir(app: &App) -> PathBuf {
    root(app).join(".pre-v0.0.3")
}

/// Where the 0.0.2 preview build kept them; still counted and freed.
fn pre_v2_dir_preview(app: &App) -> PathBuf {
    root(app).join(".pre-v0.0.2")
}

fn migrated_marker(app: &App) -> PathBuf {
    chain::sessions_dir(&root(app)).join(".migrated-v2")
}

/// Folds every saved prompt from before 0.0.3 into session chains, oldest
/// first, in the background. Originals are copied to `.pre-v0.0.3/` and
/// stay readable until "Free space" in Settings removes them.
pub async fn migrate_chains(app: Arc<App>) {
    if migrated_marker(&app).exists() || !root(&app).join("blobs").exists() {
        app.chains.migration.lock().done = true;
        return;
    }
    {
        let mut m = app.chains.migration.lock();
        m.running = true;
    }
    // Oldest request first, so chains grow the way they were made.
    let mut order: Vec<(i64, String, String)> = app.ledger.with_data(|d| {
        let mut by_hash: BTreeMap<String, (i64, String)> = BTreeMap::new();
        for r in d
            .rows()
            .filter(|r| r.record.is_some() && r.prompt_blob.len() == 64)
        {
            let e = by_hash
                .entry(r.prompt_blob.clone())
                .or_insert((r.ts_ms, r.meta.session_id.clone()));
            if r.ts_ms < e.0 {
                *e = (r.ts_ms, r.meta.session_id.clone());
            }
        }
        by_hash.into_iter().map(|(h, (t, s))| (t, h, s)).collect()
    });
    order.sort();
    app.chains.migration.lock().total = order.len();
    let backup_root = pre_v2_dir(&app);
    for (_, hash, session_hint) in order {
        let a = app.clone();
        let backup = backup_root.clone();
        let r = tokio::task::spawn_blocking(move || -> Result<Option<u64>> {
            let _blobs = a.rt.block_on(a.records.blobs.lock());
            let _guard = a.records.mutation.lock();
            let path = local_path(&a, &blob_stem(&hash), Kind::Prompt);
            let Ok(raw) = std::fs::read(&path) else {
                return Ok(None);
            };
            let plain = a.crypto.open_file(&raw)?;
            if chain::is_folded(&plain) {
                return Ok(None);
            }
            let copy = backup.join(chain::blob_rel_path(&hash));
            if !copy.exists() {
                write_atomic(&copy, &raw)?;
            }
            let plan = a.chains.plan(
                &root(&a),
                &plain,
                &hash,
                Some(session_hint.as_str()).filter(|s| !s.is_empty()),
                "",
                "",
                "",
                &blob_s3_key(&a),
                &blob_reader(&a),
            )?;
            // Prove it unfolds before replacing the original.
            let check = chain::unfold(&plan.folded, &mut |r| blob_reader(&a)(&r.sha256))?;
            if !check.intact() || check.bytes != plain {
                bail!("the folded prompt did not unfold to the original; left as it was");
            }
            blob_writer_keeping(&a, true)(&hash, &plan.folded)?;
            Ok(Some(
                plain.len() as u64 - (plan.folded.len() as u64).min(plain.len() as u64),
            ))
        })
        .await;
        {
            let mut m = app.chains.migration.lock();
            match r {
                Ok(Ok(Some(saved))) => {
                    m.folded += 1;
                    m.saved_bytes += saved;
                }
                Ok(Ok(None)) => m.skipped += 1,
                Ok(Err(e)) => {
                    m.skipped += 1;
                    m.error = Some(format!("{e:#}"));
                }
                Err(e) => m.error = Some(e.to_string()),
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = std::fs::create_dir_all(chain::sessions_dir(&root(&app)));
    let _ = std::fs::write(migrated_marker(&app), Local::now().to_rfc3339());
    let done = {
        let mut m = app.chains.migration.lock();
        m.running = false;
        m.done = true;
        m.clone()
    };
    log::info!(
        "chain: migration done: {} prompts folded, {} skipped, {} MB saved",
        done.folded,
        done.skipped,
        done.saved_bytes / 1_048_576
    );
}

/// Bytes held by pre-0.0.3 originals (and the 0.0.2 preview's).
pub fn pre_v2_bytes(app: &App) -> u64 {
    fn walk(p: &Path) -> u64 {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| {
                        let p = e.path();
                        if p.is_dir() {
                            walk(&p)
                        } else {
                            e.metadata().map(|m| m.len()).unwrap_or(0)
                        }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    walk(&pre_v2_dir(app)) + walk(&pre_v2_dir_preview(app))
}

/// "Free space": removes the pre-0.0.3 originals (and the 0.0.2 preview's)
/// and forgets their keys.
pub fn free_space(app: &App) -> Result<u64> {
    let _guard = app.records.mutation.lock();
    let bytes = pre_v2_bytes(app);
    fn forget_all(app: &App, p: &Path) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    forget_all(app, &p);
                } else if let Ok(raw) = std::fs::read(&p) {
                    app.crypto.forget(&raw);
                }
            }
        }
    }
    for dir in [pre_v2_dir(app), pre_v2_dir_preview(app)] {
        forget_all(app, &dir);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(bytes)
}

pub fn set_note(app: &App, stem: &str, note: Note) -> Result<()> {
    let _guard = app.records.mutation.lock();
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
    let old = std::fs::read(notes_file(app)).ok();
    let sealed = app.crypto.seal(&json)?;
    write_atomic(&notes_file(app), &sealed)?;
    if let Some(old) = old {
        app.crypto.forget(&old);
    }
    Ok(())
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
    if let Some(p) = v
        .get("input")
        .and_then(|v| v.as_array())
        .filter(|a| a.iter().any(|m| m.get("role").is_some()))
    {
        for m in p {
            msgs.push(serde_json::json!({"role":m["role"].as_str().unwrap_or("user"),"text":text_of(&m["content"])}));
        }
    } else if let Some(p) = v.get("prompt").or_else(|| v.get("input")) {
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
    if let Some(out) = v["output"].as_array() {
        for m in out {
            if let Some(parts) = m["content"].as_array() {
                for p in parts {
                    if let Some(t) = p["text"].as_str().or(p["refusal"].as_str()) {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    if let Some(t) = v["output_text"].as_str() {
        if text.is_empty() {
            text.push_str(t);
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

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::test_support;
    #[tokio::test]
    async fn encrypted_cross_model_dedup_and_partial_deletion() {
        let app = test_support::app();
        app.crypto
            .fixture_per_file(app.paths.base.join("keystore.tkt"));
        let a=br#"{"model":"one","temperature":0.2,"messages":[{"role":"user","content":"Explain Rust ownership"}]}"#.to_vec();
        let b=br#"{"messages":[{"content":"Explain Rust ownership","role":"user"}],"temperature":0.8,"model":"two"}"#.to_vec();
        let response =
            br#"{"choices":[{"message":{"content":"Every value has one owner."}}]}"#.to_vec();
        let mut first = Job {
            rec: test_support::record("one"),
            prompt: Some(a.clone()),
            response: Some(response.clone()),
            _permit: None,
        };
        let mut second = Job {
            rec: test_support::record("two"),
            prompt: Some(b.clone()),
            response: Some(response.clone()),
            _permit: None,
        };
        write_job(&app, &mut first).unwrap();
        app.ledger.append(&first.rec).unwrap();
        write_job(&app, &mut second).unwrap();
        app.ledger.append(&second.rec).unwrap();
        app.crypto.flush().unwrap();
        app.ledger.flush().unwrap();
        assert_eq!(first.rec.prompt_blob, second.rec.prompt_blob);
        assert_ne!(first.rec.record, second.rec.record);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &read(&app, &second.rec.record, Kind::Prompt).await.unwrap()
            )
            .unwrap(),
            serde_json::from_slice::<serde_json::Value>(&b).unwrap()
        );
        let target = || Some([first.rec.record.clone()].into_iter().collect());
        std::fs::remove_file(local_path(&app, &first.rec.record, Kind::Prompt)).unwrap();
        assert_eq!(
            reconcile(&app, target()).await.unwrap(),
            0,
            "a response alone preserves the history"
        );
        std::fs::remove_file(local_path(&app, &first.rec.record, Kind::Response)).unwrap();
        assert_eq!(reconcile(&app, target()).await.unwrap(), 1);
        let kept = app
            .ledger
            .find(&first.rec.request_id)
            .expect("the counts stay");
        assert!(kept.record.is_none() && kept.output == first.rec.output_tokens);
        assert!(
            read(&app, &second.rec.record, Kind::Prompt).await.is_ok(),
            "a surviving request still owns the shared prompt key"
        );
        collect_blobs(&app).await.unwrap();
        assert!(local_path(&app, &blob_stem(&second.rec.prompt_blob), Kind::Prompt).exists());
        let count_only = test_support::record("counts");
        app.ledger.append(&count_only).unwrap();
        let targets = Some([second.rec.record.clone()].into_iter().collect());
        for k in Kind::ALL {
            std::fs::remove_file(local_path(&app, &second.rec.record, k)).unwrap();
        }
        app.config.write().s3.bucket = "configured-but-offline".into();
        assert_eq!(reconcile(&app, targets.clone()).await.unwrap(), 0);
        app.config.write().s3.bucket.clear();
        assert_eq!(reconcile(&app, targets).await.unwrap(), 1);
        // Reusing a prompt before garbage collection must retain a working key.
        let mut third = Job {
            rec: test_support::record("three"),
            prompt: Some(a),
            response: Some(response),
            _permit: None,
        };
        write_job(&app, &mut third).unwrap();
        app.ledger.append(&third.rec).unwrap();
        assert!(read(&app, &third.rec.record, Kind::Prompt).await.is_ok());
        delete_files(&app, &third.rec.record).await.unwrap();
        app.ledger
            .forget_records(&[third.rec.request_id].into_iter().collect())
            .unwrap();
        collect_blobs(&app).await.unwrap();
        assert!(!local_path(&app, &blob_stem(&second.rec.prompt_blob), Kind::Prompt).exists());
        assert!(app.ledger.find(&count_only.request_id).is_some());
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
    /// The model sees the request the agent sent; the chain lives only on disk.
    #[tokio::test]
    async fn session_chains_fold_on_disk_and_unfold_exactly() {
        let app = test_support::app();
        app.crypto
            .fixture_per_file(app.paths.base.join("keystore.tkt"));
        let turn = |n: usize| {
            let mut s = String::from("{\"model\":\"qwen\",\"stream\":true,\"messages\":[{\"role\":\"system\",\"content\":\"You are opencode.\\n\\nWorking directory: /Users/me/roc-app\\n\"}");
            for i in 0..n {
                s.push_str(&format!(",{{\"role\":\"user\",\"content\":\"edit file{i}.rs\\n\\n{}\"}},{{\"role\":\"assistant\",\"content\":\"done {i}\\n\\n\\n\"}}", "y".repeat(400)));
            }
            s.push_str(",{\"role\":\"user\",\"content\":\"next\"}]}");
            s.into_bytes()
        };
        let response = br#"{"choices":[{"message":{"content":"ok"}}]}"#.to_vec();
        let mut jobs = Vec::new();
        for n in 1..=5 {
            let mut rec = test_support::record("qwen");
            rec.input_tokens = 1000;
            let mut job = Job {
                rec,
                prompt: Some(turn(n)),
                response: Some(response.clone()),
                _permit: None,
            };
            write_job(&app, &mut job).unwrap();
            app.ledger.append(&job.rec).unwrap();
            jobs.push(job);
        }
        app.crypto.flush().unwrap();
        app.ledger.flush().unwrap();
        // One session, growing depth, repeated prefix counted as tokens.
        let session = jobs[0].rec.meta.session_id.clone();
        assert!(session.starts_with("s_"));
        assert!(jobs.iter().all(|j| j.rec.meta.session_id == session));
        assert_eq!(jobs[0].rec.meta.prefix_tokens, 0);
        assert!(
            jobs[4].rec.meta.prefix_tokens > 700,
            "{}",
            jobs[4].rec.meta.prefix_tokens
        );
        assert!(!jobs[4].rec.meta.chain_node.is_empty());
        // On disk: the fifth blob is far smaller than its content, and folded.
        let on_disk = |j: &Job| {
            std::fs::read(local_path(
                &app,
                &blob_stem(&j.rec.prompt_blob),
                Kind::Prompt,
            ))
            .unwrap()
        };
        let plain5 = app.crypto.open_file(&on_disk(&jobs[4])).unwrap();
        assert!(chain::is_folded(&plain5));
        assert!(
            plain5.len() < turn(5).len() / 2,
            "{} vs {}",
            plain5.len(),
            turn(5).len()
        );
        assert!(
            !plain5.windows(20).any(|w| w == &turn(5)[100..120]),
            "the shared prefix is not stored twice"
        );
        assert!(plain5.starts_with(b"{{toktail:chain v=2 session=\"s_"));
        assert!(
            plain5.windows(14).any(|w| w == b"{{toktail:ref "),
            "the file points at its parent by path and checksum"
        );
        // Reading gives back the exact request, params and all.
        for (i, j) in jobs.iter().enumerate() {
            let got = read(&app, &j.rec.record, Kind::Prompt).await.unwrap();
            let want: serde_json::Value = serde_json::from_slice(&turn(i + 1)).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&got).unwrap(),
                want,
                "turn {}",
                i + 1
            );
        }
        let u = read_blob(&app, &jobs[4].rec.prompt_blob).await.unwrap();
        assert!(u.intact() && u.depth == 4);
        // Erase turn 3: its files go, the chain is patched, the rest still read.
        delete_files(&app, &jobs[2].rec.record).await.unwrap();
        app.ledger
            .forget_records(&[jobs[2].rec.request_id.clone()].into_iter().collect())
            .unwrap();
        assert_eq!(
            release_blobs(&app, vec![jobs[2].rec.prompt_blob.clone()])
                .await
                .unwrap(),
            1
        );
        assert!(!local_path(&app, &blob_stem(&jobs[2].rec.prompt_blob), Kind::Prompt).exists());
        for (i, j) in jobs.iter().enumerate() {
            if i == 2 {
                continue;
            }
            let got = read(&app, &j.rec.record, Kind::Prompt).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&got).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&turn(i + 1)).unwrap(),
                "turn {} after erasing turn 3",
                i + 1
            );
        }
        let s = app.chains.session(&root(&app), &session).unwrap();
        assert!(s.by_content(&jobs[2].rec.prompt_blob).unwrap().missing);
        assert_eq!(s.by_content(&jobs[3].rec.prompt_blob).unwrap().depth, 2);
        // A blob lost behind Toktail's back is marked, and the rest shows the gap.
        std::fs::remove_file(local_path(
            &app,
            &blob_stem(&jobs[0].rec.prompt_blob),
            Kind::Prompt,
        ))
        .unwrap();
        let u = read_blob(&app, &jobs[1].rec.prompt_blob).await.unwrap();
        assert!(!u.intact());
        assert!(u.bytes.starts_with(b"{{toktail:missing "));
        assert!(
            app.chains
                .session(&root(&app), &session)
                .unwrap()
                .by_content(&jobs[0].rec.prompt_blob)
                .unwrap()
                .missing
        );
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test]
    async fn pre_v2_blobs_migrate_in_place_with_originals_kept() {
        let app = test_support::app();
        app.crypto
            .fixture_per_file(app.paths.base.join("keystore.tkt"));
        let turn = |n: usize| {
            format!(
                "{{\"messages\":[{{\"role\":\"system\",\"content\":\"{}\"}}{}]}}",
                "s".repeat(300),
                (0..n)
                    .map(|i| format!(
                        ",{{\"role\":\"user\",\"content\":\"q{i} {}\"}}",
                        "z".repeat(200)
                    ))
                    .collect::<String>()
            )
            .into_bytes()
        };
        // Three v1 blobs: raw content, plus records pointing at them.
        let mut recs = Vec::new();
        for n in 1..=3 {
            let content = turn(n);
            let hash = hex::encode(Sha256::digest(&content));
            write_atomic(
                &local_path(&app, &blob_stem(&hash), Kind::Prompt),
                &app.crypto.seal(&content).unwrap(),
            )
            .unwrap();
            let mut rec = test_support::record("m");
            rec.timestamp += chrono::Duration::seconds(n as i64);
            rec.prompt_blob = hash.clone();
            let stem = app.records.allocate(&root(&app), &rec.host, rec.timestamp);
            let reference = serde_json::to_vec(
                &serde_json::json!({"toktail_ref":1, "hash":hash, "params":{"model":"m"}}),
            )
            .unwrap();
            write_atomic(
                &local_path(&app, &stem, Kind::Prompt),
                &app.crypto.seal(&reference).unwrap(),
            )
            .unwrap();
            write_atomic(
                &local_path(&app, &stem, Kind::Response),
                &app.crypto.seal(b"{}").unwrap(),
            )
            .unwrap();
            rec.record = stem;
            app.ledger.append(&rec).unwrap();
            recs.push(rec);
        }
        app.ledger.flush().unwrap();
        migrate_chains(app.clone()).await;
        let m = app.chains.migration.lock().clone();
        assert!(m.done && m.folded == 3 && m.error.is_none(), "{m:?}");
        assert!(m.saved_bytes > 0);
        for (i, r) in recs.iter().enumerate() {
            let got = read(&app, &r.record, Kind::Prompt).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&got).unwrap();
            assert_eq!(
                v["messages"],
                serde_json::from_slice::<serde_json::Value>(&turn(i + 1)).unwrap()["messages"]
            );
            let plain = app
                .crypto
                .open_file(
                    &std::fs::read(local_path(&app, &blob_stem(&r.prompt_blob), Kind::Prompt))
                        .unwrap(),
                )
                .unwrap();
            assert!(chain::is_folded(&plain));
            // The original is kept, readable, until "Free space".
            let copy = pre_v2_dir(&app).join(chain::blob_rel_path(&r.prompt_blob));
            assert_eq!(
                app.crypto
                    .open_file(&std::fs::read(&copy).unwrap())
                    .unwrap(),
                turn(i + 1)
            );
        }
        assert_eq!(
            chain::list_sessions(&root(&app)).len(),
            1,
            "one session, found by prefix"
        );
        assert!(pre_v2_bytes(&app) > 0);
        // A 0.0.2 preview build left its originals here; they count and go too.
        let preview = pre_v2_dir_preview(&app).join("blobs/aa/old.prompt");
        write_atomic(&preview, b"0123456789").unwrap();
        let before = pre_v2_bytes(&app);
        assert_eq!(free_space(&app).unwrap(), before);
        assert!(!pre_v2_dir(&app).exists());
        assert!(!pre_v2_dir_preview(&app).exists());
        // Running again is a no-op.
        migrate_chains(app.clone()).await;
        assert_eq!(app.chains.migration.lock().folded, 3);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }

    #[tokio::test]
    async fn legacy_capture_remains_readable() {
        let app = test_support::app();
        let stem = "laptop/2026/09/30/2026-09-30-12-00-00";
        let original = br#"{"model":"old","prompt":"hello"}"#;
        write_atomic(
            &local_path(&app, stem, Kind::Prompt),
            &app.crypto.seal(original).unwrap(),
        )
        .unwrap();
        assert_eq!(read(&app, stem, Kind::Prompt).await.unwrap(), original);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;
    #[tokio::test]
    async fn only_confirmed_remote_404_prunes_missing_capture() {
        let app = crate::test_support::app();
        std::fs::create_dir_all(root(&app)).unwrap();
        let mut rec = crate::test_support::record("m");
        rec.record = "laptop/2026/09/30/2026-09-30-00-00-00".into();
        app.ledger.append(&rec).unwrap();
        let status = Arc::new(std::sync::atomic::AtomicU16::new(403));
        let state = status.clone();
        let router = axum::Router::new().route(
            "/{*path}",
            axum::routing::head(move || {
                let s = state.clone();
                async move { axum::http::StatusCode::from_u16(s.load(Ordering::SeqCst)).unwrap() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let old = std::env::var(crate::secrets::ENV_SECRET).ok();
        std::env::set_var(crate::secrets::ENV_SECRET, "synthetic-fixture-secret");
        {
            let mut c = app.config.write();
            c.s3.enabled = true;
            c.capture.s3_upload = true;
            c.s3.endpoint = format!("http://{addr}");
            c.s3.bucket = "fixture".into();
            c.s3.access_key_id = "fixture-key".into();
        }
        let target = || Some([rec.record.clone()].into_iter().collect());
        for code in [403, 503, 200] {
            status.store(code, Ordering::SeqCst);
            assert_eq!(
                reconcile(&app, target()).await.unwrap(),
                0,
                "HTTP {code} is not confirmed absence"
            );
        }
        status.store(404, Ordering::SeqCst);
        assert_eq!(reconcile(&app, target()).await.unwrap(), 1);
        let kept = app.ledger.find(&rec.request_id).expect("the counts stay");
        assert!(kept.record.is_none());
        if let Some(old) = old {
            std::env::set_var(crate::secrets::ENV_SECRET, old);
        } else {
            std::env::remove_var(crate::secrets::ENV_SECRET);
        }
        task.abort();
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
