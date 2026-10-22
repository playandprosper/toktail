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
    /// Answers whose agent hung up before they finished, kept a while for
    /// the agent's retry (by same-input key).
    orphans: std::collections::HashMap<String, Orphan>,
}

/// A finished answer nobody received yet.
struct Orphan {
    id: u64,
    control: Arc<crate::flights::FlightControl>,
    until: Instant,
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
    /// Identical requests share this (host, endpoint, body, credential id).
    key: Option<String>,
    agent: String,
    session: String,
    project: String,
    /// Something has gone back to the agent; it can no longer be rerouted.
    released: bool,
    /// The agent hung up; when Toktail will stop it unless a retry joins.
    gone_since: Option<Instant>,
    grace_secs: u64,
    /// Answered by another request in flight.
    joined_to: Option<u64>,
    /// Requests answered by this one.
    joiners: u32,
    rerouted_from: Option<String>,
    control: Arc<crate::flights::FlightControl>,
    /// Size of the request body: the prompt the model has to read.
    prompt_bytes: u64,
    /// Every retry that has waited on this one's answer, including those
    /// whose agent gave up again.
    retries_seen: u32,
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
    pub agent: String,
    pub session: String,
    pub project: String,
    /// An identical request started earlier and is still running.
    pub duplicate_of: Option<u64>,
    /// The agent hung up on it.
    pub agent_gone: bool,
    /// Seconds left before Toktail stops it, unless a retry joins.
    pub stops_in_secs: Option<u64>,
    pub joined_to: Option<u64>,
    pub joiners: u32,
    pub rerouted_from: Option<String>,
    /// Nothing has gone back to the agent yet: it can still be rerouted.
    pub can_reroute: bool,
    /// About how many tokens of prompt it carries.
    pub prompt_tokens_est: u64,
    /// Its place among prompts waiting to be read by the same model on the
    /// same host (1 = first in line; LM Studio reads them one at a time).
    pub queue_pos: Option<u32>,
    pub queue_len: u32,
    /// It waits on an answer that already finished (kept for this retry).
    pub joined_finished: bool,
}

/// Requests piling up on one model on one host, and what Toktail does about it.
#[derive(Debug, Clone, Serialize)]
pub struct Stack {
    pub host: String,
    pub model: String,
    /// Prompts waiting to be read, oldest first (the first is being read).
    pub line: Vec<u64>,
    pub line_tokens_est: u64,
    /// Copies of one input: the original and the retries it answers.
    pub groups: Vec<RetryGroup>,
    /// How long the oldest has been waiting.
    pub oldest_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryGroup {
    pub original: u64,
    pub agent: String,
    pub prompt_tokens_est: u64,
    /// The original's agent hung up (its retries still want the answer).
    pub original_gone: bool,
    /// How many times the agent has sent this input (the original included).
    pub attempts: u32,
    /// The original already finished; its answer was kept for the retries.
    pub original_finished: bool,
    /// Retries waiting on the original's answer instead of being read again.
    pub answered_by_original: Vec<u64>,
    /// Copies of the same input still running on their own (merge them).
    pub running_separately: Vec<u64>,
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
    /// Answered from Toktail's prompt cache; no model ran.
    #[serde(default)]
    pub cache_hit: bool,
    /// The coding agent that sent it, when known (opencode, claude-code…).
    #[serde(default)]
    pub agent: String,
    /// What happened to it in Toktail: "cancelled", "rerouted from Studio",
    /// "answered by #12", "agent hung up"…
    #[serde(default)]
    pub note: String,
}

const KEEP_FINISHED: usize = 300;

/// Removes its request from the in-flight list when dropped, however it ends.
pub struct FlightGuard {
    activity: Arc<Activity>,
    id: u64,
    control: Arc<crate::flights::FlightControl>,
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
    fn with<R>(&self, f: impl FnOnce(&mut Flight) -> R) -> Option<R> {
        self.activity.inner.lock().flying.get_mut(&self.id).map(f)
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn control(&self) -> Arc<crate::flights::FlightControl> {
        self.control.clone()
    }
    /// Who sent it, and the key identical requests share.
    pub fn describe(
        &self,
        key: Option<String>,
        agent: &str,
        session: &str,
        project: &str,
        prompt_bytes: u64,
    ) {
        self.with(|f| {
            f.key = key;
            f.agent = agent.to_string();
            f.session = session.to_string();
            f.project = project.to_string();
            f.prompt_bytes = prompt_bytes;
        });
    }
    /// Rerouted: now running on `host` with `model`.
    pub fn rerouted(&self, from: &str, host: &str, model: Option<&str>) {
        self.with(|f| {
            f.rerouted_from.get_or_insert_with(|| from.to_string());
            f.host = host.to_string();
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                f.model = m.to_string();
            }
            f.started_at = Local::now();
        });
    }
    /// Bytes have gone back to the agent.
    pub fn released(&self) {
        self.with(|f| f.released = true);
    }
    /// The agent hung up; Toktail stops it after `grace_secs` unless a
    /// retry joins.
    pub fn agent_gone(&self, grace_secs: u64) {
        self.with(|f| {
            f.gone_since.get_or_insert_with(Instant::now);
            f.grace_secs = grace_secs;
        });
    }
    /// Now answered by request `leader`.
    pub fn joined(&self, leader: u64) {
        let mut i = self.activity.inner.lock();
        if let Some(f) = i.flying.get_mut(&self.id) {
            f.joined_to = Some(leader);
            f.released = true;
        }
        if let Some(l) = i.flying.get_mut(&leader) {
            l.joiners += 1;
            l.retries_seen += 1;
        }
    }
    /// No longer waiting on another request's answer.
    pub fn unjoin(&self) {
        let mut i = self.activity.inner.lock();
        let leader = i.flying.get_mut(&self.id).and_then(|f| {
            f.released = false;
            f.joined_to.take()
        });
        if let Some(l) = leader.and_then(|l| i.flying.get_mut(&l)) {
            l.joiners = l.joiners.saturating_sub(1);
        }
    }
    /// How many other requests wait for this one's answer.
    pub fn joiners(&self) -> u32 {
        self.with(|f| f.joiners).unwrap_or(0)
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        let mut i = self.activity.inner.lock();
        if let Some(f) = i.flying.remove(&self.id) {
            if let Some(l) = f.joined_to.and_then(|l| i.flying.get_mut(&l)) {
                l.joiners = l.joiners.saturating_sub(1);
            }
            // Its agent hung up and it finished anyway: keep the answer for
            // the retry that's probably on its way, instead of reading the
            // same prompt again.
            let answered = {
                let rd = self.control.tee.read(usize::MAX);
                rd.head.is_some() && rd.done.as_ref().is_some_and(|d| d.ok) && !rd.overflow
            };
            if let (Some(key), Some(_), true, true) = (
                f.key,
                f.gone_since,
                f.joiners == 0 && f.grace_secs > 0,
                answered,
            ) {
                i.orphans.insert(
                    key,
                    Orphan {
                        id: self.id,
                        control: self.control.clone(),
                        until: Instant::now() + Duration::from_secs(f.grace_secs),
                    },
                );
            }
        }
        drop(i);
        // However it ended, anyone waiting on its answer hears that it did.
        self.control.tee.finish(crate::flights::Summary::default());
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
        let control = Arc::new(crate::flights::FlightControl::default());
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
                key: None,
                agent: String::new(),
                session: String::new(),
                project: String::new(),
                released: false,
                gone_since: None,
                grace_secs: 0,
                joined_to: None,
                joiners: 0,
                rerouted_from: None,
                control: control.clone(),
                prompt_bytes: 0,
                retries_seen: 0,
            },
        );
        FlightGuard {
            activity: self.clone(),
            id,
            control,
        }
    }

    /// The steering wheel of a request in flight.
    pub fn control_of(&self, id: u64) -> Option<Arc<crate::flights::FlightControl>> {
        self.inner.lock().flying.get(&id).map(|f| f.control.clone())
    }

    /// The model a request in flight runs on now.
    pub fn model_of(&self, id: u64) -> Option<String> {
        self.inner.lock().flying.get(&id).map(|f| f.model.clone())
    }

    /// The endpoint of a request in flight.
    pub fn endpoint_of(&self, id: u64) -> Option<String> {
        self.inner
            .lock()
            .flying
            .get(&id)
            .map(|f| f.endpoint.clone())
    }

    /// The earliest request in flight with this key that can still answer
    /// others (not itself answered by another, not `except`).
    pub fn leader_for(
        &self,
        key: &str,
        except: u64,
    ) -> Option<(u64, Arc<crate::flights::FlightControl>)> {
        let mut i = self.inner.lock();
        let running = i
            .flying
            .iter()
            .filter(|(id, f)| {
                **id != except
                    && f.joined_to.is_none()
                    && f.key.as_deref() == Some(key)
                    && f.control.tee.joinable()
            })
            .min_by_key(|(id, _)| **id)
            .map(|(id, f)| (*id, f.control.clone()));
        if running.is_some() {
            return running;
        }
        // Or one that finished after its agent hung up.
        let now = Instant::now();
        i.orphans.retain(|_, o| o.until > now);
        i.orphans.get(key).map(|o| (o.id, o.control.clone()))
    }

    /// Whether `id` is an answer kept for retries (it already finished).
    pub fn is_orphan(&self, id: u64) -> bool {
        self.inner.lock().orphans.values().any(|o| o.id == id)
    }

    /// Answers every other running copy of `leader`'s input with `leader`'s
    /// answer. Copies already streaming their own answer are left alone.
    pub fn collapse(&self, leader: u64) -> Result<usize, String> {
        let i = self.inner.lock();
        let l = i
            .flying
            .get(&leader)
            .ok_or("That request already finished.")?;
        if l.joined_to.is_some() {
            return Err("That one is itself waiting on another request.".into());
        }
        let key = l.key.clone().ok_or("It has no input to compare.")?;
        if !l.control.tee.joinable() {
            return Err("Its answer is too big to share.".into());
        }
        let mut n = 0;
        for (id, f) in &i.flying {
            if *id != leader
                && f.key.as_deref() == Some(&key)
                && f.joined_to.is_none()
                && !f.released
            {
                f.control.send(crate::flights::Cmd::Join(leader));
                n += 1;
            }
        }
        Ok(n)
    }

    /// Where requests are piling up: per host and model, the prompts waiting
    /// to be read and the copies of one input Toktail answers once.
    pub fn stacks(&self) -> Vec<Stack> {
        let i = self.inner.lock();
        let now = Instant::now();
        let mut by: BTreeMap<(String, String), Vec<(u64, &Flight)>> = BTreeMap::new();
        for (id, f) in &i.flying {
            by.entry((f.host.clone(), f.model.clone()))
                .or_default()
                .push((*id, f));
        }
        let mut out = Vec::new();
        for ((host, model), fl) in by {
            let line: Vec<(u64, &Flight)> = fl
                .iter()
                .filter(|(_, f)| f.output == 0 && f.joined_to.is_none())
                .copied()
                .collect();
            let mut groups: Vec<RetryGroup> = Vec::new();
            // Originals: not themselves joined, with joiners or later copies.
            for (id, f) in &fl {
                let Some(key) = f.key.as_ref() else { continue };
                if f.joined_to.is_some() || groups.iter().any(|g| g.running_separately.contains(id))
                {
                    continue;
                }
                let joined: Vec<u64> = i
                    .flying
                    .iter()
                    .filter(|(_, g)| g.joined_to == Some(*id))
                    .map(|(j, _)| *j)
                    .collect();
                let separate: Vec<u64> = fl
                    .iter()
                    .filter(|(o, g)| o > id && g.joined_to.is_none() && g.key.as_ref() == Some(key))
                    .map(|(o, _)| *o)
                    .collect();
                if joined.is_empty() && separate.is_empty() {
                    continue;
                }
                groups.push(RetryGroup {
                    original: *id,
                    agent: f.agent.clone(),
                    prompt_tokens_est: crate::flights::tokens_estimate(f.prompt_bytes),
                    original_gone: f.gone_since.is_some(),
                    attempts: 1 + f.retries_seen.max(joined.len() as u32) + separate.len() as u32,
                    original_finished: false,
                    answered_by_original: joined,
                    running_separately: separate,
                });
            }
            // Retries waiting on an answer that already finished.
            for o in i.orphans.values().filter(|o| o.until > now) {
                let joined: Vec<u64> = fl
                    .iter()
                    .filter(|(_, g)| g.joined_to == Some(o.id))
                    .map(|(j, _)| *j)
                    .collect();
                if !joined.is_empty() {
                    groups.push(RetryGroup {
                        original: o.id,
                        agent: fl
                            .iter()
                            .find(|(j, _)| *j == joined[0])
                            .map(|(_, g)| g.agent.clone())
                            .unwrap_or_default(),
                        prompt_tokens_est: fl
                            .iter()
                            .find(|(j, _)| *j == joined[0])
                            .map(|(_, g)| crate::flights::tokens_estimate(g.prompt_bytes))
                            .unwrap_or(0),
                        original_gone: true,
                        attempts: 1 + joined.len() as u32,
                        original_finished: true,
                        answered_by_original: joined,
                        running_separately: vec![],
                    });
                }
            }
            if line.len() < 2 && groups.is_empty() {
                continue;
            }
            out.push(Stack {
                host,
                model,
                line_tokens_est: line
                    .iter()
                    .map(|(_, f)| crate::flights::tokens_estimate(f.prompt_bytes))
                    .sum(),
                oldest_ms: fl
                    .iter()
                    .map(|(_, f)| now.duration_since(f.started).as_millis() as u64)
                    .max()
                    .unwrap_or(0),
                line: line.iter().map(|(id, _)| *id).collect(),
                groups,
            });
        }
        out
    }

    /// Asks request `id` to do something, after checking it can.
    pub fn command(&self, id: u64, cmd: crate::flights::Cmd) -> Result<String, String> {
        use crate::flights::Cmd;
        let i = self.inner.lock();
        let f = i.flying.get(&id).ok_or("That request already finished.")?;
        let msg = match &cmd {
            Cmd::Cancel => format!("Cancelling #{id}."),
            Cmd::Reroute { host, model } => {
                if f.released {
                    return Err(
                        "It has already started answering; it can only be cancelled now.".into(),
                    );
                }
                if f.joined_to.is_some() {
                    return Err(
                        "It's waiting on another request's answer; cancel that one, or this."
                            .into(),
                    );
                }
                format!(
                    "Rerouting #{id} to {host}{}.",
                    if model.is_empty() {
                        String::new()
                    } else {
                        format!(" ({model})")
                    }
                )
            }
            Cmd::Join(to) => {
                if f.released {
                    return Err("It has already started answering.".into());
                }
                let l = i
                    .flying
                    .get(to)
                    .ok_or("The other request already finished.")?;
                if *to == id || l.key.is_none() || l.key != f.key {
                    return Err("Only identical requests can share an answer.".into());
                }
                if !l.control.tee.joinable() {
                    return Err("The other request's answer is too big to share.".into());
                }
                format!("#{id} will get #{to}'s answer.")
            }
            Cmd::None => return Err("Nothing to do.".into()),
        };
        f.control.send(cmd);
        Ok(msg)
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
        let flying: Vec<FlightView> = i
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
                    agent: f.agent.clone(),
                    session: f.session.clone(),
                    project: f.project.clone(),
                    duplicate_of: f.key.as_ref().and_then(|k| {
                        i.flying
                            .iter()
                            .filter(|(o, g)| {
                                *o < id && g.key.as_ref() == Some(k) && g.joined_to.is_none()
                            })
                            .map(|(o, _)| *o)
                            .next()
                    }),
                    agent_gone: f.gone_since.is_some(),
                    stops_in_secs: f
                        .gone_since
                        .filter(|_| f.joiners == 0)
                        .map(|g| f.grace_secs.saturating_sub(now.duration_since(g).as_secs())),
                    joined_to: f.joined_to,
                    joiners: f.joiners,
                    rerouted_from: f.rerouted_from.clone(),
                    can_reroute: !f.released && f.joined_to.is_none(),
                    prompt_tokens_est: crate::flights::tokens_estimate(f.prompt_bytes),
                    queue_pos: None,
                    queue_len: 0,
                    joined_finished: f.joined_to.is_some_and(|l| !i.flying.contains_key(&l)),
                }
            })
            .collect();
        let mut flying = flying;
        let lines: BTreeMap<(String, String), Vec<u64>> = flying
            .iter()
            .filter(|v| v.output == 0 && v.joined_to.is_none())
            .fold(BTreeMap::new(), |mut m, v| {
                m.entry((v.host.clone(), v.model.clone()))
                    .or_insert_with(Vec::new)
                    .push(v.id);
                m
            });
        for v in &mut flying {
            if let Some(line) = lines.get(&(v.host.clone(), v.model.clone())) {
                if let Some(p) = line.iter().position(|x| *x == v.id) {
                    v.queue_pos = Some(p as u32 + 1);
                    v.queue_len = line.len() as u32;
                }
            }
        }
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
            cache_hit: false,
            agent: String::new(),
            note: String::new(),
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
