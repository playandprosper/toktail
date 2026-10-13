// SPDX-License-Identifier: Apache-2.0
//! Live throughput: tokens per second over the last few minutes, counted as
//! they stream through the proxy. This is what Pip's tail draws and what
//! decides whether Pip is napping, munching, or caught in a stampede.
//!
//! In memory only. The CSV stays the record of what happened; this is the
//! pulse of what is happening right now.

use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WINDOW_SECS: u64 = 300;

pub struct Live {
    started: Instant,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// (second since start, output tokens, input tokens), oldest first.
    secs: VecDeque<(u64, u64, u64)>,
    active: i64,
    last_activity: Option<Instant>,
    typical_tps: Option<(f64, Instant)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PipState {
    /// Nothing to count. Nap time!
    Idle,
    /// Nom nom nom...
    Processing,
    /// Holy tokens! We've got a stampede!
    Stampede,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Output tokens per second, oldest first, one entry per second.
    pub per_second: Vec<u64>,
    pub active_requests: i64,
    /// Output tokens per second, averaged over the last 5 seconds.
    pub tokens_per_sec: f64,
    pub output_last_minute: u64,
    pub input_last_minute: u64,
    pub seconds_since_activity: Option<u64>,
}

/// Decrements the active-request count when dropped, however a request ends.
pub struct ActiveGuard(Arc<Live>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut i = self.0.inner.lock();
        i.active = (i.active - 1).max(0);
        i.last_activity = Some(Instant::now());
    }
}

impl Live {
    pub fn new() -> Arc<Live> {
        Arc::new(Live {
            started: Instant::now(),
            inner: Mutex::new(Inner::default()),
        })
    }

    fn now_sec(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn begin(self: &Arc<Self>) -> ActiveGuard {
        let mut i = self.inner.lock();
        i.active += 1;
        i.last_activity = Some(Instant::now());
        ActiveGuard(self.clone())
    }

    /// Adds tokens to the current second.
    pub fn add(&self, output: u64, input: u64) {
        if output == 0 && input == 0 {
            return;
        }
        let now = self.now_sec();
        let mut i = self.inner.lock();
        match i.secs.back_mut() {
            Some(last) if last.0 == now => {
                last.1 += output;
                last.2 += input;
            }
            _ => i.secs.push_back((now, output, input)),
        }
        while i.secs.front().is_some_and(|f| f.0 + WINDOW_SECS < now) {
            i.secs.pop_front();
        }
        i.last_activity = Some(Instant::now());
    }

    pub fn snapshot(&self, seconds: usize) -> Snapshot {
        let seconds = seconds.clamp(1, WINDOW_SECS as usize);
        let now = self.now_sec();
        let i = self.inner.lock();
        let mut per_second = vec![0u64; seconds];
        let (mut out_min, mut in_min, mut last5) = (0u64, 0u64, 0u64);
        for &(s, o, inp) in &i.secs {
            let age = now.saturating_sub(s);
            if (age as usize) < seconds {
                per_second[seconds - 1 - age as usize] += o;
            }
            if age < 60 {
                out_min += o;
                in_min += inp;
            }
            // The current second is still filling, so average seconds 1..=5.
            if (1..=5).contains(&age) {
                last5 += o;
            }
        }
        let current: u64 = i.secs.back().filter(|b| b.0 == now).map_or(0, |b| b.1);
        let tokens_per_sec = (last5 as f64 / 5.0).max(current as f64);
        Snapshot {
            per_second,
            active_requests: i.active,
            tokens_per_sec,
            output_last_minute: out_min,
            input_last_minute: in_min,
            seconds_since_activity: i.last_activity.map(|t| t.elapsed().as_secs()),
        }
    }

    /// Cached "usual single-request speed" so the stampede threshold adapts
    /// to the host. `compute` runs at most once a minute.
    pub fn typical_tps(&self, compute: impl FnOnce() -> f64) -> f64 {
        {
            let i = self.inner.lock();
            if let Some((v, at)) = i.typical_tps {
                if at.elapsed() < Duration::from_secs(60) {
                    return v;
                }
            }
        }
        let v = compute();
        self.inner.lock().typical_tps = Some((v, Instant::now()));
        v
    }
}

/// Pip's mood from the live numbers.
pub fn pip_state(s: &Snapshot, stampede_tps: f64) -> PipState {
    let busy_recently = s.seconds_since_activity.is_some_and(|t| t < 8);
    if s.active_requests == 0 && s.tokens_per_sec < 0.5 && !busy_recently {
        PipState::Idle
    } else if s.active_requests >= 4 || (stampede_tps > 0.0 && s.tokens_per_sec >= stampede_tps) {
        PipState::Stampede
    } else {
        PipState::Processing
    }
}

// ------------------------------------------------------------ activity

/// What is streaming right now, request by request, and what just finished.
/// The portal's live feed reads this every second. Counts and timings only:
/// never prompt or response text.
pub struct Activity {
    next: AtomicU64,
    inner: Mutex<ActivityInner>,
}

#[derive(Default)]
struct ActivityInner {
    flying: BTreeMap<u64, Flight>,
    done: VecDeque<Finished>,
    seq: u64,
}

struct Flight {
    host: String,
    model: String,
    endpoint: String,
    streamed: bool,
    started: Instant,
    started_at: DateTime<Local>,
    output: u64,
    first_token: Option<Instant>,
}

/// A request in flight, as the portal sees it.
#[derive(Debug, Clone, Serialize)]
pub struct FlightView {
    pub id: u64,
    pub host: String,
    pub model: String,
    pub endpoint: String,
    pub streamed: bool,
    pub started_at: String,
    pub elapsed_ms: u64,
    /// Output tokens streamed so far (about one per delta).
    pub output: u64,
    pub ttft_ms: Option<u64>,
    /// Output tokens per second since the first token.
    pub tokens_per_sec: Option<f64>,
}

/// A request that just finished.
#[derive(Debug, Clone, Serialize)]
pub struct Finished {
    pub seq: u64,
    pub request_id: String,
    pub host: String,
    pub model: String,
    pub endpoint: String,
    pub status: u16,
    pub input: u64,
    pub output: u64,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub outcome: String,
    pub signal: String,
    pub finished_at: String,
}

const KEEP_FINISHED: usize = 300;

/// Removes its request from the in-flight list when dropped, however it ends.
pub struct FlightGuard {
    activity: Arc<Activity>,
    id: u64,
}

impl FlightGuard {
    /// Streamed output so far, and when the first token arrived.
    pub fn progress(&self, output: u64, first_token: Option<Instant>) {
        if let Some(f) = self.activity.inner.lock().flying.get_mut(&self.id) {
            f.output = output;
            if f.first_token.is_none() {
                f.first_token = first_token;
            }
        }
    }
    /// The model, once the response names it.
    pub fn set_model(&self, model: &str) {
        if let Some(f) = self.activity.inner.lock().flying.get_mut(&self.id) {
            if f.model != model {
                f.model = model.to_string();
            }
        }
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        self.activity.inner.lock().flying.remove(&self.id);
    }
}

impl Activity {
    pub fn new() -> Arc<Activity> {
        Arc::new(Activity {
            next: AtomicU64::new(1),
            inner: Mutex::new(ActivityInner::default()),
        })
    }

    pub fn begin(
        self: &Arc<Self>,
        host: &str,
        model: Option<&str>,
        endpoint: &str,
        streamed: bool,
    ) -> FlightGuard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.inner.lock().flying.insert(
            id,
            Flight {
                host: host.to_string(),
                model: model.unwrap_or("unknown").to_string(),
                endpoint: endpoint.to_string(),
                streamed,
                started: Instant::now(),
                started_at: Local::now(),
                output: 0,
                first_token: None,
            },
        );
        FlightGuard {
            activity: self.clone(),
            id,
        }
    }

    /// Records a finished request; `seq` is filled in here.
    pub fn finish(&self, mut f: Finished) {
        let mut i = self.inner.lock();
        i.seq += 1;
        f.seq = i.seq;
        i.done.push_back(f);
        while i.done.len() > KEEP_FINISHED {
            i.done.pop_front();
        }
    }

    /// Everything in flight, and up to `limit` finished requests newer than `since`.
    pub fn snapshot(&self, since: u64, limit: usize) -> (Vec<FlightView>, Vec<Finished>, u64) {
        let i = self.inner.lock();
        let now = Instant::now();
        let flying = i
            .flying
            .iter()
            .map(|(id, f)| {
                let ttft = f.first_token.map(|t| t.duration_since(f.started));
                let gen = f.first_token.map(|t| now.duration_since(t).as_secs_f64());
                FlightView {
                    id: *id,
                    host: f.host.clone(),
                    model: f.model.clone(),
                    endpoint: f.endpoint.clone(),
                    streamed: f.streamed,
                    started_at: f.started_at.to_rfc3339(),
                    elapsed_ms: now.duration_since(f.started).as_millis() as u64,
                    output: f.output,
                    ttft_ms: ttft.map(|d| d.as_millis() as u64),
                    tokens_per_sec: gen
                        .filter(|g| *g > 0.25 && f.output > 0)
                        .map(|g| f.output as f64 / g),
                }
            })
            .collect();
        let mut done: Vec<Finished> = i
            .done
            .iter()
            .rev()
            .filter(|f| f.seq > since)
            .take(limit)
            .cloned()
            .collect();
        done.reverse();
        (flying, done, i.seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_moods() {
        let live = Live::new();
        let idle = live.snapshot(60);
        assert_eq!(pip_state(&idle, 100.0), PipState::Idle);

        let g = live.begin();
        live.add(40, 10);
        live.add(2, 0);
        let s = live.snapshot(60);
        assert_eq!(s.per_second.len(), 60);
        assert_eq!(*s.per_second.last().unwrap(), 42);
        assert_eq!(s.output_last_minute, 42);
        assert_eq!(s.active_requests, 1);
        assert_eq!(pip_state(&s, 100.0), PipState::Processing);
        assert_eq!(pip_state(&s, 10.0), PipState::Stampede);
        drop(g);
        assert_eq!(live.snapshot(5).active_requests, 0);

        let guards: Vec<_> = (0..4).map(|_| live.begin()).collect();
        assert_eq!(pip_state(&live.snapshot(5), 0.0), PipState::Stampede);
        drop(guards);
    }

    #[test]
    fn activity_tracks_flights_and_finishes() {
        let a = Activity::new();
        let g = a.begin("studio", Some("qwen"), "/v1/chat/completions", true);
        g.progress(12, Some(Instant::now()));
        let (fl, done, seq) = a.snapshot(0, 10);
        assert_eq!((fl.len(), fl[0].output, done.len(), seq), (1, 12, 0, 0));
        drop(g);
        a.finish(Finished {
            seq: 0,
            request_id: "r".into(),
            host: "studio".into(),
            model: "qwen".into(),
            endpoint: "/v1/chat/completions".into(),
            status: 200,
            input: 5,
            output: 12,
            duration_ms: 100,
            ttft_ms: Some(10),
            tokens_per_sec: Some(133.0),
            outcome: "complete".into(),
            signal: "brief".into(),
            finished_at: String::new(),
        });
        let (fl, done, seq) = a.snapshot(0, 10);
        assert_eq!((fl.len(), done.len(), seq), (0, 1, 1));
        assert!(a.snapshot(1, 10).1.is_empty(), "nothing newer than seq 1");
    }
}
