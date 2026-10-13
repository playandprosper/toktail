// SPDX-License-Identifier: Apache-2.0
//! A bounded in-memory lexical index. Saved bodies are decrypted only on demand;
//! there is no secondary plaintext database or embedding service.
use crate::{
    app::App,
    ledger::{Filter, Row},
    records::{self, Kind},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 48 * 1024 * 1024;
const MAX_DOCS: usize = 10_000;
#[derive(Default)]
struct Cache {
    docs: HashMap<String, Doc>,
    bytes: usize,
    failed: HashMap<String, Instant>,
}
pub struct Index(Mutex<Cache>);
#[derive(Clone)]
struct Doc {
    row: Row,
    prompt: String,
    response: String,
    terms: HashMap<String, usize>,
    size: usize,
}

pub fn tokens(s: &str) -> Vec<String> {
    let stop = [
        "the",
        "a",
        "an",
        "and",
        "or",
        "of",
        "to",
        "for",
        "in",
        "on",
        "my",
        "me",
        "show",
        "find",
        "prompts",
        "models",
        "how",
        "many",
        "did",
        "last",
        "month",
        "processed",
        "please",
        "history",
        "saved",
        "records",
        "model",
        "that",
        "were",
        "please",
        "answer",
        "what",
        "which",
        "with",
    ];
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !stop.contains(t))
        .take(2048)
        .map(String::from)
        .collect()
}
fn clipped(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
impl Index {
    pub fn new() -> Self {
        Self(Mutex::new(Cache::default()))
    }
    pub fn clear(&self) {
        *self.0.lock() = Cache::default();
    }
    pub fn remove(&self, id: &str) {
        let mut c = self.0.lock();
        if let Some(d) = c.docs.remove(id) {
            c.bytes = c.bytes.saturating_sub(d.size);
        }
        c.failed.remove(id);
    }
    pub fn preview(&self, id: &str) -> Option<String> {
        self.0.lock().docs.get(id).map(|d| clipped(&d.prompt, 180))
    }
    pub fn insert(&self, mut row: Row, prompt: &[u8], response: &[u8]) {
        if row.outcome.is_empty() {
            let (o, s, n) = crate::intelligence::classify(row.status, response);
            row.outcome = o;
            row.signal = s;
            row.response_chars = n;
        }
        let p = records::summarize_prompt(prompt);
        let msgs = p["messages"].as_array();
        let prompt = msgs
            .map(|a| {
                a.iter()
                    .rev()
                    .filter(|m| m["role"] == "user")
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .filter_map(|m| m["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let r = records::summarize_response(response);
        let prompt = clipped(&prompt, 6000);
        let response = clipped(r["content"].as_str().unwrap_or(""), 10000);
        let mut terms = HashMap::new();
        for t in tokens(&format!("{} {} {prompt} {response}", row.model, row.host)) {
            *terms.entry(t).or_insert(0) += 1;
        }
        let size = prompt.len() + response.len() + terms.len() * 80 + 3000;
        let mut c = self.0.lock();
        if c.docs.contains_key(&row.request_id)
            || c.docs.len() >= MAX_DOCS
            || c.bytes + size > MAX_BYTES
        {
            return;
        }
        c.bytes += size;
        c.failed.remove(&row.request_id);
        c.docs.insert(
            row.request_id.clone(),
            Doc {
                row,
                prompt,
                response,
                terms,
                size,
            },
        );
    }
    fn contains(&self, id: &str) -> bool {
        let c = self.0.lock();
        c.docs.contains_key(id)
            || c.failed
                .get(id)
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
            || c.bytes > MAX_BYTES * 9 / 10
    }
    pub fn matching_ids(&self, query: &str) -> HashSet<String> {
        let q = tokens(query);
        if q.is_empty() {
            return HashSet::new();
        }
        self.0
            .lock()
            .docs
            .iter()
            .filter(|(_, d)| q.iter().any(|t| d.terms.contains_key(t)))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Search {
    #[serde(default)]
    pub query: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub host: Option<String>,
    pub model: Option<String>,
    pub outcome: Option<String>,
    pub signal: Option<String>,
    pub limit: Option<usize>,
}
impl Search {
    pub fn filter(&self) -> Filter {
        Filter {
            from: self.from.as_deref().and_then(|s| s.parse().ok()),
            to: self.to.as_deref().and_then(|s| s.parse().ok()),
            hosts: self
                .host
                .as_ref()
                .map(|s| [s.clone()].into_iter().collect()),
            models: self
                .model
                .as_ref()
                .map(|s| [s.clone()].into_iter().collect()),
        }
    }
    fn matches(&self, r: &Row) -> bool {
        self.filter().matches(r)
            && self
                .outcome
                .as_ref()
                .is_none_or(|s| r.outcome.is_empty() || &r.outcome == s)
            && self.signal.as_ref().is_none_or(|s| {
                r.signal.is_empty()
                    || &r.signal == s
                    || (s == "noteworthy"
                        && ["structured", "substantial", "tool_call"].contains(&r.signal.as_str()))
            })
    }
}
#[derive(Serialize)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub model: String,
    pub host: String,
    pub date: String,
    pub outcome: String,
    pub signal: String,
    pub prompt: String,
    pub response: String,
    pub score: f64,
    pub href: String,
}

async fn hydrate(app: &Arc<App>, search: &Search, count: usize) {
    app.retrieval
        .0
        .lock()
        .failed
        .retain(|_, t| t.elapsed() < Duration::from_secs(60));
    let mut rows = app.ledger.with_data(|d| {
        d.rows()
            .filter(|r| r.record.is_some() && search.matches(r))
            .cloned()
            .collect::<Vec<_>>()
    });
    rows.sort_by_key(|r| std::cmp::Reverse(r.ts_ms));
    rows.retain(|r| !app.retrieval.contains(&r.request_id));
    let a = app.clone();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut changes = HashMap::new();
    for row in rows.into_iter().take(count) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let stem = row.record.as_deref().unwrap();
        let files = tokio::time::timeout(remaining, async {
            (
                records::read(&a, stem, Kind::Prompt).await,
                records::read(&a, stem, Kind::Response).await,
            )
        })
        .await;
        match files {
            Ok((Ok(p), Ok(r))) => {
                if row.outcome.is_empty() {
                    let (o, s, n) = crate::intelligence::classify(row.status, &r);
                    changes.insert(row.request_id.clone(), (o, s, n));
                }
                a.retrieval.insert(row, &p, &r);
            }
            Err(_) => break,
            _ => {
                a.retrieval
                    .0
                    .lock()
                    .failed
                    .insert(row.request_id, Instant::now());
            }
        }
    }
    let a = a.clone();
    let _ = tokio::task::spawn_blocking(move || a.ledger.update_signals(&changes)).await;
}

pub async fn warm(app: &Arc<App>) {
    hydrate(app, &Search::default(), 500).await;
}

pub async fn search(app: &Arc<App>, q: &Search) -> serde_json::Value {
    hydrate(app, q, 800).await;
    let rows = app.ledger.with_data(|d| {
        d.rows()
            .filter(|r| r.record.is_some() && q.matches(r))
            .cloned()
            .collect::<Vec<_>>()
    });
    let ids: HashSet<_> = rows.iter().map(|r| r.request_id.clone()).collect();
    let notes = app.records.notes_snapshot();
    let terms = tokens(&q.query)
        .into_iter()
        .take(32)
        .collect::<HashSet<_>>();
    let cache = app.retrieval.0.lock();
    let docs: Vec<_> = cache
        .docs
        .values()
        .filter(|d| ids.contains(&d.row.request_id))
        .collect();
    let total = docs.len() as f64;
    let avg = docs
        .iter()
        .map(|d| d.terms.values().sum::<usize>())
        .sum::<usize>() as f64
        / total.max(1.0);
    let frequencies: HashMap<_, _> = terms
        .iter()
        .map(|t| {
            (
                t.clone(),
                docs.iter().filter(|d| d.terms.contains_key(t)).count() as f64,
            )
        })
        .collect();
    let mut hits = Vec::new();
    for d in &docs {
        if q.outcome.as_ref().is_some_and(|s| &d.row.outcome != s) {
            continue;
        }
        if q.signal.as_ref().is_some_and(|s| {
            if s == "noteworthy" {
                !["structured", "substantial", "tool_call"].contains(&d.row.signal.as_str())
            } else {
                &d.row.signal != s
            }
        }) {
            continue;
        }
        let note = d.row.record.as_deref().and_then(|s| notes.get(s));
        let note_text = note
            .map(|n| format!("{} {} {}", n.title, n.notes, n.tags.join(" ")))
            .unwrap_or_default()
            .to_lowercase();
        let mut score = 0.0;
        for t in &terms {
            let tf = *d.terms.get(t).unwrap_or(&0) as f64;
            let df = *frequencies.get(t).unwrap_or(&0.0);
            let len = d.terms.values().sum::<usize>() as f64;
            if tf > 0.0 {
                score += (1.0 + (total - df + 0.5) / (df + 0.5)).ln() * (tf * 2.2)
                    / (tf + 1.2 * (0.25 + 0.75 * len / avg.max(1.0)));
            }
            if note_text.contains(t) {
                score += 1.0;
            }
        }
        if !terms.is_empty() && score == 0.0 {
            continue;
        }
        hits.push((score, d));
    }
    hits.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.row.ts_ms.cmp(&a.1.row.ts_ms))
    });
    let result: Vec<_> = hits
        .into_iter()
        .take(q.limit.unwrap_or(8).clamp(1, 20))
        .map(|(score, d)| Hit {
            id: d.row.request_id.clone(),
            title: d
                .row
                .record
                .as_deref()
                .and_then(|s| notes.get(s))
                .filter(|n| !n.title.is_empty())
                .map(|n| n.title.clone())
                .unwrap_or_else(|| clipped(&d.prompt, 120)),
            host: d.row.host.to_string(),
            model: d.row.model.to_string(),
            date: d.row.date.to_string(),
            outcome: d.row.outcome.clone(),
            signal: d.row.signal.clone(),
            prompt: clipped(&d.prompt, 2400),
            response: clipped(&d.response, 3600),
            score,
            href: format!("#/prompts/{}", d.row.request_id),
        })
        .collect();
    serde_json::json!({"records":result,"coverage":{"complete":docs.len()==rows.len(),"indexed":docs.len(),"captured":rows.len(),"limit_bytes":MAX_BYTES},"classification":"Refusal and noteworthy flags are deterministic heuristics; they are not a quality guarantee."})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lexical_terms() {
        assert_eq!(
            tokens("Show me the prompts about RUST borrow-checker"),
            vec!["about", "rust", "borrow", "checker"]
        );
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    #[tokio::test]
    async fn legacy_refusals_are_classified_and_body_terms_are_searchable() {
        let app = crate::test_support::app();
        let mut rec = crate::test_support::record("legacy");
        rec.outcome.clear();
        rec.signal.clear();
        rec.record = "laptop/2026/09/30/2026-09-30-01-00-00".into();
        let prompt=serde_json::to_vec(&serde_json::json!({"model":"legacy","messages":[{"role":"user","content":"context ".repeat(500)}]})).unwrap();
        let response=br#"{"choices":[{"message":{"content":"I cannot assist with that request. The unique keyword is quicksort.","refusal":"This request was refused."}}]}"#;
        for (kind, bytes) in [
            (Kind::Prompt, prompt.as_slice()),
            (Kind::Response, response.as_slice()),
        ] {
            let path = records::local_path(&app, &rec.record, kind);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, app.crypto.seal(bytes).unwrap()).unwrap();
        }
        app.ledger.append(&rec).unwrap();
        let result = search(
            &app,
            &Search {
                outcome: Some("refused".into()),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result["records"][0]["id"], rec.request_id);
        assert_eq!(app.ledger.find(&rec.request_id).unwrap().outcome, "refused");
        let result = search(
            &app,
            &Search {
                query: "quicksort".into(),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            result["records"][0]["id"], rec.request_id,
            "a long request cannot hide response terms from retrieval"
        );
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
