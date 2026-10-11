// SPDX-License-Identifier: Apache-2.0
//! The machines behind your hosts, and what they save you.
//!
//! Each machine has a serial number, what you paid, when you bought it and a
//! lifecycle (72 months unless you say otherwise). Its price is spread evenly
//! over that lifecycle; after it, the machine is paid off. Savings for a
//! month = what that month's tokens would have cost at public API prices,
//! minus the month's share of hardware and the electricity spent generating.
//!
//! Kept in `hardware.json` next to config.toml (no secrets, no prompts).
//! "Replace" retires a machine on a date and starts a new one on the same
//! host, so lifetime numbers stay with the machine that earned them.

use crate::config::CostsConfig;
use crate::ledger::{Data, Row};
use crate::pricing::{self, PriceBook};
use anyhow::{bail, Result};
use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Machine {
    pub id: String,
    pub name: String,
    /// "Mac Studio (M3 Ultra, 512 GB)".
    pub model: String,
    pub serial: String,
    pub price_usd: f64,
    pub purchased: NaiveDate,
    pub lifecycle_months: u32,
    /// Which watched host this machine serves.
    pub host: String,
    /// Power draw while generating, in watts.
    pub watts: f64,
    /// Power draw the rest of the time (0 to leave idle power out).
    pub idle_watts: f64,
    /// Set when the machine was replaced or retired.
    pub retired: Option<NaiveDate>,
    pub replaced_by: Option<String>,
    pub notes: String,
}

impl Default for Machine {
    fn default() -> Self {
        Machine {
            id: String::new(),
            name: "Mac Studio".into(),
            model: String::new(),
            serial: String::new(),
            price_usd: 0.0,
            purchased: Local::now().date_naive(),
            lifecycle_months: 72,
            host: "local".into(),
            watts: 180.0,
            idle_watts: 0.0,
            retired: None,
            replaced_by: None,
            notes: String::new(),
        }
    }
}

impl Machine {
    /// Last day this machine served (today if still in service).
    pub fn end(&self, today: NaiveDate) -> NaiveDate {
        self.retired.unwrap_or(today).min(today)
    }
    pub fn serves(&self, r: &Row, today: NaiveDate) -> bool {
        *r.host == *self.host
            && r.date >= self.purchased
            && (self.retired.is_none() || r.date < self.end(today))
    }
    /// The day the lifecycle ends.
    pub fn lifecycle_end(&self) -> NaiveDate {
        self.purchased
            .checked_add_months(chrono::Months::new(self.lifecycle_months.max(1)))
            .unwrap_or(self.purchased)
    }
    /// Hardware cost per day while inside the lifecycle.
    pub fn per_day(&self) -> f64 {
        let days = (self.lifecycle_end() - self.purchased).num_days().max(1) as f64;
        self.price_usd.max(0.0) / days
    }
    /// Amortized hardware cost between two dates (inclusive), within service and lifecycle.
    pub fn amortized(&self, from: NaiveDate, to: NaiveDate, today: NaiveDate) -> f64 {
        let start = from.max(self.purchased);
        let end = to
            .min(self.end(today))
            .min(self.lifecycle_end() - chrono::Days::new(1));
        if end < start {
            return 0.0;
        }
        ((end - start).num_days() + 1) as f64 * self.per_day()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fleet {
    #[serde(default)]
    pub machines: Vec<Machine>,
}

pub fn path(base: &Path) -> PathBuf {
    base.join("hardware.json")
}

impl Fleet {
    pub fn load(base: &Path) -> Fleet {
        std::fs::read(path(base))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, base: &Path) -> Result<()> {
        let p = path(base);
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    /// Adds or updates a machine; returns its id.
    pub fn upsert(&mut self, mut m: Machine) -> Result<String> {
        validate(&m)?;
        if m.id.is_empty() {
            m.id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
            self.machines.push(m.clone());
        } else if let Some(slot) = self.machines.iter_mut().find(|x| x.id == m.id) {
            *slot = m.clone();
        } else {
            bail!("no machine with id {}", m.id);
        }
        Ok(m.id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.machines.len();
        self.machines.retain(|m| m.id != id);
        for m in &mut self.machines {
            if m.replaced_by.as_deref() == Some(id) {
                m.replaced_by = None;
            }
        }
        self.machines.len() != before
    }

    /// Retires `old_id` on `new.purchased` and puts `new` on the same host.
    pub fn replace(&mut self, old_id: &str, mut new: Machine) -> Result<String> {
        let Some(old) = self.machines.iter().find(|m| m.id == old_id).cloned() else {
            bail!("no machine with id {old_id}");
        };
        if old.retired.is_some() {
            bail!("{} was already retired", old.name);
        }
        if new.purchased < old.purchased {
            bail!("the new machine can't start before the old one was bought");
        }
        new.id.clear();
        new.host = old.host.clone();
        new.retired = None;
        new.replaced_by = None;
        let id = self.upsert(new.clone())?;
        if let Some(o) = self.machines.iter_mut().find(|m| m.id == old_id) {
            o.retired = Some(new.purchased);
            o.replaced_by = Some(id.clone());
        }
        Ok(id)
    }
}

fn validate(m: &Machine) -> Result<()> {
    if m.name.trim().is_empty() {
        bail!("Give the machine a name.");
    }
    if !(0.0..=10_000_000.0).contains(&m.price_usd) {
        bail!("The price looks wrong.");
    }
    if !(1..=600).contains(&m.lifecycle_months) {
        bail!("The lifecycle must be between 1 and 600 months.");
    }
    if !(0.0..=100_000.0).contains(&m.watts) || !(0.0..=100_000.0).contains(&m.idle_watts) {
        bail!("The power draw looks wrong.");
    }
    if m.purchased > Local::now().date_naive() + chrono::Days::new(1) {
        bail!("The purchase date is in the future.");
    }
    if let Some(r) = m.retired {
        if r < m.purchased {
            bail!("A machine can't be retired before it was bought.");
        }
    }
    if m.serial.len() > 64 || m.name.len() > 120 || m.model.len() > 160 || m.notes.len() > 4000 {
        bail!("One of the fields is too long.");
    }
    Ok(())
}

// ------------------------------------------------------------------ reports

#[derive(Debug, Clone, Default, Serialize)]
pub struct MachineReport {
    #[serde(flatten)]
    pub machine: Machine,
    pub active: bool,
    pub age_days: i64,
    pub lifecycle_end: NaiveDate,
    /// 0..=1+ of the lifecycle used (can pass 1 when kept longer).
    pub lifecycle_used: f64,
    pub months_left: f64,
    pub monthly_cost: f64,
    pub amortized_so_far: f64,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Output tokens per second while generating: last 7 / 30 days and lifetime.
    pub tps_7d: Option<f64>,
    pub tps_30d: Option<f64>,
    pub tps_lifetime: Option<f64>,
    /// Weekly generation speed, oldest first (up to 26 weeks): (week start, tps).
    pub tps_weekly: Vec<(NaiveDate, f64)>,
    pub busy_hours: f64,
    pub electricity: f64,
    /// What the same tokens would have cost at the baseline model's prices.
    pub cloud_value: f64,
    pub net_savings: f64,
    /// cloud_value / (price + electricity): ≥ 1 means it has paid for itself.
    pub payback: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Period {
    /// "2026-10" or "2026".
    pub label: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub hardware: f64,
    pub electricity: f64,
    pub local: f64,
    /// Cost at each compared model, by price-book id.
    pub cloud: BTreeMap<String, f64>,
    pub baseline_cloud: f64,
    pub savings: f64,
    /// True for the current, still-running month or year.
    pub partial: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Savings {
    pub baseline: String,
    pub baseline_name: String,
    pub compared: Vec<(String, String)>,
    pub usd_per_kwh: f64,
    pub machines: Vec<MachineReport>,
    pub monthly: Vec<Period>,
    pub yearly: Vec<Period>,
    pub total: Period,
    /// Monthly savings at the last 30 days' pace.
    pub pace_monthly: f64,
    pub pace_yearly: f64,
    /// Hosts with traffic but no machine assigned (their tokens count as
    /// cloud value with no hardware cost).
    pub unassigned_hosts: Vec<String>,
    pub default_lifecycle_months: u32,
}

fn month_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap()
}

fn next_month(d: NaiveDate) -> NaiveDate {
    month_start(d)
        .checked_add_months(chrono::Months::new(1))
        .unwrap()
}

fn tps(out: u64, ms: u64) -> Option<f64> {
    (ms > 0 && out > 0).then(|| out as f64 * 1000.0 / ms as f64)
}

pub fn savings(
    data: &Data,
    fleet: &Fleet,
    book: &PriceBook,
    cfg: &CostsConfig,
    default_lifecycle: u32,
) -> Savings {
    let today = Local::now().date_naive();
    let compared_ids = book.compared(cfg);
    let baseline = if book.find(&cfg.baseline).is_some() {
        cfg.baseline.clone()
    } else {
        compared_ids.first().cloned().unwrap_or_default()
    };
    let price = cfg.electricity_usd_per_kwh.max(0.0);
    let models: Vec<&pricing::PublicModel> = book
        .models
        .iter()
        .filter(|m| compared_ids.contains(&m.id) || m.id == baseline)
        .collect();

    // Range of months: earliest purchase or data, to this month.
    let first_data = data.rows().map(|r| r.date).min();
    let first_buy = fleet.machines.iter().map(|m| m.purchased).min();
    let first = [first_data, first_buy]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(today)
        .min(today);

    let mut months: BTreeMap<NaiveDate, Period> = BTreeMap::new();
    let mut m = month_start(first);
    while m <= today {
        let end = (next_month(m) - chrono::Days::new(1)).min(today);
        months.insert(
            m,
            Period {
                label: m.format("%Y-%m").to_string(),
                start: m,
                end,
                partial: end == today && next_month(m) > today,
                ..Default::default()
            },
        );
        m = next_month(m);
    }

    // Per-machine accumulators.
    struct Acc {
        req: u64,
        inp: u64,
        out: u64,
        gen_out: u64,
        gen_ms: u64,
        out7: u64,
        ms7: u64,
        out30: u64,
        ms30: u64,
        weekly: BTreeMap<NaiveDate, (u64, u64)>,
        intervals: BTreeMap<NaiveDate, Vec<(i64, i64)>>,
        cloud: f64,
    }
    let mut accs: Vec<Acc> = fleet
        .machines
        .iter()
        .map(|_| Acc {
            req: 0,
            inp: 0,
            out: 0,
            gen_out: 0,
            gen_ms: 0,
            out7: 0,
            ms7: 0,
            out30: 0,
            ms30: 0,
            weekly: BTreeMap::new(),
            intervals: BTreeMap::new(),
            cloud: 0.0,
        })
        .collect();
    let d7 = today - chrono::Days::new(7);
    let d30 = today - chrono::Days::new(30);
    let w26 = today - chrono::Days::new(26 * 7);
    let mut unassigned: BTreeMap<String, u64> = BTreeMap::new();
    let mut pace_cloud = 0.0;

    for r in data.rows() {
        if pricing::is_embedding(r) || (r.input == 0 && r.output == 0) {
            continue;
        }
        let mut base_cost = 0.0;
        let p = months.get_mut(&month_start(r.date));
        if let Some(p) = p {
            p.requests += 1;
            p.input_tokens += r.input;
            p.output_tokens += r.output;
            for pm in &models {
                let (i, o, q) = pm.cost_of(r);
                *p.cloud.entry(pm.id.clone()).or_default() += i + o + q;
                if pm.id == baseline {
                    base_cost = i + o + q;
                }
            }
            p.baseline_cloud += base_cost;
        }
        if r.date > d30 {
            pace_cloud += base_cost;
        }
        let owner = fleet.machines.iter().position(|m| m.serves(r, today));
        match owner {
            Some(i) => {
                let a = &mut accs[i];
                a.req += 1;
                a.inp += r.input;
                a.out += r.output;
                a.cloud += base_cost;
                if r.output > 0 && r.gen_ms > 0 {
                    a.gen_out += r.output;
                    a.gen_ms += r.gen_ms;
                    if r.date > d7 {
                        a.out7 += r.output;
                        a.ms7 += r.gen_ms;
                    }
                    if r.date > d30 {
                        a.out30 += r.output;
                        a.ms30 += r.gen_ms;
                    }
                    if r.date > w26 {
                        let wk = r.date
                            - chrono::Days::new(r.date.weekday().num_days_from_monday() as u64);
                        let e = a.weekly.entry(wk).or_default();
                        e.0 += r.output;
                        e.1 += r.gen_ms;
                    }
                }
                a.intervals
                    .entry(r.date)
                    .or_default()
                    .push((r.ts_ms - r.duration_ms as i64, r.ts_ms));
            }
            None => *unassigned.entry(r.host.to_string()).or_default() += 1,
        }
    }

    // Hardware and electricity per machine, spread over months.
    let mut reports = Vec::new();
    let mut pace_local = 0.0;
    for (m, a) in fleet.machines.iter().zip(accs) {
        let mut busy_total = 0.0;
        let mut elec_total = 0.0;
        for (day, iv) in a.intervals {
            let hours = pricing::busy_ms(iv) as f64 / 3_600_000.0;
            let cost = hours * m.watts / 1000.0 * price;
            busy_total += hours;
            elec_total += cost;
            if let Some(p) = months.get_mut(&month_start(day)) {
                p.electricity += cost;
            }
            if day > d30 {
                pace_local += cost;
            }
        }
        // Idle power, every day in service.
        let end = m.end(today);
        if m.idle_watts > 0.0 && end >= m.purchased {
            for p in months.values_mut() {
                let s = p.start.max(m.purchased);
                let e = p.end.min(end);
                if e >= s {
                    let days = ((e - s).num_days() + 1) as f64;
                    let c = days * 24.0 * m.idle_watts / 1000.0 * price;
                    p.electricity += c;
                    elec_total += c;
                }
            }
            if end > d30 {
                let s = (d30 + chrono::Days::new(1)).max(m.purchased);
                pace_local +=
                    ((end - s).num_days() + 1).max(0) as f64 * 24.0 * m.idle_watts / 1000.0 * price;
            }
        }
        for p in months.values_mut() {
            p.hardware += m.amortized(p.start, p.end, today);
        }
        pace_local += m.amortized(d30 + chrono::Days::new(1), today, today);
        let amortized_so_far = m.amortized(m.purchased, today, today);
        let age_days = (m.end(today) - m.purchased).num_days().max(0);
        let life_days = (m.lifecycle_end() - m.purchased).num_days().max(1);
        let lifecycle_used =
            (today.min(m.end(today)) - m.purchased).num_days().max(0) as f64 / life_days as f64;
        let months_left = if m.retired.is_some() {
            0.0
        } else {
            ((m.lifecycle_end() - today).num_days().max(0) as f64) / 30.4375
        };
        let net = a.cloud - amortized_so_far - elec_total;
        reports.push(MachineReport {
            machine: m.clone(),
            active: m.retired.is_none(),
            age_days,
            lifecycle_end: m.lifecycle_end(),
            lifecycle_used,
            months_left,
            monthly_cost: m.price_usd / m.lifecycle_months.max(1) as f64,
            amortized_so_far,
            requests: a.req,
            input_tokens: a.inp,
            output_tokens: a.out,
            tps_7d: tps(a.out7, a.ms7),
            tps_30d: tps(a.out30, a.ms30),
            tps_lifetime: tps(a.gen_out, a.gen_ms),
            tps_weekly: a
                .weekly
                .into_iter()
                .filter_map(|(w, (o, ms))| tps(o, ms).map(|t| (w, t)))
                .collect(),
            busy_hours: busy_total,
            electricity: elec_total,
            cloud_value: a.cloud,
            net_savings: net,
            payback: if m.price_usd + elec_total > 0.0 {
                a.cloud / (m.price_usd + elec_total)
            } else {
                0.0
            },
        });
    }

    let mut monthly: Vec<Period> = months.into_values().collect();
    for p in &mut monthly {
        p.local = p.hardware + p.electricity;
        p.savings = p.baseline_cloud - p.local;
    }
    let mut years: BTreeMap<i32, Period> = BTreeMap::new();
    let mut total = Period {
        label: "All time".into(),
        start: first,
        end: today,
        ..Default::default()
    };
    for p in &monthly {
        let y = years.entry(p.start.year()).or_insert_with(|| Period {
            label: p.start.year().to_string(),
            start: p.start,
            end: p.end,
            partial: p.start.year() == today.year(),
            ..Default::default()
        });
        for t in [y, &mut total] {
            t.end = p.end;
            t.requests += p.requests;
            t.input_tokens += p.input_tokens;
            t.output_tokens += p.output_tokens;
            t.hardware += p.hardware;
            t.electricity += p.electricity;
            t.local += p.local;
            t.baseline_cloud += p.baseline_cloud;
            t.savings += p.savings;
            for (k, v) in &p.cloud {
                *t.cloud.entry(k.clone()).or_default() += v;
            }
        }
    }
    let pace_monthly = (pace_cloud - pace_local) * 30.4375 / 30.0;
    Savings {
        baseline_name: book
            .find(&baseline)
            .map(|m| m.name.clone())
            .unwrap_or_default(),
        baseline,
        compared: models
            .iter()
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect(),
        usd_per_kwh: price,
        machines: reports,
        monthly,
        yearly: years.into_values().collect(),
        total,
        pace_monthly,
        pace_yearly: pace_monthly * 12.0,
        unassigned_hosts: unassigned.into_keys().collect(),
        default_lifecycle_months: default_lifecycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn amortization_spreads_over_lifecycle() {
        let m = Machine {
            price_usd: 7200.0,
            purchased: d(2024, 1, 1),
            lifecycle_months: 72,
            ..Default::default()
        };
        let today = d(2026, 10, 11);
        let per_day = m.per_day();
        assert!(
            (per_day * (m.lifecycle_end() - m.purchased).num_days() as f64 - 7200.0).abs() < 1e-6
        );
        let jan = m.amortized(d(2024, 1, 1), d(2024, 1, 31), today);
        assert!((jan - 31.0 * per_day).abs() < 1e-9);
        // Nothing before purchase or after the lifecycle.
        assert_eq!(m.amortized(d(2023, 1, 1), d(2023, 12, 31), today), 0.0);
        assert_eq!(
            m.amortized(d(2031, 1, 1), d(2031, 1, 31), d(2031, 6, 1)),
            0.0
        );
    }

    #[test]
    fn replace_retires_old_machine() {
        let mut f = Fleet::default();
        let a = f
            .upsert(Machine {
                name: "Studio A".into(),
                price_usd: 4000.0,
                purchased: d(2024, 1, 1),
                ..Default::default()
            })
            .unwrap();
        let b = f
            .replace(
                &a,
                Machine {
                    name: "Studio B".into(),
                    price_usd: 9000.0,
                    purchased: d(2026, 6, 1),
                    ..Default::default()
                },
            )
            .unwrap();
        let old = f.machines.iter().find(|m| m.id == a).unwrap();
        assert_eq!(old.retired, Some(d(2026, 6, 1)));
        assert_eq!(old.replaced_by.as_deref(), Some(b.as_str()));
        assert!(f.replace(&a, Machine::default()).is_err());
        assert!(f
            .upsert(Machine {
                name: "".into(),
                ..Default::default()
            })
            .is_err());
    }
}
