// SPDX-License-Identifier: Apache-2.0
//! Pip's commentary: while a coding agent works, Pip says one line about
//! what just happened, in its own voice, using the same local model that
//! just answered the agent.
//!
//! The rules keep it light on the model:
//! - only for requests from a coding agent Toktail recognises (opencode,
//!   Claude Code, Codex, goose, or whatever a `X-Toktail-Agent` header says);
//! - only when the host is on this machine or the local network (never a
//!   public API: your work never goes to a model you didn't choose for it);
//! - the same model the agent used, through the same Toktail port, so the
//!   request is counted (tagged `client = commentary`) but never saved as a
//!   record and never commented on itself;
//! - at most one line every `commentary.every_minutes` (5 by default), and
//!   once per prompt;
//! - the input is only the agent's newest turn and the answer to it, trimmed
//!   to about 1,500 tokens and scrubbed of anything key-shaped;
//! - the output is one sentence.
//!
//! Pressing `u` or Space on the dashboard asks for a line right away about
//! the most recent agent request (a 15-second cooldown applies).

use crate::app::App;
use crate::tokens;
use axum::http::HeaderMap;
use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A request Pip could comment on.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub request_id: String,
    pub host: String,
    pub listen: String,
    pub model: String,
    pub agent: String,
    pub project: String,
    /// The agent's newest user turn, scrubbed and trimmed.
    pub turn: String,
    /// The model's answer to it, scrubbed and trimmed.
    pub response: String,
    pub at: Instant,
}

/// One thing Pip said.
#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub seq: u64,
    pub at: DateTime<Local>,
    pub text: String,
    pub host: String,
    pub model: String,
    pub agent: String,
    pub project: String,
    pub request_id: String,
    /// True for the `u` / Space easter egg.
    pub asked: bool,
}

#[derive(Default)]
struct Inner {
    last_auto: Option<Instant>,
    last_asked: Option<Instant>,
    busy: bool,
    seq: u64,
    lines: VecDeque<Line>,
    latest: Option<Candidate>,
    /// Agents Pip has greeted this run (host, agent).
    greeted: Vec<(String, String)>,
}

#[derive(Default)]
pub struct State {
    inner: Mutex<Inner>,
}

pub const ASKED_COOLDOWN: Duration = Duration::from_secs(15);
const MAX_TURN_CHARS: usize = 4000;
const MAX_RESPONSE_CHARS: usize = 2500;

impl State {
    pub fn new() -> State {
        State::default()
    }
    pub fn recent(&self, n: usize) -> Vec<Line> {
        self.inner
            .lock()
            .lines
            .iter()
            .rev()
            .take(n)
            .cloned()
            .collect()
    }
    pub fn latest(&self) -> Option<Line> {
        self.inner.lock().lines.back().cloned()
    }
    pub fn seq(&self) -> u64 {
        self.inner.lock().seq
    }
    fn push(&self, mut line: Line) -> Line {
        let mut i = self.inner.lock();
        i.seq += 1;
        line.seq = i.seq;
        i.lines.push_back(line.clone());
        while i.lines.len() > 40 {
            i.lines.pop_front();
        }
        line
    }
}

// ------------------------------------------------------------ detection

/// Which coding agent sent a request, and what it's working on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detected {
    pub agent: String,
    pub project: String,
    /// A session id the client stated (`X-Toktail-Session`).
    pub session: String,
}

/// Reads the agent from headers first (`X-Toktail-Agent`, the user agent),
/// then from the system prompt, which every agent signs in its own way.
pub fn detect(headers: &HeaderMap, body: &Value) -> Detected {
    let h = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut d = Detected {
        agent: normalize_agent(&h("x-toktail-agent")),
        project: h("x-toktail-project").chars().take(80).collect(),
        session: h("x-toktail-session").chars().take(100).collect(),
    };
    let ua = h("user-agent").to_ascii_lowercase();
    if d.agent.is_empty() {
        d.agent = agent_from_text(&ua);
    }
    let system = system_text(body);
    if d.agent.is_empty() {
        d.agent = agent_from_text(&system.to_ascii_lowercase());
    }
    if d.project.is_empty() {
        d.project = project_from_text(&system);
    }
    d
}

fn normalize_agent(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "" => String::new(),
        "claude" | "claude-code" | "claudecode" | "claude_code" => "claudecode".into(),
        "open-code" | "opencode" => "opencode".into(),
        other => other.chars().take(24).collect(),
    }
}

fn agent_from_text(t: &str) -> String {
    if t.contains("opencode") {
        "opencode".into()
    } else if t.contains("claude-cli") || t.contains("claude code") || t.contains("claude-code") {
        "claudecode".into()
    } else if t.contains("codex") {
        "codex".into()
    } else if t.contains("goose") {
        "goose".into()
    } else {
        String::new()
    }
}

/// The system prompt text of an OpenAI, Anthropic or Responses request.
pub fn system_text(v: &Value) -> String {
    let mut out = String::new();
    let push = |out: &mut String, c: &Value| match c {
        Value::String(s) => out.push_str(s),
        Value::Array(parts) => {
            for p in parts {
                if let Some(t) = p.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                    out.push('\n');
                }
            }
        }
        _ => {}
    };
    if let Some(s) = v.get("system").or_else(|| v.get("instructions")) {
        push(&mut out, s);
    }
    for m in v["messages"].as_array().into_iter().flatten() {
        if m["role"] == "system" || m["role"] == "developer" {
            push(&mut out, &m["content"]);
        }
    }
    for m in v["input"].as_array().into_iter().flatten() {
        if m["role"] == "system" || m["role"] == "developer" {
            push(&mut out, &m["content"]);
        }
    }
    out.chars().take(20_000).collect()
}

/// The newest user turn, as text.
pub fn last_user_turn(v: &Value) -> String {
    let text_of = |c: &Value| -> String {
        match c {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    };
    for key in ["messages", "input"] {
        if let Some(arr) = v[key].as_array() {
            if let Some(m) = arr.iter().rev().find(|m| m["role"] == "user") {
                return text_of(&m["content"]);
            }
        }
    }
    v["prompt"].as_str().map(String::from).unwrap_or_default()
}

/// A project name from the working directory an agent states in its system
/// prompt ("Working directory: /Users/andrei/friends_of/roc", `<cwd>…</cwd>`,
/// "cwd": "…").
pub fn project_from_text(t: &str) -> String {
    let lower = t.to_ascii_lowercase();
    let markers = [
        "working directory:",
        "working directory is",
        "current working directory:",
        "cwd:",
        "\"cwd\":",
        "<cwd>",
        "workdir:",
    ];
    for m in markers {
        if let Some(i) = lower.find(m) {
            let rest = &t[i + m.len()..];
            let rest = rest.trim_start_matches([' ', '"', '`', '*']);
            let path: String = rest
                .chars()
                .take_while(|c| {
                    !c.is_whitespace()
                        && *c != '"'
                        && *c != '<'
                        && *c != '`'
                        && *c != ','
                        && *c != '*'
                })
                .collect();
            let name = path
                .trim_end_matches('/')
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_string();
            if name.len() > 1 && name.len() <= 60 && !name.starts_with('.') {
                return name;
            }
        }
    }
    String::new()
}

// ------------------------------------------------------------ the lines

/// A commentary request is counted, but it is never commented on, never
/// saved as a record, and never cached.
pub const CLIENT: &str = "commentary";

fn trim(s: &str, n: usize) -> String {
    let s = tokens::scrub(s);
    if s.chars().count() <= n {
        return s;
    }
    let head: String = s.chars().take(n / 2).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(n / 2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\n…\n{tail}")
}

/// Builds a candidate, when the request came from a recognised agent.
pub fn candidate(
    request_id: &str,
    host: &crate::config::HostConfig,
    model: &str,
    detected: &Detected,
    turn: &str,
    response: &str,
) -> Option<Candidate> {
    if detected.agent.is_empty() || host.is_public() || turn.trim().is_empty() {
        return None;
    }
    Some(Candidate {
        request_id: request_id.to_string(),
        host: host.id.clone(),
        listen: host.listen.clone(),
        model: model.to_string(),
        agent: detected.agent.clone(),
        project: detected.project.clone(),
        turn: trim(turn, MAX_TURN_CHARS),
        response: trim(response, MAX_RESPONSE_CHARS),
        at: Instant::now(),
    })
}

fn lan_ok(app: &App, host_id: &str) -> bool {
    let cfg = app.config.read();
    cfg.host(host_id).is_some_and(|h| {
        !h.is_public()
            && tokens::is_private_url(
                &crate::proxy::normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone()),
            )
    })
}

/// Pip's first words for an agent it just noticed.
fn greeting(agent: &str, project: &str) -> String {
    let what = match agent {
        "opencode" => "opencode",
        "claudecode" => "Claude Code",
        "codex" => "Codex",
        "goose" => "goose",
        other => other,
    };
    if project.is_empty() {
        format!("You're working on a very important project with {what}, I see!")
    } else {
        format!(
            "You're working on a very important project, I see! {what} and {project}, off we go."
        )
    }
}

/// Offers a request to Pip. Pip takes it when the host is local, nothing is
/// in flight, and the last line was long enough ago (or `asked` is set).
pub fn offer(app: &Arc<App>, c: Candidate, asked: bool) {
    let cfg = app.config.read().commentary.clone();
    if !cfg.enabled && !asked {
        return;
    }
    if !lan_ok(app, &c.host) {
        return;
    }
    let first = {
        let mut i = app.commentary.inner.lock();
        i.latest = Some(c.clone());
        let key = (c.host.clone(), c.agent.clone());
        let first = !i.greeted.contains(&key);
        if first {
            i.greeted.push(key);
        }
        let every = Duration::from_secs(cfg.every_minutes.max(1) as u64 * 60);
        let due = if asked {
            i.last_asked.is_none_or(|t| t.elapsed() >= ASKED_COOLDOWN)
        } else {
            first || i.last_auto.is_none_or(|t| t.elapsed() >= every)
        };
        if !due || i.busy {
            return;
        }
        i.busy = true;
        if asked {
            i.last_asked = Some(Instant::now());
        } else {
            i.last_auto = Some(Instant::now());
        }
        first
    };
    if first && !asked {
        // The first line for a new agent is Pip's own; the model's lines follow.
        let line = app.commentary.push(Line {
            seq: 0,
            at: Local::now(),
            text: greeting(&c.agent, &c.project),
            host: c.host.clone(),
            model: c.model.clone(),
            agent: c.agent.clone(),
            project: c.project.clone(),
            request_id: c.request_id.clone(),
            asked: false,
        });
        log::info!("pip: {}", line.text);
        app.commentary.inner.lock().busy = false;
        return;
    }
    let a = app.clone();
    app.rt.spawn(async move {
        let r = tokio::time::timeout(Duration::from_secs(90), ask_model(&a, &c)).await;
        match r {
            Ok(Ok(text)) => {
                let line = a.commentary.push(Line {
                    seq: 0,
                    at: Local::now(),
                    text,
                    host: c.host.clone(),
                    model: c.model.clone(),
                    agent: c.agent.clone(),
                    project: c.project.clone(),
                    request_id: c.request_id.clone(),
                    asked,
                });
                log::info!("pip: {}", line.text);
            }
            Ok(Err(e)) => log::debug!("pip had nothing to say: {e:#}"),
            Err(_) => log::debug!("pip had nothing to say: the model took too long"),
        }
        a.commentary.inner.lock().busy = false;
    });
}

/// The easter egg: a line about the latest agent request, now.
pub fn ask_now(app: &Arc<App>) -> Result<(), String> {
    let c = app
        .commentary
        .inner
        .lock()
        .latest
        .clone()
        .filter(|c| c.at.elapsed() < Duration::from_secs(3600))
        .ok_or("No agent request in the last hour to talk about.")?;
    {
        let i = app.commentary.inner.lock();
        if i.busy {
            return Err("Pip is still thinking about the last one.".into());
        }
        if i.last_asked.is_some_and(|t| t.elapsed() < ASKED_COOLDOWN) {
            return Err("Give Pip a few seconds.".into());
        }
    }
    offer(app, c, true);
    Ok(())
}

const PERSONA: &str = "You are Pip, the Token Fox: the small, cheerful mascot of Toktail, which counts the tokens a developer's local AI models process. A coding agent is working on the developer's behalf and you just watched its latest exchange with the model. Say ONE short, whimsical, encouraging sentence about what the agent is doing, as if you were watching over the developer's shoulder. Under 140 characters. Mention the project by name when you know it. Plain words only: no code, no quotes, no lists, no emoji, no preamble, no second sentence.";

async fn ask_model(app: &Arc<App>, c: &Candidate) -> anyhow::Result<String> {
    let agent_name = match c.agent.as_str() {
        "opencode" => "opencode".to_string(),
        "claudecode" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        "goose" => "goose".to_string(),
        other => other.to_string(),
    };
    let user = format!(
        "Agent: {agent_name}.{}\n\nThe agent's newest request (trimmed):\n{}\n\nThe model's answer (trimmed):\n{}\n\nPip, one sentence:",
        if c.project.is_empty() {
            String::new()
        } else {
            format!(" Project: {}.", c.project)
        },
        c.turn,
        c.response
    );
    let body = json!({
        "model": c.model,
        "stream": false,
        "temperature": 0.8,
        "max_tokens": 80,
        "messages": [
            {"role": "system", "content": PERSONA},
            {"role": "user", "content": user},
        ],
    });
    let url = format!("{}/v1/chat/completions", crate::proxy::local_url(&c.listen));
    let resp = app
        .chat_client
        .post(&url)
        .header("content-type", "application/json")
        .header("X-Toktail-Client", CLIENT)
        .header("X-Toktail-Cache", "bypass")
        .body(body.to_string())
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("the model answered HTTP {}", resp.status());
    }
    let v: Value = serde_json::from_slice(&resp.bytes().await?)?;
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("no text in the answer"))?;
    Ok(tidy(text))
}

/// One clean sentence: the first non-empty line, without quotes or a
/// "Pip:" prefix, cut at 160 characters, ending in punctuation.
pub fn tidy(text: &str) -> String {
    let mut line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("<think"))
        .unwrap_or("")
        .trim_matches(|c| c == '"' || c == '“' || c == '”' || c == '*' || c == '`')
        .to_string();
    for p in ["Pip:", "pip:", "Pip says:", "Sentence:"] {
        if let Some(rest) = line.strip_prefix(p) {
            line = rest.trim().to_string();
        }
    }
    let line = tokens::scrub(&line);
    let mut out: String = line.chars().take(160).collect();
    if out.chars().count() == 160 {
        if let Some(i) = out.rfind(' ') {
            out.truncate(i);
        }
        out.push('…');
    }
    if !out.ends_with(['.', '!', '?', '…']) && !out.is_empty() {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_and_projects() {
        let mut h = HeaderMap::new();
        h.insert(
            "user-agent",
            "claude-cli/2.1.0 (external, cli)".parse().unwrap(),
        );
        let d = detect(&h, &json!({"messages":[]}));
        assert_eq!(d.agent, "claudecode");
        let body = json!({"messages":[{"role":"system","content":"You are opencode, an AI coding agent.\nWorking directory: /Users/andrei/friends_of/roc\n"},{"role":"user","content":"fix the tests"}]});
        let d = detect(&HeaderMap::new(), &body);
        assert_eq!((d.agent.as_str(), d.project.as_str()), ("opencode", "roc"));
        assert_eq!(last_user_turn(&body), "fix the tests");
        let mut h = HeaderMap::new();
        h.insert("x-toktail-agent", "goose".parse().unwrap());
        h.insert("x-toktail-project", "toktail".parse().unwrap());
        h.insert("x-toktail-session", "abc".parse().unwrap());
        let d = detect(&h, &json!({"messages":[]}));
        assert_eq!(
            (d.agent.as_str(), d.project.as_str(), d.session.as_str()),
            ("goose", "toktail", "abc")
        );
        assert_eq!(
            project_from_text("<env>\n  <cwd>/home/me/apps/www.toktail.app</cwd>\n</env>"),
            "www.toktail.app"
        );
        assert_eq!(project_from_text("nothing here"), "");
    }

    #[test]
    fn tidying() {
        assert_eq!(
            tidy("\"That roc application looks really cool!\"\nMore."),
            "That roc application looks really cool!"
        );
        assert_eq!(tidy("Pip: nice work on toktail"), "nice work on toktail.");
        assert!(tidy(&"a ".repeat(200)).chars().count() <= 161);
        assert!(!tidy("the key is sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ").contains("ABCDEFG"));
    }

    #[tokio::test]
    async fn rate_limits_and_lan_only() {
        let app = crate::test_support::app();
        {
            let mut c = app.config.write();
            c.hosts = vec![
                crate::config::HostConfig {
                    id: "laptop".into(),
                    name: "Laptop".into(),
                    ..Default::default()
                },
                crate::config::HostConfig {
                    id: "cloud".into(),
                    name: "Cloud".into(),
                    kind: "anthropic".into(),
                    upstream: "https://api.anthropic.com".into(),
                    listen: "127.0.0.1:1".into(),
                    ..Default::default()
                },
            ];
        }
        let d = Detected {
            agent: "opencode".into(),
            project: "roc".into(),
            ..Default::default()
        };
        let cfg = app.config.read().clone();
        assert!(
            candidate("r", cfg.host("cloud").unwrap(), "m", &d, "do x", "done").is_none(),
            "public hosts never get commentary"
        );
        let c = candidate("r", cfg.host("laptop").unwrap(), "m", &d, "do x", "done").unwrap();
        offer(&app, c.clone(), false);
        let first = app.commentary.latest().expect("a greeting");
        assert!(
            first.text.contains("very important project") && first.text.contains("roc"),
            "{}",
            first.text
        );
        // The next automatic line is rate limited (and would need a model anyway).
        offer(&app, c.clone(), false);
        assert_eq!(app.commentary.seq(), 1);
        // The easter egg has a cooldown of its own.
        assert!(ask_now(&app).is_ok());
        assert!(ask_now(&app).is_err());
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
