// SPDX-License-Identifier: Apache-2.0
//! Autopilot: keeps an opencode session that roc runs working until you
//! tell it to stop.
//!
//! roc publishes opencode's own server for each session (roc.rs), so
//! Toktail can see what the agent is doing and talk to it the way the TUI
//! does (opencode.rs). With autopilot on for a session, every few seconds
//! Toktail looks at it:
//!
//! - **Working**: nothing to do. If it stays busy with no new message for
//!   `silence_minutes`, you hear that it may be stuck (once).
//! - **Asked a question**: you're told (bell, notification, the session
//!   page), and nothing is sent until it's answered. If you don't answer
//!   within `question_timeout_minutes`, autopilot answers with your default
//!   answer and marks it as such.
//! - **Stopped and quiet for `grace_secs`**: autopilot sends the next prompt.
//!   If the agent said it's finished, that is your `done_template` (a firm
//!   "it isn't, keep going"), otherwise your `continue_template`. If you set
//!   a verifier, its output goes in, so "not done" comes with a work list.
//!
//! It stops sending when you say stop (the turn in progress finishes) or
//! stop now (the turn is aborted), and on its own only for things a person
//! has to fix: the same reply `repeat_limit` times, `error_limit` failed
//! turns in a row (with a diagnosis), the turn or token limit you set, or
//! the session ending. If the agent writes `NEED_HUMAN: <reason>`, you're
//! told and autopilot keeps nudging gently until you look. Runs survive a
//! Toktail restart and pick up where they were.
//!
//! Everything autopilot sends is your words, from Settings → Autopilot.

use crate::app::App;
use crate::opencode::{self, Client, Status};
use crate::roc::{self, RocSession};
use chrono::Local;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often running pilots are looked at.
const TICK: Duration = Duration::from_secs(5);
/// History entries kept per run.
const HISTORY: usize = 400;
/// What the agent writes to reach you.
pub const NEED_HUMAN: &str = "NEED_HUMAN:";

/// One autopilot run on one roc session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pilot {
    /// roc's session id.
    pub roc_id: String,
    pub label: String,
    pub project: String,
    pub workdir: String,
    pub model: String,
    pub container: String,
    /// The opencode session it drives (`ses_…`).
    pub session: String,
    /// What you want done, added to every prompt (optional).
    pub goal: String,
    /// running, stopping (after this turn), paused or stopped.
    pub state: String,
    /// Why it's paused or stopped.
    pub reason: String,
    pub started_at: String,
    pub started_ms: i64,
    /// "you" (the portal) or "pip" (the chat).
    pub started_by: String,
    pub turns: u32,
    pub done_claims: u32,
    pub done_in_row: u32,
    pub errors_in_row: u32,
    pub repeats_in_row: u32,
    pub last_reply_hash: String,
    /// The last assistant message autopilot answered.
    pub answered_message: String,
    pub last_prompt_at: Option<String>,
    pub last_prompt_ms: i64,
    /// The agent wrote NEED_HUMAN: this, and you haven't looked yet.
    pub need_human: Option<String>,
    /// "May be stuck" was said for the current quiet spell.
    pub stuck_flagged: bool,
    pub questions: Vec<QuestionLog>,
    pub history: Vec<Entry>,
    /// Bumped by every change from outside the loop (you, Pip), so a step
    /// that ran meanwhile keeps those changes.
    pub rev: u64,
}

/// What a step started from, to merge with changes made meanwhile.
struct Base {
    len: usize,
    rev: u64,
    state: String,
    need_human: Option<String>,
}

impl Base {
    fn of(p: &Pilot) -> Base {
        Base {
            len: p.history.len(),
            rev: p.rev,
            state: p.state.clone(),
            need_human: p.need_human.clone(),
        }
    }
}

/// Something that happened in a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub at: String,
    /// prompt, reply, question, answer, note, pause, stop, error, verifier.
    pub kind: String,
    pub text: String,
}

/// A question the agent asked, and how it was answered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QuestionLog {
    pub id: String,
    pub asked_at: String,
    pub asked_ms: i64,
    pub questions: Value,
    pub answered_at: Option<String>,
    pub answers: Option<Vec<Vec<String>>>,
    /// "you", "auto" (the default answer), or "tui" (answered elsewhere).
    pub by: String,
}

impl Pilot {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "running" | "stopping" | "paused")
    }

    fn log(&mut self, kind: &str, text: impl Into<String>) {
        self.history.push(Entry {
            at: Local::now().to_rfc3339(),
            kind: kind.into(),
            text: text.into(),
        });
        if self.history.len() > HISTORY {
            let cut = self.history.len() - HISTORY;
            self.history.drain(..cut);
        }
    }

    fn pending_questions(&self) -> usize {
        self.questions
            .iter()
            .filter(|q| q.answered_at.is_none())
            .count()
    }

    /// A short line for lists and Pip.
    pub fn summary(&self) -> Value {
        json!({
            "state": self.state, "reason": self.reason, "turns": self.turns,
            "done_claims": self.done_claims, "done_in_row": self.done_in_row,
            "pending_questions": self.pending_questions(), "need_human": self.need_human,
            "started_at": self.started_at, "started_by": self.started_by, "goal": self.goal,
            "last_prompt_at": self.last_prompt_at, "session": self.session,
        })
    }
}

/// All runs, live state that isn't saved, and the loop's wake-up.
pub struct Autopilot {
    pilots: Mutex<BTreeMap<String, Pilot>>,
    loaded: Mutex<bool>,
    pub wake: tokio::sync::Notify,
    /// roc id -> (busy since, last message activity ms) while busy.
    busy: Mutex<HashMap<String, (Instant, i64)>>,
}

impl Autopilot {
    pub fn new() -> Autopilot {
        Autopilot {
            pilots: Mutex::new(BTreeMap::new()),
            loaded: Mutex::new(false),
            wake: tokio::sync::Notify::new(),
            busy: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, roc_id: &str) -> Option<Pilot> {
        self.pilots.lock().get(roc_id).cloned()
    }

    pub fn all(&self) -> Vec<Pilot> {
        self.pilots.lock().values().cloned().collect()
    }
}

fn file(app: &App) -> std::path::PathBuf {
    app.paths.base.join("autopilot.json")
}

/// Reads saved runs once (so they resume after a restart).
pub fn load(app: &App) {
    let mut loaded = app.autopilot.loaded.lock();
    if *loaded {
        return;
    }
    *loaded = true;
    if let Some(map) = std::fs::read(file(app))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, Pilot>>(&b).ok())
    {
        *app.autopilot.pilots.lock() = map;
    }
}

fn save(app: &App) {
    let map = app.autopilot.pilots.lock().clone();
    // Keep finished runs for a week, so their history can still be read.
    let week_ago = Local::now().timestamp_millis() - 7 * 86_400_000;
    let map: BTreeMap<String, Pilot> = map
        .into_iter()
        .filter(|(_, p)| p.active() || p.last_prompt_ms.max(p.started_ms) > week_ago)
        .collect();
    if let Ok(b) = serde_json::to_vec_pretty(&map) {
        if let Err(e) = crate::portal_auth::write_private(&file(app), &b) {
            log::warn!("autopilot: could not save: {e}");
        }
    }
}

fn update<R>(app: &App, roc_id: &str, f: impl FnOnce(&mut Pilot) -> R) -> Option<R> {
    let r = {
        let mut m = app.autopilot.pilots.lock();
        m.get_mut(roc_id).map(f)
    };
    if r.is_some() {
        save(app);
    }
    r
}

/// A change from outside the loop (you, Pip): bumps the revision.
fn update_ext<R>(app: &App, roc_id: &str, f: impl FnOnce(&mut Pilot) -> R) -> Option<R> {
    update(app, roc_id, |p| {
        p.rev += 1;
        f(p)
    })
}

fn alert(app: &App, level: &str, title: String, message: String, roc_id: &str, desktop: bool) {
    crate::budgets::raise(
        app,
        "autopilot",
        level,
        title,
        message,
        &format!("#/agents/{roc_id}"),
        desktop,
    );
}

// ------------------------------------------------------------------ reading the agent

/// Lowercased, with runs of whitespace as one space.
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn reply_hash(s: &str) -> String {
    hex::encode(&Sha256::digest(normalize(s).as_bytes())[..12])
}

/// Does this reply say the work is finished? Plain phrases, no model.
pub fn claims_done(text: &str) -> bool {
    const PHRASES: &[&str] = &[
        "task is complete",
        "task is now complete",
        "tasks are complete",
        "tasks are now complete",
        "work is complete",
        "work is now complete",
        "work is done",
        "everything is complete",
        "everything is done",
        "all done",
        "i'm done",
        "i am done",
        "i'm finished",
        "i am finished",
        "i have completed",
        "i've completed",
        "i have finished",
        "i've finished",
        "is now complete",
        "are now complete",
        "implementation is complete",
        "nothing left to do",
        "nothing else to do",
        "nothing more to do",
        "no further work",
        "no remaining work",
        "status: complete",
        "fully complete",
        "fully implemented",
        "completed all",
        "finished all",
        "all requirements are met",
        "all requirements have been met",
        "all requirements are satisfied",
        "all items are complete",
        "all tasks completed",
        "all tasks are done",
        "task complete",
        "work complete",
        "100% complete",
        "is complete and verified",
    ];
    let t = normalize(&text.replace(['’', '‘'], "'"));
    PHRASES.iter().any(|p| t.contains(p))
}

/// What the agent wrote after NEED_HUMAN:, if it did.
pub fn need_human(text: &str) -> Option<String> {
    let i = text.find(NEED_HUMAN)?;
    let rest = text[i + NEED_HUMAN.len()..].trim();
    let line = rest.lines().next().unwrap_or("").trim();
    Some(if line.is_empty() {
        "(no reason given)".into()
    } else {
        line.chars().take(400).collect()
    })
}

/// Says what to fix when turns keep failing.
pub fn diagnose(error: &str) -> String {
    let e = error.to_lowercase();
    let fix = if e.contains("context")
        && (e.contains("length")
            || e.contains("window")
            || e.contains("exceed")
            || e.contains("too long"))
    {
        "The conversation no longer fits the model's context window. Load the model with a bigger context in LM Studio, or lower limit.context in roc's state.json so opencode compacts the history sooner, then start a new opencode session."
    } else if e.contains("401")
        || e.contains("403")
        || e.contains("unauthorized")
        || e.contains("api key")
        || e.contains("forbidden")
    {
        "The model server refused the API key. Check ROC_AI_API_TOKEN (or the Toktail gateway key roc uses)."
    } else if e.contains("timeout") || e.contains("timed out") || e.contains("aborted") {
        "Requests are timing out before the model answers. A large local model can take longer than opencode's 5-minute default to read a big prompt: raise provider.<id>.options.timeout in ~/.local/roc/agents/opencode.json (e.g. 1800000)."
    } else if e.contains("econnrefused")
        || e.contains("fetch failed")
        || e.contains("connect")
        || e.contains("unreachable")
        || e.contains("dns")
    {
        "The model server isn't answering. Check that LM Studio (or your host) is up and that roc's -ai-host is right."
    } else if e.contains("not found")
        || e.contains("404")
        || e.contains("no model")
        || e.contains("not loaded")
    {
        "The model isn't loaded on the server. Load it in LM Studio (roc -list shows which workers are available)."
    } else if e.contains("empty") {
        "The model keeps answering with nothing. It may be overloaded, or its context may be full."
    } else {
        "Look at the error in opencode's TUI or Toktail's Right now card; autopilot can't fix this one by itself."
    };
    fix.to_string()
}

/// Fills a template's placeholders.
pub fn render(template: &str, goal: &str, verifier: &str, turn: u32, done_claims: u32) -> String {
    let goal = if goal.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nThe human's goal for this session: {}", goal.trim())
    };
    let verifier = if verifier.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{}", verifier.trim_end())
    };
    template
        .replace("{{goal}}", &goal)
        .replace("{{verifier}}", &verifier)
        .replace("{{turn}}", &turn.to_string())
        .replace("{{done_claims}}", &done_claims.to_string())
        .trim()
        .to_string()
}

/// What to do about an idle session, decided from its last turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Too soon, or already answered.
    Wait,
    /// Send this kind of prompt (continue or done).
    Send { done: bool },
    /// Pause, and say why.
    Pause(String),
    /// Stop, and say why.
    Stop(String),
}

/// The last turn: its final assistant message and whether it used tools.
#[derive(Debug, Clone, Default)]
pub struct LastTurn {
    pub reply: Option<opencode::Message>,
    /// Any assistant message since the last user message called a tool.
    pub worked: bool,
    /// The last message is a user message (a prompt not answered yet).
    pub unanswered_prompt_ms: Option<i64>,
}

/// Groups recent messages into the last turn.
pub fn last_turn(msgs: &[opencode::Message]) -> LastTurn {
    let mut t = LastTurn::default();
    if let Some(last) = msgs.last() {
        if last.role == "user" {
            t.unanswered_prompt_ms = Some(last.created);
            return t;
        }
    }
    let start = msgs
        .iter()
        .rposition(|m| m.role == "user")
        .map(|i| i + 1)
        .unwrap_or(0);
    let turn = &msgs[start..];
    t.worked = turn.iter().any(|m| !m.tools.is_empty());
    t.reply = turn.iter().rev().find(|m| m.role == "assistant").cloned();
    t
}

/// Decides what an idle session gets. Updates the pilot's counters.
pub fn decide(
    p: &mut Pilot,
    cfg: &crate::config::AutopilotConfig,
    turn: &LastTurn,
    now_ms: i64,
) -> Decision {
    let grace = cfg.grace_secs as i64 * 1000;
    let silence = cfg.silence_minutes.max(1) as i64 * 60_000;
    if let Some(at) = turn.unanswered_prompt_ms {
        // A prompt opencode never answered (the model call died without a
        // message): after the silence, count it as a failure and try again.
        if now_ms - at < silence || now_ms - p.last_prompt_ms < silence {
            return Decision::Wait;
        }
        p.errors_in_row += 1;
        p.log("error", "The last prompt got no answer at all.");
        if p.errors_in_row >= cfg.error_limit.max(1) {
            return Decision::Stop(format!(
                "{} prompts in a row got no answer. {}",
                p.errors_in_row,
                diagnose("no answer timeout")
            ));
        }
        return Decision::Send { done: false };
    }
    let Some(reply) = turn.reply.as_ref() else {
        // A brand-new session: nothing to answer yet. Start it once.
        if p.turns == 0 && now_ms - p.started_ms >= grace.min(5_000) {
            return Decision::Send { done: false };
        }
        return Decision::Wait;
    };
    if reply.id == p.answered_message {
        return Decision::Wait;
    }
    let finished = reply.completed.unwrap_or(reply.created);
    if now_ms - finished < grace {
        return Decision::Wait;
    }
    p.answered_message = reply.id.clone();
    // Failures: model errors, and replies with nothing in them.
    let empty = reply.text.trim().is_empty()
        && reply.tools.is_empty()
        && !turn.worked
        && reply.output_tokens < 10;
    if let Some(e) = reply.error.as_ref().or(empty.then_some(&String::new())) {
        p.errors_in_row += 1;
        let what = if e.is_empty() {
            "an empty reply".to_string()
        } else {
            e.clone()
        };
        p.log("error", format!("The turn failed: {what}"));
        if p.errors_in_row >= cfg.error_limit.max(1) {
            let why = if e.is_empty() { "empty" } else { e.as_str() };
            return Decision::Stop(format!(
                "{} failed turns in a row (last: {what}). {}",
                p.errors_in_row,
                diagnose(why)
            ));
        }
        return Decision::Send { done: false };
    }
    p.errors_in_row = 0;
    // The same words again, with no work in between.
    let h = reply_hash(&reply.text);
    if h == p.last_reply_hash && !turn.worked {
        p.repeats_in_row += 1;
    } else {
        p.repeats_in_row = 0;
    }
    p.last_reply_hash = h;
    if p.repeats_in_row + 1 >= cfg.repeat_limit.max(2) {
        return Decision::Pause(format!(
            "The agent gave the same reply {} times in a row without doing any work.",
            p.repeats_in_row + 1
        ));
    }
    if let Some(why) = need_human(&reply.text) {
        p.need_human = Some(why);
    }
    let done = claims_done(&reply.text);
    if done {
        p.done_claims += 1;
        p.done_in_row += 1;
    } else {
        p.done_in_row = 0;
    }
    if cfg.max_turns > 0 && p.turns >= cfg.max_turns {
        return Decision::Stop(format!(
            "Reached the limit of {} prompts for one run (Settings → Autopilot).",
            cfg.max_turns
        ));
    }
    Decision::Send { done }
}

// ------------------------------------------------------------------ doing it

/// Finds a command a GUI app may not have on its PATH.
pub fn find_command(setting: &str, name: &str) -> String {
    let s = setting.trim();
    if !s.is_empty() {
        return crate::config::expand_tilde(s).display().to_string();
    }
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        home.join(".local/bin").join(name),
        home.join("bin").join(name),
        home.join(".orbstack/bin").join(name),
        std::path::PathBuf::from("/opt/homebrew/bin").join(name),
        std::path::PathBuf::from("/usr/local/bin").join(name),
        std::path::PathBuf::from("/usr/bin").join(name),
        std::path::PathBuf::from("/Applications/Docker.app/Contents/Resources/bin").join(name),
    ];
    candidates
        .iter()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// A PATH with the usual tool folders, for roc and docker started by a GUI app.
fn tool_path() -> std::ffi::OsString {
    let home = dirs::home_dir().unwrap_or_default();
    let mut parts: Vec<std::path::PathBuf> = if cfg!(windows) {
        vec![]
    } else {
        vec![
            "/opt/homebrew/bin".into(),
            "/usr/local/bin".into(),
            home.join(".local/bin"),
            home.join(".orbstack/bin"),
            "/Applications/Docker.app/Contents/Resources/bin".into(),
        ]
    };
    if let Some(p) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&p));
    }
    if !cfg!(windows) {
        parts.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(std::path::PathBuf::from));
    }
    std::env::join_paths(parts).unwrap_or_default()
}

/// Runs the verifier in the agent's container; its output for the prompt.
async fn run_verifier(cfg: &crate::config::AutopilotConfig, s: &RocSession) -> Option<String> {
    let cmd = cfg.verifier_command.trim();
    if cmd.is_empty() || s.container.is_empty() {
        return None;
    }
    let docker = find_command(&cfg.docker_command, "docker");
    let mut c = tokio::process::Command::new(&docker);
    c.args(["exec", "-w", &s.workdir, &s.container, "sh", "-lc", cmd])
        .env("PATH", tool_path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let limit = Duration::from_secs(cfg.verifier_timeout_secs.clamp(5, 3600));
    let out = match tokio::time::timeout(limit, c.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Some(format!("(The verifier `{cmd}` couldn't run: {e}.)")),
        Err(_) => {
            return Some(format!(
                "(The verifier `{cmd}` took longer than {}s and was stopped.)",
                limit.as_secs()
            ))
        }
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        text.push('\n');
        text.push_str(&err);
    }
    let text = crate::tokens::scrub(text.trim());
    let max = cfg.verifier_max_chars.clamp(200, 100_000);
    let shown: String = if text.chars().count() > max {
        let mut t: String = text.chars().take(max).collect();
        t.push_str("\n… (cut)");
        t
    } else {
        text
    };
    let code = out
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "a signal".into());
    Some(format!(
        "The verifier (`{cmd}`) exited with {code}:\n```\n{shown}\n```"
    ))
}

/// Tokens Toktail counted for this run (its host and model, since it began).
pub fn tokens_used(app: &App, host: Option<&str>, model: &str, since_ms: i64) -> u64 {
    app.ledger.with_data(|d| {
        d.rows()
            .filter(|r| {
                r.ts_ms >= since_ms && &*r.model == model && host.is_none_or(|h| &*r.host == h)
            })
            .map(|r| r.input + r.output)
            .sum()
    })
}

fn client_for(app: &App, s: &RocSession) -> anyhow::Result<Client> {
    let state = roc::state_path(&app.config.read().autopilot.roc_state);
    let url = s
        .control_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("roc didn't publish this session's opencode server (roc 0.3 or later does, for opencode's TUI)"))?;
    let pw = roc::password(&state, &s.id)
        .ok_or_else(|| anyhow::anyhow!("can't read roc's control password for this session"))?;
    Client::new(url, s.control_user.as_deref().unwrap_or("opencode"), &pw)
}

/// The roc session with this id (or a prefix, label or project name).
pub fn find_session(app: &App, target: &str) -> Result<RocSession, String> {
    let view = roc::read(&app.config.read().autopilot.roc_state);
    if let Some(p) = view.problem {
        return Err(p);
    }
    let t = target.trim().to_lowercase();
    let squash = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
    };
    let hits: Vec<&RocSession> = view
        .sessions
        .iter()
        .filter(|s| {
            t.is_empty()
                || s.id.starts_with(&t)
                || squash(&s.label) == squash(&t)
                || s.project().to_lowercase() == t
                || s.workdir.to_lowercase() == t
        })
        .collect();
    let drivable: Vec<&RocSession> = hits.iter().copied().filter(|s| s.controllable()).collect();
    match (drivable.len(), hits.len()) {
        (1, _) => Ok(drivable[0].clone()),
        (0, 0) => Err(if view.sessions.is_empty() {
            "roc isn't running any sessions.".into()
        } else {
            format!(
                "No roc session matches '{target}'. Running: {}.",
                view.sessions.iter().map(|s| format!("{} ({}, {})", s.label, s.binary, s.project())).collect::<Vec<_>>().join(", ")
            )
        }),
        (0, _) => Err("That session isn't an opencode TUI with roc's remote control (roc 0.3+). Codex, Claude Code, goose and `opencode run` can't be driven.".into()),
        _ => Err(format!(
            "Several sessions match: {}. Say which (label like Q #2, or project).",
            drivable.iter().map(|s| format!("{} ({})", s.label, s.project())).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Starts (or resumes) autopilot on a roc session.
pub async fn start(app: &Arc<App>, target: &str, goal: &str, by: &str) -> Result<Pilot, String> {
    load(app);
    let s = find_session(app, target)?;
    let client = client_for(app, &s).map_err(|e| e.to_string())?;
    let sessions = client.sessions().await.map_err(|e| e.to_string())?;
    // Drive the conversation the TUI shows (the latest one), or start one.
    let session = match sessions.first() {
        Some((id, _, _)) => id.clone(),
        None => client
            .create_session(&format!("Autopilot: {}", s.project()))
            .await
            .map_err(|e| e.to_string())?,
    };
    let _ = client.select_in_tui(&session).await;
    let now = Local::now();
    let goal: String = goal.trim().chars().take(4000).collect();
    let mut p = app
        .autopilot
        .get(&s.id)
        .filter(|p| p.session == session)
        .unwrap_or_default();
    let resumed = p.active() || !p.roc_id.is_empty();
    p.roc_id = s.id.clone();
    p.label = s.label.clone();
    p.project = s.project();
    p.workdir = s.workdir.clone();
    p.model = s.model.clone();
    p.container = s.container.clone();
    p.session = session;
    if !goal.is_empty() || !resumed {
        p.goal = goal;
    }
    p.state = "running".into();
    p.reason.clear();
    p.started_at = now.to_rfc3339();
    p.started_ms = now.timestamp_millis();
    p.started_by = by.into();
    p.errors_in_row = 0;
    p.repeats_in_row = 0;
    p.stuck_flagged = false;
    p.rev += 1;
    let who = if by == "pip" {
        "Pip (you asked in chat)"
    } else {
        "you"
    };
    p.log("note", format!("Autopilot started by {who}."));
    app.autopilot.pilots.lock().insert(s.id.clone(), p.clone());
    save(app);
    app.autopilot.wake.notify_one();
    Ok(p)
}

/// Stops autopilot: after this turn (`now` = false) or at once.
pub async fn stop(app: &Arc<App>, target: &str, now: bool, by: &str) -> Result<Pilot, String> {
    load(app);
    let id = resolve_pilot(app, target)?;
    if now {
        if let Ok(s) = find_session(app, &id) {
            if let (Ok(c), Some(p)) = (client_for(app, &s), app.autopilot.get(&id)) {
                let _ = c.abort(&p.session).await;
            }
        }
    }
    update_ext(app, &id, |p| {
        if now {
            p.state = "stopped".into();
            p.reason = format!("Stopped at once by {by}; the turn in progress was aborted.");
        } else {
            p.state = "stopping".into();
            p.reason = format!("Stopping: {by} asked; the turn in progress finishes first.");
        }
        p.need_human = None;
        let r = p.reason.clone();
        p.log("stop", r);
        p.clone()
    })
    .ok_or_else(|| "No autopilot run there.".into())
}

/// The roc id of a run, by id, prefix, label or project.
fn resolve_pilot(app: &App, target: &str) -> Result<String, String> {
    let t = target.trim().to_lowercase();
    let squash = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
    };
    let all = app.autopilot.all();
    let hits: Vec<&Pilot> = all
        .iter()
        .filter(|p| p.active())
        .filter(|p| {
            t.is_empty()
                || p.roc_id.starts_with(&t)
                || squash(&p.label) == squash(&t)
                || p.project.to_lowercase() == t
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0].roc_id.clone()),
        0 => {
            // A finished run can still be addressed by its exact id.
            all.iter()
                .find(|p| p.roc_id == target.trim())
                .map(|p| p.roc_id.clone())
                .ok_or_else(|| "Autopilot isn't running there.".into())
        }
        _ => Err(format!(
            "Autopilot runs on several sessions: {}. Say which.",
            hits.iter()
                .map(|p| format!("{} ({})", p.label, p.project))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Clears the "needs you" flag (you've looked).
pub fn acknowledge(app: &App, roc_id: &str) {
    update_ext(app, roc_id, |p| {
        if p.need_human.take().is_some() {
            p.log("note", "You looked at the agent's request for help.");
        }
        if p.state == "paused" {
            p.state = "running".into();
            p.reason.clear();
            p.repeats_in_row = 0;
            p.log("note", "Resumed by you.");
        }
    });
    app.autopilot.wake.notify_one();
}

/// Answers a question (from the session page).
pub async fn answer(
    app: &Arc<App>,
    roc_id: &str,
    question: &str,
    answers: Vec<Vec<String>>,
    by: &str,
) -> Result<(), String> {
    let s = find_session(app, roc_id)?;
    let c = client_for(app, &s).map_err(|e| e.to_string())?;
    c.answer(question, &answers)
        .await
        .map_err(|e| e.to_string())?;
    load(app);
    update_ext(app, roc_id, |p| {
        if let Some(q) = p.questions.iter_mut().find(|q| q.id == question) {
            q.answered_at = Some(Local::now().to_rfc3339());
            q.answers = Some(answers.clone());
            q.by = by.into();
        }
        p.log(
            "answer",
            format!(
                "{by} answered: {}",
                answers
                    .iter()
                    .map(|a| a.join(", "))
                    .collect::<Vec<_>>()
                    .join(" / ")
            ),
        );
    });
    Ok(())
}

/// Sends your own message to the agent (from the session page).
pub async fn send_message(app: &Arc<App>, roc_id: &str, text: &str) -> Result<(), String> {
    let s = find_session(app, roc_id)?;
    let c = client_for(app, &s).map_err(|e| e.to_string())?;
    let session = match app
        .autopilot
        .get(roc_id)
        .map(|p| p.session)
        .filter(|s| !s.is_empty())
    {
        Some(id) => id,
        None => c
            .sessions()
            .await
            .map_err(|e| e.to_string())?
            .first()
            .map(|x| x.0.clone())
            .ok_or("opencode has no session yet")?,
    };
    c.prompt(&session, text).await.map_err(|e| e.to_string())?;
    update_ext(app, roc_id, |p| {
        p.last_prompt_ms = Local::now().timestamp_millis();
        p.log("prompt", format!("(you) {text}"));
    });
    Ok(())
}

/// Ends the roc session itself (`roc -kill`), stopping autopilot first.
pub async fn kill(app: &Arc<App>, roc_id: &str) -> Result<String, String> {
    let _ = stop(app, roc_id, true, "you").await;
    let cfg = app.config.read().autopilot.clone();
    let roc_cmd = find_command(&cfg.roc_command, "roc");
    let state = roc::state_path(&cfg.roc_state);
    let out = tokio::process::Command::new(&roc_cmd)
        .args(["-kill", roc_id])
        .env("ROC_STATE", &state)
        .env("PATH", tool_path())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("couldn't run {roc_cmd}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .trim()
    .to_string();
    if out.status.success() {
        update_ext(app, roc_id, |p| {
            p.state = "stopped".into();
            p.reason = "You ended the roc session.".into();
            p.log("stop", format!("roc -kill: {text}"));
        });
        Ok(text)
    } else {
        Err(if text.is_empty() {
            format!("roc -kill failed ({})", out.status)
        } else {
            text
        })
    }
}

/// One look at one run.
async fn step(app: &Arc<App>, roc_id: &str, s: &RocSession) {
    let Some(mut p) = app.autopilot.get(roc_id) else {
        return;
    };
    let base = Base::of(&p);
    let client = match client_for(app, s) {
        Ok(c) => c,
        Err(e) => {
            update(app, roc_id, |p| {
                p.state = "stopped".into();
                p.reason = format!("Can't reach the session: {e}");
            });
            return;
        }
    };
    let cfg = app.config.read().autopilot.clone();
    let now = Local::now();
    let now_ms = now.timestamp_millis();

    // Questions first: nothing is sent while the agent waits for an answer.
    let questions = match client.questions().await {
        Ok(q) => q
            .into_iter()
            .filter(|q| q.session == p.session)
            .collect::<Vec<_>>(),
        Err(e) => {
            log::debug!("autopilot {roc_id}: {e:#}");
            return;
        }
    };
    let mut new_questions = Vec::new();
    for q in &questions {
        if !p.questions.iter().any(|x| x.id == q.id) {
            p.questions.push(QuestionLog {
                id: q.id.clone(),
                asked_at: now.to_rfc3339(),
                asked_ms: now_ms,
                questions: q.questions.clone(),
                ..Default::default()
            });
            let text = question_text(&q.questions);
            p.log("question", text.clone());
            new_questions.push(text);
        }
    }
    // Questions answered somewhere else (the TUI) are no longer pending.
    for q in p.questions.iter_mut().filter(|q| q.answered_at.is_none()) {
        if !questions.iter().any(|x| x.id == q.id) {
            q.answered_at = Some(now.to_rfc3339());
            q.by = "tui".into();
        }
    }
    // Your default answer, after the time you gave yourself.
    let mut auto_answered = Vec::new();
    if p.state == "running" && cfg.question_timeout_minutes > 0 {
        for q in p.questions.iter_mut().filter(|q| q.answered_at.is_none()) {
            if now_ms - q.asked_ms >= cfg.question_timeout_minutes as i64 * 60_000 {
                let answers = default_answers(&q.questions, &cfg.question_default_answer);
                if client.answer(&q.id, &answers).await.is_ok() {
                    q.answered_at = Some(now.to_rfc3339());
                    q.answers = Some(answers.clone());
                    q.by = "auto".into();
                    auto_answered.push(
                        answers
                            .iter()
                            .map(|a| a.join(", "))
                            .collect::<Vec<_>>()
                            .join(" / "),
                    );
                }
            }
        }
    }
    for a in &auto_answered {
        p.log(
            "answer",
            format!(
                "No answer from you in {} min, so autopilot answered: {a}",
                cfg.question_timeout_minutes
            ),
        );
    }
    for t in &new_questions {
        alert(
            app,
            "warn",
            format!("{} asks you something", p.label),
            t.clone(),
            roc_id,
            true,
        );
    }
    let waiting = p.questions.iter().any(|q| q.answered_at.is_none());

    // What is it doing?
    let status = match client.statuses().await {
        Ok(m) => m.get(&p.session).cloned().unwrap_or(Status::Idle),
        Err(e) => {
            log::debug!("autopilot {roc_id}: {e:#}");
            commit(app, roc_id, p, &base);
            return;
        }
    };
    let msgs = client.messages(&p.session, 30).await.unwrap_or_default();
    let activity = msgs
        .iter()
        .map(|m| m.completed.unwrap_or(m.created))
        .max()
        .unwrap_or(0);
    match &status {
        Status::Busy | Status::Retry(..) => {
            let silence = Duration::from_secs(cfg.silence_minutes.max(1) * 60);
            let stuck = {
                let mut b = app.autopilot.busy.lock();
                let e = b
                    .entry(roc_id.to_string())
                    .or_insert((Instant::now(), activity));
                if e.1 != activity {
                    *e = (Instant::now(), activity);
                }
                e.0.elapsed() >= silence
            };
            if stuck && !p.stuck_flagged && !waiting {
                p.stuck_flagged = true;
                let inflight = in_flight_for(app, &p.model);
                let msg = format!(
                    "opencode has been busy for {} min without a new message. {}",
                    cfg.silence_minutes,
                    if inflight > 0 {
                        format!("Toktail sees {inflight} request(s) for {} in flight, so the model may still be reading a long prompt.", p.model)
                    } else {
                        "Toktail sees no request for its model in flight.".into()
                    }
                );
                p.log("note", msg.clone());
                alert(
                    app,
                    "warn",
                    format!("{} may be stuck", p.label),
                    msg,
                    roc_id,
                    true,
                );
            }
            if let Status::Retry(n, why) = &status {
                if *n >= 3 && !p.stuck_flagged {
                    p.log(
                        "error",
                        format!("opencode is retrying the model (attempt {n}): {why}"),
                    );
                }
            }
            commit(app, roc_id, p, &base);
            return;
        }
        Status::Idle => {
            app.autopilot.busy.lock().remove(roc_id);
            p.stuck_flagged = false;
        }
    }
    if waiting {
        commit(app, roc_id, p, &base);
        return;
    }
    if p.state == "stopping" {
        p.state = "stopped".into();
        p.reason = "Stopped by you; the last turn finished.".into();
        p.log("stop", "The turn finished; autopilot stopped.");
        commit(app, roc_id, p, &base);
        return;
    }
    if p.state != "running" {
        commit(app, roc_id, p, &base);
        return;
    }
    let turn = last_turn(&msgs);
    let reply_text = turn
        .reply
        .as_ref()
        .map(|r| r.text.clone())
        .unwrap_or_default();
    let reply_id = turn
        .reply
        .as_ref()
        .map(|r| r.id.clone())
        .unwrap_or_default();
    let prior_need = p.need_human.clone();
    let decision = decide(&mut p, &cfg, &turn, now_ms);
    if decision != Decision::Wait && !reply_id.is_empty() && p.answered_message == reply_id {
        let excerpt: String = reply_text.chars().take(1200).collect();
        if !excerpt.trim().is_empty() {
            p.log("reply", excerpt);
        }
    }
    if p.need_human.is_some() && p.need_human != prior_need {
        let why = p.need_human.clone().unwrap_or_default();
        p.log("note", format!("The agent asks for you: {why}"));
        alert(
            app,
            "bad",
            format!("{} needs you", p.label),
            why,
            roc_id,
            true,
        );
    }
    if cfg.done_claims_alert > 0
        && p.done_in_row == cfg.done_claims_alert
        && matches!(decision, Decision::Send { done: true })
    {
        let msg = format!("The agent has said it's finished {} times in a row. Autopilot keeps going; look in when you can, in case it's confused or blocked.", p.done_in_row);
        p.log("note", msg.clone());
        alert(
            app,
            "warn",
            format!("{} may be stuck", p.label),
            msg,
            roc_id,
            false,
        );
    }
    let host = roc::toktail_host(&app.config.read(), &s.ai_host);
    match decision {
        Decision::Wait => commit(app, roc_id, p, &base),
        Decision::Pause(why) => {
            p.state = "paused".into();
            p.reason = why.clone();
            p.log("pause", why.clone());
            alert(
                app,
                "warn",
                format!("Autopilot paused on {}", p.label),
                why,
                roc_id,
                true,
            );
            commit(app, roc_id, p, &base);
        }
        Decision::Stop(why) => {
            p.state = "stopped".into();
            p.reason = why.clone();
            p.log("stop", why.clone());
            alert(
                app,
                "bad",
                format!("Autopilot stopped on {}", p.label),
                why,
                roc_id,
                true,
            );
            commit(app, roc_id, p, &base);
        }
        Decision::Send { done } => {
            if cfg.token_ceiling > 0 {
                let used = tokens_used(app, host.as_deref(), &p.model, p.started_ms);
                if used >= cfg.token_ceiling {
                    let why = format!(
                        "This run used {} tokens, past the ceiling of {} (Settings → Autopilot).",
                        crate::app::fmt_thousands(used),
                        crate::app::fmt_thousands(cfg.token_ceiling)
                    );
                    p.state = "stopped".into();
                    p.reason = why.clone();
                    p.log("stop", why.clone());
                    alert(
                        app,
                        "warn",
                        format!("Autopilot stopped on {}", p.label),
                        why,
                        roc_id,
                        true,
                    );
                    commit(app, roc_id, p, &base);
                    return;
                }
            }
            // Save the counters before the slow part (the verifier).
            commit(app, roc_id, p.clone(), &base);
            let verifier = run_verifier(&cfg, s).await.unwrap_or_default();
            let template = if done {
                &cfg.done_template
            } else {
                &cfg.continue_template
            };
            let text = render(template, &p.goal, &verifier, p.turns + 1, p.done_claims);
            // The run may have been stopped while the verifier ran.
            if app
                .autopilot
                .get(roc_id)
                .is_none_or(|x| x.state != "running")
            {
                return;
            }
            match client.prompt(&p.session, &text).await {
                Ok(()) => {
                    update(app, roc_id, |x| {
                        x.turns += 1;
                        x.last_prompt_at = Some(Local::now().to_rfc3339());
                        x.last_prompt_ms = Local::now().timestamp_millis();
                        if !verifier.is_empty() {
                            x.log("verifier", verifier.clone());
                        }
                        x.log("prompt", text.clone());
                    });
                }
                Err(e) => {
                    update(app, roc_id, |x| {
                        x.log("error", format!("Couldn't send the prompt: {e:#}"))
                    });
                }
            }
        }
    }
}

/// Saves a pilot changed during a step. Changes you (or Pip) made while the
/// step ran win: a stop, a resume, an answer, an acknowledgement.
fn commit(app: &App, roc_id: &str, mut p: Pilot, base: &Base) {
    {
        let mut m = app.autopilot.pilots.lock();
        let Some(cur) = m.get(roc_id) else { return };
        if cur.rev != base.rev {
            if cur.state != base.state {
                p.state = cur.state.clone();
                p.reason = cur.reason.clone();
            }
            if cur.need_human.is_none() && base.need_human.is_some() {
                p.need_human = None;
            }
            p.goal = cur.goal.clone();
            p.session = cur.session.clone();
            p.started_at = cur.started_at.clone();
            p.started_ms = cur.started_ms;
            p.started_by = cur.started_by.clone();
            for q in p.questions.iter_mut() {
                if let Some(c) = cur.questions.iter().find(|c| c.id == q.id) {
                    if c.answered_at.is_some() && q.answered_at.is_none() {
                        *q = c.clone();
                    }
                }
            }
            if cur.last_prompt_ms > p.last_prompt_ms {
                p.last_prompt_ms = cur.last_prompt_ms;
            }
            // Entries logged meanwhile, after the ones this step added.
            if cur.history.len() > base.len {
                p.history.extend(cur.history[base.len..].iter().cloned());
            }
            p.rev = cur.rev;
        }
        m.insert(roc_id.to_string(), p);
    }
    save(app);
}

/// Requests in flight for a model, from Toktail's own live feed.
fn in_flight_for(app: &App, model: &str) -> usize {
    let (flying, _, _) = app.activity.snapshot(u64::MAX, 0);
    flying.iter().filter(|f| f.model == model).count()
}

/// One readable line for a question request.
pub fn question_text(qs: &Value) -> String {
    qs.as_array()
        .map(|a| {
            a.iter()
                .map(|q| {
                    let opts = q["options"]
                        .as_array()
                        .map(|o| {
                            o.iter()
                                .filter_map(|x| x["label"].as_str())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        })
                        .unwrap_or_default();
                    let base = q["question"].as_str().unwrap_or("").to_string();
                    if opts.is_empty() {
                        base
                    } else {
                        format!("{base} ({opts})")
                    }
                })
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default()
}

/// Your default answer, for every question in a request: typed text where
/// the agent allows it, otherwise its first option.
pub fn default_answers(qs: &Value, default: &str) -> Vec<Vec<String>> {
    qs.as_array()
        .map(|a| {
            a.iter()
                .map(|q| {
                    let custom_ok = q["custom"].as_bool() != Some(false);
                    let first = q["options"][0]["label"].as_str();
                    match (custom_ok, first) {
                        (true, _) | (false, None) => vec![default.to_string()],
                        (false, Some(l)) => vec![l.to_string()],
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The loop: looks at every run every few seconds (or when woken).
pub async fn run(app: Arc<App>) {
    load(&app);
    loop {
        tick(&app).await;
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {},
            _ = app.autopilot.wake.notified() => {},
        }
    }
}

async fn tick(app: &Arc<App>) {
    let active: Vec<String> = app
        .autopilot
        .all()
        .into_iter()
        .filter(|p| p.active())
        .map(|p| p.roc_id)
        .collect();
    if active.is_empty() {
        return;
    }
    let setting = app.config.read().autopilot.roc_state.clone();
    let view = tokio::task::spawn_blocking(move || roc::read(&setting))
        .await
        .unwrap_or_default();
    if view.problem.is_some() {
        return;
    }
    for id in active {
        match view.sessions.iter().find(|s| s.id == id) {
            Some(s) => step(app, &id, s).await,
            None => {
                let label = app.autopilot.get(&id).map(|p| p.label).unwrap_or_default();
                update(app, &id, |p| {
                    p.state = "stopped".into();
                    p.reason = "The roc session ended.".into();
                    p.log("stop", "The roc session ended.");
                });
                alert(
                    app,
                    "info",
                    format!("{label}'s session ended"),
                    "Autopilot stopped with it.".into(),
                    &id,
                    false,
                );
            }
        }
    }
}

// ------------------------------------------------------------------ what the portal shows

/// Every roc session with what autopilot knows about it (no network calls).
pub fn overview(app: &App) -> Value {
    load(app);
    let cfg = app.config.read().clone();
    let view = roc::read(&cfg.autopilot.roc_state);
    let pilots = app.autopilot.all();
    let sessions: Vec<Value> = view
        .sessions
        .iter()
        .map(|s| {
            let p = pilots.iter().find(|p| p.roc_id == s.id);
            json!({
                "id": s.id, "label": s.label, "binary": s.binary, "model": s.model, "project": s.project(),
                "workdir": s.workdir, "started_at": s.started_at, "status": s.status,
                "controllable": s.controllable(),
                "toktail_host": roc::toktail_host(&cfg, &s.ai_host).map(|h| crate::pip_agent::host_name(app, &h)),
                "autopilot": p.map(|p| p.summary()),
            })
        })
        .collect();
    // Runs whose session is gone, for their history.
    let ended: Vec<Value> = pilots
        .iter()
        .filter(|p| !view.sessions.iter().any(|s| s.id == p.roc_id))
        .map(|p| json!({ "id": p.roc_id, "label": p.label, "project": p.project, "autopilot": p.summary() }))
        .collect();
    json!({
        "state_file": view.state_file, "problem": view.problem,
        "sessions": sessions, "ended": ended,
        "attention": pilots.iter().filter(|p| p.active() && (p.need_human.is_some() || p.pending_questions() > 0 || p.state == "paused")).count(),
        "running": pilots.iter().filter(|p| p.state == "running" || p.state == "stopping").count(),
    })
}

/// One session, live: its autopilot run, opencode's status, recent messages,
/// questions and todos.
pub async fn detail(app: &Arc<App>, roc_id: &str) -> Value {
    load(app);
    let cfg = app.config.read().clone();
    let view = roc::read(&cfg.autopilot.roc_state);
    let s = view.sessions.iter().find(|s| s.id == roc_id).cloned();
    let pilot = app.autopilot.get(roc_id);
    let mut out = json!({
        "id": roc_id,
        "session": s.as_ref().map(|s| json!({
            "label": s.label, "binary": s.binary, "model": s.model, "project": s.project(), "workdir": s.workdir,
            "container": s.container, "started_at": s.started_at, "status": s.status, "controllable": s.controllable(),
            "control_url": s.control_url,
            "toktail_host": roc::toktail_host(&cfg, &s.ai_host).map(|h| crate::pip_agent::host_name(app, &h)),
        })),
        "autopilot": pilot,
        "live": Value::Null,
    });
    let Some(s) = s.filter(|s| s.controllable()) else {
        return out;
    };
    let c = match client_for(app, &s) {
        Ok(c) => c,
        Err(e) => {
            out["live"] = json!({ "error": e.to_string() });
            return out;
        }
    };
    let sessions = c.sessions().await;
    let current = pilot
        .as_ref()
        .map(|p| p.session.clone())
        .filter(|x| !x.is_empty())
        .or_else(|| {
            sessions
                .as_ref()
                .ok()
                .and_then(|v| v.first().map(|x| x.0.clone()))
        });
    let (status, questions, msgs, todos) = match &current {
        Some(sid) => {
            let (st, q, m, t) = tokio::join!(
                c.statuses(),
                c.questions(),
                c.messages(sid, 12),
                c.todos(sid)
            );
            (
                st.map(|m| m.get(sid).cloned().unwrap_or(Status::Idle)),
                q.map(|v| {
                    v.into_iter()
                        .filter(|q| &q.session == sid)
                        .collect::<Vec<_>>()
                }),
                m,
                t,
            )
        }
        None => (Ok(Status::Idle), Ok(vec![]), Ok(vec![]), Ok(Value::Null)),
    };
    if let Err(e) = &status {
        out["live"] = json!({ "error": format!("{e:#}") });
        return out;
    }
    let status_text = match status.as_ref().ok() {
        Some(Status::Busy) => "working".to_string(),
        Some(Status::Retry(n, why)) => format!("retrying the model (attempt {n}): {why}"),
        _ => "idle".to_string(),
    };
    let messages: Vec<Value> = msgs
        .unwrap_or_default()
        .iter()
        .map(|m| {
            json!({
                "id": m.id, "role": m.role, "text": crate::tokens::scrub(&m.text.chars().take(6000).collect::<String>()),
                "tools": m.tools, "finish": m.finish, "error": m.error, "created": m.created, "completed": m.completed,
                "claims_done": m.role == "assistant" && claims_done(&m.text),
            })
        })
        .collect();
    out["live"] = json!({
        "opencode_session": current,
        "opencode_sessions": sessions.unwrap_or_default().iter().take(10).map(|(id, title, upd)| json!({"id": id, "title": title, "updated": upd})).collect::<Vec<_>>(),
        "status": status_text,
        "questions": questions.unwrap_or_default(),
        "messages": messages,
        "todos": todos.unwrap_or(Value::Null),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutopilotConfig;

    fn msg(id: &str, role: &str, text: &str, done_ms: i64) -> opencode::Message {
        opencode::Message {
            id: id.into(),
            role: role.into(),
            text: text.into(),
            created: done_ms - 1000,
            completed: Some(done_ms),
            output_tokens: 50,
            finish: Some("stop".into()),
            ..Default::default()
        }
    }

    fn pilot() -> Pilot {
        Pilot {
            roc_id: "abc".into(),
            state: "running".into(),
            turns: 1,
            started_ms: 0,
            ..Default::default()
        }
    }

    #[test]
    fn done_claims_and_need_human() {
        assert!(claims_done("All tasks are complete. The work is done."));
        assert!(claims_done("I’ve completed the refactor."));
        assert!(claims_done("STATUS: COMPLETE"));
        assert!(!claims_done("I fixed main.go; next I'll do the handlers."));
        assert_eq!(
            need_human("ok\nNEED_HUMAN: which database?\nmore").as_deref(),
            Some("which database?")
        );
        assert_eq!(need_human("all good"), None);
    }

    #[test]
    fn templates_fill_in() {
        let t = render(
            "Go.{{goal}}{{verifier}} #{{turn}}",
            "ship it",
            "87 files fail",
            3,
            1,
        );
        assert_eq!(
            t,
            "Go.\n\nThe human's goal for this session: ship it\n\n87 files fail #3"
        );
        assert_eq!(render("Go.{{goal}}{{verifier}}", "", "", 1, 0), "Go.");
    }

    #[test]
    fn waits_for_the_grace_then_sends_once() {
        let cfg = AutopilotConfig::default();
        let mut p = pilot();
        let msgs = vec![
            msg("u1", "user", "go", 1_000),
            msg("a1", "assistant", "Edited three files, more to do.", 10_000),
        ];
        let t = last_turn(&msgs);
        assert_eq!(
            decide(&mut p, &cfg, &t, 30_000),
            Decision::Wait,
            "inside the grace period"
        );
        assert_eq!(
            decide(&mut p, &cfg, &t, 80_000),
            Decision::Send { done: false }
        );
        assert_eq!(
            decide(&mut p, &cfg, &t, 90_000),
            Decision::Wait,
            "that reply is answered"
        );
    }

    #[test]
    fn done_claims_get_the_done_template() {
        let cfg = AutopilotConfig::default();
        let mut p = pilot();
        let t = last_turn(&[
            msg("u", "user", "go", 0),
            msg("a", "assistant", "The task is complete.", 1_000),
        ]);
        assert_eq!(
            decide(&mut p, &cfg, &t, 100_000),
            Decision::Send { done: true }
        );
        assert_eq!((p.done_claims, p.done_in_row), (1, 1));
    }

    #[test]
    fn repeats_pause_and_errors_stop_with_a_diagnosis() {
        let cfg = AutopilotConfig::default();
        let mut p = pilot();
        for i in 0..2 {
            let t = last_turn(&[
                msg("u", "user", "go", 0),
                msg(&format!("a{i}"), "assistant", "Same words.", 1_000),
            ]);
            assert_eq!(
                decide(&mut p, &cfg, &t, 100_000),
                Decision::Send { done: false }
            );
        }
        let t = last_turn(&[
            msg("u", "user", "go", 0),
            msg("a9", "assistant", "same   WORDS.", 1_000),
        ]);
        assert!(matches!(
            decide(&mut p, &cfg, &t, 100_000),
            Decision::Pause(_)
        ));

        let mut p = pilot();
        for i in 0..3 {
            let mut m = msg(&format!("e{i}"), "assistant", "", 1_000);
            m.error = Some("This model's maximum context length is 32768 tokens".into());
            let d = decide(
                &mut p,
                &cfg,
                &last_turn(&[msg("u", "user", "go", 0), m]),
                100_000,
            );
            if i < 2 {
                assert_eq!(d, Decision::Send { done: false });
            } else {
                let Decision::Stop(why) = d else {
                    panic!("{d:?}")
                };
                assert!(why.contains("context window"), "{why}");
            }
        }
    }

    #[test]
    fn work_between_identical_replies_is_progress() {
        let cfg = AutopilotConfig::default();
        let mut p = pilot();
        for i in 0..4 {
            let mut tool = msg(&format!("t{i}"), "assistant", "", 900);
            tool.tools = vec!["edit".into()];
            let t = last_turn(&[
                msg("u", "user", "go", 0),
                tool,
                msg(&format!("a{i}"), "assistant", "Fixed one more file.", 1_000),
            ]);
            assert_eq!(
                decide(&mut p, &cfg, &t, 100_000),
                Decision::Send { done: false }
            );
        }
    }

    #[test]
    fn unanswered_prompts_wait_for_the_silence() {
        let cfg = AutopilotConfig::default();
        let mut p = pilot();
        p.last_prompt_ms = 0;
        let t = last_turn(&[
            msg("a", "assistant", "x", 0),
            msg("u", "user", "Continue.", 1_000),
        ]);
        assert_eq!(decide(&mut p, &cfg, &t, 60_000), Decision::Wait);
        assert_eq!(
            decide(&mut p, &cfg, &t, 11 * 60_000),
            Decision::Send { done: false }
        );
        assert_eq!(p.errors_in_row, 1);
    }

    #[test]
    fn turn_limit_stops() {
        let cfg = AutopilotConfig {
            max_turns: 2,
            ..Default::default()
        };
        let mut p = pilot();
        p.turns = 2;
        let t = last_turn(&[
            msg("u", "user", "go", 0),
            msg("a", "assistant", "More work done.", 1_000),
        ]);
        assert!(matches!(
            decide(&mut p, &cfg, &t, 100_000),
            Decision::Stop(_)
        ));
    }

    /// A pretend opencode server: one session whose agent says it's done,
    /// then a question. Records what autopilot sends.
    async fn fake_opencode(sent: Arc<Mutex<Vec<String>>>, question: Arc<Mutex<bool>>) -> String {
        use axum::routing::{get, post};
        let s2 = sent.clone();
        let q2 = question.clone();
        let q3 = question.clone();
        let app = axum::Router::new()
            .route("/session", get(|| async { axum::Json(json!([{"id": "ses_1", "title": "t", "time": {"updated": 5}}])) }))
            .route("/session/status", get(|| async { axum::Json(json!({})) }))
            .route("/question", get(move || {
                let q = q2.clone();
                async move {
                    if *q.lock() {
                        axum::Json(json!([{"id": "que_1", "sessionID": "ses_1", "questions": [{"question": "Which DB?", "header": "DB", "options": [{"label": "Postgres"}]}]}]))
                    } else {
                        axum::Json(json!([]))
                    }
                }
            }))
            .route("/question/{id}/reply", post(move || {
                let q = q3.clone();
                async move {
                    *q.lock() = false;
                    axum::Json(json!(true))
                }
            }))
            .route("/session/{id}/message", get(|| async {
                axum::Json(json!([
                    {"info": {"id": "msg_u", "role": "user", "time": {"created": 1000}}, "parts": [{"type": "text", "text": "go"}]},
                    {"info": {"id": "msg_a", "role": "assistant", "finish": "stop", "time": {"created": 2000, "completed": 3000}, "tokens": {"output": 40}},
                     "parts": [{"type": "text", "text": "All tasks are complete."}]}
                ]))
            }))
            .route("/session/{id}/prompt_async", post(move |body: String| {
                let s = s2.clone();
                async move {
                    let v: Value = serde_json::from_str(&body).unwrap();
                    s.lock().push(v["parts"][0]["text"].as_str().unwrap().to_string());
                    axum::http::StatusCode::NO_CONTENT
                }
            }))
            .route("/tui/select-session", post(|| async { axum::Json(json!(true)) }))
            .route("/session/{id}/abort", post(|| async { axum::Json(json!(true)) }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn fake_roc(url: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("toktail-ap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("control")).unwrap();
        std::fs::write(d.join("control/abc123def456"), "pw").unwrap();
        let st = json!({"version": 1, "config": {"ai": {"host": "http://127.0.0.1:1/v1", "models": {}}},
            "sessions": {"abc123def456": {"id": "abc123def456", "binary": "opencode", "worker": 2, "model": "m",
                "workdir": "/w/planning", "container": "", "started_at": "x", "status": "running",
                "control": {"kind": "opencode", "url": url, "username": "opencode"}}}});
        std::fs::write(d.join("state.json"), st.to_string()).unwrap();
        d.join("state.json")
    }

    #[tokio::test]
    async fn drives_a_session_answers_waits_for_questions_and_stops() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let question = Arc::new(Mutex::new(false));
        let url = fake_opencode(sent.clone(), question.clone()).await;
        let app = crate::test_support::app();
        {
            let mut c = app.config.write();
            c.autopilot.roc_state = fake_roc(&url).display().to_string();
            c.autopilot.grace_secs = 5;
            c.autopilot.question_timeout_minutes = 0;
        }
        let p = start(&app, "planning", "finish the docs", "you")
            .await
            .unwrap();
        assert_eq!((p.session.as_str(), p.state.as_str()), ("ses_1", "running"));
        tick(&app).await;
        {
            let s = sent.lock();
            assert_eq!(s.len(), 1, "one prompt for the one reply");
            assert!(
                s[0].starts_with("I hear that you believe this is finished"),
                "done template: {}",
                s[0]
            );
            assert!(s[0].contains("finish the docs"), "goal is in it");
        }
        tick(&app).await;
        assert_eq!(sent.lock().len(), 1, "the same reply isn't answered twice");
        // A question: logged, and nothing else is sent while it waits.
        *question.lock() = true;
        update(&app, "abc123def456", |p| p.answered_message.clear());
        tick(&app).await;
        let p = app.autopilot.get("abc123def456").unwrap();
        assert_eq!(p.questions.len(), 1);
        assert!(p.questions[0].answered_at.is_none());
        assert_eq!(sent.lock().len(), 1, "waits for the answer");
        answer(
            &app,
            "abc123def456",
            "que_1",
            vec![vec!["Postgres".into()]],
            "you",
        )
        .await
        .unwrap();
        assert_eq!(
            app.autopilot.get("abc123def456").unwrap().questions[0].by,
            "you"
        );
        tick(&app).await;
        assert_eq!(sent.lock().len(), 2, "answered: it goes on");
        // Stop: the turn finishes, then nothing more.
        stop(&app, "planning", false, "you").await.unwrap();
        update(&app, "abc123def456", |p| p.answered_message.clear());
        tick(&app).await;
        let p = app.autopilot.get("abc123def456").unwrap();
        assert_eq!(p.state, "stopped");
        assert_eq!(sent.lock().len(), 2);
        // Saved for a restart.
        let saved: BTreeMap<String, Pilot> =
            serde_json::from_slice(&std::fs::read(file(&app)).unwrap()).unwrap();
        assert_eq!(saved["abc123def456"].turns, 2);
    }

    #[tokio::test]
    async fn your_changes_during_a_step_win() {
        let app = crate::test_support::app();
        let mut p = pilot();
        p.questions.push(QuestionLog {
            id: "que_1".into(),
            ..Default::default()
        });
        app.autopilot.pilots.lock().insert("abc".into(), p.clone());
        // A step starts from this copy...
        let base = Base::of(&p);
        let mut stale = p.clone();
        stale.log("reply", "from the step");
        // ...while you answer the question and stop the run.
        update_ext(&app, "abc", |x| {
            x.questions[0].answered_at = Some("now".into());
            x.questions[0].by = "you".into();
            x.log("answer", "you answered");
        });
        update_ext(&app, "abc", |x| {
            x.state = "stopping".into();
            x.log("stop", "you stopped it");
        });
        commit(&app, "abc", stale, &base);
        let got = app.autopilot.get("abc").unwrap();
        assert_eq!(got.state, "stopping");
        assert_eq!(got.questions[0].by, "you");
        let kinds: Vec<&str> = got.history.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, vec!["reply", "answer", "stop"]);
    }

    #[test]
    fn default_answers_respect_custom() {
        let qs = json!([
            {"question": "DB?", "options": [{"label": "Postgres"}, {"label": "SQLite"}]},
            {"question": "Pick", "custom": false, "options": [{"label": "A"}, {"label": "B"}]}
        ]);
        assert_eq!(
            default_answers(&qs, "Your call."),
            vec![vec!["Your call.".to_string()], vec!["A".to_string()]]
        );
        assert_eq!(question_text(&qs), "DB? (Postgres / SQLite) · Pick (A / B)");
    }
}
