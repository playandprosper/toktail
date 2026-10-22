// SPDX-License-Identifier: Apache-2.0
//! The Agents page: the coding sessions roc runs, autopilot on each, the
//! agent's questions and your answers, and Settings → Autopilot. Behind the
//! portal's sign-in, like every other page.

use crate::app::App;
use crate::portal_settings::{bad_request, server_error};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/agents", get(list))
        .route("/api/agents/settings", get(get_settings).post(set_settings))
        .route("/api/agents/{id}", get(detail))
        .route("/api/agents/{id}/autopilot", post(autopilot))
        .route("/api/agents/{id}/answer", post(answer))
        .route("/api/agents/{id}/message", post(message))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

async fn list(State(app): State<Arc<App>>) -> Json<Value> {
    let a = app.clone();
    Json(
        tokio::task::spawn_blocking(move || crate::autopilot::overview(&a))
            .await
            .unwrap_or(Value::Null),
    )
}

async fn detail(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    if !valid_id(&id) {
        return bad_request("bad session id");
    }
    Json(crate::autopilot::detail(&app, &id).await).into_response()
}

#[derive(Debug, Deserialize)]
struct AutopilotIn {
    /// start, stop, stop_now, resume, acknowledge, kill.
    action: String,
    #[serde(default)]
    goal: String,
}

async fn autopilot(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(b): Json<AutopilotIn>,
) -> Response {
    if !valid_id(&id) {
        return bad_request("bad session id");
    }
    let r: Result<Value, String> = match b.action.as_str() {
        "start" | "resume" => crate::autopilot::start(&app, &id, &b.goal, "you")
            .await
            .map(|p| json!(p.summary())),
        "stop" => crate::autopilot::stop(&app, &id, false, "you")
            .await
            .map(|p| json!(p.summary())),
        "stop_now" => crate::autopilot::stop(&app, &id, true, "you")
            .await
            .map(|p| json!(p.summary())),
        "acknowledge" => {
            crate::autopilot::acknowledge(&app, &id);
            Ok(json!({ "ok": true }))
        }
        "kill" => crate::autopilot::kill(&app, &id)
            .await
            .map(|t| json!({ "ok": true, "roc": t })),
        _ => Err("Unknown action.".into()),
    };
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => bad_request(e),
    }
}

#[derive(Debug, Deserialize)]
struct AnswerIn {
    question: String,
    answers: Vec<Vec<String>>,
}

async fn answer(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(b): Json<AnswerIn>,
) -> Response {
    if !valid_id(&id) || !b.question.starts_with("que") {
        return bad_request("bad question");
    }
    let answers: Vec<Vec<String>> = b
        .answers
        .into_iter()
        .map(|a| {
            a.into_iter()
                .map(|s| s.chars().take(4000).collect::<String>())
                .filter(|s| !s.trim().is_empty())
                .collect()
        })
        .collect();
    if answers.is_empty() || answers.iter().any(|a| a.is_empty()) {
        return bad_request("Answer every question.");
    }
    match crate::autopilot::answer(&app, &id, &b.question, answers, "you").await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad_request(e),
    }
}

#[derive(Debug, Deserialize)]
struct MessageIn {
    text: String,
}

async fn message(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(b): Json<MessageIn>,
) -> Response {
    let text = b.text.trim();
    if !valid_id(&id) || text.is_empty() || text.len() > 50_000 {
        return bad_request("Write a message (up to 50,000 characters).");
    }
    match crate::autopilot::send_message(&app, &id, text).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bad_request(e),
    }
}

fn settings_json(app: &App) -> Value {
    let c = app.config.read().autopilot.clone();
    json!({
        "settings": c,
        "defaults": crate::config::AutopilotConfig::default(),
        "found": {
            "roc_state": crate::roc::state_path(&c.roc_state).display().to_string(),
            "roc": crate::autopilot::find_command(&c.roc_command, "roc"),
            "docker": crate::autopilot::find_command(&c.docker_command, "docker"),
        },
        "need_human": crate::autopilot::NEED_HUMAN,
    })
}

async fn get_settings(State(app): State<Arc<App>>) -> Json<Value> {
    Json(settings_json(&app))
}

async fn set_settings(
    State(app): State<Arc<App>>,
    Json(mut c): Json<crate::config::AutopilotConfig>,
) -> Response {
    c.grace_secs = c.grace_secs.clamp(5, 3600);
    c.silence_minutes = c.silence_minutes.clamp(1, 24 * 60);
    c.verifier_timeout_secs = c.verifier_timeout_secs.clamp(5, 3600);
    c.verifier_max_chars = c.verifier_max_chars.clamp(200, 100_000);
    c.repeat_limit = c.repeat_limit.clamp(2, 100);
    c.error_limit = c.error_limit.clamp(1, 100);
    c.question_timeout_minutes = c.question_timeout_minutes.min(7 * 24 * 60);
    for t in [&mut c.continue_template, &mut c.done_template] {
        *t = t.chars().take(20_000).collect();
        if t.trim().is_empty() {
            return bad_request("A template can't be empty: it's what autopilot sends.");
        }
    }
    c.question_default_answer = c.question_default_answer.chars().take(4000).collect();
    if c.question_default_answer.trim().is_empty() {
        return bad_request(
            "Write a default answer (or set the wait to 0 so autopilot always waits for you).",
        );
    }
    let mut cfg = app.config.read().clone();
    cfg.autopilot = c;
    if let Err(e) = crate::config::save(&app.paths.config_file, &cfg) {
        return server_error(format!("could not save settings: {e:#}"));
    }
    *app.config.write() = cfg;
    app.autopilot.wake.notify_one();
    Json(settings_json(&app)).into_response()
}
