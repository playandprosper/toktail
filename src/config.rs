// SPDX-License-Identifier: Apache-2.0
//! Configuration file (`config.toml`) and on-disk locations.
//!
//! Everything lives under one base directory, by default
//! `~/Library/Application Support/Toktail/` on macOS. Set the
//! `TOKTAIL_HOME` environment variable to use another base directory.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub portal: PortalConfig,
    pub storage: StorageConfig,
    pub s3: S3Config,
    pub app: AppConfig,
    pub costs: CostsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    /// Address your LLM clients talk to. LM Studio normally owns 1234, so
    /// move LM Studio to the `upstream` port and let Toktail take 1234.
    /// Use "0.0.0.0:1234" if other machines on your network call this host.
    pub listen: String,
    /// Where LM Studio's server actually listens.
    pub upstream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PortalConfig {
    /// The dashboard only ever binds to loopback; only the port is configurable.
    pub port: u16,
    /// Pip runs out from the middle of the window when the portal opens.
    pub intro_animation: bool,
    /// Pip hops along the trail at the bottom of the portal.
    pub pip_roams: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Folder holding the monthly CSV files. Empty = `<base>/data`.
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct S3Config {
    pub enabled: bool,
    /// Full endpoint URL, e.g. https://<account>.r2.cloudflarestorage.com,
    /// http://nas.local:9000 (MinIO) or https://s3.us-west-004.backblazeb2.com
    pub endpoint: String,
    /// "auto" for Cloudflare R2, "us-east-1" for MinIO unless you changed it.
    pub region: String,
    pub bucket: String,
    /// Key prefix inside the bucket; files land at <prefix><filename>.
    pub prefix: String,
    /// Path-style addressing (endpoint/bucket/key). Needed by MinIO and most NAS targets.
    pub path_style: bool,
    /// The access key ID. The secret key is NOT stored here: it lives in the
    /// macOS Keychain (run `Toktail --set-s3-secret`) or in the
    /// TOKTAIL_S3_SECRET_KEY environment variable.
    pub access_key_id: String,
    pub sync_interval_minutes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub launch_at_login: bool,
    /// Show today's output-token count next to Pip in the menu bar.
    pub show_count_in_menu_bar: bool,
    /// Combined tokens/sec at which Pip declares a stampede. 0 = automatic
    /// (three times this host's usual single-request speed).
    pub stampede_tokens_per_sec: f64,
}

/// What-it-would-have-cost comparison against public API list prices.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostsConfig {
    /// Off until the user turns it on in the portal's settings.
    pub show_comparison: bool,
    /// Price-book id of the model savings are measured against.
    pub baseline: String,
    /// Price-book ids shown in the comparison. Empty = the default set.
    pub compare: Vec<String>,
    /// Your electricity price, used for the cost of running locally.
    pub electricity_usd_per_kwh: f64,
    /// Average power draw of the host while it is generating.
    pub watts_while_generating: f64,
    /// What the host cost, for a payback estimate. 0 = don't show one.
    pub hardware_cost_usd: f64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        ProxyConfig {
            listen: "127.0.0.1:1234".into(),
            upstream: "http://127.0.0.1:1235".into(),
        }
    }
}

impl Default for PortalConfig {
    fn default() -> Self {
        PortalConfig {
            port: 8765,
            intro_animation: true,
            pip_roams: true,
        }
    }
}

impl Default for S3Config {
    fn default() -> Self {
        S3Config {
            enabled: false,
            endpoint: String::new(),
            region: "auto".into(),
            bucket: String::new(),
            prefix: "toktail/".into(),
            path_style: true,
            access_key_id: String::new(),
            sync_interval_minutes: 15,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            launch_at_login: true,
            show_count_in_menu_bar: true,
            stampede_tokens_per_sec: 0.0,
        }
    }
}

impl Default for CostsConfig {
    fn default() -> Self {
        CostsConfig {
            show_comparison: false,
            baseline: "claude-sonnet-5.5".into(),
            compare: Vec::new(),
            electricity_usd_per_kwh: 0.18,
            watts_while_generating: 250.0,
            hardware_cost_usd: 0.0,
        }
    }
}

/// Resolved locations derived from the base directory and config.
#[derive(Debug, Clone)]
pub struct Paths {
    pub base: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub sync_state: PathBuf,
}

pub fn base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("TOKTAIL_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    app_support().join("Toktail")
}

fn app_support() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
}

/// Toktail was called TokenLedger in its first release. Move that folder
/// (config, CSV history, sync state) over once, so nothing is lost.
fn migrate_from_tokenledger(base: &Path) {
    if std::env::var("TOKTAIL_HOME").is_ok_and(|p| !p.is_empty()) || base.exists() {
        return;
    }
    let old = app_support().join("TokenLedger");
    if old.is_dir() {
        match std::fs::rename(&old, base) {
            Ok(()) => log::info!("moved {} to {}", old.display(), base.display()),
            Err(e) => log::warn!("could not move {}: {e}", old.display()),
        }
    }
}

/// Loads `config.toml`, creating it with defaults on first run.
/// Returns the config, its paths, and whether this was the first run.
pub fn load() -> Result<(Config, Paths, bool)> {
    let base = base_dir();
    migrate_from_tokenledger(&base);
    std::fs::create_dir_all(&base).with_context(|| format!("creating {}", base.display()))?;
    let config_file = base.join("config.toml");
    let first_run = !config_file.exists();
    let config: Config = if first_run {
        let c = Config::default();
        save(&config_file, &c)?;
        c
    } else {
        let text = std::fs::read_to_string(&config_file)
            .with_context(|| format!("reading {}", config_file.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", config_file.display()))?
    };
    let data_dir = if config.storage.data_dir.trim().is_empty() {
        base.join("data")
    } else {
        expand_tilde(config.storage.data_dir.trim())
    };
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let paths = Paths {
        sync_state: base.join("s3-sync-state.json"),
        config_file,
        data_dir,
        base,
    };
    Ok((config, paths, first_run))
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    let body = toml::to_string_pretty(config)?;
    let text = format!(
        "# Toktail configuration. Restart Toktail after editing by hand;\n\
         # changes made in the portal's Settings apply right away.\n\
         # The S3 secret key is never stored here; run `Toktail --set-s3-secret`.\n\n{body}"
    );
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}
