//! Shared application state handed to every service.

use crate::config::{Config, Paths};
use crate::ledger::Ledger;
use chrono::{DateTime, Local};
use parking_lot::RwLock;
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
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub proxy_ok: bool,
    pub proxy_msg: String,
    pub s3: S3Status,
}

#[derive(Debug, Clone, Default)]
pub struct S3Status {
    pub enabled: bool,
    pub in_progress: bool,
    pub last_ok: Option<DateTime<Local>>,
    pub last_error: Option<String>,
    pub files_uploaded_total: u64,
}

impl App {
    pub fn portal_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.config.read().portal.port)
    }

    pub fn set_proxy_status(&self, ok: bool, msg: impl Into<String>) {
        let mut s = self.status.write();
        s.proxy_ok = ok;
        s.proxy_msg = msg.into();
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
    let (v, unit) = if n >= 1_000_000_000 {
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
        assert_eq!(fmt_compact(1234), "1.23K");
        assert_eq!(fmt_compact(12_345_678), "12.3M");
        assert_eq!(fmt_compact(2_500_000_000), "2.50B");
    }
}
