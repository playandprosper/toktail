// SPDX-License-Identifier: Apache-2.0
//! What your locally rendered tokens would have cost on public APIs.
//!
//! Prices come from `assets/pricing.json` (built in, dated) unless the user
//! puts their own `pricing.json` in Toktail's folder. Every row in the CSV is
//! priced on its own date and size, so price changes and long-prompt tiers
//! land on the right requests. The local side is electricity: the time the
//! host was actually generating (overlapping requests counted once) times the
//! configured power draw and kWh price.

use crate::config::CostsConfig;
use crate::ledger::{Data, Filter, Row};
use anyhow::Context;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

const BUILT_IN: &str = include_str!("../assets/pricing.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceBook {
    pub as_of: String,
    #[serde(default = "usd")]
    pub currency: String,
    #[serde(default)]
    pub notes: Vec<String>,
    pub models: Vec<PublicModel>,
    /// "built-in" or the path of the user's override file.
    #[serde(default, skip_deserializing)]
    pub origin: String,
}

fn usd() -> String {
    "USD".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicModel {
    pub id: String,
    pub provider: String,
    pub name: String,
    /// USD per 1M input tokens.
    pub input: f64,
    /// USD per 1M output tokens.
    pub output: f64,
    /// Flat fee per request (e.g. Perplexity's search fee).
    #[serde(default)]
    pub per_request: f64,
    /// Higher rates when a prompt is longer than a threshold.
    #[serde(default)]
    pub tiers: Vec<Tier>,
    /// Scheduled price changes, applied by the date of each request.
    #[serde(default)]
    pub changes: Vec<Change>,
    /// Part of the comparison unless the user picks their own set.
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tier {
    pub above_input_tokens: u64,
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub from: NaiveDate,
    pub input: f64,
    pub output: f64,
}

impl PublicModel {
    /// (input, output) USD per 1M tokens for a request on `date` with `input_tokens`.
    pub fn rates(&self, date: NaiveDate, input_tokens: u64) -> (f64, f64) {
        let mut r = (self.input, self.output);
        let mut changes: Vec<&Change> = self.changes.iter().filter(|c| c.from <= date).collect();
        changes.sort_by_key(|c| c.from);
        if let Some(c) = changes.last() {
            r = (c.input, c.output);
        }
        let mut best: Option<&Tier> = None;
        for t in &self.tiers {
            if input_tokens > t.above_input_tokens
                && best.is_none_or(|b| t.above_input_tokens > b.above_input_tokens)
            {
                best = Some(t);
            }
        }
        if let Some(t) = best {
            r = (t.input, t.output);
        }
        r
    }

    pub fn cost_of(&self, r: &Row) -> (f64, f64, f64) {
        let (i, o) = self.rates(r.date, r.input);
        (
            r.input as f64 * i / 1e6,
            r.output as f64 * o / 1e6,
            self.per_request,
        )
    }
}

impl PriceBook {
    pub fn built_in() -> PriceBook {
        let mut b: PriceBook =
            serde_json::from_str(BUILT_IN).expect("built-in pricing.json is valid");
        b.origin = "built-in".into();
        b
    }

    /// The user's `pricing.json` in the base folder if present and valid,
    /// otherwise the built-in price list.
    pub fn load(base: &Path) -> PriceBook {
        let path = base.join("pricing.json");
        if path.exists() {
            match std::fs::read_to_string(&path)
                .context("reading")
                .and_then(|t| serde_json::from_str::<PriceBook>(&t).context("parsing"))
            {
                Ok(mut b) => {
                    b.origin = path.display().to_string();
                    return b;
                }
                Err(e) => log::warn!("{}: {e:#}; using built-in prices", path.display()),
            }
        }
        Self::built_in()
    }

    pub fn find(&self, id: &str) -> Option<&PublicModel> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Ids in the comparison: the user's choice, or the defaults.
    pub fn compared(&self, cfg: &CostsConfig) -> Vec<String> {
        let chosen: Vec<String> = cfg
            .compare
            .iter()
            .filter(|id| self.find(id).is_some())
            .cloned()
            .collect();
        if !chosen.is_empty() {
            return chosen;
        }
        self.models
            .iter()
            .filter(|m| m.default)
            .map(|m| m.id.clone())
            .collect()
    }
}

#[derive(Debug, Default, Serialize)]
pub struct LocalCost {
    pub busy_hours: f64,
    pub kwh: f64,
    pub cost: f64,
    pub watts: f64,
    pub usd_per_kwh: f64,
}

#[derive(Debug, Serialize)]
pub struct ModelCost {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub input_rate: f64,
    pub output_rate: f64,
    pub per_request: f64,
    pub input_cost: f64,
    pub output_cost: f64,
    pub request_cost: f64,
    pub total: f64,
    pub compared: bool,
    pub note: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct DayCost {
    pub date: NaiveDate,
    pub local: f64,
    /// Price-book id -> USD for that day.
    pub models: BTreeMap<String, f64>,
}

#[derive(Debug, Serialize)]
pub struct CostReport {
    pub as_of: String,
    pub origin: String,
    pub currency: String,
    pub notes: Vec<String>,
    pub baseline: String,
    pub hardware_cost_usd: f64,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub excluded_embedding_requests: u64,
    pub local: LocalCost,
    pub models: Vec<ModelCost>,
    pub daily: Vec<DayCost>,
}

pub fn is_embedding(r: &Row) -> bool {
    r.endpoint.contains("embedding")
}

/// Per day: request intervals (for busy time) and cost per price-book id.
type DayAcc = (Vec<(i64, i64)>, BTreeMap<String, f64>);

/// Merged length of [start, end) intervals in milliseconds.
pub fn busy_ms(mut intervals: Vec<(i64, i64)>) -> i64 {
    intervals.sort_unstable();
    let mut total = 0;
    let mut cur: Option<(i64, i64)> = None;
    for (s, e) in intervals {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    total
}

pub fn report(data: &Data, f: &Filter, book: &PriceBook, cfg: &CostsConfig) -> CostReport {
    let compared = book.compared(cfg);
    let mut models: Vec<ModelCost> = book
        .models
        .iter()
        .map(|m| ModelCost {
            id: m.id.clone(),
            provider: m.provider.clone(),
            name: m.name.clone(),
            input_rate: m.input,
            output_rate: m.output,
            per_request: m.per_request,
            input_cost: 0.0,
            output_cost: 0.0,
            request_cost: 0.0,
            total: 0.0,
            compared: compared.contains(&m.id),
            note: m.note.clone(),
            source: m.source.clone(),
        })
        .collect();
    let mut daily: BTreeMap<NaiveDate, DayAcc> = BTreeMap::new();
    let (mut requests, mut input, mut output, mut excluded) = (0u64, 0u64, 0u64, 0u64);

    for r in data.rows().filter(|r| f.matches(r)) {
        if is_embedding(r) {
            excluded += 1;
            continue;
        }
        if r.input == 0 && r.output == 0 {
            continue;
        }
        requests += 1;
        input += r.input;
        output += r.output;
        let day = daily.entry(r.date).or_default();
        day.0.push((r.ts_ms - r.duration_ms as i64, r.ts_ms));
        for (m, mc) in book.models.iter().zip(models.iter_mut()) {
            let (ic, oc, rc) = m.cost_of(r);
            mc.input_cost += ic;
            mc.output_cost += oc;
            mc.request_cost += rc;
            *day.1.entry(m.id.clone()).or_default() += ic + oc + rc;
        }
    }
    for mc in &mut models {
        mc.total = mc.input_cost + mc.output_cost + mc.request_cost;
    }

    let watts = cfg.watts_while_generating.max(0.0);
    let price = cfg.electricity_usd_per_kwh.max(0.0);
    let mut local = LocalCost {
        watts,
        usd_per_kwh: price,
        ..Default::default()
    };
    let daily: Vec<DayCost> = daily
        .into_iter()
        .map(|(date, (intervals, m))| {
            let hours = busy_ms(intervals) as f64 / 3_600_000.0;
            let kwh = hours * watts / 1000.0;
            local.busy_hours += hours;
            local.kwh += kwh;
            local.cost += kwh * price;
            DayCost {
                date,
                local: kwh * price,
                models: m,
            }
        })
        .collect();

    CostReport {
        as_of: book.as_of.clone(),
        origin: book.origin.clone(),
        currency: book.currency.clone(),
        notes: book.notes.clone(),
        baseline: if book.find(&cfg.baseline).is_some() {
            cfg.baseline.clone()
        } else {
            compared.first().cloned().unwrap_or_default()
        },
        hardware_cost_usd: cfg.hardware_cost_usd.max(0.0),
        requests,
        input_tokens: input,
        output_tokens: output,
        excluded_embedding_requests: excluded,
        local,
        models,
        daily,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_book_is_sane() {
        let b = PriceBook::built_in();
        assert!(b.models.len() >= 10);
        for m in &b.models {
            assert!(m.input >= 0.0 && m.output > 0.0, "{}", m.id);
            assert!(m.source.starts_with("https://"), "{}", m.id);
        }
        let mut ids: Vec<_> = b.models.iter().map(|m| &m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), b.models.len(), "ids are unique");
        assert!(
            b.find(&CostsConfig::default().baseline).is_some(),
            "default baseline exists"
        );
    }

    #[test]
    fn tiers_and_changes() {
        let b = PriceBook::built_in();
        let haiku = b.find("claude-haiku-5.5").unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(haiku.rates(d, 1_000), (0.10, 0.50));
        assert_eq!(haiku.rates(d, 150_000), (0.50, 2.50));
        let flash = b.find("gemini-3.8-flash").unwrap();
        assert_eq!(flash.rates(d, 10), (0.75, 3.75));
        assert_eq!(
            flash.rates(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap(), 10),
            (1.50, 7.50)
        );
    }

    #[test]
    fn overlapping_requests_count_once() {
        assert_eq!(busy_ms(vec![(0, 10), (5, 15), (20, 30)]), 25);
        assert_eq!(busy_ms(vec![]), 0);
        assert_eq!(busy_ms(vec![(0, 100), (10, 20)]), 100);
    }
}
