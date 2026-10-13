// SPDX-License-Identifier: Apache-2.0
//! Per-model performance, computed from the CSV: how fast each model
//! generates, how long it takes to start, how often it fails or refuses, and
//! what its tokens would have cost in the cloud. Feeds the Models page and
//! Pip's `model_stats` tool.

use crate::ledger::{Data, Filter, Row};
use crate::pricing::{self, PriceBook};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The value below which `q` of the samples fall (nearest rank).
pub fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Spread {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub mean: Option<f64>,
    pub samples: usize,
}

fn spread(mut v: Vec<f64>) -> Spread {
    v.retain(|x| x.is_finite());
    v.sort_by(|a, b| a.total_cmp(b));
    let mean = (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    Spread {
        p50: quantile(&v, 0.5),
        p95: quantile(&v, 0.95),
        mean,
        samples: v.len(),
    }
}

#[derive(Default)]
struct Acc {
    requests: u64,
    input: u64,
    output: u64,
    errors: u64,
    cancelled: u64,
    refused: u64,
    noteworthy: u64,
    estimated: u64,
    tps: Vec<f64>,
    ttft: Vec<f64>,
    duration: Vec<f64>,
    gen_out: u64,
    gen_ms: u64,
    cloud: f64,
    first: Option<NaiveDate>,
    last: Option<NaiveDate>,
    last_ts: i64,
    hosts: BTreeSet<String>,
    per_day: BTreeMap<NaiveDate, (u64, u64)>,
}

impl Acc {
    fn add(&mut self, r: &Row, cost: f64) {
        self.requests += 1;
        self.input += r.input;
        self.output += r.output;
        match r.status {
            499 => self.cancelled += 1,
            s if s >= 400 => self.errors += 1,
            _ => {}
        }
        if r.outcome == "refused" {
            self.refused += 1;
        }
        if ["structured", "substantial", "tool_call"].contains(&r.signal.as_str()) {
            self.noteworthy += 1;
        }
        if &*r.source == "estimated" {
            self.estimated += 1;
        }
        if r.status < 400 {
            if let Some(t) = r.tokens_per_sec() {
                self.tps.push(t);
            }
            if let Some(t) = r.ttft_ms {
                self.ttft.push(t as f64);
            }
            self.duration.push(r.duration_ms as f64);
            if r.output > 0 && r.gen_ms > 0 {
                self.gen_out += r.output;
                self.gen_ms += r.gen_ms;
            }
        }
        self.cloud += cost;
        self.first = Some(self.first.map_or(r.date, |f| f.min(r.date)));
        self.last = Some(self.last.map_or(r.date, |l| l.max(r.date)));
        self.last_ts = self.last_ts.max(r.ts_ms);
        self.hosts.insert(r.host.to_string());
        let d = self.per_day.entry(r.date).or_default();
        d.0 += 1;
        d.1 += r.input + r.output;
    }

    fn finish(self, key: String, host: Option<String>, days: &[NaiveDate]) -> ModelStats {
        let ok = self.requests - self.errors - self.cancelled;
        ModelStats {
            model: key,
            host,
            requests: self.requests,
            input: self.input,
            output: self.output,
            combined: self.input + self.output,
            errors: self.errors,
            cancelled: self.cancelled,
            refused: self.refused,
            noteworthy: self.noteworthy,
            estimated: self.estimated,
            error_rate: ratio(self.errors, self.requests),
            refusal_rate: ratio(self.refused, ok),
            tokens_per_sec: spread(self.tps),
            // Output-weighted speed, the same measure the dashboard uses.
            weighted_tps: (self.gen_ms > 0)
                .then(|| self.gen_out as f64 * 1000.0 / self.gen_ms as f64),
            ttft_ms: spread(self.ttft),
            duration_ms: spread(self.duration),
            avg_input: (self.requests > 0).then(|| self.input as f64 / self.requests as f64),
            avg_output: (self.requests > 0).then(|| self.output as f64 / self.requests as f64),
            cloud_value: self.cloud,
            first_date: self.first,
            last_date: self.last,
            last_used_ms: self.last_ts,
            hosts: self.hosts.into_iter().collect(),
            daily_tokens: days
                .iter()
                .map(|d| self.per_day.get(d).map_or(0, |x| x.1))
                .collect(),
        }
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStats {
    pub model: String,
    /// Set on the per-host breakdown rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub combined: u64,
    pub errors: u64,
    pub cancelled: u64,
    pub refused: u64,
    pub noteworthy: u64,
    pub estimated: u64,
    pub error_rate: f64,
    pub refusal_rate: f64,
    pub tokens_per_sec: Spread,
    pub weighted_tps: Option<f64>,
    pub ttft_ms: Spread,
    pub duration_ms: Spread,
    pub avg_input: Option<f64>,
    pub avg_output: Option<f64>,
    pub cloud_value: f64,
    pub first_date: Option<NaiveDate>,
    pub last_date: Option<NaiveDate>,
    pub last_used_ms: i64,
    pub hosts: Vec<String>,
    /// Combined tokens per day across the range, oldest first.
    pub daily_tokens: Vec<u64>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub days: Vec<NaiveDate>,
    pub baseline: String,
    pub baseline_name: String,
    pub models: Vec<ModelStats>,
    /// The same, split by host (model × host).
    pub by_host: Vec<ModelStats>,
}

/// Per-model statistics for the rows the filter keeps (embeddings left out
/// of speed, but counted).
pub fn models(data: &Data, f: &Filter, book: &PriceBook, baseline: &str) -> Report {
    let base = book.find(baseline);
    let mut per: BTreeMap<String, Acc> = BTreeMap::new();
    let mut per_host: BTreeMap<(String, String), Acc> = BTreeMap::new();
    let (mut lo, mut hi): (Option<NaiveDate>, Option<NaiveDate>) = (None, None);
    for r in data.rows().filter(|r| f.matches(r)) {
        let cost = match base {
            Some(m) if !pricing::is_embedding(r) => {
                let (i, o, q) = m.cost_of(r);
                i + o + if r.input + r.output > 0 { q } else { 0.0 }
            }
            _ => 0.0,
        };
        per.entry(r.model.to_string()).or_default().add(r, cost);
        per_host
            .entry((r.model.to_string(), r.host.to_string()))
            .or_default()
            .add(r, cost);
        lo = Some(lo.map_or(r.date, |x| x.min(r.date)));
        hi = Some(hi.map_or(r.date, |x| x.max(r.date)));
    }
    let from = f.from.or(lo);
    let to = f.to.or(hi);
    let mut days = Vec::new();
    if let (Some(a), Some(b)) = (from, to) {
        let mut d = a;
        while d <= b && days.len() < 800 {
            days.push(d);
            d = d.succ_opt().unwrap_or(d);
            if d == a {
                break;
            }
        }
    }
    let mut models: Vec<ModelStats> = per
        .into_iter()
        .map(|(k, a)| a.finish(k, None, &days))
        .collect();
    models.sort_by_key(|m| std::cmp::Reverse(m.combined));
    let mut by_host: Vec<ModelStats> = per_host
        .into_iter()
        .map(|((m, h), a)| a.finish(m, Some(h), &days))
        .collect();
    by_host.sort_by_key(|m| std::cmp::Reverse(m.combined));
    Report {
        from,
        to,
        days,
        baseline: baseline.to_string(),
        baseline_name: base.map(|m| m.name.clone()).unwrap_or_default(),
        models,
        by_host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_use_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        assert_eq!(quantile(&v, 0.5), Some(50.0));
        assert_eq!(quantile(&v, 0.95), Some(95.0));
        assert_eq!(quantile(&[7.0], 0.95), Some(7.0));
        assert_eq!(quantile(&[], 0.5), None);
    }

    #[tokio::test]
    async fn per_model_stats_from_the_csv() {
        let app = crate::test_support::app();
        for (model, out, status) in [("a", 100, 200), ("a", 300, 200), ("b", 10, 500)] {
            let mut r = crate::test_support::record(model);
            r.output_tokens = out;
            r.status = status;
            r.ttft_ms = Some(100);
            r.duration_ms = 1100;
            app.ledger.append(&r).unwrap();
        }
        let book = PriceBook::built_in();
        let rep = app
            .ledger
            .with_data(|d| models(d, &Filter::default(), &book, "claude-sonnet-5.5"));
        let a = rep.models.iter().find(|m| m.model == "a").unwrap();
        assert_eq!((a.requests, a.output, a.errors), (2, 400, 0));
        assert_eq!(
            a.weighted_tps,
            Some(200.0),
            "400 tokens over 2 seconds of generation"
        );
        assert!(a.cloud_value > 0.0);
        let b = rep.models.iter().find(|m| m.model == "b").unwrap();
        assert_eq!(
            (b.errors, b.tokens_per_sec.samples),
            (1, 0),
            "failed requests don't count toward speed"
        );
        assert_eq!(rep.days.len(), 1);
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
