//! The local dashboard ("portal"). Bound to 127.0.0.1 only, with a Host
//! header check so other websites can't reach it through DNS rebinding.
//! All page assets are embedded; nothing is loaded from the internet.

use crate::app::App;
use crate::ledger::{self, Filter};
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
            "TokenLedger's portal only answers on this Mac.",
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
    Json(json!({ "app": "tokenledger", "version": env!("CARGO_PKG_VERSION") }))
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
        models.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
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
                "tokenledger-{}-to-{}.csv",
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
