// SPDX-License-Identifier: Apache-2.0
//! "This computer" in Settings: how much Toktail itself uses (memory, CPU,
//! threads, disk) and how the computer it runs on is doing (memory in use,
//! memory pressure, swap, CPU), sampled every 2 seconds and kept for an
//! hour, in memory only.
//!
//! Memory pressure is the operating system's own measure where it has one:
//! macOS's pressure level (normal, warning, critical) and Linux's pressure
//! stall information (the share of the last 10 seconds tasks waited for
//! memory). Elsewhere it is left out rather than guessed.

use crate::app::App;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

const EVERY: Duration = Duration::from_secs(2);
/// One hour of samples.
const KEEP: usize = 1800;

/// One moment.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Sample {
    /// Milliseconds since the epoch.
    pub t: i64,
    /// Toktail's resident memory, bytes.
    pub rss: u64,
    /// Toktail's CPU, percent of one core.
    pub cpu: f32,
    pub threads: u32,
    /// The computer's memory in use and in total, bytes.
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// The computer's CPU, percent of all cores.
    pub sys_cpu: f32,
    /// 0-100 (Linux: share of time stalled on memory; macOS: 0 normal, 50
    /// warning, 100 critical). None where the OS has no such measure.
    pub pressure: Option<f32>,
    /// Requests in flight through Toktail.
    pub in_flight: u32,
}

#[derive(Default)]
struct Inner {
    samples: VecDeque<Sample>,
    /// Disk use of Toktail's folders, refreshed every minute.
    disk: Value,
    disk_at: Option<Instant>,
    /// macOS pressure, refreshed every 10 s (it needs a subprocess).
    pressure: Option<f32>,
    pressure_at: Option<Instant>,
}

#[derive(Default)]
pub struct HostStatus {
    inner: Mutex<Inner>,
}

impl HostStatus {
    pub fn new() -> HostStatus {
        HostStatus::default()
    }
}

fn dir_size(p: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 500_000 {
                return total;
            }
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn disk_use(app: &App) -> Value {
    let base = app.paths.base.clone();
    let records = crate::records::root(app);
    let data = app.paths.data_dir.clone();
    let parts = [
        ("history", data.clone()),
        ("records", records.clone()),
        ("cache", base.join("cache")),
        ("summaries", base.join("summaries")),
        ("market", base.join("market")),
    ];
    let mut out = serde_json::Map::new();
    let mut total = 0;
    for (k, p) in parts {
        let n = dir_size(&p);
        total += n;
        out.insert(
            k.into(),
            json!({ "bytes": n, "path": p.display().to_string() }),
        );
    }
    out.insert("total".into(), json!(total));
    Value::Object(out)
}

#[cfg(target_os = "linux")]
fn pressure_now() -> Option<f32> {
    let t = std::fs::read_to_string("/proc/pressure/memory").ok()?;
    let line = t.lines().find(|l| l.starts_with("some"))?;
    line.split_whitespace()
        .find_map(|w| w.strip_prefix("avg10="))
        .and_then(|v| v.parse::<f32>().ok())
}

#[cfg(target_os = "macos")]
fn pressure_now() -> Option<f32> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.memorystatus_vm_pressure_level"])
        .output()
        .ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "1" => Some(0.0),
        "2" => Some(50.0),
        "4" => Some(100.0),
        _ => None,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn pressure_now() -> Option<f32> {
    None
}

/// Samples every 2 seconds while Toktail runs.
pub async fn run(app: Arc<App>) {
    let pid = Pid::from_u32(std::process::id());
    let mut sys = System::new();
    loop {
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        let (rss, cpu, threads) = sys
            .process(pid)
            .map(|p| {
                (
                    p.memory(),
                    p.cpu_usage(),
                    p.tasks().map(|t| t.len() as u32).unwrap_or(0),
                )
            })
            .unwrap_or_default();
        let threads = if threads > 0 { threads } else { thread_count() };
        let in_flight = app.activity.snapshot(u64::MAX, 0).0.len() as u32;
        let (need_disk, need_pressure) = {
            let i = app.host_status.inner.lock();
            (
                i.disk_at
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(60)),
                i.pressure_at.is_none_or(|t| {
                    t.elapsed()
                        > Duration::from_secs(if cfg!(target_os = "macos") { 10 } else { 0 })
                }),
            )
        };
        let disk = if need_disk {
            let a = app.clone();
            tokio::task::spawn_blocking(move || disk_use(&a)).await.ok()
        } else {
            None
        };
        let pressure = if need_pressure {
            Some(
                tokio::task::spawn_blocking(pressure_now)
                    .await
                    .ok()
                    .flatten(),
            )
        } else {
            None
        };
        {
            let mut i = app.host_status.inner.lock();
            if let Some(d) = disk {
                i.disk = d;
                i.disk_at = Some(Instant::now());
            }
            if let Some(p) = pressure {
                i.pressure = p;
                i.pressure_at = Some(Instant::now());
            }
            let s = Sample {
                t: chrono::Local::now().timestamp_millis(),
                rss,
                cpu,
                threads,
                mem_used: sys.used_memory(),
                mem_total: sys.total_memory(),
                swap_used: sys.used_swap(),
                swap_total: sys.total_swap(),
                sys_cpu: sys.global_cpu_usage(),
                pressure: i.pressure,
                in_flight,
            };
            i.samples.push_back(s);
            while i.samples.len() > KEEP {
                i.samples.pop_front();
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Threads, where sysinfo doesn't list them (macOS, Windows).
fn thread_count() -> u32 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/task")
            .map(|d| d.count() as u32)
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// The latest sample, the last `minutes` of history and disk use.
pub fn snapshot(app: &App, minutes: usize) -> Value {
    let i = app.host_status.inner.lock();
    let keep = (minutes.clamp(1, 60) * 60 / EVERY.as_secs() as usize).max(1);
    let series: Vec<&Sample> = i
        .samples
        .iter()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let now = i.samples.back().cloned();
    let la = System::load_average();
    json!({
        "now": now,
        "series": series,
        "every_secs": EVERY.as_secs(),
        "disk": i.disk,
        "system": {
            "os": System::long_os_version(),
            "host": System::host_name(),
            "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            "uptime_secs": System::uptime(),
            "load": if cfg!(windows) { Value::Null } else { json!([la.one, la.five, la.fifteen]) },
            "pressure_kind": if cfg!(target_os = "macos") { "macOS memory pressure level" } else if cfg!(target_os = "linux") { "time stalled on memory (PSI, last 10 s)" } else { "" },
        },
        "toktail": { "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION") },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn samples_and_snapshot() {
        let app = crate::test_support::app();
        let a = app.clone();
        let h = tokio::spawn(run(a));
        // The first sample is taken at once.
        for _ in 0..100 {
            if !app.host_status.inner.lock().samples.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        h.abort();
        let v = snapshot(&app, 5);
        assert!(v["now"]["rss"].as_u64().unwrap() > 0, "{v}");
        assert!(v["now"]["mem_total"].as_u64().unwrap() > 0);
        assert_eq!(v["series"].as_array().unwrap().len(), 1);
        assert!(v["disk"]["total"].is_u64());
    }

    #[test]
    fn sizes_folders() {
        let d = std::env::temp_dir().join(format!("toktail-du-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/x"), [0u8; 1000]).unwrap();
        std::fs::write(d.join("y"), [0u8; 24]).unwrap();
        assert_eq!(dir_size(&d), 1024);
        let _ = std::fs::remove_dir_all(d);
    }
}
