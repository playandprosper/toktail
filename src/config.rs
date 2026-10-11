//! Configuration file (`config.toml`) and on-disk locations.
//!
//! Everything lives under one base directory, by default
//! `~/Library/Application Support/TokenLedger/` on macOS. Set the
//! `TOKENLEDGER_HOME` environment variable to use another base directory.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub portal: PortalConfig,
    pub storage: StorageConfig,
    pub s3: S3Config,
    pub app: AppConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    /// Address your LLM clients talk to. LM Studio normally owns 1234, so
    /// move LM Studio to the `upstream` port and let TokenLedger take 1234.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
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
    /// macOS Keychain (run `TokenLedger --set-s3-secret`) or in the
    /// TOKENLEDGER_S3_SECRET_KEY environment variable.
    pub access_key_id: String,
    pub sync_interval_minutes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub launch_at_login: bool,
    /// Show today's output-token count next to the menu bar icon.
    pub show_count_in_menu_bar: bool,
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
        PortalConfig { port: 8765 }
    }
}

impl Default for S3Config {
    fn default() -> Self {
        S3Config {
            enabled: false,
            endpoint: String::new(),
            region: "auto".into(),
            bucket: String::new(),
            prefix: "tokenledger/".into(),
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
        }
    }
}

/// Resolved locations derived from the base directory and config.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub sync_state: PathBuf,
}

pub fn base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("TOKENLEDGER_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("TokenLedger")
}

/// Loads `config.toml`, creating it with defaults on first run.
/// Returns the config, its paths, and whether this was the first run.
pub fn load() -> Result<(Config, Paths, bool)> {
    let base = base_dir();
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
    };
    Ok((config, paths, first_run))
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    let body = toml::to_string_pretty(config)?;
    let text = format!(
        "# TokenLedger configuration. Restart TokenLedger after editing.\n\
         # The S3 secret key is never stored here; run `TokenLedger --set-s3-secret`.\n\n{body}"
    );
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}
