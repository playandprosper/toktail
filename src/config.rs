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
    /// Token budgets: a limit per day, week or month, for everything or one
    /// host or model. Toktail warns at 80% and again at 100%.
    pub budgets: Vec<BudgetConfig>,
    pub alerts: AlertsConfig,
    /// Toktail's own prompt cache: an identical request gets the saved answer.
    pub cache: CacheConfig,
    /// Pip's one-line commentary on what your agents are doing.
    pub commentary: CommentaryConfig,
    /// Daily market prices for the cost comparison.
    pub market: MarketConfig,
    /// Requests in flight: identical retries, and requests agents give up on.
    pub flights: FlightsConfig,
    /// Toktail's API and MCP server for other apps (api.rs).
    pub api: ApiConfig,
    /// Keeping roc's opencode sessions working (autopilot.rs).
    pub autopilot: AutopilotConfig,
    /// Pip's one-line summary of every prompt (summaries.rs).
    pub summaries: SummariesConfig,
}

/// Pip's "what's this prompt?" sentences (see summaries.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SummariesConfig {
    /// Ask the same model for one sentence per prompt, when it's idle.
    pub enabled: bool,
    /// Pip says each new summary in its speech bubble (instead of its
    /// whimsical commentary).
    pub bubbles: bool,
    /// Prompts that may wait for a sentence; the oldest are dropped.
    pub max_queue: usize,
}

impl Default for SummariesConfig {
    fn default() -> Self {
        SummariesConfig {
            enabled: true,
            bubbles: true,
            max_queue: 200,
        }
    }
}

/// Autopilot: Toktail keeps an opencode session that roc runs working until
/// you stop it (see autopilot.rs). Everything it sends on your behalf is in
/// these templates, so you decide its words.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutopilotConfig {
    /// roc's state file (empty = ~/.local/roc/state.json).
    pub roc_state: String,
    /// The roc command, for "end the session" (empty = find it).
    pub roc_command: String,
    /// The docker command, for the verifier (empty = find it).
    pub docker_command: String,
    /// Seconds a session stays idle before autopilot answers it.
    pub grace_secs: u64,
    /// Minutes without a new message before autopilot acts anyway (a turn
    /// that never finished) or tells you a busy session looks stuck.
    pub silence_minutes: u64,
    /// Sent when the agent stops without claiming it's finished.
    pub continue_template: String,
    /// Sent when the agent says it's finished.
    pub done_template: String,
    /// A command run in the agent's container before each prompt (e.g. a
    /// script listing files that don't meet your skills); its output goes
    /// into the prompt as {{verifier}}. Empty = none.
    pub verifier_command: String,
    pub verifier_timeout_secs: u64,
    pub verifier_max_chars: usize,
    /// Prompts autopilot may send in one run (0 = until you stop it).
    pub max_turns: u32,
    /// Tokens (input + output, counted by Toktail for the session's model)
    /// one run may use (0 = no limit).
    pub token_ceiling: u64,
    /// Pause after this many identical replies in a row.
    pub repeat_limit: u32,
    /// Stop after this many failed turns in a row (model errors, empty
    /// replies), and say what to fix.
    pub error_limit: u32,
    /// Tell you the agent may be stuck after this many "I'm done" replies
    /// in a row (autopilot keeps going).
    pub done_claims_alert: u32,
    /// Answer an agent's question yourself within this many minutes, or
    /// autopilot sends the default answer (0 = always wait for you).
    pub question_timeout_minutes: u64,
    pub question_default_answer: String,
}

/// Autopilot's default words when the agent stops without saying it's done.
pub const DEFAULT_CONTINUE: &str = "Continue. Re-read AGENTS.md and PLAN.md, take the next unfinished item, apply the skills it requires, and verify it before you move on.{{goal}}{{verifier}}\n\nIf you are blocked and need the human, write NEED_HUMAN: and the reason.";
/// Autopilot's default words when the agent says it's done.
pub const DEFAULT_DONE: &str = "I hear that you believe this is finished. The human has reviewed the work and it isn't: the requirements in AGENTS.md, and the skills they name, are not yet applied everywhere they apply. Finishing one file is not finishing the task. Keep going: find the next file that doesn't meet them, bring it up to standard, verify it, and continue.{{goal}}{{verifier}}\n\nIf you are blocked and need the human, write NEED_HUMAN: and the reason.";

impl Default for AutopilotConfig {
    fn default() -> Self {
        AutopilotConfig {
            roc_state: String::new(),
            roc_command: String::new(),
            docker_command: String::new(),
            grace_secs: 60,
            silence_minutes: 10,
            continue_template: DEFAULT_CONTINUE.into(),
            done_template: DEFAULT_DONE.into(),
            verifier_command: String::new(),
            verifier_timeout_secs: 600,
            verifier_max_chars: 6000,
            max_turns: 0,
            token_ceiling: 0,
            repeat_limit: 3,
            error_limit: 3,
            done_claims_alert: 5,
            question_timeout_minutes: 15,
            question_default_answer:
                "Use your best judgment: follow AGENTS.md and the skills it names, and keep going."
                    .into(),
        }
    }
}

/// Toktail's public API (`/api/v1`) and MCP server (`/mcp`), on a port of its
/// own (see api.rs). Every request needs a key; keys are kept as hashes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    pub enabled: bool,
    /// The port; it listens on 127.0.0.1 unless `allow_network` is on.
    pub port: u16,
    /// Listen on every network interface so other machines (an Open WebUI
    /// server, say) can connect. Keys still decide what each one may do.
    pub allow_network: bool,
    /// Let keys with the `records` scope read whole prompts and responses
    /// even when saved records are encrypted. Without encryption they always
    /// can; with it, they get short snippets unless this is on.
    pub decrypted_bodies: bool,
    /// Issued keys (hashes only).
    pub keys: Vec<ApiKey>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            enabled: true,
            port: 8766,
            allow_network: false,
            decrypted_bodies: false,
            keys: vec![],
        }
    }
}

/// One key for the API and MCP server. Only its SHA-256 is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiKey {
    /// Short id (`tkm_` + first 8 hex of the hash) shown in Settings.
    pub id: String,
    /// SHA-256 of the full key, hex.
    pub hash: String,
    /// What it's for ("Open WebUI").
    pub label: String,
    pub created: String,
    /// `read` (usage, models, costs, budgets, live) and `records` (saved
    /// prompts, responses and session chains).
    pub scopes: Vec<String>,
    /// Usable from other machines. Off = only connections from this
    /// computer (which includes its Docker containers).
    pub network: bool,
}

impl Default for ApiKey {
    fn default() -> Self {
        ApiKey {
            id: String::new(),
            hash: String::new(),
            label: "Client".into(),
            created: String::new(),
            scopes: vec!["read".into()],
            network: false,
        }
    }
}

/// How Toktail handles requests that pile up (see flights.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlightsConfig {
    /// An identical request already running answers a new one (an agent's
    /// retry, say) instead of the model doing the work twice.
    pub join_duplicates: bool,
    /// When an agent hangs up on a request nobody else waits for, keep it
    /// running this long for a retry to join; 0 stops it at once.
    pub abandoned_grace_secs: u64,
}

impl Default for FlightsConfig {
    fn default() -> Self {
        FlightsConfig {
            join_duplicates: true,
            abandoned_grace_secs: 180,
        }
    }
}

/// Toktail's prompt cache (see cache.rs). Off until turned on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    /// How long an answer stays good, in hours.
    pub ttl_hours: u32,
    /// Disk the cache may use, in megabytes; the oldest entries go first.
    pub max_mb: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            enabled: false,
            ttl_hours: 72,
            max_mb: 2048,
        }
    }
}

/// Pip's commentary: one sentence from the same local model, at most once
/// every `every_minutes`, about what a coding agent just did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentaryConfig {
    pub enabled: bool,
    pub every_minutes: u32,
}

impl Default for CommentaryConfig {
    fn default() -> Self {
        CommentaryConfig {
            enabled: true,
            every_minutes: 5,
        }
    }
}

/// Where daily market prices come from (see market.rs). Keys, when a source
/// wants one, go to the secret store, never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarketConfig {
    pub enabled: bool,
    /// Which source to try first today; the other is the fallback. Alternates.
    pub next_source: String,
    pub litellm_url: String,
    pub openrouter_url: String,
}

impl Default for MarketConfig {
    fn default() -> Self {
        MarketConfig {
            enabled: true,
            next_source: "litellm".into(),
            litellm_url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json".into(),
            openrouter_url: "https://openrouter.ai/api/v1/models".into(),
        }
    }
}

/// One token budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Short id, unique among budgets.
    pub id: String,
    pub name: String,
    /// A host id, or empty for every host.
    pub host: String,
    /// A model name, or empty for every model.
    pub model: String,
    /// "combined", "input" or "output".
    pub metric: String,
    /// "day", "week" (Monday to Sunday) or "month".
    pub period: String,
    /// Tokens allowed per period.
    pub limit: u64,
    /// Send a notification at 80% and at 100%.
    pub notify: bool,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            id: String::new(),
            name: "Daily tokens".into(),
            host: String::new(),
            model: String::new(),
            metric: "combined".into(),
            period: "day".into(),
            limit: 10_000_000,
            notify: true,
        }
    }
}

/// When Toktail taps you on the shoulder (macOS notifications).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertsConfig {
    /// Budgets crossing 80% and 100% (per budget, see `notify`).
    pub budgets: bool,
    /// A host's LM Studio stops answering, and when it comes back.
    pub host_down: bool,
    /// A stampede starts (four or more requests at once, or three times your
    /// usual speed).
    pub stampede: bool,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig {
            budgets: true,
            host_down: true,
            stampede: false,
        }
    }
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
    /// "lmstudio" (the default: a local server whose token Toktail may hold),
    /// "anthropic", "openai" or "other" (a public API: the agent's own key
    /// passes through, or a gateway key stands in for a stored one).
    pub kind: String,
    /// Toktail-issued keys agents may use instead of the provider's.
    pub gateway_keys: Vec<crate::tokens::GatewayKey>,
    /// Add the stored provider key to requests from this machine that carry
    /// none. Never for requests from the network.
    pub inject_key: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            id: "local".into(),
            name: if cfg!(target_os = "macos") {
                "This Mac"
            } else {
                "This PC"
            }
            .into(),
            upstream: "http://127.0.0.1:1235".into(),
            listen: "127.0.0.1:1234".into(),
            enabled: true,
            kind: "lmstudio".into(),
            gateway_keys: Vec::new(),
            inject_key: true,
        }
    }
}

impl HostConfig {
    /// A public API (Anthropic, OpenAI, another hosted provider): keys pass
    /// through or come from a gateway key; Toktail never replaces them with
    /// a stored LM Studio token.
    pub fn is_public(&self) -> bool {
        self.kind != "lmstudio"
    }
    /// Which header the provider expects its key in.
    pub fn key_header(&self) -> &'static str {
        if self.kind == "anthropic" {
            "x-api-key"
        } else {
            "authorization"
        }
    }
    /// Listening beyond this machine.
    pub fn allows_lan(&self) -> bool {
        let bind = self.listen.rsplit_once(':').map(|(h, _)| h).unwrap_or("");
        matches!(bind, "0.0.0.0" | "::" | "[::]" | "")
    }
}

/// Host kinds Settings accepts.
pub const HOST_KINDS: [&str; 4] = ["lmstudio", "anthropic", "openai", "other"];

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
    /// Price the part of each prompt that repeats the previous turn at the
    /// provider's cached-input rate, the way an agent is really billed.
    pub cache_aware: bool,
    /// Use the daily market prices (market.rs) on top of the built-in list.
    pub market_prices: bool,
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
            cache_aware: false,
            market_prices: true,
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
    for h in &mut config.hosts {
        if !HOST_KINDS.contains(&h.kind.as_str()) {
            h.kind = "lmstudio".into();
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
