// SPDX-License-Identifier: Apache-2.0
//! Market prices: once a calendar day, Toktail fetches a public model price
//! list and keeps a dated snapshot, so the cloud comparison uses what the
//! APIs charge now and what they charged on the day of each request.
//!
//! Two sources take turns: LiteLLM's `model_prices_and_context_window.json`
//! (a plain file on GitHub) and OpenRouter's `/api/v1/models`. Both are
//! public and need no key. If one fails, the other is tried the same day. An
//! OpenRouter key can be stored in Settings for the day that list ever needs
//! one; it is sent only to openrouter.ai.
//!
//! Snapshots live in `<base>/market/YYYY-MM-DD.<source>.json`, normalized to
//! Toktail's own shape (USD per million tokens). Loading applies every
//! snapshot, oldest first, to the price book as dated price changes, so a
//! request from last month is priced at last month's rates.

use crate::app::App;
use crate::config::MarketConfig;
use crate::pricing::{Change, PriceBook, PublicModel};
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const SOURCES: [&str; 2] = ["litellm", "openrouter"];

/// One model's prices as a source reports them, USD per 1M tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quote {
    pub id: String,
    #[serde(default)]
    pub provider: String,
    pub input: f64,
    pub output: f64,
    /// Price of a prompt-cache read, when the source lists one.
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// A day's price list from one source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub date: NaiveDate,
    pub source: String,
    pub fetched_at: String,
    pub quotes: Vec<Quote>,
}

pub fn dir(base: &Path) -> PathBuf {
    base.join("market")
}

fn snapshot_path(base: &Path, date: NaiveDate, source: &str) -> PathBuf {
    dir(base).join(format!("{date}.{source}.json"))
}

/// Every stored snapshot, oldest first.
pub fn load_all(base: &Path) -> Vec<Snapshot> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir(base)) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            match std::fs::read_to_string(&p)
                .context("reading")
                .and_then(|t| serde_json::from_str::<Snapshot>(&t).context("parsing"))
            {
                Ok(s) => out.push(s),
                Err(err) => log::warn!("market: skipping {}: {err:#}", p.display()),
            }
        }
    }
    out.sort_by(|a, b| (a.date, &a.source).cmp(&(b.date, &b.source)));
    out
}

/// The newest snapshot's date, if any.
pub fn latest_date(base: &Path) -> Option<NaiveDate> {
    load_all(base).last().map(|s| s.date)
}

pub fn save(base: &Path, s: &Snapshot) -> Result<PathBuf> {
    let d = dir(base);
    std::fs::create_dir_all(&d)?;
    let p = snapshot_path(base, s.date, &s.source);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

// ------------------------------------------------------------------ parsing

fn per_token_to_million(v: &Value) -> Option<f64> {
    let x = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (x.is_finite() && x >= 0.0).then_some(x * 1e6)
}

/// LiteLLM: `{ "<model>": { "input_cost_per_token": 3e-6, "output_cost_per_token": 1.5e-5,
/// "cache_read_input_token_cost": 3e-7, "litellm_provider": "anthropic", "mode": "chat" } }`.
pub fn parse_litellm(text: &str) -> Result<Vec<Quote>> {
    let v: Value = serde_json::from_str(text).context("LiteLLM price list isn't JSON")?;
    let obj = v
        .as_object()
        .context("LiteLLM price list isn't a JSON object")?;
    let mut out = Vec::new();
    for (id, m) in obj {
        if id == "sample_spec" {
            continue;
        }
        let mode = m.get("mode").and_then(Value::as_str).unwrap_or("chat");
        if !matches!(mode, "chat" | "completion" | "responses") {
            continue;
        }
        let (Some(input), Some(output)) = (
            m.get("input_cost_per_token").and_then(per_token_to_million),
            m.get("output_cost_per_token")
                .and_then(per_token_to_million),
        ) else {
            continue;
        };
        out.push(Quote {
            id: id.clone(),
            provider: m
                .get("litellm_provider")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            input,
            output,
            cache_read: m
                .get("cache_read_input_token_cost")
                .and_then(per_token_to_million),
            cache_write: m
                .get("cache_creation_input_token_cost")
                .and_then(per_token_to_million),
        });
    }
    anyhow::ensure!(!out.is_empty(), "LiteLLM price list has no chat models");
    Ok(out)
}

/// OpenRouter: `{ "data": [ { "id": "anthropic/claude-sonnet-4", "pricing":
/// { "prompt": "0.000003", "completion": "0.000015", "input_cache_read": "…" } } ] }`.
pub fn parse_openrouter(text: &str) -> Result<Vec<Quote>> {
    let v: Value = serde_json::from_str(text).context("OpenRouter model list isn't JSON")?;
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .context("OpenRouter model list has no data")?;
    let mut out = Vec::new();
    for m in data {
        let Some(id) = m.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(p) = m.get("pricing") else {
            continue;
        };
        let (Some(input), Some(output)) = (
            p.get("prompt").and_then(per_token_to_million),
            p.get("completion").and_then(per_token_to_million),
        ) else {
            continue;
        };
        out.push(Quote {
            provider: id.split('/').next().unwrap_or("").to_string(),
            id: id.to_string(),
            input,
            output,
            cache_read: p.get("input_cache_read").and_then(per_token_to_million),
            cache_write: p.get("input_cache_write").and_then(per_token_to_million),
        });
    }
    anyhow::ensure!(!out.is_empty(), "OpenRouter model list is empty");
    Ok(out)
}

// ------------------------------------------------------------------ matching

/// "anthropic/claude-sonnet-4.5", "claude-sonnet-4-5-20250929", "Claude Sonnet 4.5"
/// all become "claude-sonnet-4-5".
pub fn canon(id: &str) -> String {
    let tail = id.rsplit('/').next().unwrap_or(id).to_ascii_lowercase();
    let tail = tail.strip_suffix(":free").unwrap_or(&tail);
    let mut s: String = tail
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Drop a trailing date stamp like -20250929 or -2025-09-29.
    loop {
        let parts: Vec<&str> = s.rsplitn(2, '-').collect();
        if parts.len() == 2
            && parts[0].len() >= 2
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && (parts[0].len() == 8 || parts[0].len() == 2 || parts[0].len() == 4)
            && s.matches('-').count() > 2
        {
            s = parts[1].to_string();
        } else {
            break;
        }
    }
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Resellers and clouds that list other providers' models, often at other prices.
const RESELLERS: [&str; 18] = [
    "azure",
    "azure_ai",
    "bedrock",
    "bedrock_converse",
    "vertex_ai",
    "vertex_ai-language-models",
    "deepinfra",
    "aihubmix",
    "together_ai",
    "fireworks_ai",
    "openrouter",
    "sagemaker",
    "cloudflare",
    "replicate",
    "watsonx",
    "github_copilot",
    "groq",
    "nscale",
];

/// Names a provider goes by in the lists ("Google" models are listed under "gemini").
fn provider_aliases(prov: &str) -> &'static [&'static str] {
    match prov {
        "google" => &["google", "gemini"],
        "anthropic" => &["anthropic", "claude"],
        "mistral" => &["mistral", "mistralai"],
        "moonshot" => &["moonshot", "moonshotai"],
        "xai" => &["xai", "x-ai"],
        "openai" => &["openai"],
        "deepseek" => &["deepseek"],
        "perplexity" => &["perplexity"],
        _ => &[],
    }
}

/// The quote for a price-book model: its `market_ids` first, then the
/// canonical id, preferring the provider's own listing over resellers.
pub fn find<'a>(m: &PublicModel, quotes: &'a [Quote]) -> Option<&'a Quote> {
    for want in &m.market_ids {
        if let Some(q) = quotes.iter().find(|q| q.id.eq_ignore_ascii_case(want)) {
            return Some(q);
        }
    }
    let want = canon(&m.id);
    let prov = m.provider.to_ascii_lowercase();
    let prov = prov.split_whitespace().next().unwrap_or("").to_string();
    let mut names: Vec<&str> = provider_aliases(&prov).to_vec();
    if names.is_empty() && !prov.is_empty() {
        names.push(&prov);
    }
    let score = |q: &Quote, exact: bool| -> i32 {
        let qp = q.provider.to_ascii_lowercase();
        let prefix = q.id.split('/').next().unwrap_or("").to_ascii_lowercase();
        let mut s = if exact { 10 } else { 0 };
        if names.iter().any(|n| qp == *n) {
            s += 3;
        }
        if !q.id.contains('/') {
            s += 2;
        } else if names.iter().any(|n| prefix == *n) {
            s += 1;
        }
        // "perplexity/google/gemini…": a reseller's copy.
        if q.id.matches('/').count() >= 2 {
            s -= 5;
        }
        if RESELLERS.contains(&qp.as_str()) || RESELLERS.contains(&prefix.as_str()) {
            s -= 5;
        }
        s
    };
    // Exact canonical matches first; else a -preview / -latest edition.
    let edition = |q: &Quote| {
        let c = canon(&q.id);
        if c == want {
            Some(true)
        } else if c == format!("{want}-preview") || c == format!("{want}-latest") {
            Some(false)
        } else {
            None
        }
    };
    quotes
        .iter()
        .filter_map(|q| edition(q).map(|e| (q, e)))
        .max_by_key(|(q, e)| score(q, *e))
        .map(|(q, _)| q)
}

/// Applies snapshots to the book, oldest first, as dated price changes. A
/// model's current rates become those of the newest snapshot that lists it.
pub fn apply(book: &mut PriceBook, snaps: &[Snapshot]) -> usize {
    let mut touched = std::collections::BTreeSet::new();
    for s in snaps {
        for m in &mut book.models {
            let Some(q) = find(m, &s.quotes) else {
                continue;
            };
            let current = m.rates(s.date, 0);
            if (current.0 - q.input).abs() > 1e-9 || (current.1 - q.output).abs() > 1e-9 {
                m.changes.retain(|c| c.from != s.date);
                m.changes.push(Change {
                    from: s.date,
                    input: q.input,
                    output: q.output,
                });
                m.changes.sort_by_key(|c| c.from);
            }
            if let Some(cr) = q.cache_read {
                m.cached_input = Some(cr);
            }
            m.market_source = Some(format!("{} {}", s.source, s.date));
            touched.insert(m.id.clone());
        }
    }
    if let Some(last) = snaps.last() {
        book.as_of = last.date.to_string();
        book.market = Some(format!("{} ({})", last.source, last.date));
    }
    touched.len()
}

/// The price book with the user's override and every snapshot applied.
pub fn book_with_market(base: &Path, cfg_market_prices: bool) -> PriceBook {
    let mut book = PriceBook::load(base);
    if cfg_market_prices {
        let snaps = load_all(base);
        if !snaps.is_empty() {
            let n = apply(&mut book, &snaps);
            log::info!(
                "market: {} snapshots applied, {n} models priced from the market",
                snaps.len()
            );
        }
    }
    book
}

// ------------------------------------------------------------------ fetching

async fn fetch(client: &reqwest::Client, cfg: &MarketConfig, source: &str) -> Result<Vec<Quote>> {
    let (url, key) = match source {
        "openrouter" => (
            cfg.openrouter_url.clone(),
            crate::secrets::get_named_secret("openrouter-key"),
        ),
        _ => (cfg.litellm_url.clone(), None),
    };
    let mut req = client.get(&url).timeout(Duration::from_secs(30)).header(
        "user-agent",
        format!("toktail/{}", env!("CARGO_PKG_VERSION")),
    );
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let r = req
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    anyhow::ensure!(r.status().is_success(), "{url} answered {}", r.status());
    let text = r.text().await?;
    match source {
        "openrouter" => parse_openrouter(&text),
        _ => parse_litellm(&text),
    }
}

/// Today's check: tries the next source in the rotation, then the other.
/// Returns the stored snapshot.
pub async fn check_today(app: &Arc<App>, client: &reqwest::Client) -> Result<Snapshot> {
    let cfg = app.config.read().market.clone();
    let first = if SOURCES.contains(&cfg.next_source.as_str()) {
        cfg.next_source.clone()
    } else {
        "litellm".into()
    };
    let second = SOURCES
        .iter()
        .find(|s| **s != first)
        .copied()
        .unwrap_or("openrouter");
    let today = Local::now().date_naive();
    let mut last_err = None;
    for source in [first.as_str(), second] {
        match fetch(client, &cfg, source).await {
            Ok(quotes) => {
                let snap = Snapshot {
                    date: today,
                    source: source.to_string(),
                    fetched_at: Local::now().to_rfc3339(),
                    quotes,
                };
                save(&app.paths.base, &snap)?;
                // Rotate: whichever answered, the other goes first tomorrow.
                let other = SOURCES
                    .iter()
                    .find(|s| **s != source)
                    .copied()
                    .unwrap_or("litellm");
                {
                    let mut c = app.config.write();
                    c.market.next_source = other.to_string();
                    let _ = crate::config::save(&app.paths.config_file, &c);
                }
                reload(app);
                log::info!(
                    "market: {} prices for {} models from {source}",
                    today,
                    snap.quotes.len()
                );
                return Ok(snap);
            }
            Err(e) => {
                log::warn!("market: {source}: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no price source answered")))
}

/// Rebuilds the in-memory price book from disk (override + snapshots).
pub fn reload(app: &App) {
    let use_market = app.config.read().costs.market_prices;
    *app.prices.write() = book_with_market(&app.paths.base, use_market);
}

/// Checks once per calendar day while the app runs. The first check happens
/// a minute after launch if today's hasn't happened yet.
pub async fn run(app: Arc<App>) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        let enabled = app.config.read().market.enabled && !app.demo;
        if enabled {
            let today = Local::now().date_naive();
            let done = latest_date(&app.paths.base).is_some_and(|d| d >= today);
            if !done {
                if let Err(e) = check_today(&app, &client).await {
                    log::warn!("market: today's price check failed: {e:#}");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(30 * 60)).await;
    }
}

/// For Settings: what's stored and when the next check is due.
pub fn summary(base: &Path) -> Value {
    let snaps = load_all(base);
    let by_date: BTreeMap<String, Vec<String>> = snaps.iter().fold(BTreeMap::new(), |mut m, s| {
        m.entry(s.date.to_string())
            .or_default()
            .push(s.source.clone());
        m
    });
    serde_json::json!({
        "snapshots": snaps.len(),
        "days": by_date.len(),
        "latest": snaps.last().map(|s| serde_json::json!({ "date": s.date, "source": s.source, "models": s.quotes.len(), "fetched_at": s.fetched_at })),
        "dir": dir(base).display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LITELLM: &str = r#"{
      "sample_spec": {"input_cost_per_token": 0, "output_cost_per_token": 0, "mode": "chat"},
      "claude-sonnet-4-5-20250929": {"input_cost_per_token": 3e-06, "output_cost_per_token": 1.5e-05, "cache_read_input_token_cost": 3e-07, "cache_creation_input_token_cost": 3.75e-06, "litellm_provider": "anthropic", "mode": "chat"},
      "text-embedding-3-small": {"input_cost_per_token": 2e-08, "output_cost_per_token": 0, "litellm_provider": "openai", "mode": "embedding"},
      "gpt-4.1": {"input_cost_per_token": 2e-06, "output_cost_per_token": 8e-06, "litellm_provider": "openai", "mode": "chat"}
    }"#;
    const OPENROUTER: &str = r#"{"data": [
      {"id": "anthropic/claude-sonnet-4.5", "name": "Anthropic: Claude Sonnet 4.5", "pricing": {"prompt": "0.000003", "completion": "0.000015", "input_cache_read": "0.0000003"}},
      {"id": "openai/gpt-4.1", "pricing": {"prompt": "0.000002", "completion": "0.000008"}},
      {"id": "meta-llama/llama-3.3-70b-instruct:free", "pricing": {"prompt": "0", "completion": "0"}}
    ]}"#;

    #[test]
    fn parses_both_sources_into_usd_per_million() {
        let l = parse_litellm(LITELLM).unwrap();
        assert_eq!(l.len(), 2, "embeddings and the sample spec are skipped");
        let s = l
            .iter()
            .find(|q| q.id.starts_with("claude-sonnet"))
            .unwrap();
        assert!((s.input - 3.0).abs() < 1e-9 && (s.output - 15.0).abs() < 1e-9);
        assert_eq!(s.cache_read.map(|x| (x * 100.0).round() / 100.0), Some(0.3));
        assert_eq!(s.provider, "anthropic");
        let o = parse_openrouter(OPENROUTER).unwrap();
        assert_eq!(o.len(), 3);
        assert_eq!(o[0].provider, "anthropic");
        assert!((o[0].input - 3.0).abs() < 1e-9);
        assert!(parse_litellm("[]").is_err());
        assert!(parse_openrouter(r#"{"data": []}"#).is_err());
    }

    #[test]
    fn prefers_the_provider_over_resellers() {
        let m = PublicModel {
            id: "mistral-large".into(),
            provider: "Mistral".into(),
            name: "Mistral Large".into(),
            input: 0.5,
            output: 1.5,
            per_request: 0.0,
            tiers: vec![],
            changes: vec![],
            default: false,
            note: String::new(),
            source: String::new(),
            cached_input: None,
            market_ids: vec![],
            market_source: None,
        };
        let q = |id: &str, prov: &str, i: f64| Quote {
            id: id.into(),
            provider: prov.into(),
            input: i,
            output: i * 3.0,
            cache_read: None,
            cache_write: None,
        };
        let quotes = vec![
            q("azure_ai/mistral-large", "azure_ai", 4.0),
            q("mistral/mistral-large", "mistral", 0.5),
            q("deepinfra/mistral-large", "deepinfra", 0.3),
        ];
        assert_eq!(find(&m, &quotes).unwrap().id, "mistral/mistral-large");
        let quotes = vec![
            q("azure_ai/mistral-large", "azure_ai", 4.0),
            q("mistral-large", "mistral", 0.5),
        ];
        assert_eq!(find(&m, &quotes).unwrap().id, "mistral-large");
        let explicit = PublicModel {
            market_ids: vec!["deepinfra/mistral-large".into()],
            ..m.clone()
        };
        let quotes = vec![
            q("mistral-large", "mistral", 0.5),
            q("deepinfra/mistral-large", "deepinfra", 0.3),
        ];
        assert_eq!(
            find(&explicit, &quotes).unwrap().id,
            "deepinfra/mistral-large",
            "market_ids win"
        );
    }

    #[test]
    fn canonical_ids_line_up_across_sources() {
        assert_eq!(canon("anthropic/claude-sonnet-4.5"), "claude-sonnet-4-5");
        assert_eq!(canon("claude-sonnet-4-5-20250929"), "claude-sonnet-4-5");
        assert_eq!(canon("openai/gpt-4.1"), "gpt-4-1");
        assert_eq!(
            canon("meta-llama/llama-3.3-70b-instruct:free"),
            "llama-3-3-70b-instruct"
        );
        assert_eq!(canon("Claude Sonnet 4.5"), "claude-sonnet-4-5");
    }

    #[test]
    fn snapshots_become_dated_price_changes() {
        let mut book = PriceBook::built_in();
        let id = book.models[0].id.clone();
        let before = book.models[0].rates(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 0);
        let d1 = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let q = |i, o| Quote {
            id: id.clone(),
            provider: book.models[0].provider.clone(),
            input: i,
            output: o,
            cache_read: Some(i / 10.0),
            cache_write: None,
        };
        let snaps = vec![
            Snapshot {
                date: d1,
                source: "litellm".into(),
                fetched_at: String::new(),
                quotes: vec![q(1.0, 2.0)],
            },
            Snapshot {
                date: d2,
                source: "openrouter".into(),
                fetched_at: String::new(),
                quotes: vec![q(4.0, 8.0)],
            },
        ];
        assert_eq!(apply(&mut book, &snaps), 1);
        let m = &book.models[0];
        assert_eq!(
            m.rates(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 0),
            before,
            "history before the first snapshot keeps the built-in price"
        );
        assert_eq!(
            m.rates(NaiveDate::from_ymd_opt(2026, 9, 15).unwrap(), 0),
            (1.0, 2.0)
        );
        assert_eq!(
            m.rates(NaiveDate::from_ymd_opt(2026, 10, 2).unwrap(), 0),
            (4.0, 8.0)
        );
        assert_eq!(m.cached_input, Some(0.4));
        assert_eq!(book.as_of, "2026-10-01");
        // Applying the same snapshots again changes nothing.
        let n = m.changes.len();
        apply(&mut book, &snaps);
        assert_eq!(book.models[0].changes.len(), n);
    }

    #[test]
    fn snapshots_round_trip_on_disk() {
        let base = std::env::temp_dir().join(format!("toktail-market-{}", uuid::Uuid::new_v4()));
        let s = Snapshot {
            date: NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(),
            source: "litellm".into(),
            fetched_at: "x".into(),
            quotes: parse_litellm(LITELLM).unwrap(),
        };
        save(&base, &s).unwrap();
        let all = load_all(&base);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].quotes, s.quotes);
        assert_eq!(latest_date(&base), Some(s.date));
        assert_eq!(summary(&base)["days"], 1);
        std::fs::remove_dir_all(&base).ok();
    }
}

#[cfg(test)]
mod live_fixture_tests {
    //! Run against a real LiteLLM download when TOKTAIL_LITELLM_JSON points at one.
    use super::*;

    #[test]
    fn real_litellm_list_matches_the_built_in_models() {
        let Ok(p) = std::env::var("TOKTAIL_LITELLM_JSON") else {
            return;
        };
        let text = std::fs::read_to_string(p).unwrap();
        let quotes = parse_litellm(&text).unwrap();
        assert!(quotes.len() > 500, "{} quotes", quotes.len());
        let book = PriceBook::built_in();
        let matched: Vec<String> = book
            .models
            .iter()
            .filter_map(|m| {
                find(m, &quotes)
                    .map(|q| format!("{} <- {} ({} in / {} out)", m.id, q.id, q.input, q.output))
            })
            .collect();
        eprintln!("{}", matched.join("\n"));
        assert!(
            matched.len() >= book.models.len() / 2,
            "only {} of {} matched",
            matched.len(),
            book.models.len()
        );
    }
}
