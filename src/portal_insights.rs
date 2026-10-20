// SPDX-License-Identifier: Apache-2.0
//! Models leaderboard, budgets, alerts and the Prometheus endpoint.

use crate::app::App;
use crate::portal::Q;
use crate::portal_settings::bad_request;
use crate::{budgets, stats};
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Local;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/models/stats", get(model_stats))
        .route("/api/budgets", get(get_budgets).post(set_budgets))
        .route("/api/alerts", get(get_alerts))
        .route("/api/alerts/clear", post(clear_alerts))
        .route("/metrics", get(metrics))
}

async fn model_stats(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    let book = app.prices.read().clone();
    let (baseline, cache_aware) = {
        let c = app.config.read();
        (c.costs.baseline.clone(), c.costs.cache_aware)
    };
    let a = app.clone();
    match tokio::task::spawn_blocking(move || {
        a.ledger
            .with_data(|d| stats::models(d, &f, &book, &baseline, cache_aware))
    })
    .await
    {
        Ok(r) => Json(r).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub fn budgets_json(app: &App) -> serde_json::Value {
    let cfg = app.config.read().clone();
    let list = app
        .ledger
        .with_data(|d| budgets::status(d, &cfg.budgets, Local::now()));
    let list: Vec<serde_json::Value> = list
        .into_iter()
        .map(|s| {
            let scope = budgets::scope_name(app, &s.budget);
            let mut v = serde_json::to_value(&s).unwrap_or_default();
            v["scope"] = json!(scope);
            v
        })
        .collect();
    json!({ "budgets": list, "alerts": cfg.alerts })
}

async fn get_budgets(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(budgets_json(&app))
}

#[derive(Deserialize)]
struct BudgetsIn {
    budgets: Option<Vec<crate::config::BudgetConfig>>,
    alerts: Option<crate::config::AlertsConfig>,
}

async fn set_budgets(State(app): State<Arc<App>>, Json(b): Json<BudgetsIn>) -> Response {
    let mut cfg = app.config.read().clone();
    if let Some(list) = b.budgets {
        match budgets::validate(list) {
            Ok(l) => cfg.budgets = l,
            Err(e) => return bad_request(e),
        }
    }
    if let Some(a) = b.alerts {
        cfg.alerts = a;
    }
    if let Err(e) = crate::config::save(&app.paths.config_file, &cfg) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save settings: {e:#}"),
        )
            .into_response();
    }
    *app.config.write() = cfg;
    Json(budgets_json(&app)).into_response()
}

#[derive(Deserialize)]
struct AlertsQ {
    since: Option<u64>,
}

async fn get_alerts(
    State(app): State<Arc<App>>,
    Query(q): Query<AlertsQ>,
) -> Json<serde_json::Value> {
    let list = match q.since {
        Some(s) => app.alerts.since(s),
        None => app.alerts.recent(50),
    };
    Json(json!({ "alerts": list }))
}

async fn clear_alerts(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    app.alerts.clear();
    Json(json!({ "ok": true }))
}

/// Prometheus label value escaping.
fn lv(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Prometheus text format: all-time counters from the CSV, live gauges, host
/// health and budgets. Scrape it from this Mac at http://127.0.0.1:<port>/metrics.
pub fn metrics_text(app: &App) -> String {
    #[derive(Default)]
    struct T {
        requests: u64,
        errors: u64,
        input: u64,
        output: u64,
        gen_out: u64,
        gen_ms: u64,
    }
    let per: BTreeMap<(String, String), T> = app.ledger.with_data(|d| {
        let mut m: BTreeMap<(String, String), T> = BTreeMap::new();
        for r in d.rows() {
            let t = m
                .entry((r.host.to_string(), r.model.to_string()))
                .or_default();
            t.requests += 1;
            if r.status >= 400 {
                t.errors += 1;
            }
            t.input += r.input;
            t.output += r.output;
            if r.output > 0 && r.gen_ms > 0 {
                t.gen_out += r.output;
                t.gen_ms += r.gen_ms;
            }
        }
        m
    });
    let mut o = String::new();
    let family = |o: &mut String, name: &str, kind: &str, help: &str| {
        let _ = writeln!(o, "# HELP {name} {help}\n# TYPE {name} {kind}");
    };
    family(
        &mut o,
        "toktail_tokens_total",
        "counter",
        "Tokens counted, by host, model and direction.",
    );
    for ((h, m), t) in &per {
        let _ = writeln!(
            o,
            "toktail_tokens_total{{host=\"{}\",model=\"{}\",direction=\"input\"}} {}",
            lv(h),
            lv(m),
            t.input
        );
        let _ = writeln!(
            o,
            "toktail_tokens_total{{host=\"{}\",model=\"{}\",direction=\"output\"}} {}",
            lv(h),
            lv(m),
            t.output
        );
    }
    family(
        &mut o,
        "toktail_requests_total",
        "counter",
        "Inference requests, by host and model.",
    );
    for ((h, m), t) in &per {
        let _ = writeln!(
            o,
            "toktail_requests_total{{host=\"{}\",model=\"{}\"}} {}",
            lv(h),
            lv(m),
            t.requests
        );
    }
    family(
        &mut o,
        "toktail_request_errors_total",
        "counter",
        "Requests that ended with HTTP 400 or above (499 = the client left).",
    );
    for ((h, m), t) in &per {
        let _ = writeln!(
            o,
            "toktail_request_errors_total{{host=\"{}\",model=\"{}\"}} {}",
            lv(h),
            lv(m),
            t.errors
        );
    }
    family(
        &mut o,
        "toktail_generation_seconds_total",
        "counter",
        "Time spent generating output, by host and model.",
    );
    for ((h, m), t) in &per {
        let _ = writeln!(
            o,
            "toktail_generation_seconds_total{{host=\"{}\",model=\"{}\"}} {:.3}",
            lv(h),
            lv(m),
            t.gen_ms as f64 / 1000.0
        );
    }
    let cfg = app.config.read().clone();
    let st = app.status.read().clone();
    family(
        &mut o,
        "toktail_host_up",
        "gauge",
        "1 when Toktail is listening for the host and its LM Studio answers.",
    );
    family(
        &mut o,
        "toktail_output_tokens_per_second",
        "gauge",
        "Live output tokens per second (5-second average).",
    );
    family(
        &mut o,
        "toktail_requests_in_flight",
        "gauge",
        "Requests streaming right now.",
    );
    for h in &cfg.hosts {
        let hs = st.hosts.get(&h.id).cloned().unwrap_or_default();
        let up = h.enabled && hs.proxy_ok && hs.upstream.ok;
        let s = app.live_for(&h.id).snapshot(10);
        let id = lv(&h.id);
        let _ = writeln!(o, "toktail_host_up{{host=\"{id}\"}} {}", u8::from(up));
        let _ = writeln!(
            o,
            "toktail_output_tokens_per_second{{host=\"{id}\"}} {:.2}",
            s.tokens_per_sec
        );
        let _ = writeln!(
            o,
            "toktail_requests_in_flight{{host=\"{id}\"}} {}",
            s.active_requests
        );
    }
    if !cfg.budgets.is_empty() {
        family(
            &mut o,
            "toktail_budget_used_ratio",
            "gauge",
            "Share of each budget used this period (1 = at the limit).",
        );
        let list = app
            .ledger
            .with_data(|d| budgets::status(d, &cfg.budgets, Local::now()));
        for s in list {
            let _ = writeln!(
                o,
                "toktail_budget_used_ratio{{budget=\"{}\",period=\"{}\"}} {:.4}",
                lv(&s.budget.id),
                s.budget.period,
                s.ratio
            );
        }
    }
    o
}

async fn metrics(State(app): State<Arc<App>>) -> Response {
    let a = app.clone();
    let body = tokio::task::spawn_blocking(move || metrics_text(&a))
        .await
        .unwrap_or_default();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn metrics_are_prometheus_text() {
        let app = crate::test_support::app();
        let mut r = crate::test_support::record("qwen/qwen3-8b");
        r.input_tokens = 7;
        r.output_tokens = 3;
        app.ledger.append(&r).unwrap();
        let t = super::metrics_text(&app);
        assert!(t.contains("# TYPE toktail_tokens_total counter"));
        assert!(t.contains(
            "toktail_tokens_total{host=\"laptop\",model=\"qwen/qwen3-8b\",direction=\"input\"} 7"
        ));
        assert!(t.contains("# TYPE toktail_host_up gauge"));
        std::fs::remove_dir_all(&app.paths.base).ok();
    }
}
