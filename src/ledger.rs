// SPDX-License-Identifier: Apache-2.0
//! The CSV ledger: the single source of truth.
//!
//! One append-only file per month, `tokens-YYYY-MM.csv`, one row per
//! inference request. Every number the menu bar and dashboard show is
//! computed from these files. The in-memory copy is only a cache: if a file
//! changes on disk (edited, restored, deleted) it is re-read on the next query.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, TimeZone, Timelike};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub const HEADER: [&str; 32] = [
    "request_id",
    "timestamp",
    "local_date",
    "model",
    "endpoint",
    "streamed",
    "status",
    "input_tokens",
    "output_tokens",
    "duration_ms",
    "ttft_ms",
    "output_tokens_per_sec",
    "usage_source",
    "prompt_sha256",
    "host",
    "record",
    "outcome",
    "signal",
    "response_chars",
    "prompt_blob",
    "prompt_key",
    "response_key",
    "reference_key",
    // 0.0.3: who paid, what the provider cached, Toktail's own cache, and
    // where the request sits in its conversation's chain.
    "token_id",
    "cache_read_tokens",
    "cache_write_tokens",
    "cache_hit",
    "session_id",
    "chain_node",
    "prefix_tokens",
    "agent",
    "client",
];

/// Columns added in 0.0.3. Kept together so every place that builds a row
/// only has one more field to fill, and old CSVs read back with defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// A one-way id of the credential the client sent (`tk_…`), or the id of
    /// the Toktail gateway key it used (`tkg_…`). Never the credential itself.
    pub token_id: String,
    /// Input tokens the provider served from its prompt cache (Anthropic
    /// `cache_read_input_tokens`, OpenAI `cached_tokens`). Part of `input`.
    pub cache_read: u64,
    /// Input tokens the provider wrote to its prompt cache. Part of `input`.
    pub cache_write: u64,
    /// True when Toktail answered from its own prompt cache: the counts are
    /// the original request's, nothing was generated.
    pub cache_hit: bool,
    /// The conversation this request belongs to (hex), when its prompt was
    /// saved and chained.
    pub session_id: String,
    /// This request's node in the session's Merkle chain (hex).
    pub chain_node: String,
    /// Input tokens that repeat the previous request in the same session: the
    /// part a provider would have served from cache.
    pub prefix_tokens: u64,
    /// The coding agent that sent it, when Toktail could tell:
    /// "opencode", "claudecode", "codex", "goose" or "".
    pub agent: String,
    /// What made the request on Toktail's behalf: "" for an app, "pip" for
    /// Pip's chat, "portal-chat", or "commentary" for Pip's one-liners.
    pub client: String,
}

/// One finished request, ready to be appended.
#[derive(Debug, Clone)]
pub struct Record {
    pub request_id: String,
    pub timestamp: DateTime<Local>,
    pub model: String,
    pub endpoint: String,
    pub streamed: bool,
    /// HTTP status from LM Studio; 499 = the client disconnected mid-stream.
    pub status: u16,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    /// "reported" (LM Studio's own count), "estimated" (counted stream deltas) or "none".
    pub usage_source: &'static str,
    pub prompt_sha256: String,
    /// Which watched host served it.
    pub host: String,
    /// Saved prompt/response files, as a path stem under the capture folder
    /// ("studio/2026/10/11/2026-10-11-14-03-22"). Empty when not captured.
    pub record: String,
    pub outcome: String,
    pub signal: String,
    pub response_chars: u64,
    pub prompt_blob: String,
    pub prompt_key: String,
    pub response_key: String,
    pub reference_key: String,
    pub meta: Meta,
}

/// Generation time: from first token to end for streams, whole request otherwise.
fn gen_ms(duration_ms: u64, ttft_ms: Option<u64>) -> u64 {
    match ttft_ms {
        Some(t) if t < duration_ms => duration_ms - t,
        _ => duration_ms,
    }
}

fn tps(output: u64, gen_ms: u64) -> Option<f64> {
    (output > 0 && gen_ms > 0).then(|| output as f64 * 1000.0 / gen_ms as f64)
}

/// A row as cached in memory.
#[derive(Debug, Clone)]
pub struct Row {
    pub request_id: String,
    pub ts_ms: i64,
    pub offset_secs: i32,
    pub date: NaiveDate,
    pub hour: u8,
    /// 0 = Monday
    pub weekday: u8,
    pub model: Arc<str>,
    pub endpoint: Arc<str>,
    pub streamed: bool,
    pub status: u16,
    pub input: u64,
    pub output: u64,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub gen_ms: u64,
    pub source: Arc<str>,
    pub fp: Option<[u8; 32]>,
    pub host: Arc<str>,
    pub record: Option<Box<str>>,
    pub outcome: String,
    pub signal: String,
    pub response_chars: u64,
    pub prompt_blob: String,
    pub prompt_key: String,
    pub response_key: String,
    pub reference_key: String,
    pub meta: Meta,
}

impl Row {
    pub fn timestamp(&self) -> DateTime<FixedOffset> {
        let off =
            FixedOffset::east_opt(self.offset_secs).unwrap_or(FixedOffset::east_opt(0).unwrap());
        off.timestamp_millis_opt(self.ts_ms)
            .single()
            .unwrap_or_else(|| off.timestamp_opt(0, 0).unwrap())
    }
    pub fn fp_hex(&self) -> String {
        self.fp.map(hex::encode).unwrap_or_default()
    }
    pub fn tokens_per_sec(&self) -> Option<f64> {
        tps(self.output, self.gen_ms)
    }
    fn to_csv_fields(&self) -> [String; 32] {
        [
            self.request_id.clone(),
            self.timestamp()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
            self.date.to_string(),
            self.model.to_string(),
            self.endpoint.to_string(),
            self.streamed.to_string(),
            self.status.to_string(),
            self.input.to_string(),
            self.output.to_string(),
            self.duration_ms.to_string(),
            self.ttft_ms.map(|t| t.to_string()).unwrap_or_default(),
            tps(self.output, self.gen_ms)
                .map(|t| format!("{t:.2}"))
                .unwrap_or_default(),
            self.source.to_string(),
            self.fp_hex(),
            self.host.to_string(),
            self.record.as_deref().unwrap_or("").to_string(),
            self.outcome.clone(),
            self.signal.clone(),
            self.response_chars.to_string(),
            self.prompt_blob.clone(),
            self.prompt_key.clone(),
            self.response_key.clone(),
            self.reference_key.clone(),
            self.meta.token_id.clone(),
            self.meta.cache_read.to_string(),
            self.meta.cache_write.to_string(),
            self.meta.cache_hit.to_string(),
            self.meta.session_id.clone(),
            self.meta.chain_node.clone(),
            self.meta.prefix_tokens.to_string(),
            self.meta.agent.clone(),
            self.meta.client.clone(),
        ]
    }
}

/// Columns as read back from disk. Matched by header name, so columns may be
/// reordered or added in a spreadsheet without breaking anything.
#[derive(Debug, Deserialize)]
struct CsvRow {
    #[serde(default)]
    request_id: String,
    timestamp: String,
    #[serde(default)]
    local_date: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    streamed: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    input_tokens: String,
    #[serde(default)]
    output_tokens: String,
    #[serde(default)]
    duration_ms: String,
    #[serde(default)]
    ttft_ms: String,
    #[serde(default)]
    usage_source: String,
    #[serde(default)]
    prompt_sha256: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    record: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    signal: String,
    #[serde(default)]
    response_chars: String,
    #[serde(default)]
    prompt_blob: String,
    #[serde(default)]
    prompt_key: String,
    #[serde(default)]
    response_key: String,
    #[serde(default)]
    reference_key: String,
    #[serde(default)]
    token_id: String,
    #[serde(default)]
    cache_read_tokens: String,
    #[serde(default)]
    cache_write_tokens: String,
    #[serde(default)]
    cache_hit: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    chain_node: String,
    #[serde(default)]
    prefix_tokens: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    client: String,
}

#[derive(Default)]
struct FileData {
    len: u64,
    mtime: Option<SystemTime>,
    rows: Vec<Row>,
}

#[derive(Default)]
pub struct Data {
    files: BTreeMap<String, FileData>,
    intern: HashMap<String, Arc<str>>,
    last_scan: Option<Instant>,
}

impl Data {
    fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(a) = self.intern.get(s) {
            return a.clone();
        }
        let a: Arc<str> = Arc::from(s);
        self.intern.insert(s.to_string(), a.clone());
        a
    }

    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.files.values().flat_map(|f| f.rows.iter())
    }

    pub fn file_names(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

pub struct Ledger {
    dir: PathBuf,
    data: RwLock<Data>,
    write_lock: Mutex<()>,
    /// Host for rows written before Toktail tracked more than one host.
    default_host: String,
    dirty: Mutex<HashSet<String>>,
}

pub fn file_name_for(date: NaiveDate) -> String {
    format!("tokens-{:04}-{:02}.csv", date.year(), date.month())
}

fn is_ledger_file(name: &str) -> bool {
    name.starts_with("tokens-") && name.ends_with(".csv")
}

fn meta(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

impl Ledger {
    pub fn open(dir: PathBuf, default_host: &str) -> Result<Arc<Ledger>> {
        std::fs::create_dir_all(&dir)?;
        let l = Arc::new(Ledger {
            dir,
            data: RwLock::new(Data::default()),
            write_lock: Mutex::new(()),
            default_host: default_host.to_string(),
            dirty: Mutex::new(HashSet::new()),
        });
        l.migrate_headers();
        l.refresh(true);
        Ok(l)
    }

    /// Files from before the `host` and `record` columns get them added once,
    /// so every file has the same header. Old rows belong to the default host.
    fn migrate_headers(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().to_string();
            if !is_ledger_file(&name) {
                continue;
            }
            let path = e.path();
            let header_ok = csv::Reader::from_path(&path)
                .and_then(|mut r| r.headers().cloned())
                .map(|h| h.iter().eq(HEADER.iter().copied()))
                .unwrap_or(true);
            if header_ok {
                continue;
            }
            let mut tmp = Data::default();
            match parse_file(&path, &mut tmp, &self.default_host) {
                Ok((rows, _)) => {
                    // Files from 0.4 were backed up as .pre-v0.5.bak; files from the
                    // 0.5/0.6 line get a .pre-v0.0.3.bak. Each backup is made once.
                    let backup = path.with_extension(if header_has_host(&path) {
                        "csv.pre-v0.0.3.bak"
                    } else {
                        "csv.pre-v0.5.bak"
                    });
                    if !backup.exists() && std::fs::copy(&path, &backup).is_err() {
                        log::warn!("{name}: could not create migration backup");
                        continue;
                    }
                    if let Err(e) = write_rows(&path, rows.iter()) {
                        log::warn!("{name}: could not add the new columns: {e:#}");
                    } else {
                        log::info!("{name}: brought the header up to {} columns", HEADER.len());
                    }
                }
                Err(e) => log::warn!("{name}: {e:#}"),
            }
        }
    }

    /// Removes rows by request id, rewriting only the files that hold them.
    /// Returns how many rows were removed.
    pub fn delete_rows(&self, ids: &HashSet<String>) -> Result<usize> {
        self.refresh(true);
        let _w = self.write_lock.lock();
        let mut removed = 0;
        let targets: Vec<(String, Vec<Row>)> = {
            let data = self.data.read();
            data.files
                .iter()
                .filter(|(_, f)| f.rows.iter().any(|r| ids.contains(&r.request_id)))
                .map(|(n, f)| {
                    (
                        n.clone(),
                        f.rows
                            .iter()
                            .filter(|r| !ids.contains(&r.request_id))
                            .cloned()
                            .collect(),
                    )
                })
                .collect()
        };
        for (name, keep) in targets {
            let before = self
                .data
                .read()
                .files
                .get(&name)
                .map_or(0, |f| f.rows.len());
            removed += before - keep.len();
            write_rows(&self.dir.join(&name), keep.iter())?;
        }
        drop(_w);
        self.refresh(true);
        Ok(removed)
    }

    /// Clears the `record` column (and the key ids that went with the files)
    /// of these rows: their files were deleted but the counts stay. The CSV is
    /// the token history, so losing a saved prompt never loses a count.
    /// Returns how many rows changed.
    pub fn forget_records(&self, ids: &HashSet<String>) -> Result<usize> {
        self.refresh(true);
        let _w = self.write_lock.lock();
        let mut changed = 0;
        let targets: Vec<(String, Vec<Row>)> = {
            let data = self.data.read();
            data.files
                .iter()
                .filter(|(_, f)| {
                    f.rows
                        .iter()
                        .any(|r| r.record.is_some() && ids.contains(&r.request_id))
                })
                .map(|(n, f)| {
                    let rows = f
                        .rows
                        .iter()
                        .map(|r| {
                            let mut r = r.clone();
                            if r.record.is_some() && ids.contains(&r.request_id) {
                                r.record = None;
                                r.prompt_blob.clear();
                                r.prompt_key.clear();
                                r.response_key.clear();
                                r.reference_key.clear();
                                changed += 1;
                            }
                            r
                        })
                        .collect();
                    (n.clone(), rows)
                })
                .collect()
        };
        for (name, rows) in targets {
            write_rows(&self.dir.join(&name), rows.iter())?;
        }
        drop(_w);
        self.refresh(true);
        Ok(changed)
    }

    /// Persist lazily discovered signals for legacy captures in one rewrite per month.
    pub fn update_signals(&self, changes: &HashMap<String, (String, String, u64)>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        self.refresh(true);
        let guard = self.write_lock.lock();
        let targets: Vec<_> = self
            .data
            .read()
            .files
            .iter()
            .filter(|(_, f)| f.rows.iter().any(|r| changes.contains_key(&r.request_id)))
            .map(|(name, f)| {
                let mut rows = f.rows.clone();
                for r in &mut rows {
                    if let Some((outcome, signal, chars)) = changes.get(&r.request_id) {
                        r.outcome = outcome.clone();
                        r.signal = signal.clone();
                        r.response_chars = *chars;
                    }
                }
                (name.clone(), rows)
            })
            .collect();
        for (name, rows) in targets {
            write_rows(&self.dir.join(name), rows.iter())?;
        }
        drop(guard);
        self.refresh(true);
        Ok(())
    }

    /// One row by request id.
    pub fn find(&self, id: &str) -> Option<Row> {
        self.with_data(|d| d.rows().find(|r| r.request_id == id).cloned())
    }

    pub fn flush(&self) -> Result<()> {
        let _w = self.write_lock.lock();
        let names = self.dirty.lock().clone();
        for name in names {
            OpenOptions::new()
                .write(true)
                .open(self.dir.join(&name))?
                .sync_data()?;
            self.dirty.lock().remove(&name);
        }
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends immediately to CSV/cache; durability is grouped every two seconds.
    pub fn append(&self, rec: &Record) -> Result<()> {
        let _w = self.write_lock.lock();
        let date = rec.timestamp.date_naive();
        let name = file_name_for(date);
        let path = self.dir.join(&name);
        let before = meta(&path);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let needs_header = before.is_none_or(|(len, _)| len == 0);
        let gen = gen_ms(rec.duration_ms, rec.ttft_ms);
        let mut buf = Vec::new();
        {
            let mut w = csv::WriterBuilder::new()
                .has_headers(false)
                .from_writer(&mut buf);
            if needs_header {
                w.write_record(HEADER)?;
            }
            w.write_record([
                rec.request_id.as_str(),
                &rec.timestamp
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                &date.to_string(),
                &rec.model,
                &rec.endpoint,
                if rec.streamed { "true" } else { "false" },
                &rec.status.to_string(),
                &rec.input_tokens.to_string(),
                &rec.output_tokens.to_string(),
                &rec.duration_ms.to_string(),
                &rec.ttft_ms.map(|t| t.to_string()).unwrap_or_default(),
                &tps(rec.output_tokens, gen)
                    .map(|t| format!("{t:.2}"))
                    .unwrap_or_default(),
                rec.usage_source,
                &rec.prompt_sha256,
                &rec.host,
                &rec.record,
                &rec.outcome,
                &rec.signal,
                &rec.response_chars.to_string(),
                &rec.prompt_blob,
                &rec.prompt_key,
                &rec.response_key,
                &rec.reference_key,
                &rec.meta.token_id,
                &rec.meta.cache_read.to_string(),
                &rec.meta.cache_write.to_string(),
                &rec.meta.cache_hit.to_string(),
                &rec.meta.session_id,
                &rec.meta.chain_node,
                &rec.meta.prefix_tokens.to_string(),
                &rec.meta.agent,
                &rec.meta.client,
            ])?;
            w.flush()?;
        }
        file.write_all(&buf)?;
        self.dirty.lock().insert(name.clone());
        drop(file);
        let after = meta(&path);

        // Keep the cache in step if it was in step before this write;
        // otherwise leave it stale so the next query re-reads the file.
        let mut data = self.data.write();
        let in_sync = match (data.files.get(&name), before) {
            (Some(f), Some((len, mtime))) => f.len == len && f.mtime == mtime,
            (None, None) => true,
            (None, Some((0, _))) => true,
            _ => false,
        };
        if in_sync {
            let model = data.intern(&rec.model);
            let endpoint = data.intern(&rec.endpoint);
            let source = data.intern(rec.usage_source);
            let host = data.intern(&rec.host);
            let ts = rec.timestamp.fixed_offset();
            let row = Row {
                request_id: rec.request_id.clone(),
                ts_ms: ts.timestamp_millis(),
                offset_secs: ts.offset().local_minus_utc(),
                date,
                hour: ts.hour() as u8,
                weekday: ts.weekday().num_days_from_monday() as u8,
                model,
                endpoint,
                streamed: rec.streamed,
                status: rec.status,
                input: rec.input_tokens,
                output: rec.output_tokens,
                duration_ms: rec.duration_ms,
                ttft_ms: rec.ttft_ms,
                gen_ms: gen,
                source,
                fp: parse_fp(&rec.prompt_sha256),
                host,
                record: (!rec.record.is_empty()).then(|| rec.record.clone().into_boxed_str()),
                outcome: rec.outcome.clone(),
                signal: rec.signal.clone(),
                response_chars: rec.response_chars,
                prompt_blob: rec.prompt_blob.clone(),
                prompt_key: rec.prompt_key.clone(),
                response_key: rec.response_key.clone(),
                reference_key: rec.reference_key.clone(),
                meta: rec.meta.clone(),
            };
            let f = data.files.entry(name).or_default();
            f.rows.push(row);
            if let Some((len, mtime)) = after {
                f.len = len;
                f.mtime = mtime;
            }
        }
        Ok(())
    }

    /// Re-reads any CSV whose size or modification time changed since it was
    /// cached, and drops files that disappeared. Rate-limited unless forced.
    pub fn refresh(&self, force: bool) {
        {
            let d = self.data.read();
            if !force
                && d.last_scan
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
            {
                return;
            }
        }
        let _w = self.write_lock.lock();
        let mut names: Vec<String> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| is_ledger_file(n))
                .collect(),
            Err(e) => {
                log::warn!("cannot list {}: {e}", self.dir.display());
                return;
            }
        };
        names.sort();
        let mut data = self.data.write();
        data.files.retain(|k, _| names.contains(k));
        for name in names {
            let path = self.dir.join(&name);
            let Some((len, mtime)) = meta(&path) else {
                continue;
            };
            let fresh = data
                .files
                .get(&name)
                .is_some_and(|f| f.len == len && f.mtime == mtime);
            if fresh {
                continue;
            }
            match parse_file(&path, &mut data, &self.default_host) {
                Ok((rows, bad)) => {
                    if bad > 0 {
                        log::warn!("{name}: skipped {bad} unreadable rows");
                    }
                    data.files.insert(name, FileData { len, mtime, rows });
                }
                Err(e) => log::warn!("{name}: {e:#}"),
            }
        }
        data.last_scan = Some(Instant::now());
    }

    /// Runs `f` against an up-to-date view of the CSV data.
    pub fn with_data<T>(&self, f: impl FnOnce(&Data) -> T) -> T {
        self.refresh(false);
        f(&self.data.read())
    }
}

/// True when a CSV already has the 0.5-era columns (so a migration is from
/// 0.5/0.6, not from 0.4).
fn header_has_host(path: &Path) -> bool {
    csv::Reader::from_path(path)
        .and_then(|mut r| r.headers().cloned())
        .map(|h| h.iter().any(|c| c == "host"))
        .unwrap_or(false)
}

fn parse_fp(s: &str) -> Option<[u8; 32]> {
    let v = hex::decode(s.trim()).ok()?;
    v.try_into().ok()
}

/// Writes rows (with the current header) to `path` atomically.
fn write_rows<'a>(path: &Path, rows: impl Iterator<Item = &'a Row>) -> Result<()> {
    let tmp = path.with_extension("csv.tmp");
    {
        let mut w = csv::Writer::from_path(&tmp)?;
        w.write_record(HEADER)?;
        for r in rows {
            w.write_record(r.to_csv_fields())?;
        }
        w.flush()?;
        w.into_inner()?.sync_data()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn parse_file(path: &Path, data: &mut Data, default_host: &str) -> Result<(Vec<Row>, usize)> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_path(path)?;
    let mut rows = Vec::new();
    let mut bad = 0;
    for rec in rdr.deserialize::<CsvRow>() {
        let Ok(r) = rec else {
            bad += 1;
            continue;
        };
        let Ok(ts) = DateTime::parse_from_rfc3339(r.timestamp.trim()) else {
            bad += 1;
            continue;
        };
        let date =
            NaiveDate::parse_from_str(r.local_date.trim(), "%Y-%m-%d").unwrap_or(ts.date_naive());
        let num = |s: &str| s.trim().parse::<u64>().unwrap_or(0);
        let ttft = r.ttft_ms.trim().parse::<u64>().ok();
        let duration_ms = num(&r.duration_ms);
        let model = if r.model.trim().is_empty() {
            "unknown"
        } else {
            r.model.trim()
        };
        let source = if r.usage_source.trim().is_empty() {
            "reported"
        } else {
            r.usage_source.trim()
        };
        rows.push(Row {
            request_id: r.request_id,
            ts_ms: ts.timestamp_millis(),
            offset_secs: ts.offset().local_minus_utc(),
            date,
            hour: ts.hour() as u8,
            weekday: ts.weekday().num_days_from_monday() as u8,
            model: data.intern(model),
            endpoint: data.intern(r.endpoint.trim()),
            streamed: r.streamed.trim().eq_ignore_ascii_case("true"),
            status: r.status.trim().parse().unwrap_or(200),
            input: num(&r.input_tokens),
            output: num(&r.output_tokens),
            duration_ms,
            ttft_ms: ttft,
            gen_ms: gen_ms(duration_ms, ttft),
            source: data.intern(source),
            fp: parse_fp(&r.prompt_sha256),
            host: data.intern(if r.host.trim().is_empty() {
                default_host
            } else {
                r.host.trim()
            }),
            record: (!r.record.trim().is_empty())
                .then(|| r.record.trim().to_string().into_boxed_str()),
            outcome: r.outcome,
            signal: r.signal,
            response_chars: num(&r.response_chars),
            prompt_blob: r.prompt_blob,
            prompt_key: r.prompt_key,
            response_key: r.response_key,
            reference_key: r.reference_key,
            meta: Meta {
                token_id: r.token_id.trim().to_string(),
                cache_read: num(&r.cache_read_tokens),
                cache_write: num(&r.cache_write_tokens),
                cache_hit: r.cache_hit.trim().eq_ignore_ascii_case("true"),
                session_id: r.session_id.trim().to_string(),
                chain_node: r.chain_node.trim().to_string(),
                prefix_tokens: num(&r.prefix_tokens),
                agent: r.agent.trim().to_string(),
                client: r.client.trim().to_string(),
            },
        });
    }
    Ok((rows, bad))
}

// ---------------------------------------------------------------- queries

#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub models: Option<HashSet<String>>,
    pub hosts: Option<HashSet<String>>,
}

impl Filter {
    pub fn matches(&self, r: &Row) -> bool {
        self.from.is_none_or(|f| r.date >= f)
            && self.to.is_none_or(|t| r.date <= t)
            && self.models.as_ref().is_none_or(|m| m.contains(&*r.model))
            && self.hosts.as_ref().is_none_or(|h| h.contains(&*r.host))
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Totals {
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    /// Output tokens and milliseconds from requests with a measurable generation time.
    pub gen_out: u64,
    pub gen_ms: u64,
    pub errors: u64,
    pub estimated: u64,
    /// Requests Toktail answered from its own prompt cache (their tokens are
    /// counted above, since the client received them, but nothing ran).
    pub cache_hits: u64,
    /// Tokens (in + out) those hits handed back without a model running.
    pub cache_saved: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Totals {
    pub fn add(&mut self, r: &Row) {
        self.requests += 1;
        self.input += r.input;
        self.output += r.output;
        self.cache_read += r.meta.cache_read;
        self.cache_write += r.meta.cache_write;
        if r.meta.cache_hit {
            self.cache_hits += 1;
            self.cache_saved += r.input + r.output;
        }
        if r.output > 0 && r.gen_ms > 0 && !r.meta.cache_hit {
            self.gen_out += r.output;
            self.gen_ms += r.gen_ms;
        }
        if r.status >= 400 {
            self.errors += 1;
        }
        if &*r.source == "estimated" {
            self.estimated += 1;
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DailyEntry {
    pub date: NaiveDate,
    pub host: String,
    pub model: String,
    #[serde(flatten)]
    pub totals: Totals,
}

/// Totals per (local date, host, model), so the portal can split by either.
pub fn daily(data: &Data, f: &Filter) -> Vec<DailyEntry> {
    let mut map: BTreeMap<(NaiveDate, Arc<str>, Arc<str>), Totals> = BTreeMap::new();
    for r in data.rows().filter(|r| f.matches(r)) {
        map.entry((r.date, r.host.clone(), r.model.clone()))
            .or_default()
            .add(r);
    }
    map.into_iter()
        .map(|((date, host, model), totals)| DailyEntry {
            date,
            host: host.to_string(),
            model: model.to_string(),
            totals,
        })
        .collect()
}

/// Today's totals per host, for the menu bar and the host strip.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn day_by_host(data: &Data, day: NaiveDate) -> BTreeMap<String, Totals> {
    let mut map: BTreeMap<String, Totals> = BTreeMap::new();
    for r in data.rows().filter(|r| r.date == day) {
        map.entry(r.host.to_string()).or_default().add(r);
    }
    map
}

#[derive(Debug, Serialize)]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub struct ModelTotals {
    pub model: String,
    #[serde(flatten)]
    pub totals: Totals,
}

/// Today's totals per model, for the menu bar.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn day_by_model(data: &Data, day: NaiveDate) -> Vec<ModelTotals> {
    let mut map: BTreeMap<Arc<str>, Totals> = BTreeMap::new();
    for r in data.rows().filter(|r| r.date == day) {
        map.entry(r.model.clone()).or_default().add(r);
    }
    let mut v: Vec<ModelTotals> = map
        .into_iter()
        .map(|(m, totals)| ModelTotals {
            model: m.to_string(),
            totals,
        })
        .collect();
    v.sort_by_key(|m| std::cmp::Reverse(m.totals.output));
    v
}

#[derive(Debug, Serialize)]
pub struct HeatCell {
    pub weekday: u8,
    pub hour: u8,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
}

/// Activity by weekday and hour of day (local time at capture).
pub fn heatmap(data: &Data, f: &Filter) -> Vec<HeatCell> {
    let mut grid = [[(0u64, 0u64, 0u64); 24]; 7];
    for r in data.rows().filter(|r| f.matches(r)) {
        let c = &mut grid[r.weekday.min(6) as usize][r.hour.min(23) as usize];
        c.0 += 1;
        c.1 += r.input;
        c.2 += r.output;
    }
    let mut out = Vec::with_capacity(168);
    for (wd, hours) in grid.iter().enumerate() {
        for (h, c) in hours.iter().enumerate() {
            out.push(HeatCell {
                weekday: wd as u8,
                hour: h as u8,
                requests: c.0,
                input: c.1,
                output: c.2,
            });
        }
    }
    out
}

#[derive(Debug, Serialize)]
pub struct RecentRow {
    pub request_id: String,
    pub timestamp: String,
    pub host: String,
    pub model: String,
    pub endpoint: String,
    pub streamed: bool,
    pub status: u16,
    pub input: u64,
    pub output: u64,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub source: String,
    pub prompt_sha256: String,
    /// True when the prompt and response were saved.
    pub captured: bool,
    pub outcome: String,
    pub signal: String,
    pub response_chars: u64,
    /// Their path stem under the capture folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(flatten)]
    pub meta: Meta,
}

impl From<&Row> for RecentRow {
    fn from(r: &Row) -> Self {
        RecentRow {
            request_id: r.request_id.clone(),
            timestamp: r
                .timestamp()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            host: r.host.to_string(),
            model: r.model.to_string(),
            endpoint: r.endpoint.to_string(),
            streamed: r.streamed,
            status: r.status,
            input: r.input,
            output: r.output,
            duration_ms: r.duration_ms,
            ttft_ms: r.ttft_ms,
            tokens_per_sec: tps(r.output, r.gen_ms).map(|t| (t * 10.0).round() / 10.0),
            source: r.source.to_string(),
            prompt_sha256: r.fp_hex(),
            captured: r.record.is_some(),
            outcome: r.outcome.clone(),
            signal: r.signal.clone(),
            response_chars: r.response_chars,
            record: r.record.as_deref().map(String::from),
            meta: r.meta.clone(),
        }
    }
}

pub fn recent(data: &Data, f: &Filter, limit: usize) -> Vec<RecentRow> {
    let mut rows: Vec<&Row> = data.rows().filter(|r| f.matches(r)).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.ts_ms));
    rows.into_iter().take(limit).map(RecentRow::from).collect()
}

/// A page of requests for the prompt browser, newest first (or by `sort`).
pub fn page(
    data: &Data,
    f: &Filter,
    keep: impl Fn(&Row) -> bool,
    sort: &str,
    offset: usize,
    limit: usize,
) -> (usize, Vec<RecentRow>) {
    let mut rows: Vec<&Row> = data.rows().filter(|r| f.matches(r) && keep(r)).collect();
    match sort {
        "oldest" => rows.sort_by_key(|r| r.ts_ms),
        "output" => rows.sort_by_key(|r| std::cmp::Reverse(r.output)),
        "input" => rows.sort_by_key(|r| std::cmp::Reverse(r.input)),
        "fastest" => rows.sort_by(|a, b| {
            b.tokens_per_sec()
                .unwrap_or(0.0)
                .total_cmp(&a.tokens_per_sec().unwrap_or(0.0))
        }),
        "slowest" => rows.sort_by(|a, b| {
            a.tokens_per_sec()
                .unwrap_or(f64::MAX)
                .total_cmp(&b.tokens_per_sec().unwrap_or(f64::MAX))
        }),
        "longest" => rows.sort_by_key(|r| std::cmp::Reverse(r.duration_ms)),
        "combined" => rows.sort_by_key(|r| std::cmp::Reverse(r.input + r.output)),
        _ => rows.sort_by_key(|r| std::cmp::Reverse(r.ts_ms)),
    }
    let total = rows.len();
    (
        total,
        rows.into_iter()
            .skip(offset)
            .take(limit)
            .map(RecentRow::from)
            .collect(),
    )
}

/// Where `value` falls among `values` (0 = lowest, 100 = highest).
pub fn percentile_rank(values: &mut [f64], value: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let below = values.partition_point(|v| *v < value);
    let equal = values[below..].partition_point(|v| *v <= value);
    Some(((below as f64 + equal as f64 / 2.0) / values.len() as f64 * 100.0).clamp(0.0, 100.0))
}

#[derive(Debug, Serialize)]
pub struct Repeat {
    pub prompt_sha256: String,
    pub count: u64,
    pub input_tokens: u64,
    pub first_seen: String,
    pub last_seen: String,
    pub models: Vec<String>,
}

/// Prompts sent more than once (by fingerprint), most repeated first.
pub fn repeats(data: &Data, f: &Filter, limit: usize) -> (u64, Vec<Repeat>) {
    struct Acc {
        count: u64,
        input: u64,
        first: i64,
        last: i64,
        first_row: Option<(i64, i32)>,
        last_row: Option<(i64, i32)>,
        models: HashSet<Arc<str>>,
    }
    let mut map: HashMap<[u8; 32], Acc> = HashMap::new();
    for r in data.rows().filter(|r| f.matches(r)) {
        let Some(fp) = r.fp else { continue };
        let a = map.entry(fp).or_insert(Acc {
            count: 0,
            input: 0,
            first: i64::MAX,
            last: i64::MIN,
            first_row: None,
            last_row: None,
            models: HashSet::new(),
        });
        a.count += 1;
        a.input += r.input;
        if r.ts_ms < a.first {
            a.first = r.ts_ms;
            a.first_row = Some((r.ts_ms, r.offset_secs));
        }
        if r.ts_ms > a.last {
            a.last = r.ts_ms;
            a.last_row = Some((r.ts_ms, r.offset_secs));
        }
        a.models.insert(r.model.clone());
    }
    let fmt = |x: Option<(i64, i32)>| {
        x.and_then(|(ms, off)| {
            FixedOffset::east_opt(off)?
                .timestamp_millis_opt(ms)
                .single()
        })
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_default()
    };
    let mut reps: Vec<(&[u8; 32], &Acc)> = map.iter().filter(|(_, a)| a.count > 1).collect();
    let repeated_requests: u64 = reps.iter().map(|(_, a)| a.count - 1).sum();
    reps.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.input.cmp(&a.1.input)));
    let out = reps
        .into_iter()
        .take(limit)
        .map(|(fp, a)| {
            let mut models: Vec<String> = a.models.iter().map(|m| m.to_string()).collect();
            models.sort();
            Repeat {
                prompt_sha256: hex::encode(fp),
                count: a.count,
                input_tokens: a.input,
                first_seen: fmt(a.first_row),
                last_seen: fmt(a.last_row),
                models,
            }
        })
        .collect();
    (repeated_requests, out)
}

/// Rows matching the filter, re-serialized in the ledger's own CSV format.
pub fn export_csv(data: &Data, f: &Filter) -> Result<Vec<u8>> {
    let mut rows: Vec<&Row> = data.rows().filter(|r| f.matches(r)).collect();
    rows.sort_by_key(|r| r.ts_ms);
    let mut w = csv::Writer::from_writer(Vec::new());
    w.write_record(HEADER)?;
    for r in rows {
        w.write_record(r.to_csv_fields())?;
    }
    w.into_inner().map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(model: &str, inp: u64, out: u64, fp: &str) -> Record {
        Record {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Local::now(),
            model: model.into(),
            endpoint: "/v1/chat/completions".into(),
            streamed: true,
            status: 200,
            input_tokens: inp,
            output_tokens: out,
            duration_ms: 2000,
            ttft_ms: Some(500),
            usage_source: "reported",
            prompt_sha256: fp.into(),
            host: "studio".into(),
            record: String::new(),
            outcome: String::new(),
            signal: String::new(),
            response_chars: 0,
            prompt_blob: String::new(),
            prompt_key: String::new(),
            response_key: String::new(),
            reference_key: String::new(),
            meta: Meta::default(),
        }
    }

    #[test]
    fn records_hosts_paging_and_forgetting() {
        let dir = std::env::temp_dir().join(format!("tl-test-{}", uuid::Uuid::new_v4()));
        let l = Ledger::open(dir.clone(), "studio").unwrap();
        let mut a = rec("qwen", 100, 50, "");
        a.record = "studio/2026/10/11/2026-10-11-14-03-22".into();
        let mut b = rec("gemma", 300, 10, "");
        b.host = "mini".into();
        l.append(&a).unwrap();
        l.append(&b).unwrap();
        let f = Filter {
            hosts: Some(["mini".to_string()].into_iter().collect()),
            ..Default::default()
        };
        let (n, rows) = l.with_data(|d| page(d, &f, |_| true, "newest", 0, 10));
        assert_eq!((n, rows[0].host.as_str()), (1, "mini"));
        let (n, rows) = l.with_data(|d| {
            page(
                d,
                &Filter::default(),
                |r| r.record.is_some(),
                "combined",
                0,
                10,
            )
        });
        assert_eq!(n, 1);
        assert_eq!(
            rows[0].record.as_deref(),
            Some("studio/2026/10/11/2026-10-11-14-03-22")
        );
        let ids: HashSet<String> = [a.request_id.clone()].into_iter().collect();
        assert_eq!(l.forget_records(&ids).unwrap(), 1);
        let row = l.find(&a.request_id).unwrap();
        assert!(row.record.is_none());
        assert_eq!(row.output, 50, "the counts stay");
        let by = l.with_data(|d| day_by_host(d, Local::now().date_naive()));
        assert_eq!(by["mini"].input, 300);
    }

    #[test]
    fn append_then_read_back_and_detect_external_edits() {
        let dir = std::env::temp_dir().join(format!("tl-test-{}", uuid::Uuid::new_v4()));
        let l = Ledger::open(dir.clone(), "studio").unwrap();
        let fp = "ab".repeat(32);
        l.append(&rec("qwen", 100, 50, &fp)).unwrap();
        l.append(&rec("llama, \"quoted\"", 10, 5, &fp)).unwrap();
        let today = Local::now().date_naive();
        let by = l.with_data(|d| day_by_model(d, today));
        assert_eq!(by.len(), 2);
        assert_eq!(by[0].model, "qwen");
        assert_eq!(by[0].totals.output, 50);
        assert_eq!(by[0].totals.gen_ms, 1500);

        // A fresh ledger (cold cache) reads the same numbers from disk.
        let l2 = Ledger::open(dir.clone(), "studio").unwrap();
        let (n, reps) = l2.with_data(|d| repeats(d, &Filter::default(), 10));
        assert_eq!(n, 1);
        assert_eq!(reps[0].count, 2);
        assert_eq!(reps[0].models.len(), 2);

        // Editing the file on disk is picked up: it is the source of truth.
        let path = dir.join(file_name_for(today));
        let text = std::fs::read_to_string(&path).unwrap();
        let first_data_line = text.lines().nth(1).unwrap().to_string();
        std::fs::write(
            &path,
            format!("{}\n{}\n", HEADER.join(","), first_data_line),
        )
        .unwrap();
        l.refresh(true);
        let total: u64 = l.with_data(|d| d.rows().map(|r| r.output).sum());
        assert_eq!(total, 50);

        let csv =
            String::from_utf8(l.with_data(|d| export_csv(d, &Filter::default())).unwrap()).unwrap();
        assert!(csv.starts_with("request_id,timestamp"));
        std::fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn old_files_get_host_and_record_columns() {
        let dir = std::env::temp_dir().join(format!("tl-mig-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tokens-2026-09.csv"),
            "request_id,timestamp,local_date,model,endpoint,streamed,status,input_tokens,output_tokens,duration_ms,ttft_ms,output_tokens_per_sec,usage_source,prompt_sha256\n\
             a,2026-09-01T10:00:00.000-07:00,2026-09-01,m,/v1/chat/completions,true,200,10,5,100,10,55.56,reported,\n",
        )
        .unwrap();
        let l = Ledger::open(dir.clone(), "studio").unwrap();
        let text = std::fs::read_to_string(dir.join("tokens-2026-09.csv")).unwrap();
        assert_eq!(text.lines().next().unwrap(), HEADER.join(","));
        assert!(dir.join("tokens-2026-09.csv.pre-v0.5.bak").exists());
        assert_eq!(&*l.find("a").unwrap().host, "studio");
        std::fs::remove_dir_all(dir).ok();
    }
}
