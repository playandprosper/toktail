// SPDX-License-Identifier: Apache-2.0
//! The local dashboard ("portal"). Bound to 127.0.0.1 only, with a Host
//! header check so other websites can't reach it through DNS rebinding.
//! All page assets are embedded; nothing is loaded from the internet.

use crate::app::App;
use crate::config;
use crate::ledger::{self, Filter};
use crate::pricing;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Local, NaiveDate};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const PIP_SVG: &str = include_str!("../assets/pip.svg");
const APP_CSS: &str = include_str!("../assets/app.css");
const PIP_JS: &str = include_str!("../assets/pip.js");
const DASHBOARD_JS: &str = include_str!("../assets/dashboard.js");
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/ping", get(ping))
        .route("/api/meta", get(meta))
        .route("/api/daily", get(daily))
        .route("/api/heatmap", get(heatmap))
        .route("/api/recent", get(recent))
        .route("/api/repeats", get(repeats))
        .route("/api/export.csv", get(export_csv))
        .route("/api/sync", post(sync_now))
        .route("/pip.svg", get(pip_svg))
        .route(
            "/app.css",
            get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS) }),
        )
        .route(
            "/pip.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    PIP_JS,
                )
            }),
        )
        .route(
            "/dashboard.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    DASHBOARD_JS,
                )
            }),
        )
        .route("/api/live", get(live))
        .route("/api/costs", get(costs))
        .route("/api/settings", get(get_settings).post(set_settings))
        .layer(middleware::from_fn_with_state(app.clone(), local_only))
        .with_state(app)
}

pub async fn serve(app: Arc<App>, listener: std::net::TcpListener) {
    listener.set_nonblocking(true).ok();
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            log::error!("portal listener: {e}");
            return;
        }
    };
    log::info!("portal at {}", app.portal_url());
    if let Err(e) = axum::serve(listener, router(app)).await {
        log::error!("portal stopped: {e}");
    }
}

async fn local_only(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let port = app.config.read().portal.port;
    let allowed = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| allowed.iter().any(|a| a.eq_ignore_ascii_case(h)));
    let origin_ok = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .is_none_or(|o| {
            allowed
                .iter()
                .any(|a| o.eq_ignore_ascii_case(&format!("http://{a}")))
        });
    if !host_ok || !origin_ok {
        return (
            StatusCode::FORBIDDEN,
            "Toktail's portal only answers on this Mac.",
        )
            .into_response();
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'",
        ),
    );
    resp
}

#[derive(Debug, Deserialize)]
struct Q {
    from: Option<String>,
    to: Option<String>,
    /// Model names separated by '|'. Absent = all models.
    models: Option<String>,
    limit: Option<usize>,
}

impl Q {
    fn filter(&self) -> Filter {
        let date = |s: &Option<String>| {
            s.as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        };
        Filter {
            from: date(&self.from),
            to: date(&self.to),
            models: self.models.as_ref().map(|m| {
                m.split('|')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<HashSet<_>>()
            }),
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ping() -> Json<serde_json::Value> {
    Json(json!({ "app": "toktail", "version": env!("CARGO_PKG_VERSION") }))
}

async fn meta(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let (models, first, last, rows, files) = app.ledger.with_data(|d| {
        let mut per: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        let mut first: Option<NaiveDate> = None;
        let mut last: Option<NaiveDate> = None;
        let mut n = 0u64;
        for r in d.rows() {
            n += 1;
            let e = per.entry(r.model.to_string()).or_default();
            e.0 += r.output;
            e.1 += r.input;
            e.2 += 1;
            first = Some(first.map_or(r.date, |f| f.min(r.date)));
            last = Some(last.map_or(r.date, |l| l.max(r.date)));
        }
        let mut models: Vec<_> = per.into_iter().collect();
        models.sort_by_key(|m| std::cmp::Reverse(m.1 .0));
        let models: Vec<_> = models
            .into_iter()
            .map(|(m, (o, i, q))| json!({"model": m, "output": o, "input": i, "requests": q}))
            .collect();
        (models, first, last, n, d.file_names())
    });
    let st = app.status.read().clone();
    let cfg = app.config.read().clone();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "repository": REPOSITORY,
        "portal": { "intro_animation": cfg.portal.intro_animation, "pip_roams": cfg.portal.pip_roams },
        "costs_enabled": cfg.costs.show_comparison,
        "today": Local::now().date_naive(),
        "utc_offset_minutes": Local::now().offset().local_minus_utc() / 60,
        "models": models,
        "first_date": first,
        "last_date": last,
        "rows": rows,
        "files": files,
        "data_dir": app.ledger.dir().display().to_string(),
        "proxy": { "ok": st.proxy_ok, "message": st.proxy_msg, "listen": cfg.proxy.listen, "upstream": cfg.proxy.upstream },
        "s3": {
            "enabled": st.s3.enabled,
            "in_progress": st.s3.in_progress,
            "last_ok": st.s3.last_ok.map(|t| t.to_rfc3339()),
            "last_error": st.s3.last_error,
            "target": if cfg.s3.enabled { format!("{}/{}/{}", cfg.s3.endpoint.trim_end_matches('/'), cfg.s3.bucket, cfg.s3.prefix.trim_start_matches('/')) } else { String::new() },
        },
    }))
}

async fn daily(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    Json(app.ledger.with_data(|d| ledger::daily(d, &f))).into_response()
}

async fn heatmap(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    Json(app.ledger.with_data(|d| ledger::heatmap(d, &f))).into_response()
}

async fn recent(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    let limit = q.limit.unwrap_or(200).min(5000);
    Json(app.ledger.with_data(|d| ledger::recent(d, &f, limit))).into_response()
}

async fn repeats(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    let limit = q.limit.unwrap_or(25).min(500);
    let (repeated, top) = app.ledger.with_data(|d| ledger::repeats(d, &f, limit));
    Json(json!({ "repeated_requests": repeated, "top": top })).into_response()
}

async fn export_csv(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    match app.ledger.with_data(|d| ledger::export_csv(d, &f)) {
        Ok(bytes) => {
            let name = format!(
                "toktail-{}-to-{}.csv",
                f.from
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "start".into()),
                f.to.map(|d| d.to_string()).unwrap_or_else(|| "now".into())
            );
            (
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{name}\""),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn sync_now(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let enabled = app.config.read().s3.enabled;
    if enabled {
        app.sync_now.notify_one();
    }
    Json(json!({ "triggered": enabled }))
}

async fn pip_svg() -> Response {
    ([(header::CONTENT_TYPE, "image/svg+xml")], PIP_SVG).into_response()
}

#[derive(Debug, Deserialize)]
struct LiveQ {
    seconds: Option<usize>,
}

/// Pip's pulse: live tokens/sec and mood. Polled about once a second.
async fn live(State(app): State<Arc<App>>, Query(q): Query<LiveQ>) -> Json<serde_json::Value> {
    let (snap, state) = app.pip(q.seconds.unwrap_or(120));
    Json(json!({
        "state": state,
        "per_second": snap.per_second,
        "tokens_per_sec": snap.tokens_per_sec,
        "active_requests": snap.active_requests,
        "output_last_minute": snap.output_last_minute,
        "input_last_minute": snap.input_last_minute,
        "seconds_since_activity": snap.seconds_since_activity,
        "stampede_tps": app.stampede_tps(),
        "typical_tps": app.typical_tps(),
    }))
}

async fn costs(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let f = q.filter();
    let cfg = app.config.read().costs.clone();
    let book = app.prices.read().clone();
    Json(
        app.ledger
            .with_data(|d| pricing::report(d, &f, &book, &cfg)),
    )
    .into_response()
}

fn settings_json(app: &App) -> serde_json::Value {
    let cfg = app.config.read().clone();
    let book = app.prices.read();
    json!({
        "portal": { "intro_animation": cfg.portal.intro_animation, "pip_roams": cfg.portal.pip_roams },
        "costs": {
            "show_comparison": cfg.costs.show_comparison,
            "baseline": cfg.costs.baseline,
            "compare": book.compared(&cfg.costs),
            "electricity_usd_per_kwh": cfg.costs.electricity_usd_per_kwh,
            "watts_while_generating": cfg.costs.watts_while_generating,
            "hardware_cost_usd": cfg.costs.hardware_cost_usd,
        },
        "app": { "stampede_tokens_per_sec": cfg.app.stampede_tokens_per_sec },
        "prices": {
            "as_of": book.as_of,
            "origin": book.origin,
            "models": book.models.iter().map(|m| json!({
                "id": m.id, "provider": m.provider, "name": m.name,
                "input": m.input, "output": m.output, "per_request": m.per_request, "default": m.default,
            })).collect::<Vec<_>>(),
        },
        "config_file": app.paths.config_file.display().to_string(),
    })
}

async fn get_settings(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(settings_json(&app))
}

#[derive(Debug, Default, Deserialize)]
struct SettingsIn {
    portal: Option<PortalIn>,
    costs: Option<CostsIn>,
    app: Option<AppIn>,
}
#[derive(Debug, Default, Deserialize)]
struct PortalIn {
    intro_animation: Option<bool>,
    pip_roams: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
struct CostsIn {
    show_comparison: Option<bool>,
    baseline: Option<String>,
    compare: Option<Vec<String>>,
    electricity_usd_per_kwh: Option<f64>,
    watts_while_generating: Option<f64>,
    hardware_cost_usd: Option<f64>,
}
#[derive(Debug, Default, Deserialize)]
struct AppIn {
    stampede_tokens_per_sec: Option<f64>,
}

fn finite_nonneg(v: f64, max: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0 && v <= max).then_some(v)
}

/// Applies settings changed in the portal and writes them to config.toml.
async fn set_settings(State(app): State<Arc<App>>, Json(body): Json<SettingsIn>) -> Response {
    let known: Vec<String> = app
        .prices
        .read()
        .models
        .iter()
        .map(|m| m.id.clone())
        .collect();
    let mut cfg = app.config.read().clone();
    if let Some(p) = body.portal {
        if let Some(v) = p.intro_animation {
            cfg.portal.intro_animation = v;
        }
        if let Some(v) = p.pip_roams {
            cfg.portal.pip_roams = v;
        }
    }
    if let Some(c) = body.costs {
        if let Some(v) = c.show_comparison {
            cfg.costs.show_comparison = v;
        }
        if let Some(v) = c.baseline.filter(|b| known.contains(b)) {
            cfg.costs.baseline = v;
        }
        if let Some(v) = c.compare {
            cfg.costs.compare = v.into_iter().filter(|id| known.contains(id)).collect();
        }
        if let Some(v) = c
            .electricity_usd_per_kwh
            .and_then(|v| finite_nonneg(v, 10.0))
        {
            cfg.costs.electricity_usd_per_kwh = v;
        }
        if let Some(v) = c
            .watts_while_generating
            .and_then(|v| finite_nonneg(v, 10_000.0))
        {
            cfg.costs.watts_while_generating = v;
        }
        if let Some(v) = c
            .hardware_cost_usd
            .and_then(|v| finite_nonneg(v, 10_000_000.0))
        {
            cfg.costs.hardware_cost_usd = v;
        }
    }
    if let Some(a) = body.app {
        if let Some(v) = a
            .stampede_tokens_per_sec
            .and_then(|v| finite_nonneg(v, 1_000_000.0))
        {
            cfg.app.stampede_tokens_per_sec = v;
        }
    }
    if let Err(e) = config::save(&app.paths.config_file, &cfg) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not save settings: {e:#}"),
        )
            .into_response();
    }
    *app.config.write() = cfg;
    Json(settings_json(&app)).into_response()
}
