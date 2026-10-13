// SPDX-License-Identifier: Apache-2.0
//! Settings, hosts, hardware and savings endpoints for the portal.
//!
//! Secrets (API tokens, the encryption password, key-store tokens, the S3
//! secret key) are accepted here and stored in the Keychain (Credential
//! Manager on Windows). They are never
//! sent back: the portal only learns whether one is saved and its last four
//! characters.

use crate::app::App;
use crate::config::{self, HostConfig, MAX_HOSTS};
use crate::hardware::{self, Machine};
use crate::{proxy, s3sync, secrets};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/settings", get(get_settings).post(set_settings))
        .route("/api/hosts", post(save_host))
        .route("/api/hosts/{id}", delete(delete_host))
        .route("/api/hosts/test", post(test_host))
        .route("/api/lmstudio/test", post(test_host))
        .route("/api/encryption/test", post(test_encryption))
        .route("/api/s3/test", post(test_s3))
        .route("/api/pick-folder", post(pick_folder))
        .route("/api/hardware", get(get_hardware).post(save_machine))
        .route("/api/hardware/{id}", delete(delete_machine))
        .route("/api/hardware/{id}/replace", post(replace_machine))
        .route("/api/savings", get(savings))
}

pub fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, msg.into()).into_response()
}

fn server_error(msg: impl Into<String>) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.into()).into_response()
}

fn finite_nonneg(v: f64, max: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0 && v <= max).then_some(v)
}

/// "127.0.0.1:1234" -> ("127.0.0.1", 1234)
pub fn split_listen(listen: &str) -> (String, u16) {
    match listen.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(1234)),
        None => (listen.to_string(), 1234),
    }
}

fn hint(s: Option<String>) -> serde_json::Value {
    json!({ "saved": s.is_some(), "hint": s.as_deref().map(secrets::token_hint) })
}

fn host_json(app: &App, h: &HostConfig) -> serde_json::Value {
    let (bind, port) = split_listen(&h.listen);
    let token = app.host_token(&h.id);
    let st = app
        .status
        .read()
        .hosts
        .get(&h.id)
        .cloned()
        .unwrap_or_default();
    json!({
        "id": h.id,
        "name": h.name,
        "enabled": h.enabled,
        "url": proxy::normalize_upstream(&h.upstream).unwrap_or(h.upstream.clone()),
        "listen": h.listen,
        "listen_port": port,
        "allow_lan": bind == "0.0.0.0" || bind == "::" || bind == "[::]",
        "client_url": format!("{}/v1", proxy::local_url(&h.listen)),
        "has_token": token.is_some(),
        "token_hint": token.as_deref().map(secrets::token_hint),
        "proxy_ok": st.proxy_ok,
        "proxy_message": st.proxy_msg,
        "checked": st.upstream.checked,
        "ok": st.upstream.ok,
        "message": st.upstream.message,
        "models": st.upstream.models,
        "checked_at": st.upstream.checked_at.map(|t| t.to_rfc3339()),
    })
}

pub fn settings_json(app: &App) -> serde_json::Value {
    let cfg = app.config.read().clone();
    let book = app.prices.read().clone();
    let records_pending = app.records.pending_count();
    // Never a hint of the password itself, only whether one is saved.
    let pw_saved = match secrets::get_encryption_password() {
        Some(_) => json!({ "saved": true, "hint": "••••••••" }),
        None => json!({ "saved": false, "hint": null }),
    };
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
        "app": {
            "stampede_tokens_per_sec": cfg.app.stampede_tokens_per_sec,
            "animate_menu_bar": cfg.app.animate_menu_bar,
            "show_count_in_menu_bar": cfg.app.show_count_in_menu_bar,
            "launch_at_login": cfg.app.launch_at_login,
        },
        "hosts": cfg.hosts.iter().map(|h| host_json(app, h)).collect::<Vec<_>>(),
        "max_hosts": MAX_HOSTS,
        "portal_port": cfg.portal.port,
        "capture": {
            "enabled": cfg.capture.enabled,
            "dir": cfg.capture.dir,
            "effective_dir": cfg.capture_dir(&app.paths.base).display().to_string(),
            "s3_upload": cfg.capture.s3_upload,
            "s3_retention_days": cfg.capture.s3_retention_days,
            "local_retention_days": cfg.capture.local_retention_days,
            "pending_upload": records_pending,
            "error": app.records.errors.lock().clone(),
        },
        "encryption": {
            "mode": cfg.encryption.mode,
            "keystore": cfg.encryption.keystore,
            // Never a hint of the password itself, only whether one is saved.
            "password": pw_saved,
            "password_from_env": std::env::var(secrets::ENV_ENCRYPTION_PASSWORD).is_ok_and(|v| !v.is_empty()),
            "has_key_check": !cfg.encryption.key_check.is_empty(),
            "problem": app.crypto.problem(),
            "status": app.crypto.describe(),
            "onepassword_vault": cfg.encryption.onepassword_vault,
            "onepassword_token": hint(secrets::get_named_secret("1password-token")),
            "vault_addr": cfg.encryption.vault_addr,
            "vault_mount": cfg.encryption.vault_mount,
            "vault_path": cfg.encryption.vault_path,
            "vault_namespace": cfg.encryption.vault_namespace,
            "vault_token": hint(secrets::get_named_secret("vault-token")),
        },
        "s3": {
            "enabled": cfg.s3.enabled,
            "endpoint": cfg.s3.endpoint,
            "region": cfg.s3.region,
            "bucket": cfg.s3.bucket,
            "prefix": cfg.s3.prefix,
            "path_style": cfg.s3.path_style,
            "access_key_id": cfg.s3.access_key_id,
            "secret": hint(if cfg.s3.access_key_id.trim().is_empty() { None } else { secrets::get_s3_secret(cfg.s3.access_key_id.trim()) }),
            "sync_interval_minutes": cfg.s3.sync_interval_minutes,
        },
        "hardware": { "default_lifecycle_months": cfg.hardware.default_lifecycle_months },
        "prices": {
            "as_of": book.as_of,
            "origin": book.origin,
            "models": book.models.iter().map(|m| json!({
                "id": m.id, "provider": m.provider, "name": m.name,
                "input": m.input, "output": m.output, "per_request": m.per_request, "default": m.default,
            })).collect::<Vec<_>>(),
        },
        "config_file": app.paths.config_file.display().to_string(),
        "data_dir": app.ledger.dir().display().to_string(),
        "demo": app.demo,
        "secret_store": crate::platform::secret_store_name(),
        "os": std::env::consts::OS,
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
    capture: Option<CaptureIn>,
    encryption: Option<EncryptionIn>,
    s3: Option<S3In>,
    hardware: Option<HardwareIn>,
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
    animate_menu_bar: Option<bool>,
    show_count_in_menu_bar: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
struct CaptureIn {
    enabled: Option<bool>,
    dir: Option<String>,
    s3_upload: Option<bool>,
    s3_retention_days: Option<u32>,
    local_retention_days: Option<u32>,
}
#[derive(Debug, Default, Deserialize)]
struct EncryptionIn {
    mode: Option<String>,
    keystore: Option<String>,
    /// A new password. Absent or empty = keep the saved one.
    password: Option<String>,
    /// Accept a password that doesn't match the one files were saved with
    /// (files saved with the old one stay unreadable until it's set back).
    start_fresh: Option<bool>,
    onepassword_vault: Option<String>,
    onepassword_token: Option<String>,
    clear_onepassword_token: Option<bool>,
    vault_addr: Option<String>,
    vault_mount: Option<String>,
    vault_path: Option<String>,
    vault_namespace: Option<String>,
    vault_token: Option<String>,
    clear_vault_token: Option<bool>,
}
#[derive(Debug, Default, Deserialize)]
struct S3In {
    enabled: Option<bool>,
    endpoint: Option<String>,
    region: Option<String>,
    bucket: Option<String>,
    prefix: Option<String>,
    path_style: Option<bool>,
    access_key_id: Option<String>,
    secret: Option<String>,
    sync_interval_minutes: Option<u64>,
}
#[derive(Debug, Default, Deserialize)]
struct HardwareIn {
    default_lifecycle_months: Option<u32>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
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
    let was_s3 = cfg.s3.enabled;
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
        if let Some(v) = a.animate_menu_bar {
            cfg.app.animate_menu_bar = v;
        }
        if let Some(v) = a.show_count_in_menu_bar {
            cfg.app.show_count_in_menu_bar = v;
        }
    }
    if let Some(h) = body.hardware {
        if let Some(v) = h.default_lifecycle_months {
            if !(1..=600).contains(&v) {
                return bad_request("The lifecycle must be between 1 and 600 months.");
            }
            cfg.hardware.default_lifecycle_months = v;
        }
    }
    if let Some(c) = body.capture {
        if let Some(d) = c.dir {
            let d = d.trim().to_string();
            if !d.is_empty() {
                let p = config::expand_tilde(&d);
                if !p.is_absolute() {
                    return bad_request(
                        "Choose a full folder path, like ~/Documents/Toktail records.",
                    );
                }
                if let Err(e) = std::fs::create_dir_all(&p) {
                    return bad_request(format!("Can't use that folder: {e}"));
                }
                let probe = p.join(".toktail-write-check");
                if let Err(e) = std::fs::write(&probe, b"ok") {
                    return bad_request(format!("Toktail can't write to that folder: {e}"));
                }
                let _ = std::fs::remove_file(probe);
            }
            cfg.capture.dir = d;
        }
        if let Some(v) = c.enabled {
            cfg.capture.enabled = v;
        }
        if let Some(v) = c.s3_upload {
            cfg.capture.s3_upload = v;
        }
        if let Some(v) = c.s3_retention_days {
            cfg.capture.s3_retention_days = v.min(36_500);
        }
        if let Some(v) = c.local_retention_days {
            cfg.capture.local_retention_days = v.min(36_500);
        }
    }

    // Secrets are written only after everything else validates.
    let mut new_password: Option<String> = None;
    let mut named: Vec<(&str, Option<String>)> = Vec::new();
    let mut reconfigure_crypto = false;
    if let Some(e) = body.encryption {
        if let Some(m) = e.mode {
            if !["off", "password", "per_file"].contains(&m.as_str()) {
                return bad_request("Unknown encryption mode.");
            }
            cfg.encryption.mode = m;
        }
        if let Some(k) = e.keystore {
            if !["file", "1password", "vault"].contains(&k.as_str()) {
                return bad_request("Unknown key store.");
            }
            cfg.encryption.keystore = k;
        }
        if let Some(v) = e.onepassword_vault {
            cfg.encryption.onepassword_vault = v.trim().to_string();
        }
        if let Some(v) = e.vault_addr {
            let v = v.trim().trim_end_matches('/').to_string();
            if !v.is_empty() && !(v.starts_with("http://") || v.starts_with("https://")) {
                return bad_request("Vault's address must start with http:// or https://");
            }
            cfg.encryption.vault_addr = v;
        }
        if let Some(v) = e.vault_mount {
            cfg.encryption.vault_mount = v.trim().trim_matches('/').to_string();
        }
        if let Some(v) = e.vault_path {
            cfg.encryption.vault_path = v.trim().trim_matches('/').to_string();
        }
        if let Some(v) = e.vault_namespace {
            cfg.encryption.vault_namespace = v.trim().to_string();
        }
        if let Some(pw) = e.password.filter(|p| !p.is_empty()) {
            if pw.chars().count() < 8 {
                return bad_request("Use at least 8 characters for the encryption password.");
            }
            let key = match crate::crypto::derive_key(&pw, &cfg.encryption.salt) {
                Ok(k) => k,
                Err(e) => return server_error(format!("{e:#}")),
            };
            let fp = hex::encode(crate::crypto::fingerprint(&key));
            if !cfg.encryption.key_check.is_empty() && cfg.encryption.key_check != fp {
                if e.start_fresh != Some(true) {
                    return bad_request("That isn't the password your saved files use. To use a new password anyway, tick \"Start fresh\" (files saved with the old password will need the old one to open).");
                }
                cfg.encryption.key_check = fp;
            }
            new_password = Some(pw);
        }
        if e.clear_onepassword_token == Some(true) {
            named.push(("1password-token", None));
        } else if let Some(t) = nonempty(e.onepassword_token) {
            named.push(("1password-token", Some(t)));
        }
        if e.clear_vault_token == Some(true) {
            named.push(("vault-token", None));
        } else if let Some(t) = nonempty(e.vault_token) {
            named.push(("vault-token", Some(t)));
        }
        reconfigure_crypto = true;
    }
    let mut new_s3_secret: Option<String> = None;
    if let Some(s) = body.s3 {
        if let Some(v) = s.endpoint {
            let v = v.trim().trim_end_matches('/').to_string();
            if !v.is_empty() && !(v.starts_with("http://") || v.starts_with("https://")) {
                return bad_request(
                    "The S3 endpoint must start with https:// (or http:// for a local MinIO).",
                );
            }
            cfg.s3.endpoint = v;
        }
        if let Some(v) = s.region {
            cfg.s3.region = v.trim().to_string();
        }
        if let Some(v) = s.bucket {
            cfg.s3.bucket = v.trim().to_string();
        }
        if let Some(v) = s.prefix {
            cfg.s3.prefix = v.trim().trim_start_matches('/').to_string();
        }
        if let Some(v) = s.path_style {
            cfg.s3.path_style = v;
        }
        if let Some(v) = s.access_key_id {
            cfg.s3.access_key_id = v.trim().to_string();
        }
        if let Some(v) = s.sync_interval_minutes {
            cfg.s3.sync_interval_minutes = v.clamp(1, 1440);
        }
        new_s3_secret = nonempty(s.secret);
        if new_s3_secret.is_some() && cfg.s3.access_key_id.is_empty() {
            return bad_request("Enter the access key ID along with the secret.");
        }
        if let Some(v) = s.enabled {
            if v && (cfg.s3.endpoint.is_empty()
                || cfg.s3.bucket.is_empty()
                || cfg.s3.access_key_id.is_empty())
            {
                return bad_request(
                    "Fill in the endpoint, bucket and access key ID to turn S3 on.",
                );
            }
            cfg.s3.enabled = v;
        }
    }

    if let Some(pw) = &new_password {
        if let Err(e) = secrets::set_encryption_password(Some(pw)) {
            return server_error(format!(
                "Couldn't save the password to the {}: {e:#}",
                crate::platform::secret_store_name()
            ));
        }
    }
    for (name, v) in named {
        if let Err(e) = secrets::set_named_secret(name, v.as_deref()) {
            return server_error(format!(
                "Couldn't save to the {}: {e:#}",
                crate::platform::secret_store_name()
            ));
        }
    }
    if let Some(secret) = &new_s3_secret {
        if let Err(e) = secrets::set_s3_secret(&cfg.s3.access_key_id, Some(secret)) {
            return server_error(format!(
                "Couldn't save the S3 secret to the {}: {e:#}",
                crate::platform::secret_store_name()
            ));
        }
    }
    if let Err(e) = config::save(&app.paths.config_file, &cfg) {
        return server_error(format!("could not save settings: {e:#}"));
    }
    let s3_now = cfg.s3.enabled;
    *app.config.write() = cfg;
    if reconfigure_crypto {
        app.configure_crypto();
    }
    app.status.write().s3.enabled = s3_now;
    if s3_now && !was_s3 {
        crate::records::queue_existing(&app);
    }
    if s3_now {
        app.sync_now.notify_one();
    }
    Json(settings_json(&app)).into_response()
}

// ------------------------------------------------------------------ hosts

#[derive(Debug, Deserialize)]
struct HostIn {
    /// Existing id to update; absent = add a new host.
    id: Option<String>,
    name: String,
    url: String,
    listen_port: u16,
    #[serde(default)]
    allow_lan: bool,
    #[serde(default = "yes")]
    enabled: bool,
    token: Option<String>,
    #[serde(default)]
    clear_token: bool,
}

fn yes() -> bool {
    true
}

async fn save_host(State(app): State<Arc<App>>, Json(h): Json<HostIn>) -> Response {
    let mut cfg = app.config.read().clone();
    let name = h.name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        return bad_request("Give the host a name (up to 60 characters).");
    }
    let upstream = match proxy::normalize_upstream(&h.url) {
        Ok(u) => u,
        Err(e) => return bad_request(e),
    };
    let port = h.listen_port;
    if port == 0 {
        return bad_request("Pick a port between 1 and 65535 for Toktail to listen on.");
    }
    if port == cfg.portal.port {
        return bad_request(format!(
            "Port {port} is the portal's own port. Pick another one."
        ));
    }
    let editing = h.id.clone().filter(|i| !i.is_empty());
    for other in cfg.hosts.iter().filter(|o| Some(&o.id) != editing.as_ref()) {
        let (_, p) = split_listen(&other.listen);
        if p == port {
            return bad_request(format!("Port {port} is already used by {}.", other.name));
        }
        if proxy::loops_back(&upstream, p) {
            return bad_request(format!(
                "That address is Toktail's own port for {}. Use LM Studio's port.",
                other.name
            ));
        }
    }
    if proxy::loops_back(&upstream, port) {
        return bad_request(format!("LM Studio's address points at Toktail itself (port {port}). Use LM Studio's port, or move Toktail to another port."));
    }
    let listen = format!(
        "{}:{port}",
        if h.allow_lan { "0.0.0.0" } else { "127.0.0.1" }
    );
    let id = match &editing {
        Some(id) => {
            let Some(slot) = cfg.hosts.iter_mut().find(|x| &x.id == id) else {
                return bad_request("That host no longer exists.");
            };
            slot.name = name;
            slot.upstream = upstream;
            slot.listen = listen;
            slot.enabled = h.enabled;
            id.clone()
        }
        None => {
            if cfg.hosts.len() >= MAX_HOSTS {
                return bad_request(format!("Toktail watches up to {MAX_HOSTS} hosts."));
            }
            let base = config::slug(&name);
            let mut id = base.clone();
            let mut n = 2;
            while cfg.hosts.iter().any(|x| x.id == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            cfg.hosts.push(HostConfig {
                id: id.clone(),
                name,
                upstream,
                listen,
                enabled: h.enabled,
            });
            id
        }
    };
    if h.clear_token {
        if let Err(e) = secrets::set_host_token(&id, None) {
            return server_error(format!("{e:#}"));
        }
    } else if let Some(t) = nonempty(h.token) {
        if let Err(e) = secrets::set_host_token(&id, Some(&t)) {
            return server_error(format!("Couldn't save the API token: {e:#}"));
        }
    }
    if let Err(e) = config::save(&app.paths.config_file, &cfg) {
        return server_error(format!("could not save settings: {e:#}"));
    }
    *app.config.write() = cfg;
    app.load_host_tokens();
    app.proxy_restart.notify_one();
    app.check_upstream.notify_one();
    // Give the proxy a moment to bind so the answer shows the new state.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Json(json!({ "id": id, "settings": settings_json(&app) })).into_response()
}

async fn delete_host(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let mut cfg = app.config.read().clone();
    if cfg.hosts.len() <= 1 {
        return bad_request("Toktail needs at least one host. Disable it instead.");
    }
    let before = cfg.hosts.len();
    cfg.hosts.retain(|h| h.id != id);
    if cfg.hosts.len() == before {
        return bad_request("No such host.");
    }
    if let Err(e) = config::save(&app.paths.config_file, &cfg) {
        return server_error(format!("could not save settings: {e:#}"));
    }
    let _ = secrets::set_host_token(&id, None);
    *app.config.write() = cfg;
    app.load_host_tokens();
    app.proxy_restart.notify_one();
    Json(settings_json(&app)).into_response()
}

#[derive(Debug, Deserialize)]
struct TestIn {
    url: String,
    /// Empty = use the saved token of `id`.
    token: Option<String>,
    id: Option<String>,
}

/// "Test connection": tries an address and token before saving.
async fn test_host(
    State(app): State<Arc<App>>,
    Json(body): Json<TestIn>,
) -> Json<serde_json::Value> {
    let url = match proxy::normalize_upstream(&body.url) {
        Ok(u) => u,
        Err(e) => return Json(json!({ "ok": false, "message": e, "models": [] })),
    };
    let token = nonempty(body.token).or_else(|| {
        app.host_token(
            body.id
                .as_deref()
                .unwrap_or(&app.config.read().default_host_id()),
        )
    });
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    let started = std::time::Instant::now();
    let p = proxy::probe(&client, &url, token.as_deref()).await;
    Json(
        json!({ "ok": p.ok, "message": p.message, "models": p.models, "url": url, "ms": started.elapsed().as_millis() as u64 }),
    )
}

async fn test_encryption(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let cfg = app.config.read().encryption.clone();
    let r = tokio::task::spawn_blocking(move || crate::crypto::Crypto::test_keystore(&cfg)).await;
    match r {
        Ok(Ok(msg)) => Json(json!({ "ok": true, "message": msg })),
        Ok(Err(e)) => Json(json!({ "ok": false, "message": format!("{e:#}") })),
        Err(e) => Json(json!({ "ok": false, "message": e.to_string() })),
    }
}

async fn test_s3(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let cfg = app.config.read().s3.clone();
    match tokio::time::timeout(std::time::Duration::from_secs(20), s3sync::test(&cfg)).await {
        Ok(Ok(msg)) => Json(json!({ "ok": true, "message": msg })),
        Ok(Err(e)) => Json(json!({ "ok": false, "message": format!("{e:#}") })),
        Err(_) => Json(json!({ "ok": false, "message": "No answer within 20 seconds." })),
    }
}

/// Opens the macOS folder picker and returns the chosen path.
async fn pick_folder() -> Response {
    match crate::platform::pick_folder("Where should Toktail save prompts and responses?").await {
        Ok(p) => Json(json!({ "path": p })).into_response(),
        Err(e) => (StatusCode::NOT_IMPLEMENTED, e).into_response(),
    }
}

// ------------------------------------------------------------------ hardware

async fn get_hardware(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let fleet = app.hardware.read().clone();
    let default_lifecycle = app.config.read().hardware.default_lifecycle_months;
    Json(json!({ "machines": fleet.machines, "default_lifecycle_months": default_lifecycle }))
}

fn save_fleet(app: &App, fleet: hardware::Fleet) -> Result<(), Box<Response>> {
    fleet
        .save(&app.paths.base)
        .map_err(|e| Box::new(server_error(format!("could not save hardware.json: {e:#}"))))?;
    *app.hardware.write() = fleet;
    if app.config.read().s3.enabled {
        app.sync_now.notify_one();
    }
    Ok(())
}

fn check_host(app: &App, m: &Machine) -> Result<(), Box<Response>> {
    let cfg = app.config.read();
    let known = cfg.host(&m.host).is_some()
        || app
            .ledger
            .with_data(|d| d.rows().any(|r| *r.host == *m.host));
    if !known {
        return Err(Box::new(bad_request(
            "Pick one of your hosts for this machine.",
        )));
    }
    Ok(())
}

async fn save_machine(State(app): State<Arc<App>>, Json(m): Json<Machine>) -> Response {
    if let Err(r) = check_host(&app, &m) {
        return *r;
    }
    let mut fleet = app.hardware.read().clone();
    let id = match fleet.upsert(m) {
        Ok(id) => id,
        Err(e) => return bad_request(e.to_string()),
    };
    if let Err(r) = save_fleet(&app, fleet) {
        return *r;
    }
    Json(json!({ "id": id })).into_response()
}

async fn delete_machine(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let mut fleet = app.hardware.read().clone();
    if !fleet.remove(&id) {
        return bad_request("No such machine.");
    }
    if let Err(r) = save_fleet(&app, fleet) {
        return *r;
    }
    Json(json!({ "ok": true })).into_response()
}

async fn replace_machine(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(m): Json<Machine>,
) -> Response {
    let mut fleet = app.hardware.read().clone();
    let new_id = match fleet.replace(&id, m) {
        Ok(i) => i,
        Err(e) => return bad_request(e.to_string()),
    };
    if let Err(r) = save_fleet(&app, fleet) {
        return *r;
    }
    Json(json!({ "id": new_id })).into_response()
}

async fn savings(State(app): State<Arc<App>>) -> Response {
    let fleet = app.hardware.read().clone();
    let book = app.prices.read().clone();
    let (costs, lifecycle) = {
        let c = app.config.read();
        (c.costs.clone(), c.hardware.default_lifecycle_months)
    };
    let a = app.clone();
    match tokio::task::spawn_blocking(move || {
        a.ledger
            .with_data(|d| hardware::savings(d, &fleet, &book, &costs, lifecycle))
    })
    .await
    {
        Ok(s) => Json(s).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}
