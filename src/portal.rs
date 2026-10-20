// SPDX-License-Identifier: Apache-2.0
//! The local dashboard ("portal"). Bound to 127.0.0.1 only, with a Host
//! header check so other websites can't reach it through DNS rebinding.
//! All page assets are embedded; nothing is loaded from the internet.
//!
//! Pages: Dashboard, Prompts (records), Savings (hardware), Settings. The
//! record and settings endpoints live in portal_records.rs and
//! portal_settings.rs.

use crate::app::App;
use crate::ledger::{self, Filter};
use crate::pricing;
use crate::{portal_records, portal_settings};
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
/// Page scripts and styles, served from memory.
const ASSETS: [(&str, &str, &str); 17] = [
    (
        "/vendor/BufferGeometryUtils.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/vendor/BufferGeometryUtils.js"),
    ),
    (
        "/pip3d.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/pip3d.js"),
    ),
    (
        "/pip-model.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/pip-model.js"),
    ),
    (
        "/pip-clips.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/pip-clips.js"),
    ),
    (
        "/vendor/three.module.min.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/vendor/three.module.min.js"),
    ),
    (
        "/pip-chat.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/pip-chat.js"),
    ),
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_str!("../assets/app.css"),
    ),
    (
        "/pip.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/pip.js"),
    ),
    (
        "/common.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/common.js"),
    ),
    (
        "/dashboard.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/dashboard.js"),
    ),
    (
        "/prompts.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/prompts.js"),
    ),
    (
        "/models.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/models.js"),
    ),
    (
        "/palette.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/palette.js"),
    ),
    (
        "/savings.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/savings.js"),
    ),
    (
        "/settings.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/settings.js"),
    ),
    (
        "/stage.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/stage.js"),
    ),
    (
        "/main.js",
        "text/javascript; charset=utf-8",
        include_str!("../assets/main.js"),
    ),
];
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

pub fn router(app: Arc<App>) -> Router {
    let mut r = Router::new()
        .route("/", get(index))
        .route("/api/ping", get(ping))
        .route("/api/meta", get(meta))
        .route("/api/daily", get(daily))
        .route("/api/heatmap", get(heatmap))
        .route("/api/recent", get(recent))
        .route("/api/repeats", get(repeats))
        .route("/api/export.csv", get(export_csv))
        .route("/api/sync", post(sync_now))
        .route("/api/signals", get(signals))
        .route("/api/open-data-folder", post(open_data_folder))
        .route("/pip.svg", get(pip_svg))
        .route("/api/live", get(live))
        .route("/api/pip/commentary", get(commentary_lines))
        .route("/api/pip/commentary/now", post(commentary_now))
        .route("/api/cache", get(cache_summary))
        .route("/api/cache/clear", post(cache_clear))
        .route("/api/costs", get(costs));
    for (path, ct, body) in ASSETS {
        r = r.route(
            path,
            get(move || async move { ([(header::CONTENT_TYPE, ct)], body) }),
        );
    }
    r.merge(portal_settings::routes())
        .merge(portal_records::routes())
        .merge(crate::pip_agent::routes())
        .merge(crate::portal_insights::routes())
        .merge(crate::threads::routes())
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
pub struct Q {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Model names separated by '|'. Absent = all models.
    pub models: Option<String>,
    /// Host ids separated by '|'. Absent = all hosts.
    pub hosts: Option<String>,
    pub limit: Option<usize>,
}

impl Q {
    pub fn filter(&self) -> Filter {
        let date = |s: &Option<String>| {
            s.as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        };
        Filter {
            from: date(&self.from),
            to: date(&self.to),
            models: self.models.as_ref().map(|m| split_set(m)),
            hosts: self.hosts.as_ref().map(|m| split_set(m)),
        }
    }
}

fn split_set(s: &str) -> HashSet<String> {
    s.split('|')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ping() -> Json<serde_json::Value> {
    Json(json!({ "app": "toktail", "version": env!("CARGO_PKG_VERSION") }))
}

async fn meta(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let today = Local::now().date_naive();
    let week_ago = today - chrono::Days::new(6);
    let (models, first, last, rows, files, per_host) = app.ledger.with_data(|d| {
        let mut per: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        // host -> (all time, today, last 7 days, first date, last date)
        let mut hosts: BTreeMap<String, [ledger::Totals; 3]> = BTreeMap::new();
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
            let h = hosts.entry(r.host.to_string()).or_default();
            h[0].add(r);
            if r.date == today {
                h[1].add(r);
            }
            if r.date >= week_ago {
                h[2].add(r);
            }
        }
        let mut models: Vec<_> = per.into_iter().collect();
        models.sort_by_key(|m| std::cmp::Reverse(m.1 .0 + m.1 .1));
        let models: Vec<_> = models
            .into_iter()
            .map(|(m, (o, i, q))| json!({"model": m, "output": o, "input": i, "combined": o + i, "requests": q}))
            .collect();
        (models, first, last, n, d.file_names(), hosts)
    });
    let st = app.status.read().clone();
    let cfg = app.config.read().clone();
    let stampede = app.stampede_tps();
    let mut hosts: Vec<serde_json::Value> = cfg
        .hosts
        .iter()
        .map(|h| {
            let t = per_host.get(&h.id).cloned().unwrap_or_default();
            let hs = st.hosts.get(&h.id).cloned().unwrap_or_default();
            let snap = app.live_for(&h.id).snapshot(10);
            json!({
                "id": h.id, "name": h.name, "enabled": h.enabled, "listen": h.listen,
                "upstream": crate::proxy::normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone()),
                "proxy_ok": hs.proxy_ok, "proxy_message": hs.proxy_msg,
                "lmstudio": { "checked": hs.upstream.checked, "ok": hs.upstream.ok, "message": hs.upstream.message, "models": hs.upstream.models },
                "all_time": t[0], "today": t[1], "week": t[2],
                "state": crate::live::pip_state(&snap, stampede),
                "removed": false,
            })
        })
        .collect();
    // Hosts that were removed from Settings still have history.
    for (id, t) in &per_host {
        if cfg.host(id).is_none() {
            hosts.push(json!({ "id": id, "name": id, "enabled": false, "removed": true, "all_time": t[0], "today": t[1], "week": t[2], "state": "idle" }));
        }
    }
    let enc_mode = cfg.encryption.mode.clone();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "demo": app.demo,
        "repository": REPOSITORY,
        "portal": { "intro_animation": cfg.portal.intro_animation, "pip_roams": cfg.portal.pip_roams },
        "costs_enabled": cfg.costs.show_comparison,
        "today": today,
        "utc_offset_minutes": Local::now().offset().local_minus_utc() / 60,
        "models": models,
        "hosts": hosts,
        "max_hosts": crate::config::MAX_HOSTS,
        "first_date": first,
        "last_date": last,
        "rows": rows,
        "files": files,
        "data_dir": app.ledger.dir().display().to_string(),
        "capture": {
            "enabled": cfg.capture.enabled,
            "dir": crate::records::root(&app).display().to_string(),
            "encryption": enc_mode,
            "problem": app.crypto.problem(),
            "error": app.records.errors.lock().clone(),
            "pending_upload": app.records.pending_count(),
        },
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
    host: Option<String>,
    /// Only finished requests newer than this sequence number (the live feed
    /// passes back the `seq` it last saw).
    since: Option<u64>,
}

/// Pip's pulse: live tokens/sec and mood, combined and per host. Polled
/// about once a second.
async fn live(State(app): State<Arc<App>>, Query(q): Query<LiveQ>) -> Json<serde_json::Value> {
    let secs = q.seconds.unwrap_or(120);
    let stampede = app.stampede_tps();
    let ids: Vec<String> = app
        .config
        .read()
        .hosts
        .iter()
        .map(|h| h.id.clone())
        .collect();
    let (snap, state) = match q
        .host
        .as_deref()
        .filter(|h| !h.is_empty() && ids.iter().any(|i| i == h))
    {
        Some(h) => {
            let s = app.live_for(h).snapshot(secs);
            let st = crate::live::pip_state(&s, stampede);
            (s, st)
        }
        None => app.pip(secs),
    };
    let hosts: serde_json::Map<String, serde_json::Value> = ids
        .iter()
        .map(|id| {
            let s = app.live_for(id).snapshot(60);
            let st = crate::live::pip_state(&s, stampede);
            (id.clone(), json!({ "state": st, "tokens_per_sec": s.tokens_per_sec, "active_requests": s.active_requests, "per_second": s.per_second, "output_last_minute": s.output_last_minute, "input_last_minute": s.input_last_minute }))
        })
        .collect();
    let (in_flight, finished, seq) = app.activity.snapshot(q.since.unwrap_or(0), 60);
    Json(json!({
        "activity": { "in_flight": in_flight, "finished": finished, "seq": seq },
        "state": state,
        "per_second": snap.per_second,
        "tokens_per_sec": snap.tokens_per_sec,
        "active_requests": snap.active_requests,
        "output_last_minute": snap.output_last_minute,
        "input_last_minute": snap.input_last_minute,
        "seconds_since_activity": snap.seconds_since_activity,
        "stampede_tps": stampede,
        "typical_tps": app.typical_tps(),
        "hosts": hosts,
        "commentary": { "seq": app.commentary.seq(), "latest": app.commentary.latest() },
        "cache": app.cache.quick(),
    }))
}

/// Pip's last few speech bubbles.
async fn commentary_lines(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    Json(json!({ "seq": app.commentary.seq(), "lines": app.commentary.recent(12) }))
}

/// The `u` / Space easter egg: asks Pip to speak up about the latest exchange.
async fn commentary_now(State(app): State<Arc<App>>) -> Response {
    match crate::commentary::ask_now(&app) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(msg) => Json(json!({ "ok": false, "message": msg })).into_response(),
    }
}

async fn cache_summary(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let a = app.clone();
    let v = tokio::task::spawn_blocking(move || crate::cache::summary(&a))
        .await
        .unwrap_or(serde_json::Value::Null);
    Json(v)
}

async fn cache_clear(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let a = app.clone();
    let n = tokio::task::spawn_blocking(move || crate::cache::clear(&a))
        .await
        .unwrap_or(0);
    Json(json!({ "ok": true, "removed": n }))
}

async fn costs(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Response {
    let mut f = q.filter();
    // The comparison is about what your own hosts rendered; a public API's
    // tokens were already paid for at its prices.
    {
        let c = app.config.read();
        let local: std::collections::HashSet<String> = c
            .hosts
            .iter()
            .filter(|h| !h.is_public())
            .map(|h| h.id.clone())
            .collect();
        if c.hosts.iter().any(|h| h.is_public()) {
            f.hosts = Some(match f.hosts.take() {
                Some(h) => h.intersection(&local).cloned().collect(),
                None => local,
            });
        }
    }
    let cfg = app.config.read().costs.clone();
    let book = app.prices.read().clone();
    let fleet = app.hardware.read().clone();
    let a = app.clone();
    let f2 = f.clone();
    let report = tokio::task::spawn_blocking(move || {
        a.ledger.with_data(|d| pricing::report(d, &f2, &book, &cfg))
    })
    .await;
    let Ok(report) = report else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    // The hardware's share of this range: each machine's price spread over
    // its lifecycle, for the days in the range.
    let today = Local::now().date_naive();
    let from = f
        .from
        .or_else(|| report.daily.first().map(|d| d.date))
        .unwrap_or(today);
    let to = f.to.unwrap_or(today).min(today);
    let machines: Vec<serde_json::Value> = fleet
        .machines
        .iter()
        .filter(|m| f.hosts.as_ref().is_none_or(|h| h.contains(&m.host)))
        .map(|m| json!({ "name": m.name, "host": m.host, "amortized": m.amortized(from, to, today), "price_usd": m.price_usd, "lifecycle_months": m.lifecycle_months }))
        .filter(|v| v["amortized"].as_f64().unwrap_or(0.0) > 0.0)
        .collect();
    let amortized: f64 = machines
        .iter()
        .filter_map(|m| m["amortized"].as_f64())
        .sum();
    let mut v = serde_json::to_value(&report).unwrap_or_default();
    v["hardware"] = json!({ "amortized": amortized, "machines": machines, "has_fleet": !fleet.machines.is_empty(), "days": (to - from).num_days() + 1 });
    Json(v).into_response()
}

async fn signals(State(app): State<Arc<App>>, Query(q): Query<Q>) -> Json<serde_json::Value> {
    let f = q.filter();
    let (mut input, mut output, mut refused, mut noteworthy) = (0u128, 0u128, 0u64, 0u64);
    app.ledger.with_data(|d| {
        for r in d.rows().filter(|r| f.matches(r)) {
            input += r.input as u128;
            output += r.output as u128;
            if r.outcome == "refused" {
                refused += 1;
            }
            if ["structured", "substantial", "tool_call"].contains(&r.signal.as_str()) {
                noteworthy += 1;
            }
        }
    });
    Json(
        json!({"input":input.to_string(),"output":output.to_string(),"combined":(input+output).to_string(),"refused":refused,"noteworthy":noteworthy}),
    )
}
async fn open_data_folder(State(app): State<Arc<App>>) -> Response {
    let path = app.paths.data_dir.parent().unwrap_or(&app.paths.data_dir);
    match crate::platform::open(&path.display().to_string()) {
        Ok(_) => Json(json!({"opened":true,"path":path})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
