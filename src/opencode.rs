// SPDX-License-Identifier: Apache-2.0
//! A small client for opencode's own HTTP server (the one its TUI runs,
//! which roc publishes on 127.0.0.1 with a per-session password).
//!
//! Only what autopilot needs: which sessions are busy, the last messages,
//! sending a prompt, stopping a turn, pending questions and answering them,
//! the agent's todo list, and pointing the TUI at a session. Checked against
//! opencode 1.19's OpenAPI document (`GET /doc`).

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

/// One opencode server.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    user: String,
    password: String,
}

/// What a session is doing (`GET /session/status`): sessions not listed
/// are idle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Status {
    Idle,
    Busy,
    /// opencode is retrying a failed model call (attempt, why).
    Retry(u64, String),
}

/// One message, flattened for autopilot.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    /// The text parts, joined.
    pub text: String,
    /// Tools it called (names).
    pub tools: Vec<String>,
    /// opencode's finish reason ("stop", "tool-calls", …).
    pub finish: Option<String>,
    /// An error opencode recorded on the message (model failures).
    pub error: Option<String>,
    /// Milliseconds since the epoch.
    pub created: i64,
    pub completed: Option<i64>,
    /// Output tokens, when opencode reports them.
    pub output_tokens: u64,
}

/// A question the agent asked (opencode's `question` tool).
#[derive(Debug, Clone, Serialize)]
pub struct Question {
    pub id: String,
    pub session: String,
    /// The questions as opencode sent them (question, header, options…).
    pub questions: Value,
}

impl Client {
    pub fn new(base: &str, user: &str, password: &str) -> Result<Client> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Client {
            http,
            base: base.trim_end_matches('/').to_string(),
            user: user.to_string(),
            password: password.to_string(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let r = self
            .http
            .get(self.url(path))
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .with_context(|| format!("opencode at {} doesn't answer", self.base))?;
        let status = r.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("opencode refused the password (the session may have restarted)");
        }
        if !status.is_success() {
            bail!("opencode answered {status} for {path}");
        }
        let text = r.text().await?;
        Ok(serde_json::from_str(&text)?)
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let r = self
            .http
            .post(self.url(path))
            .basic_auth(&self.user, Some(&self.password))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .with_context(|| format!("opencode at {} doesn't answer", self.base))?;
        let status = r.status();
        if !status.is_success() {
            let text: String = r
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect();
            bail!("opencode answered {status} for {path}: {text}");
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let text = r.text().await.unwrap_or_default();
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Busy, retrying and (by omission) idle sessions.
    pub async fn statuses(&self) -> Result<HashMap<String, Status>> {
        let v = self.get("/session/status").await?;
        let mut out = HashMap::new();
        if let Some(m) = v.as_object() {
            for (id, s) in m {
                let st = match s["type"].as_str() {
                    Some("busy") => Status::Busy,
                    Some("retry") => Status::Retry(
                        s["attempt"].as_u64().unwrap_or(0),
                        s["message"].as_str().unwrap_or("").to_string(),
                    ),
                    _ => Status::Idle,
                };
                out.insert(id.clone(), st);
            }
        }
        Ok(out)
    }

    /// Top-level sessions, most recently updated first: (id, title, updated ms).
    pub async fn sessions(&self) -> Result<Vec<(String, String, i64)>> {
        let v = self.get("/session?roots=true").await?;
        let mut out: Vec<(String, String, i64)> = v
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|s| s["parentID"].is_null())
                    .map(|s| {
                        (
                            s["id"].as_str().unwrap_or("").to_string(),
                            s["title"].as_str().unwrap_or("").to_string(),
                            s["time"]["updated"].as_i64().unwrap_or(0),
                        )
                    })
                    .filter(|s| !s.0.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|s| std::cmp::Reverse(s.2));
        Ok(out)
    }

    /// Makes a session (when the TUI hasn't one yet).
    pub async fn create_session(&self, title: &str) -> Result<String> {
        let v = self.post("/session", json!({ "title": title })).await?;
        v["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("opencode made no session"))
    }

    /// The last `limit` messages of a session, oldest first.
    pub async fn messages(&self, session: &str, limit: usize) -> Result<Vec<Message>> {
        let v = self
            .get(&format!("/session/{session}/message?limit={limit}"))
            .await?;
        Ok(v.as_array()
            .map(|a| a.iter().map(parse_message).collect())
            .unwrap_or_default())
    }

    /// Sends a prompt and returns at once; the turn runs in opencode.
    pub async fn prompt(&self, session: &str, text: &str) -> Result<()> {
        self.post(
            &format!("/session/{session}/prompt_async"),
            json!({ "parts": [{ "type": "text", "text": text }] }),
        )
        .await
        .map(|_| ())
    }

    /// Stops the turn in progress.
    pub async fn abort(&self, session: &str) -> Result<()> {
        self.post(&format!("/session/{session}/abort"), json!({}))
            .await
            .map(|_| ())
    }

    /// Questions waiting for an answer, every session.
    pub async fn questions(&self) -> Result<Vec<Question>> {
        let v = self.get("/question").await?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .map(|q| Question {
                        id: q["id"].as_str().unwrap_or("").to_string(),
                        session: q["sessionID"].as_str().unwrap_or("").to_string(),
                        questions: q["questions"].clone(),
                    })
                    .filter(|q| !q.id.is_empty())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Answers a question: one list of chosen labels (or typed text) per
    /// question, in order.
    pub async fn answer(&self, question: &str, answers: &[Vec<String>]) -> Result<()> {
        self.post(
            &format!("/question/{question}/reply"),
            json!({ "answers": answers }),
        )
        .await
        .map(|_| ())
    }

    /// The agent's todo list for a session.
    pub async fn todos(&self, session: &str) -> Result<Value> {
        self.get(&format!("/session/{session}/todo")).await
    }

    /// Shows this session in the TUI, so a person at the terminal sees it.
    pub async fn select_in_tui(&self, session: &str) -> Result<()> {
        self.post("/tui/select-session", json!({ "sessionID": session }))
            .await
            .map(|_| ())
    }
}

/// Flattens one `{info, parts}` message.
pub fn parse_message(m: &Value) -> Message {
    let info = &m["info"];
    let mut text = String::new();
    let mut tools = Vec::new();
    for p in m["parts"].as_array().into_iter().flatten() {
        match p["type"].as_str() {
            Some("text") => {
                if p["synthetic"].as_bool() != Some(true) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(p["text"].as_str().unwrap_or(""));
                }
            }
            Some("tool") => tools.push(p["tool"].as_str().unwrap_or("").to_string()),
            _ => {}
        }
    }
    let error = if info["error"].is_null() {
        None
    } else {
        let e = &info["error"];
        Some(
            e["data"]["message"]
                .as_str()
                .or(e["message"].as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    e["name"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| e.to_string())
                }),
        )
    };
    Message {
        id: info["id"].as_str().unwrap_or("").to_string(),
        role: info["role"].as_str().unwrap_or("").to_string(),
        text,
        tools,
        finish: info["finish"].as_str().map(String::from),
        error,
        created: info["time"]["created"].as_i64().unwrap_or(0),
        completed: info["time"]["completed"].as_i64(),
        output_tokens: info["tokens"]["output"].as_u64().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_messages_and_errors() {
        let m = parse_message(&json!({
            "info": {"id": "msg_1", "role": "assistant", "finish": "stop", "time": {"created": 1, "completed": 2}, "tokens": {"output": 42}},
            "parts": [
                {"type": "step-start"},
                {"type": "text", "text": "Turn 1 done."},
                {"type": "tool", "tool": "bash", "state": {"status": "completed"}},
                {"type": "text", "text": "hidden", "synthetic": true},
                {"type": "text", "text": "More."}
            ]
        }));
        assert_eq!(m.text, "Turn 1 done.\nMore.");
        assert_eq!(m.tools, vec!["bash"]);
        assert_eq!(m.finish.as_deref(), Some("stop"));
        assert_eq!((m.completed, m.output_tokens), (Some(2), 42));
        let e = parse_message(&json!({
            "info": {"id": "m", "role": "assistant", "time": {"created": 1},
                     "error": {"name": "APIError", "data": {"message": "context length exceeded", "statusCode": 400}}},
            "parts": []
        }));
        assert_eq!(e.error.as_deref(), Some("context length exceeded"));
    }
}
