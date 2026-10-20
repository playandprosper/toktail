// SPDX-License-Identifier: Apache-2.0
//! Toktail's prompt cache: the same request twice gets the saved answer.
//!
//! The key is the whole request: the canonical JSON body (so key order and
//! whitespace don't matter), the endpoint, the host, and the id of the
//! credential that sent it, so one key's answers are never served to another.
//! Entries live under `<base>/cache/<2 hex>/<key>.tkc`, sealed the same way
//! saved prompts are (gzip, encrypted when encryption is on), and expire
//! after `cache.ttl_hours` (72 by default) by file time. The folder is kept
//! under `cache.max_mb`; the oldest entries go first.
//!
//! What's cached: a 200 answer that completed (not an error, not a refusal,
//! not a client that hung up), streamed or not. A streamed answer is replayed
//! exactly as the client first received it, all at once. A hit gets its own
//! CSV row with `cache_hit = true` and the original token counts, so the
//! dashboard shows what the client got without pretending a model ran.
//!
//! Bypass with `Cache-Control: no-cache` (or `no-store`), or
//! `X-Toktail-Cache: bypass`; `X-Toktail-Cache: refresh` re-asks the model
//! and replaces the entry. Every answer carries `X-Toktail-Cache: hit`,
//! `miss`, `bypass` or `off`.

use crate::app::App;
use anyhow::{bail, Context, Result};
use axum::http::HeaderMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Answers bigger than this aren't cached.
pub const MAX_ENTRY_BYTES: usize = 32 * 1024 * 1024;

/// What the client asked for, as far as the cache is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Look up, and store a miss.
    Use,
    /// Don't look up; don't store.
    Bypass,
    /// Don't look up; store the fresh answer.
    Refresh,
}

pub fn mode_from_headers(h: &HeaderMap) -> Mode {
    let v = |name: &str| {
        h.get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let x = v("x-toktail-cache");
    if x == "bypass" || x == "off" || x == "no" {
        return Mode::Bypass;
    }
    if x == "refresh" {
        return Mode::Refresh;
    }
    let cc = v("cache-control");
    if cc.contains("no-cache") || cc.contains("no-store") {
        return Mode::Bypass;
    }
    Mode::Use
}

/// The cache key for a request.
pub fn key(host: &str, endpoint: &str, token_id: &str, body: &[u8]) -> String {
    let canonical = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(v) => {
            let mut s = String::new();
            crate::usage::write_canonical(&v, &mut s);
            s.into_bytes()
        }
        Err(_) => body.to_vec(),
    };
    let mut h = Sha256::new();
    h.update(host.as_bytes());
    h.update(b"\0");
    h.update(endpoint.as_bytes());
    h.update(b"\0");
    h.update(token_id.as_bytes());
    h.update(b"\0");
    h.update(&canonical);
    hex::encode(h.finalize())
}

/// What's stored with an answer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Head {
    pub status: u16,
    pub content_type: String,
    pub streamed: bool,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub usage_source: String,
    pub outcome: String,
    pub signal: String,
    pub response_chars: u64,
    /// The request that produced it.
    pub request_id: String,
    pub created: String,
    pub endpoint: String,
}

/// A cached answer, read back.
pub struct Entry {
    pub head: Head,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Meta {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

#[derive(Default)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub stored: u64,
}

/// In-memory index of what's on disk.
pub struct PromptCache {
    index: Mutex<HashMap<String, Meta>>,
    pub stats: Mutex<Stats>,
    scanned: Mutex<bool>,
}

impl Default for PromptCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptCache {
    pub fn new() -> PromptCache {
        PromptCache {
            index: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
            scanned: Mutex::new(false),
        }
    }

    pub fn entries(&self) -> usize {
        self.index.lock().len()
    }

    pub fn bytes(&self) -> u64 {
        self.index.lock().values().map(|m| m.size).sum()
    }

    /// Counters without touching the disk (for the live poll).
    pub fn quick(&self) -> serde_json::Value {
        let s = self.stats.lock();
        serde_json::json!({
            "entries": self.entries(),
            "bytes": self.bytes(),
            "hits": s.hits,
            "misses": s.misses,
            "stored": s.stored,
        })
    }
}

pub fn dir(app: &App) -> PathBuf {
    app.paths.base.join("cache")
}

fn path_for(app: &App, key: &str) -> PathBuf {
    dir(app).join(&key[..2]).join(format!("{key}.tkc"))
}

fn ttl(app: &App) -> Duration {
    Duration::from_secs(app.config.read().cache.ttl_hours.max(1) as u64 * 3600)
}

/// Reads the folder once, so entries from an earlier run count.
pub fn scan(app: &App) {
    {
        let mut s = app.cache.scanned.lock();
        if *s {
            return;
        }
        *s = true;
    }
    let root = dir(app);
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return;
    };
    let mut index = HashMap::new();
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.len() != 64 || p.extension().and_then(|e| e.to_str()) != Some("tkc") {
                continue;
            }
            if let Ok(m) = f.metadata() {
                index.insert(
                    stem.to_string(),
                    Meta {
                        path: p,
                        size: m.len(),
                        modified: m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    },
                );
            }
        }
    }
    *app.cache.index.lock() = index;
    sweep(app);
}

/// Drops expired entries and, if the folder is over its cap, the oldest.
pub fn sweep(app: &App) {
    let ttl = ttl(app);
    let cap = app.config.read().cache.max_mb.saturating_mul(1024 * 1024);
    let now = SystemTime::now();
    let mut gone = Vec::new();
    {
        let mut index = app.cache.index.lock();
        index.retain(|k, m| {
            let expired = now
                .duration_since(m.modified)
                .map(|d| d > ttl)
                .unwrap_or(false);
            if expired {
                gone.push((k.clone(), m.path.clone()));
            }
            !expired
        });
        let mut total: u64 = index.values().map(|m| m.size).sum();
        if total > cap {
            let mut by_age: Vec<(String, Meta)> =
                index.iter().map(|(k, m)| (k.clone(), m.clone())).collect();
            by_age.sort_by_key(|(_, m)| m.modified);
            for (k, m) in by_age {
                if total <= cap {
                    break;
                }
                total -= m.size;
                index.remove(&k);
                gone.push((k, m.path));
            }
        }
    }
    for (_, p) in &gone {
        let _ = std::fs::remove_file(p);
    }
    if !gone.is_empty() {
        log::debug!("cache: removed {} entries", gone.len());
    }
}

/// Looks an answer up. `None` on a miss or an expired or unreadable entry.
pub fn get(app: &App, key: &str) -> Option<Entry> {
    scan(app);
    let meta = app.cache.index.lock().get(key).cloned()?;
    let fresh = SystemTime::now()
        .duration_since(meta.modified)
        .map(|d| d <= ttl(app))
        .unwrap_or(true);
    if !fresh {
        app.cache.index.lock().remove(key);
        let _ = std::fs::remove_file(&meta.path);
        app.cache.stats.lock().misses += 1;
        return None;
    }
    let raw = std::fs::read(&meta.path).ok()?;
    match open(app, &raw) {
        Ok(e) => {
            app.cache.stats.lock().hits += 1;
            Some(e)
        }
        Err(err) => {
            log::debug!("cache: entry {key} unreadable ({err:#}); dropping it");
            app.cache.index.lock().remove(key);
            let _ = std::fs::remove_file(&meta.path);
            app.cache.stats.lock().misses += 1;
            None
        }
    }
}

pub fn note_miss(app: &App) {
    app.cache.stats.lock().misses += 1;
}

/// Stores an answer. Oversized or non-complete answers are not stored.
pub fn put(app: &App, key: &str, head: &Head, body: &[u8]) -> Result<()> {
    if body.len() > MAX_ENTRY_BYTES {
        bail!("answer is too big to cache");
    }
    if head.status != 200 || head.outcome != "complete" {
        bail!("only complete answers are cached");
    }
    scan(app);
    let head_json = serde_json::to_vec(head)?;
    let mut plain = Vec::with_capacity(4 + head_json.len() + body.len());
    plain.extend_from_slice(&(head_json.len() as u32).to_le_bytes());
    plain.extend_from_slice(&head_json);
    plain.extend_from_slice(body);
    let sealed = app.crypto.seal(&plain).context("sealing the cache entry")?;
    let path = path_for(app, key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &sealed)?;
    std::fs::rename(&tmp, &path)?;
    let size = sealed.len() as u64;
    app.cache.index.lock().insert(
        key.to_string(),
        Meta {
            path,
            size,
            modified: SystemTime::now(),
        },
    );
    app.cache.stats.lock().stored += 1;
    sweep(app);
    Ok(())
}

fn open(app: &App, raw: &[u8]) -> Result<Entry> {
    let plain = app.crypto.open_file(raw)?;
    if plain.len() < 4 {
        bail!("short entry");
    }
    let n = u32::from_le_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
    if plain.len() < 4 + n {
        bail!("truncated entry");
    }
    let head: Head = serde_json::from_slice(&plain[4..4 + n])?;
    Ok(Entry {
        head,
        body: plain[4 + n..].to_vec(),
    })
}

/// Empties the cache.
pub fn clear(app: &App) -> usize {
    scan(app);
    let paths: Vec<PathBuf> = {
        let mut index = app.cache.index.lock();
        let v: Vec<PathBuf> = index.values().map(|m| m.path.clone()).collect();
        index.clear();
        v
    };
    for p in &paths {
        let _ = std::fs::remove_file(p);
    }
    paths.len()
}

/// Sweeps every 10 minutes for the life of the app.
pub async fn run(app: std::sync::Arc<App>) {
    loop {
        tokio::time::sleep(Duration::from_secs(600)).await;
        let a = app.clone();
        let _ = tokio::task::spawn_blocking(move || {
            scan(&a);
            sweep(&a);
        })
        .await;
    }
}

/// For Settings: what's in the cache.
pub fn summary(app: &App) -> serde_json::Value {
    scan(app);
    let s = app.cache.stats.lock();
    serde_json::json!({
        "entries": app.cache.entries(),
        "bytes": app.cache.bytes(),
        "hits": s.hits,
        "misses": s.misses,
        "stored": s.stored,
        "dir": dir(app).display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_formatting_but_not_identity() {
        let a = key(
            "studio",
            "/v1/chat/completions",
            "tk_1",
            br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#,
        );
        let b = key(
            "studio",
            "/v1/chat/completions",
            "tk_1",
            br#"{ "messages": [ {"content":"hi","role":"user"} ], "model":"m" }"#,
        );
        assert_eq!(a, b, "key order and whitespace don't matter");
        assert_ne!(
            a,
            key(
                "studio",
                "/v1/chat/completions",
                "tk_2",
                br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#
            ),
            "another credential is another cache"
        );
        assert_ne!(
            a,
            key(
                "mini",
                "/v1/chat/completions",
                "tk_1",
                br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#
            )
        );
        assert_ne!(
            a,
            key(
                "studio",
                "/v1/completions",
                "tk_1",
                br#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#
            )
        );
    }

    #[test]
    fn modes() {
        let mut h = HeaderMap::new();
        assert_eq!(mode_from_headers(&h), Mode::Use);
        h.insert("cache-control", "no-cache".parse().unwrap());
        assert_eq!(mode_from_headers(&h), Mode::Bypass);
        h.remove("cache-control");
        h.insert("x-toktail-cache", "refresh".parse().unwrap());
        assert_eq!(mode_from_headers(&h), Mode::Refresh);
    }

    #[tokio::test]
    async fn round_trip_expiry_and_cap() {
        let app = crate::test_support::app();
        app.config.write().cache.enabled = true;
        let head = Head {
            status: 200,
            content_type: "application/json".into(),
            outcome: "complete".into(),
            model: "m".into(),
            input: 10,
            output: 5,
            ..Default::default()
        };
        let k = key("laptop", "/v1/chat/completions", "", b"{\"a\":1}");
        put(&app, &k, &head, b"{\"answer\":true}").unwrap();
        let e = get(&app, &k).expect("hit");
        assert_eq!(e.body, b"{\"answer\":true}");
        assert_eq!(e.head.output, 5);
        // A refused or failed answer is never stored.
        let bad = Head {
            outcome: "refused".into(),
            ..head.clone()
        };
        assert!(put(&app, "ff".repeat(32).as_str(), &bad, b"x").is_err());
        // Expired by file time.
        app.config.write().cache.ttl_hours = 1;
        let p = path_for(&app, &k);
        let old = SystemTime::now() - Duration::from_secs(7200);
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(old).unwrap();
        app.cache.index.lock().get_mut(&k).unwrap().modified = old;
        assert!(get(&app, &k).is_none(), "expired entries are misses");
        assert!(!p.exists());
        // The cap evicts the oldest.
        app.config.write().cache.ttl_hours = 72;
        app.config.write().cache.max_mb = 0;
        put(&app, &k, &head, b"{\"answer\":true}").unwrap();
        assert_eq!(app.cache.entries(), 0, "a zero cap keeps nothing");
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
