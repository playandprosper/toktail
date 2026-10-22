// SPDX-License-Identifier: Apache-2.0
//! Finding the coding agents roc is running on this computer.
//!
//! roc (github.com/andreimerlescu/roc) runs each agent in its own Docker
//! container and keeps its bookkeeping in `~/.local/roc/state.json`. For an
//! opencode session it also publishes opencode's own server on
//! `127.0.0.1:<port>` and records it as `control: {kind, url, username}`;
//! the password is in `~/.local/roc/control/<session id>` (0600, never
//! mounted into a container). That is everything Toktail needs to watch the
//! session, answer the agent's questions and keep it working (autopilot.rs).
//!
//! Toktail only reads these files. roc owns them, and changes them under its
//! own lock with atomic renames, so a read sees either the old file or the
//! new one.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// One roc session, as Toktail sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RocSession {
    /// roc's session id (12 hex).
    pub id: String,
    /// "Q #2", or "agent #1" when roc runs without a pool.
    pub label: String,
    /// opencode, goose, claudecode or codex.
    pub binary: String,
    /// The worker's model id ("qwen3.8-27b:2").
    pub model: String,
    pub worker: u32,
    /// The agent's container ("roc-<id>").
    pub container: String,
    /// The working directory (same path on the host and in the container).
    pub workdir: String,
    pub started_at: String,
    /// roc's phase: starting, running, cleaning.
    pub status: String,
    /// Where opencode's server is, when roc published it.
    pub control_url: Option<String>,
    pub control_user: Option<String>,
    /// The roc process owning it, and the machine it runs on.
    pub pid: u32,
    pub hostname: String,
    /// The model server roc points at ("http://127.0.0.1:1240/v1").
    pub ai_host: String,
}

impl RocSession {
    /// The project's name: the last part of the working directory.
    pub fn project(&self) -> String {
        self.workdir
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// Can Toktail drive it (opencode with its server published)?
    pub fn controllable(&self) -> bool {
        self.binary == "opencode" && self.control_url.is_some()
    }
}

/// Everything read from one state file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RocView {
    /// The state file that was read.
    pub state_file: String,
    /// Why nothing could be read, if so.
    pub problem: Option<String>,
    pub sessions: Vec<RocSession>,
}

/// The state file: the setting, or roc's default.
pub fn state_path(setting: &str) -> PathBuf {
    let s = setting.trim();
    if s.is_empty() {
        return crate::config::expand_tilde("~/.local/roc/state.json");
    }
    crate::config::expand_tilde(s)
}

/// Where roc keeps a session's control password.
pub fn password_file(state_file: &Path, id: &str) -> PathBuf {
    state_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("control")
        .join(id)
}

/// Reads a session's control password (only you can read the file).
pub fn password(state_file: &Path, id: &str) -> Option<String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    std::fs::read_to_string(password_file(state_file, id))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parses roc's state.json.
pub fn parse(v: &Value) -> Vec<RocSession> {
    let ai = &v["config"]["ai"];
    let models = &ai["models"];
    let ai_host = ai["host"].as_str().unwrap_or("").to_string();
    let mut out: Vec<RocSession> = v["sessions"]
        .as_object()
        .map(|m| {
            m.values()
                .map(|s| {
                    let key = s["key"].as_str().unwrap_or("");
                    let worker = s["worker"].as_u64().unwrap_or(0) as u32;
                    let label = models[key]["label"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| {
                            if worker > 0 {
                                format!("#{worker}")
                            } else {
                                "agent".into()
                            }
                        });
                    RocSession {
                        id: s["id"].as_str().unwrap_or("").to_string(),
                        label,
                        binary: s["binary"].as_str().unwrap_or("").to_string(),
                        model: s["model"].as_str().unwrap_or("").to_string(),
                        worker,
                        container: s["container"].as_str().unwrap_or("").to_string(),
                        workdir: s["workdir"].as_str().unwrap_or("").to_string(),
                        started_at: s["started_at"].as_str().unwrap_or("").to_string(),
                        status: s["status"].as_str().unwrap_or("").to_lowercase(),
                        control_url: s["control"]["url"].as_str().map(String::from),
                        control_user: s["control"]["username"].as_str().map(String::from),
                        pid: s["pid"].as_u64().unwrap_or(0) as u32,
                        hostname: s["hostname"].as_str().unwrap_or("").to_string(),
                        ai_host: ai_host.clone(),
                    }
                })
                .filter(|s| !s.id.is_empty())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| {
        a.worker
            .cmp(&b.worker)
            .then(a.started_at.cmp(&b.started_at))
    });
    out
}

/// Reads the roc sessions running now.
pub fn read(setting: &str) -> RocView {
    let path = state_path(setting);
    let mut view = RocView {
        state_file: path.display().to_string(),
        ..Default::default()
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            view.problem = Some(format!(
                "No roc state file at {}. Is roc installed (roc -init)?",
                path.display()
            ));
            return view;
        }
        Err(e) => {
            view.problem = Some(format!("Can't read {}: {e}", path.display()));
            return view;
        }
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => view.sessions = parse(&v),
        Err(e) => view.problem = Some(format!("{} isn't valid JSON: {e}", path.display())),
    }
    view
}

/// The Toktail host a roc session's model traffic goes through, when roc
/// points at one of Toktail's ports (so its tokens are counted here).
pub fn toktail_host(cfg: &crate::config::Config, ai_host: &str) -> Option<String> {
    let u = url::Url::parse(ai_host).ok()?;
    let host = u.host_str()?.to_string();
    let port = u.port_or_known_default()?;
    let local = matches!(
        host.as_str(),
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "host.docker.internal"
    );
    cfg.hosts
        .iter()
        .find(|h| {
            let listen_port = h
                .listen
                .rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok());
            local && listen_port == Some(port)
        })
        .map(|h| h.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> Value {
        json!({
            "version": 1,
            "config": { "ai": { "host": "http://127.0.0.1:1240/v1", "models": {
                "qwen3.8-27b:2": { "label": "Q #2", "worker": 2 } } } },
            "sessions": {
                "3f9a1c2b7d4e": {
                    "id": "3f9a1c2b7d4e", "pid": 4821, "hostname": "studio", "status": "Running",
                    "binary": "opencode", "model": "qwen3.8-27b:2", "key": "qwen3.8-27b:2", "worker": 2,
                    "container": "roc-3f9a1c2b7d4e", "workdir": "/Users/andrei/friends_of/planning",
                    "started_at": "2026-10-21T10:00:00Z",
                    "control": { "kind": "opencode", "url": "http://127.0.0.1:53901", "username": "opencode" }
                },
                "aaaa00000000": {
                    "id": "aaaa00000000", "binary": "codex", "worker": 1, "key": "qwen3.8-27b",
                    "workdir": "/w/api", "started_at": "2026-10-21T09:00:00Z", "status": "running"
                }
            }
        })
    }

    #[test]
    fn reads_sessions_and_control() {
        let s = parse(&state());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].binary, "codex", "ordered by worker");
        assert!(!s[0].controllable());
        let oc = &s[1];
        assert_eq!(oc.label, "Q #2");
        assert_eq!(oc.project(), "planning");
        assert_eq!(oc.status, "running");
        assert_eq!(oc.control_url.as_deref(), Some("http://127.0.0.1:53901"));
        assert!(oc.controllable());
        assert_eq!(s[0].label, "#1", "no label in the pool: worker number");
    }

    #[test]
    fn password_file_is_next_to_the_state_and_checked() {
        let d = std::env::temp_dir().join(format!("toktail-roc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("control")).unwrap();
        let st = d.join("state.json");
        std::fs::write(d.join("control/3f9a1c2b7d4e"), "s3cret\n").unwrap();
        assert_eq!(password(&st, "3f9a1c2b7d4e").as_deref(), Some("s3cret"));
        assert_eq!(password(&st, "../state.json"), None, "no path tricks");
        assert_eq!(password(&st, "ffffffffffff"), None);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn finds_the_toktail_host_roc_uses() {
        let mut cfg = crate::config::Config::default();
        cfg.hosts.push(crate::config::HostConfig {
            id: "studio".into(),
            listen: "0.0.0.0:1240".into(),
            ..Default::default()
        });
        assert_eq!(
            toktail_host(&cfg, "http://127.0.0.1:1240/v1").as_deref(),
            Some("studio")
        );
        assert_eq!(
            toktail_host(&cfg, "http://host.docker.internal:1240/v1").as_deref(),
            Some("studio")
        );
        assert_eq!(
            toktail_host(&cfg, "http://studio.local:1234/v1"),
            None,
            "straight to LM Studio"
        );
    }

    #[test]
    fn missing_state_file_says_so() {
        let v = read("/nonexistent/roc/state.json");
        assert!(v.problem.unwrap().contains("No roc state file"));
        assert!(v.sessions.is_empty());
    }
}
