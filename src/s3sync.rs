// SPDX-License-Identifier: Apache-2.0
//! Mirrors the monthly CSV files to an S3-compatible bucket.
//!
//! Only files whose size or modification time changed since the last
//! successful upload are sent. The local CSV stays the source of truth;
//! the bucket is a copy that gets overwritten with whatever is on disk.

use crate::app::App;
use crate::config::S3Config;
use crate::secrets;
use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use s3::creds::Credentials;
use s3::{Bucket, Region};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncState {
    files: BTreeMap<String, Stamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Stamp {
    len: u64,
    mtime_ms: u128,
}

/// Runs for the life of the app; syncs while S3 is on (it can be turned on
/// and off in Settings without a restart).
pub async fn run(app: Arc<App>) {
    let mut logged_off = false;
    loop {
        let cfg = app.config.read().s3.clone();
        app.status.write().s3.enabled = cfg.enabled;
        if cfg.enabled {
            logged_off = false;
            sync_once(&app).await;
        } else if !logged_off {
            log::info!("S3 mirror is off");
            logged_off = true;
        }
        let every = Duration::from_secs(cfg.sync_interval_minutes.max(1) * 60);
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = app.sync_now.notified() => {}
        }
    }
}

/// Runs one sync, recording the outcome for the menu and dashboard.
pub async fn sync_once(app: &Arc<App>) {
    let cfg = app.config.read().s3.clone();
    if !cfg.enabled {
        return;
    }
    {
        let mut s = app.status.write();
        if s.s3.in_progress {
            return;
        }
        s.s3.in_progress = true;
    }
    let mut result = upload_changed(app, &cfg).await;
    if let Ok(n) = result {
        // Saved prompts/responses and the small sealed side files travel too.
        result = async {
            let mut n = n;
            loop {
                let before = app.records.pending_count();
                let up = crate::records::upload_pending(app).await?;
                n += up * 2;
                if up == 0 || app.records.pending_count() >= before {
                    break;
                }
            }
            n += upload_side_files(app, &cfg).await?;
            Ok::<usize, anyhow::Error>(n)
        }
        .await;
    }
    let mut s = app.status.write();
    s.s3.in_progress = false;
    s.s3.records_pending = app.records.pending_count() as u64;
    match result {
        Ok(n) => {
            if n > 0 {
                log::info!("S3: uploaded {n} file(s)");
            }
            s.s3.last_ok = Some(Local::now());
            s.s3.last_error = None;
            s.s3.files_uploaded_total += n as u64;
        }
        Err(e) => {
            log::warn!("S3 sync failed: {e:#}");
            s.s3.last_error = Some(format!("{e:#}"));
        }
    }
}

/// Last chance at quit, bounded so quitting never hangs on a slow network.
pub async fn final_sync(app: &Arc<App>) {
    if app.config.read().s3.enabled {
        let _ = tokio::time::timeout(Duration::from_secs(15), sync_once(app)).await;
    }
}

pub fn bucket(cfg: &S3Config) -> Result<Box<Bucket>> {
    if cfg.endpoint.trim().is_empty() || cfg.bucket.trim().is_empty() {
        bail!("set [s3] endpoint and bucket in config.toml");
    }
    if cfg.access_key_id.trim().is_empty() {
        bail!("set [s3] access_key_id in config.toml");
    }
    let secret = secrets::get_s3_secret(cfg.access_key_id.trim())
        .ok_or_else(|| anyhow!("no S3 secret key; run `Toktail --set-s3-secret`"))?;
    let region = Region::Custom {
        region: if cfg.region.trim().is_empty() {
            "auto".into()
        } else {
            cfg.region.trim().into()
        },
        endpoint: cfg.endpoint.trim().trim_end_matches('/').into(),
    };
    let creds = Credentials::new(
        Some(cfg.access_key_id.trim()),
        Some(&secret),
        None,
        None,
        None,
    )?;
    let b = Bucket::new(cfg.bucket.trim(), region, creds)?;
    Ok(if cfg.path_style {
        b.with_path_style()
    } else {
        b
    })
}

fn current_bucket(app: &App) -> Result<Box<Bucket>> {
    let cfg = app.config.read().s3.clone();
    if !cfg.enabled {
        bail!("S3 is off");
    }
    bucket(&cfg)
}

/// Uploads one object.
pub async fn put_object(app: &App, key: &str, bytes: &[u8], content_type: &str) -> Result<()> {
    let b = current_bucket(app)?;
    b.put_object_with_content_type(key, bytes, content_type)
        .await
        .with_context(|| format!("uploading {key}"))?;
    Ok(())
}

/// Downloads one object; `None` when it isn't there.
pub async fn get_object(app: &App, key: &str) -> Result<Option<Vec<u8>>> {
    let b = current_bucket(app)?;
    match b.get_object(key).await {
        Ok(r) => Ok(Some(r.to_vec())),
        Err(s3::error::S3Error::HttpFailWithBody(404, _)) => Ok(None),
        Err(e) => Err(anyhow!(e).context(format!("downloading {key}"))),
    }
}

/// Deletes one object (missing objects are fine).
pub async fn delete_object(app: &App, key: &str) -> Result<()> {
    let b = current_bucket(app)?;
    match b.delete_object(key).await {
        Ok(_) | Err(s3::error::S3Error::HttpFailWithBody(404, _)) => Ok(()),
        Err(e) => Err(anyhow!(e).context(format!("deleting {key}"))),
    }
}

/// Checks the S3 settings by writing, reading and deleting a tiny object.
pub async fn test(cfg: &S3Config) -> Result<String> {
    let b = bucket(cfg)?;
    let key = object_key(&cfg.prefix, ".toktail-check");
    let body = format!("toktail check {}", Local::now().to_rfc3339());
    b.put_object_with_content_type(&key, body.as_bytes(), "text/plain")
        .await
        .context("writing a test object")?;
    let back = b.get_object(&key).await.context("reading it back")?;
    let _ = b.delete_object(&key).await;
    if back.as_slice() != body.as_bytes() {
        bail!("the bucket returned something different from what was written");
    }
    Ok(format!(
        "Connected to {} / {}",
        cfg.endpoint.trim(),
        cfg.bucket.trim()
    ))
}

/// hardware.json, the sealed key store and sealed notes: needed to read the
/// records back on another Mac. All three hold nothing in the clear except
/// hardware.json, which has no prompts or secrets in it.
async fn upload_side_files(app: &Arc<App>, cfg: &S3Config) -> Result<usize> {
    let state_path = app.paths.base.join("s3-side-files.json");
    let mut state: SyncState = std::fs::read(&state_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut n = 0;
    for (name, key_name) in [
        ("hardware.json", "hardware.json"),
        ("keystore.tkt", "records/keystore.tkt"),
        ("notes.tkt", "records/notes.tkt"),
    ] {
        let path = app.paths.base.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let stamp = Stamp {
            len: meta.len(),
            mtime_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0),
        };
        if state.files.get(name) == Some(&stamp) {
            continue;
        }
        let bytes = tokio::fs::read(&path).await?;
        put_object(
            app,
            &object_key(&cfg.prefix, key_name),
            &bytes,
            "application/octet-stream",
        )
        .await?;
        state.files.insert(name.to_string(), stamp);
        n += 1;
    }
    std::fs::write(&state_path, serde_json::to_vec_pretty(&state)?)?;
    Ok(n)
}

pub fn object_key(prefix: &str, name: &str) -> String {
    let p = prefix.trim().trim_start_matches('/');
    if p.is_empty() {
        name.to_string()
    } else if p.ends_with('/') {
        format!("{p}{name}")
    } else {
        format!("{p}/{name}")
    }
}

async fn upload_changed(app: &Arc<App>, cfg: &S3Config) -> Result<usize> {
    let bucket = bucket(cfg)?;
    let state_path = app.paths.sync_state.clone();
    let mut state: SyncState = std::fs::read(&state_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let dir = app.ledger.dir().to_path_buf();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("tokens-") && n.ends_with(".csv"))
        .collect();
    names.sort();

    let mut uploaded = 0;
    for name in names {
        let path = dir.join(&name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let stamp = Stamp {
            len: meta.len(),
            mtime_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0),
        };
        if state.files.get(&name) == Some(&stamp) {
            continue;
        }
        let mut content = tokio::fs::read(&path).await?;
        // Never upload half a row if a request is being written right now.
        if let Some(last_nl) = content.iter().rposition(|&b| b == b'\n') {
            content.truncate(last_nl + 1);
        } else {
            continue;
        }
        let key = object_key(&cfg.prefix, &name);
        bucket
            .put_object_with_content_type(&key, &content, "text/csv")
            .await
            .with_context(|| format!("uploading {key}"))?;
        uploaded += 1;
        if content.len() as u64 == stamp.len {
            state.files.insert(name, stamp);
        }
        let tmp = state_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&state)?)?;
        std::fs::rename(&tmp, &state_path)?;
    }
    Ok(uploaded)
}

#[cfg(test)]
mod tests {
    use super::object_key;
    #[test]
    fn keys() {
        assert_eq!(object_key("toktail/", "a.csv"), "toktail/a.csv");
        assert_eq!(object_key("/x", "a.csv"), "x/a.csv");
        assert_eq!(object_key("", "a.csv"), "a.csv");
    }
}
