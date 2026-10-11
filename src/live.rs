// SPDX-License-Identifier: Apache-2.0
//! Live throughput: tokens per second over the last few minutes, counted as
//! they stream through the proxy. This is what Pip's tail draws and what
//! decides whether Pip is napping, munching, or caught in a stampede.
//!
//! In memory only. The CSV stays the record of what happened; this is the
//! pulse of what is happening right now.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
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
}
