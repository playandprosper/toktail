// SPDX-License-Identifier: Apache-2.0
//! Token budgets and alerts.
//!
//! A budget is a limit per day, week or month for everything, one host or one
//! model. Its use is computed from the CSV like every other number, with a
//! projection to the end of the period at the current pace. A background
//! loop raises an alert when a budget crosses 80% or 100%, when a host's LM
//! Studio stops answering (and when it comes back), and optionally when a
//! stampede starts. Alerts show in the portal and the menu bar, and as macOS
//! notifications. Each alert fires once per budget period or outage.

use crate::app::App;
use crate::config::BudgetConfig;
use crate::ledger::{Data, Row};
use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, Timelike};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// First and last day of the period that contains `today`.
pub fn period_bounds(period: &str, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    match period {
        "week" => {
            let start = today - Days::new(today.weekday().num_days_from_monday() as u64);
            (start, start + Days::new(6))
        }
        "month" => {
            let start = today.with_day(1).unwrap_or(today);
            let end = start
                .checked_add_months(Months::new(1))
                .map(|d| d - Days::new(1))
                .unwrap_or(today);
            (start, end)
        }
        _ => (today, today),
    }
}

fn counts(b: &BudgetConfig, r: &Row) -> u64 {
    match b.metric.as_str() {
        "input" => r.input,
        "output" => r.output,
        _ => r.input + r.output,
    }
}

fn in_scope(b: &BudgetConfig, r: &Row) -> bool {
    (b.host.is_empty() || *r.host == *b.host) && (b.model.is_empty() || *r.model == *b.model)
}

#[derive(Debug, Clone, Serialize)]
pub struct BudgetStatus {
    #[serde(flatten)]
    pub budget: BudgetConfig,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub used: u64,
    /// used / limit (0 when there is no limit).
    pub ratio: f64,
    /// Where the period ends up at today's pace.
    pub projected: u64,
    pub projected_ratio: f64,
    /// "ok", "warn" (80% or projected over) or "over".
    pub state: &'static str,
    /// Tokens used per day across the period so far, oldest first.
    pub daily: Vec<u64>,
}

/// How far through the period we are, 0..=1, counting the time of day.
fn elapsed_fraction(start: NaiveDate, end: NaiveDate, now: DateTime<Local>) -> f64 {
    let days = ((end - start).num_days() + 1).max(1) as f64;
    let today = now.date_naive();
    let full_days = (today - start).num_days().max(0) as f64;
    let today_part = now.num_seconds_from_midnight() as f64 / 86_400.0;
    ((full_days + today_part) / days).clamp(0.0, 1.0)
}

pub fn status(data: &Data, budgets: &[BudgetConfig], now: DateTime<Local>) -> Vec<BudgetStatus> {
    let today = now.date_naive();
    budgets
        .iter()
        .map(|b| {
            let (start, end) = period_bounds(&b.period, today);
            let n = ((today - start).num_days() + 1).max(1) as usize;
            let mut daily = vec![0u64; n];
            let mut used = 0u64;
            for r in data
                .rows()
                .filter(|r| r.date >= start && r.date <= today && in_scope(b, r))
            {
                let c = counts(b, r);
                used += c;
                daily[(r.date - start).num_days() as usize] += c;
            }
            let frac = elapsed_fraction(start, end, now);
            // Early in a period the pace is noisy; never project below what's used.
            let projected = if frac > 0.02 {
                ((used as f64 / frac).round() as u64).max(used)
            } else {
                used
            };
            let limit = b.limit.max(1) as f64;
            let ratio = if b.limit == 0 {
                0.0
            } else {
                used as f64 / limit
            };
            let projected_ratio = if b.limit == 0 {
                0.0
            } else {
                projected as f64 / limit
            };
            let state = if b.limit > 0 && ratio >= 1.0 {
                "over"
            } else if b.limit > 0 && (ratio >= 0.8 || (projected_ratio >= 1.0 && frac >= 0.25)) {
                "warn"
            } else {
                "ok"
            };
            BudgetStatus {
                budget: b.clone(),
                period_start: start,
                period_end: end,
                used,
                ratio,
                projected,
                projected_ratio,
                state,
                daily,
            }
        })
        .collect()
}

/// Checks a budget list typed into Settings, filling in missing ids.
pub fn validate(mut list: Vec<BudgetConfig>) -> Result<Vec<BudgetConfig>, String> {
    if list.len() > 50 {
        return Err("Keep it to 50 budgets.".into());
    }
    let mut seen = std::collections::HashSet::new();
    for b in &mut list {
        b.name = b.name.trim().chars().take(80).collect();
        if b.name.is_empty() {
            return Err("Give every budget a name.".into());
        }
        if !["combined", "input", "output"].contains(&b.metric.as_str()) {
            return Err(format!(
                "{}: pick combined, input or output tokens.",
                b.name
            ));
        }
        if !["day", "week", "month"].contains(&b.period.as_str()) {
            return Err(format!("{}: pick a day, a week or a month.", b.name));
        }
        if b.limit == 0 {
            return Err(format!("{}: set a limit above zero.", b.name));
        }
        b.host = b.host.trim().to_string();
        b.model = b.model.trim().to_string();
        if !crate::config::valid_host_id(&b.id) {
            b.id = crate::config::slug(&format!("{}-{}", b.name, b.period));
        }
        let base = b.id.clone();
        let mut n = 2;
        while !seen.insert(b.id.clone()) {
            b.id = format!("{base}-{n}");
            n += 1;
        }
    }
    Ok(list)
}

// ------------------------------------------------------------ alerts

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: u64,
    /// "budget", "host_down", "host_up" or "stampede".
    pub kind: String,
    /// "info", "warn" or "bad".
    pub level: String,
    pub title: String,
    pub message: String,
    pub at: DateTime<Local>,
    /// Where the portal should go when it's clicked.
    pub href: String,
}

/// Recent alerts, newest last, plus what has already been announced.
#[derive(Default)]
pub struct Alerts {
    inner: Mutex<AlertsInner>,
}

#[derive(Default)]
struct AlertsInner {
    next: u64,
    recent: VecDeque<Alert>,
    /// budget id -> (period start, highest level announced: 1 = 80%, 2 = 100%)
    budget_levels: HashMap<String, (NaiveDate, u8)>,
    /// host id -> when it was first seen down (announced once past a minute)
    down_since: HashMap<String, (Instant, bool)>,
    stampede_at: Option<Instant>,
    loaded: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    budget_levels: HashMap<String, (NaiveDate, u8)>,
    recent: Vec<Alert>,
}

impl Alerts {
    pub fn new() -> Alerts {
        Alerts::default()
    }

    pub fn recent(&self, limit: usize) -> Vec<Alert> {
        let i = self.inner.lock();
        i.recent.iter().rev().take(limit).cloned().collect()
    }

    /// Alerts after `since` (by id), oldest first.
    pub fn since(&self, since: u64) -> Vec<Alert> {
        let i = self.inner.lock();
        i.recent.iter().filter(|a| a.id > since).cloned().collect()
    }

    pub fn clear(&self) {
        self.inner.lock().recent.clear();
    }

    fn push(&self, kind: &str, level: &str, title: String, message: String, href: &str) -> Alert {
        let mut i = self.inner.lock();
        i.next += 1;
        let a = Alert {
            id: i.next,
            kind: kind.into(),
            level: level.into(),
            title,
            message,
            at: Local::now(),
            href: href.into(),
        };
        i.recent.push_back(a.clone());
        while i.recent.len() > 100 {
            i.recent.pop_front();
        }
        a
    }
}

fn state_path(app: &App) -> std::path::PathBuf {
    app.paths.base.join("alerts.json")
}

fn load(app: &App) {
    let mut i = app.alerts.inner.lock();
    if i.loaded {
        return;
    }
    i.loaded = true;
    if let Some(p) = std::fs::read(state_path(app))
        .ok()
        .and_then(|b| serde_json::from_slice::<Persisted>(&b).ok())
    {
        i.budget_levels = p.budget_levels;
        i.next = p.recent.iter().map(|a| a.id).max().unwrap_or(0);
        i.recent = p.recent.into_iter().collect();
    }
}

fn save(app: &App) {
    let p = {
        let i = app.alerts.inner.lock();
        Persisted {
            budget_levels: i.budget_levels.clone(),
            recent: i.recent.iter().cloned().collect(),
        }
    };
    if let Ok(b) = serde_json::to_vec_pretty(&p) {
        let tmp = state_path(app).with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(tmp, state_path(app));
        }
    }
}

/// Logs an alert and shows it as a desktop notification.
pub fn notify(title: &str, message: &str) {
    log::info!("alert: {title}: {message}");
    crate::platform::notify(title, message);
}

pub fn fmt_tokens(n: u64) -> String {
    crate::app::fmt_compact(n)
}

/// One pass of the alert checks. Returns the alerts it raised.
pub fn check(app: &Arc<App>) -> Vec<Alert> {
    load(app);
    let cfg = app.config.read().clone();
    let mut raised = Vec::new();
    let now = Local::now();

    // ---- budgets
    let statuses = app.ledger.with_data(|d| status(d, &cfg.budgets, now));
    for s in &statuses {
        let level: u8 = if s.ratio >= 1.0 {
            2
        } else if s.ratio >= 0.8 {
            1
        } else {
            0
        };
        let fire = {
            let mut i = app.alerts.inner.lock();
            let e = i
                .budget_levels
                .entry(s.budget.id.clone())
                .or_insert((s.period_start, 0));
            if e.0 != s.period_start {
                *e = (s.period_start, 0);
            }
            if level > e.1 {
                e.1 = level;
                true
            } else {
                false
            }
        };
        if fire {
            let scope = scope_name(app, &s.budget);
            let title = if level == 2 {
                format!("{} is over budget", s.budget.name)
            } else {
                format!("{} is at {}%", s.budget.name, (s.ratio * 100.0).floor())
            };
            let message = format!(
                "{} of {} {} tokens this {} for {}. At this pace: {}.",
                fmt_tokens(s.used),
                fmt_tokens(s.budget.limit),
                s.budget.metric,
                s.budget.period,
                scope,
                fmt_tokens(s.projected)
            );
            let a = app.alerts.push(
                "budget",
                if level == 2 { "bad" } else { "warn" },
                title,
                message,
                "#/dashboard",
            );
            if cfg.alerts.budgets && s.budget.notify {
                notify(&a.title, &a.message);
            }
            raised.push(a);
        }
    }

    // ---- hosts that stop answering for a minute, and come back
    let st = app.status.read().clone();
    for h in cfg.hosts.iter().filter(|h| h.enabled) {
        let Some(hs) = st.hosts.get(&h.id) else {
            continue;
        };
        if !hs.upstream.checked {
            continue;
        }
        let down = !hs.upstream.ok || !hs.proxy_ok;
        let mut i = app.alerts.inner.lock();
        if down {
            let e = i
                .down_since
                .entry(h.id.clone())
                .or_insert((Instant::now(), false));
            if !e.1 && e.0.elapsed() >= Duration::from_secs(60) {
                e.1 = true;
                drop(i);
                let why = if !hs.proxy_ok {
                    hs.proxy_msg.clone()
                } else {
                    hs.upstream.message.clone()
                };
                let a = app.alerts.push(
                    "host_down",
                    "bad",
                    format!("{} stopped answering", h.name),
                    why,
                    "#/settings/hosts",
                );
                if cfg.alerts.host_down {
                    notify(&a.title, &a.message);
                }
                raised.push(a);
            }
        } else if let Some((_, announced)) = i.down_since.remove(&h.id) {
            drop(i);
            if announced {
                let a = app.alerts.push(
                    "host_up",
                    "info",
                    format!("{} is back", h.name),
                    "LM Studio answers again and Toktail is counting.".into(),
                    "#/dashboard",
                );
                if cfg.alerts.host_down {
                    notify(&a.title, &a.message);
                }
                raised.push(a);
            }
        }
    }

    // ---- stampede (at most once every 10 minutes)
    let (snap, state) = app.pip(10);
    if state == crate::live::PipState::Stampede {
        let due = {
            let mut i = app.alerts.inner.lock();
            let due = i
                .stampede_at
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(600));
            if due {
                i.stampede_at = Some(Instant::now());
            }
            due
        };
        if due {
            let a = app.alerts.push(
                "stampede",
                "info",
                "Holy tokens! A stampede".into(),
                format!(
                    "{} requests at once, {} tokens/sec.",
                    snap.active_requests,
                    crate::app::fmt_thousands(snap.tokens_per_sec.round() as u64)
                ),
                "#/dashboard",
            );
            if cfg.alerts.stampede {
                notify(&a.title, &a.message);
            }
            raised.push(a);
        }
    }
    if !raised.is_empty() {
        save(app);
    }
    raised
}

pub fn scope_name(app: &App, b: &BudgetConfig) -> String {
    let host = if b.host.is_empty() {
        "all hosts".to_string()
    } else {
        app.config
            .read()
            .host(&b.host)
            .map(|h| h.name.clone())
            .unwrap_or_else(|| b.host.clone())
    };
    if b.model.is_empty() {
        host
    } else {
        format!("{} on {host}", b.model)
    }
}

/// Runs the checks every 20 seconds for the life of the app.
pub async fn run(app: Arc<App>) {
    tokio::time::sleep(Duration::from_secs(15)).await;
    loop {
        let a = app.clone();
        let _ = tokio::task::spawn_blocking(move || check(&a)).await;
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn periods() {
        assert_eq!(
            period_bounds("day", d(2026, 10, 13)),
            (d(2026, 10, 13), d(2026, 10, 13))
        );
        // 2026-10-13 is a Tuesday.
        assert_eq!(
            period_bounds("week", d(2026, 10, 13)),
            (d(2026, 10, 12), d(2026, 10, 18))
        );
        assert_eq!(
            period_bounds("month", d(2024, 2, 10)),
            (d(2024, 2, 1), d(2024, 2, 29))
        );
    }

    #[test]
    fn validation_fills_ids_and_rejects_nonsense() {
        let list = validate(vec![BudgetConfig::default(), BudgetConfig::default()]).unwrap();
        assert_eq!(list[0].id, "daily-tokens-day");
        assert_eq!(list[1].id, "daily-tokens-day-2");
        assert!(validate(vec![BudgetConfig {
            limit: 0,
            ..Default::default()
        }])
        .is_err());
        assert!(validate(vec![BudgetConfig {
            period: "year".into(),
            ..Default::default()
        }])
        .is_err());
    }

    #[tokio::test]
    async fn budgets_fire_once_per_level() {
        let app = crate::test_support::app();
        let mut r = crate::test_support::record("m");
        r.input_tokens = 850;
        r.output_tokens = 0;
        app.ledger.append(&r).unwrap();
        app.config.write().budgets = validate(vec![BudgetConfig {
            name: "Laptop".into(),
            host: "laptop".into(),
            limit: 1000,
            notify: false,
            ..Default::default()
        }])
        .unwrap();
        let s = app
            .ledger
            .with_data(|d| status(d, &app.config.read().budgets, Local::now()));
        assert_eq!((s[0].used, s[0].state), (850, "warn"));
        let first = check(&app);
        assert_eq!(first.len(), 1);
        assert!(first[0].title.contains("85%"), "{}", first[0].title);
        assert!(check(&app).is_empty(), "announced once");
        r.request_id = uuid::Uuid::new_v4().to_string();
        r.input_tokens = 200;
        app.ledger.append(&r).unwrap();
        let over = check(&app);
        assert!(over[0].title.contains("over budget"));
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
