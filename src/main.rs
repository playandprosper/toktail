//! TokenLedger — counts the tokens your LM Studio host is rendering.
//!
//!   TokenLedger                  menu bar app (default)
//!   TokenLedger --headless       no menu bar; proxy, portal and S3 only
//!   TokenLedger --set-s3-secret  store the S3 secret key in the Keychain
//!   TokenLedger fingerprint      read a request JSON on stdin, print its prompt_sha256
//!   TokenLedger --version

mod app;
mod config;
mod ledger;
mod login;
mod portal;
mod proxy;
mod s3sync;
mod secrets;
mod tray;
mod usage;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::io::Read;
use std::sync::Arc;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();
    if let Err(e) = real_main() {
        log::error!("{e:#}");
        eprintln!("TokenLedger: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--version") || has("-V") {
        println!("TokenLedger {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if has("--help") || has("-h") {
        println!(
            "TokenLedger {}\n\n  TokenLedger                  menu bar app\n  TokenLedger --headless       run without the menu bar\n  TokenLedger --set-s3-secret  store the S3 secret key in the Keychain\n  TokenLedger fingerprint      print prompt_sha256 for a request JSON on stdin\n\nConfig: {}",
            env!("CARGO_PKG_VERSION"),
            config::base_dir().join("config.toml").display()
        );
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("fingerprint") {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        let v: serde_json::Value = serde_json::from_str(&s).context("stdin is not JSON")?;
        match usage::prompt_fingerprint(&v) {
            Some(fp) => println!("{fp}"),
            None => anyhow::bail!(
                "no prompt fields ({}) in that JSON",
                usage::PROMPT_FIELDS.join(", ")
            ),
        }
        return Ok(());
    }

    let (cfg, paths, first_run) = config::load()?;

    if has("--set-s3-secret") {
        return secrets::prompt_and_store(&cfg.s3.access_key_id);
    }
    let headless = has("--headless");

    // One instance only. If the portal port answers as TokenLedger, a copy is
    // already running: open its portal and leave.
    let portal_addr = format!("127.0.0.1:{}", cfg.portal.port);
    let portal_listener = match std::net::TcpListener::bind(&portal_addr) {
        Ok(l) => l,
        Err(e) => {
            if already_running(&portal_addr) {
                log::info!("TokenLedger is already running; opening its portal");
                if !headless {
                    let _ = open_url(&format!("http://{portal_addr}/"));
                }
                return Ok(());
            }
            return Err(e).with_context(|| {
                format!("portal port {portal_addr} is in use by another program")
            });
        }
    };

    if cfg!(target_os = "macos") && (first_run || cfg.app.launch_at_login) {
        if let Err(e) = login::set_enabled(cfg.app.launch_at_login) {
            log::warn!("launch at login: {e:#}");
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("tokenledger")
        .build()?;

    let ledger = ledger::Ledger::open(paths.data_dir.clone())?;
    log::info!("CSV ledger: {}", paths.data_dir.display());
    let app = Arc::new(app::App {
        config: RwLock::new(cfg),
        paths,
        ledger,
        status: RwLock::new(app::Status::default()),
        sync_now: tokio::sync::Notify::new(),
        rt: runtime.handle().clone(),
    });

    runtime.spawn(proxy::run(app.clone()));
    runtime.spawn(portal::serve(app.clone(), portal_listener));
    runtime.spawn(s3sync::run(app.clone()));

    if first_run && !headless {
        let _ = open_url(&app.portal_url());
    }

    if headless {
        runtime.block_on(async {
            let _ = tokio::signal::ctrl_c().await;
            log::info!("shutting down");
            s3sync::final_sync(&app).await;
        });
        return Ok(());
    }
    tray::run(app)
}

fn already_running(addr: &str) -> bool {
    use std::io::Write;
    use std::time::Duration;
    let Ok(sock) = addr.parse() else { return false };
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&sock, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let req = format!("GET /api/ping HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.contains("\"tokenledger\"")
}

fn open_url(url: &str) -> std::io::Result<std::process::Child> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(cmd).arg(url).spawn()
}
