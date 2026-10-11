// SPDX-License-Identifier: Apache-2.0
//! Shared application state handed to every service.

use crate::config::{Config, Paths};
use crate::ledger::Ledger;
use crate::live::{self, Live, PipState, Snapshot};
use crate::pricing::PriceBook;
use chrono::{DateTime, Local};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Notify;

pub struct App {
    pub config: RwLock<Config>,
    pub paths: Paths,
    pub ledger: Arc<Ledger>,
    pub status: RwLock<Status>,
    /// Wakes the S3 sync loop for an immediate sync.
    pub sync_now: Notify,
    pub rt: tokio::runtime::Handle,
    /// Live tokens/sec across all hosts, for Pip.
    pub live: Arc<Live>,
    /// Live tokens/sec per host.
    pub host_live: RwLock<HashMap<String, Arc<Live>>>,
    /// Public API list prices for the cost comparison.
    pub prices: RwLock<PriceBook>,
    /// Running `--demo`: sample data, pretend traffic, no login item.
    pub demo: bool,
    /// Bearer token sent to each host's LM Studio, if it requires one.
    pub host_tokens: RwLock<HashMap<String, String>>,
    /// Makes the proxies match the host list again (after Settings change).
    pub proxy_restart: Notify,
    /// Makes the LM Studio health checks run now.
    pub check_upstream: Notify,
    /// Encryption of saved prompts and responses.
    pub crypto: crate::crypto::Crypto,
    /// Saved prompts and responses.
    pub records: crate::records::Records,
    /// The machines behind the hosts (hardware.json).
    pub hardware: RwLock<crate::hardware::Fleet>,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub s3: S3Status,
    pub hosts: HashMap<String, HostStatus>,
}

/// One watched host: is Toktail listening for it, and does LM Studio answer.
#[derive(Debug, Clone, Default)]
pub struct HostStatus {
    pub proxy_ok: bool,
    pub proxy_msg: String,
    pub upstream: UpstreamStatus,
}

/// Whether LM Studio answers at the configured address.
#[derive(Debug, Clone, Default)]
pub struct UpstreamStatus {
    pub checked: bool,
    pub ok: bool,
    pub message: String,
    pub models: Vec<String>,
    pub checked_at: Option<DateTime<Local>>,
}

#[derive(Debug, Clone, Default)]
pub struct S3Status {
    pub enabled: bool,
    pub in_progress: bool,
    pub last_ok: Option<DateTime<Local>>,
    pub last_error: Option<String>,
    pub files_uploaded_total: u64,
    pub records_pending: u64,
}

impl App {
    pub fn portal_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.config.read().portal.port)
    }

    pub fn set_proxy_status(&self, host: &str, ok: bool, msg: impl Into<String>) {
        let mut s = self.status.write();
        let h = s.hosts.entry(host.to_string()).or_default();
        h.proxy_ok = ok;
        h.proxy_msg = msg.into();
    }

    /// The live meter for one host (created on first use).
    pub fn live_for(&self, host: &str) -> Arc<Live> {
        if let Some(l) = self.host_live.read().get(host) {
            return l.clone();
        }
        self.host_live
            .write()
            .entry(host.to_string())
            .or_insert_with(Live::new)
            .clone()
    }

    pub fn host_token(&self, host: &str) -> Option<String> {
        self.host_tokens.read().get(host).cloned()
    }

    /// Re-reads every host's token from the Keychain.
    pub fn load_host_tokens(&self) {
        let ids: Vec<String> = self
            .config
            .read()
            .hosts
            .iter()
            .map(|h| h.id.clone())
            .collect();
        let mut map = HashMap::new();
        for id in ids {
            if let Some(t) = crate::secrets::get_host_token(&id) {
                map.insert(id, t);
            }
        }
        *self.host_tokens.write() = map;
    }

    /// Loads encryption keys from settings, remembering the key fingerprint
    /// the first time a password is used.
    pub fn configure_crypto(&self) {
        let enc = self.config.read().encryption.clone();
        if let Some(check) = self.crypto.configure(&self.paths.base, &enc) {
            let mut c = self.config.write();
            c.encryption.key_check = check;
            if let Err(e) = crate::config::save(&self.paths.config_file, &c) {
                log::warn!("could not save config: {e:#}");
            }
        }
    }

    /// This host's usual single-request generation speed over the last 7
    /// days (output-weighted), recomputed at most once a minute.
    pub fn typical_tps(&self) -> f64 {
        self.live.typical_tps(|| {
            let since = Local::now().date_naive() - chrono::Days::new(7);
            let (out, ms) = self.ledger.with_data(|d| {
                d.rows()
                    .filter(|r| r.date >= since && r.output > 0 && r.gen_ms > 0)
                    .fold((0u64, 0u64), |a, r| (a.0 + r.output, a.1 + r.gen_ms))
            });
            if ms > 0 {
                out as f64 * 1000.0 / ms as f64
            } else {
                0.0
            }
        })
    }

    /// Combined tokens/sec at which Pip calls it a stampede.
    pub fn stampede_tps(&self) -> f64 {
        let configured = self.config.read().app.stampede_tokens_per_sec;
        if configured > 0.0 {
            return configured;
        }
        let typical = self.typical_tps();
        if typical > 0.0 {
            typical * 3.0
        } else {
            150.0
        }
    }

    pub fn pip(&self, seconds: usize) -> (Snapshot, PipState) {
        let snap = self.live.snapshot(seconds);
        let state = live::pip_state(&snap, self.stampede_tps());
        (snap, state)
    }
}

/// 1234 -> "1,234"
pub fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 1_234_567 -> "1.23M"
pub fn fmt_compact(n: u64) -> String {
    let f = n as f64;
    let (v, unit) = if n >= 1_000_000_000_000 {
        (f / 1e12, "T")
    } else if n >= 1_000_000_000 {
        (f / 1e9, "B")
    } else if n >= 1_000_000 {
        (f / 1e6, "M")
    } else if n >= 1_000 {
        (f / 1e3, "K")
    } else {
        return n.to_string();
    };
    let s = if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    format!("{s}{unit}")
}

pub fn ago(t: DateTime<Local>) -> String {
    let secs = (Local::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats() {
        assert_eq!(fmt_thousands(0), "0");
        assert_eq!(fmt_thousands(1234567), "1,234,567");
        assert_eq!(fmt_compact(999), "999");
        assert_eq!(fmt_compact(17_000_000_000_000), "17.0T");
        assert_eq!(fmt_compact(1234), "1.23K");
        assert_eq!(fmt_compact(12_345_678), "12.3M");
        assert_eq!(fmt_compact(2_500_000_000), "2.50B");
    }
}
