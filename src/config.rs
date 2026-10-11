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
    /// Legacy single-host settings (Toktail 0.3 and earlier). Read once and
    /// turned into the first entry of `hosts`; never written back.
    #[serde(skip_serializing)]
    pub proxy: ProxyConfig,
    pub portal: PortalConfig,
    pub storage: StorageConfig,
    pub s3: S3Config,
    pub app: AppConfig,
    pub costs: CostsConfig,
    pub capture: CaptureConfig,
    pub encryption: EncryptionConfig,
    pub hardware: HardwareConfig,
    /// The LM Studio servers Toktail watches, up to `MAX_HOSTS`. Each gets its
    /// own proxy port; point an app at a host's port to have it counted.
    pub hosts: Vec<HostConfig>,
}

/// How many LM Studio hosts one Toktail can watch.
pub const MAX_HOSTS: usize = 17;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// Short id used in the CSV, file paths and S3 keys: a-z, 0-9 and "-".
    pub id: String,
    /// What the portal calls it, e.g. "Mac Studio".
    pub name: String,
    /// Where this host's LM Studio server listens.
    pub upstream: String,
    /// Where Toktail listens for this host. Apps call this address.
    pub listen: String,
    pub enabled: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            id: "local".into(),
            name: "This Mac".into(),
            upstream: "http://127.0.0.1:1235".into(),
            listen: "127.0.0.1:1234".into(),
            enabled: true,
        }
    }
}

/// Saving the prompts and responses themselves. Off unless turned on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub enabled: bool,
    /// Folder for the files. Empty = `<base>/records`.
    pub dir: String,
    /// Also upload them to S3 (when S3 is set up).
    pub s3_upload: bool,
    /// Delete files from S3 this many days after they're saved. 0 = keep forever.
    pub s3_retention_days: u32,
    /// Delete local copies this many days after they're saved. 0 = keep forever.
    /// With S3 on, a local copy is only deleted once it has been uploaded.
    pub local_retention_days: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        CaptureConfig {
            enabled: false,
            dir: String::new(),
            s3_upload: true,
            s3_retention_days: 0,
            local_retention_days: 0,
        }
    }
}

/// Encryption of saved prompts and responses at rest (locally and in S3).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EncryptionConfig {
    /// "off", "password" (one key from your password) or "per_file" (a random
    /// key for every file, kept in `keystore`).
    pub mode: String,
    /// Where per-file keys are kept: "file" (an encrypted keystore in Toktail's
    /// folder), "1password" or "vault".
    pub keystore: String,
    /// Random salt for deriving the key from the password. Not secret.
    pub salt: String,
    /// Fingerprint of the derived key, to notice a changed password. Not secret.
    pub key_check: String,
    pub onepassword_vault: String,
    pub vault_addr: String,
    pub vault_mount: String,
    pub vault_path: String,
    pub vault_namespace: String,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        EncryptionConfig {
            mode: "off".into(),
            keystore: "file".into(),
            salt: String::new(),
            key_check: String::new(),
            onepassword_vault: "Toktail".into(),
            vault_addr: "http://127.0.0.1:8200".into(),
            vault_mount: "secret".into(),
            vault_path: "toktail".into(),
            vault_namespace: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HardwareConfig {
    /// Default lifecycle for new hardware, in months.
    pub default_lifecycle_months: u32,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        HardwareConfig {
            default_lifecycle_months: 72,
        }
    }
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
    /// Pip runs in the menu bar while tokens stream. Off = a still icon.
    pub animate_menu_bar: bool,
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
            animate_menu_bar: true,
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

static BASE_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Points Toktail at another base folder (used by `--demo`).
pub fn set_base_dir(p: PathBuf) {
    let _ = BASE_OVERRIDE.set(p);
}

pub fn base_dir() -> PathBuf {
    if let Some(p) = BASE_OVERRIDE.get() {
        return p.clone();
    }
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
    if BASE_OVERRIDE.get().is_some()
        || std::env::var("TOKTAIL_HOME").is_ok_and(|p| !p.is_empty())
        || base.exists()
    {
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
    let mut config: Config = if first_run {
        Config::default()
    } else {
        let text = std::fs::read_to_string(&config_file)
            .with_context(|| format!("reading {}", config_file.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", config_file.display()))?
    };
    // Single-host configs from 0.3 and earlier become the first host.
    if config.hosts.is_empty() {
        config.hosts.push(HostConfig {
            upstream: config.proxy.upstream.clone(),
            listen: config.proxy.listen.clone(),
            ..HostConfig::default()
        });
    }
    // Ids end up in paths and S3 keys: repair hand-edited ones, keep them
    // unique, and keep at most MAX_HOSTS.
    let mut seen = std::collections::HashSet::new();
    for h in &mut config.hosts {
        if !valid_host_id(&h.id) {
            h.id = slug(if h.id.is_empty() { &h.name } else { &h.id });
        }
        let base = h.id.clone();
        let mut n = 2;
        while !seen.insert(h.id.clone()) {
            h.id = format!("{base}-{n}");
            n += 1;
        }
    }
    config.hosts.truncate(MAX_HOSTS);
    if config.encryption.salt.is_empty() {
        config.encryption.salt = hex::encode(uuid::Uuid::new_v4().as_bytes());
    }
    save(&config_file, &config)?;
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

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

/// Host ids end up in file paths and S3 keys, so they stay plain.
pub fn valid_host_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// "Mac Studio #2" -> "mac-studio-2"
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').chars().take(32).collect::<String>();
    if out.is_empty() {
        "host".into()
    } else {
        out
    }
}

impl Config {
    pub fn host(&self, id: &str) -> Option<&HostConfig> {
        self.hosts.iter().find(|h| h.id == id)
    }
    /// The host rows from before multi-host tracking belong to.
    pub fn default_host_id(&self) -> String {
        self.hosts
            .first()
            .map(|h| h.id.clone())
            .unwrap_or_else(|| "local".into())
    }
    pub fn capture_dir(&self, base: &Path) -> PathBuf {
        if self.capture.dir.trim().is_empty() {
            base.join("records")
        } else {
            expand_tilde(self.capture.dir.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Mac Studio #2"), "mac-studio-2");
        assert_eq!(slug("   "), "host");
        assert!(valid_host_id("mac-mini-1"));
        assert!(!valid_host_id("Mac"));
        assert!(!valid_host_id("a/b"));
    }
    #[test]
    fn legacy_proxy_becomes_first_host() {
        let c: Config =
            toml::from_str("[proxy]\nlisten = \"127.0.0.1:4000\"\nupstream = \"http://x:1\"\n")
                .unwrap();
        assert!(c.hosts.is_empty());
        assert_eq!(c.proxy.listen, "127.0.0.1:4000");
        let out = toml::to_string(&c).unwrap();
        assert!(!out.contains("[proxy]"));
    }
}
