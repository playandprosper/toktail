// SPDX-License-Identifier: Apache-2.0
// A Windows tray app has no console window of its own; command-line use
// attaches to the terminal it was started from (see `attach_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Copyright 2026 Play and Prosper and the Toktail contributors.
//
//! Toktail — every token leaves a trail.
//!
//! Pip the Token Fox sits between your apps and LM Studio, counts every
//! token that passes, writes it to CSV, and draws it with its tail.
//!
//!   Toktail                  menu bar / system tray app (default)
//!   Toktail --headless       no menu bar; proxy, portal and S3 only
//!   Toktail --demo           sample data and pretend traffic, to see Pip in action
//!   Toktail --set-s3-secret  store the S3 secret key in the Keychain
//!   Toktail fingerprint      read a request JSON on stdin, print its prompt_sha256
//!   Toktail open [#/page]    open the portal, signed in (--print: just print the link)
//!   Toktail --version

mod api;
mod app;
mod autopilot;
mod budgets;
mod cache;
mod chain;
mod commentary;
mod config;
mod crypto;
mod demo;
mod flights;
mod hardware;
mod hoststatus;
mod intelligence;
mod ledger;
mod live;
mod login;
mod market;
mod opencode;
mod pip_agent;
mod platform;
mod portal;
mod portal_agents;
mod portal_auth;
mod portal_insights;
mod portal_records;
mod portal_settings;
mod pricing;
mod proxy;
mod records;
mod retrieval;
mod roc;
mod s3sync;
mod secrets;
mod stats;
mod summaries;
#[cfg(test)]
mod test_support;
mod threads;
mod tokens;
#[cfg(feature = "desktop")]
mod tray;
mod usage;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::io::Read;
use std::sync::Arc;

fn main() {
    attach_console();
    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    logger.format_timestamp_secs();
    // Started at login on Windows or Linux there's no terminal: log to a file
    // (macOS's LaunchAgent already sends output to ~/Library/Logs/Toktail).
    if !cfg!(target_os = "macos") && !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        if let Some(path) = login::log_file() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                logger.target(env_logger::Target::Pipe(Box::new(f)));
            }
        }
    }
    logger.init();
    if let Err(e) = real_main() {
        log::error!("{e:#}");
        eprintln!("Toktail: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--version") || has("-V") {
        println!("Toktail {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if has("--help") || has("-h") {
        println!(
            "Toktail {} — every token leaves a trail.\n\n  Toktail                  menu bar / system tray app\n  Toktail --headless       run without the menu bar (servers, Linux)\n  Toktail --demo           try it with sample data and pretend traffic\n  Toktail --set-s3-secret  store the S3 secret key in the Keychain / Credential Manager\n  Toktail fingerprint      print prompt_sha256 for a request JSON on stdin\n  Toktail open [#/page]    open the portal signed in (--print shows the one-time link)\n\nConfig: {}\nHome:   {}",
            env!("CARGO_PKG_VERSION"),
            config::base_dir().join("config.toml").display(),
            portal::REPOSITORY
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

    if args.first().map(String::as_str) == Some("open") {
        return open_portal(&args[1..]);
    }

    let demo = has("--demo");
    if demo {
        let dir = demo::demo_dir();
        demo::prepare(&dir)?;
        log::info!("demo mode: sample data in {}", dir.display());
        config::set_base_dir(dir);
    }

    let (mut cfg, paths, first_run) = config::load()?;
    if demo && cfg.api.port == crate::config::ApiConfig::default().port {
        // Next to a real Toktail, the demo's API gets a port of its own.
        cfg.api.port = demo::API_PORT;
    }

    if has("--set-s3-secret") {
        return secrets::prompt_and_store(&cfg.s3.access_key_id);
    }
    let headless = has("--headless") || !cfg!(feature = "desktop");

    // One instance only. If the portal port answers as Toktail, a copy is
    // already running: open its portal and leave.
    let portal_addr = format!("127.0.0.1:{}", cfg.portal.port);
    let portal_listener = match std::net::TcpListener::bind(&portal_addr) {
        Ok(l) => l,
        Err(e) => {
            if already_running(&portal_addr) {
                log::info!("Toktail is already running; opening its portal");
                if !headless {
                    match portal_auth::login_link(&paths.base, cfg.portal.port, "") {
                        Ok(link) => {
                            let _ = open_url(&link);
                        }
                        Err(e) => log::warn!("could not make a sign-in link: {e}"),
                    }
                }
                return Ok(());
            }
            return Err(e).with_context(|| {
                format!("portal port {portal_addr} is in use by another program")
            });
        }
    };

    // macOS and Windows start the menu bar / tray app at login by default;
    // on Linux only when the desktop app runs (servers use systemd instead).
    let login_here = cfg!(any(target_os = "macos", windows)) || !headless;
    if !demo && login_here && (first_run || cfg.app.launch_at_login) {
        if let Err(e) = login::set_enabled(cfg.app.launch_at_login) {
            log::warn!("launch at login: {e:#}");
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("toktail")
        .build()?;

    secrets::set_base(paths.base.clone());
    if demo {
        secrets::set_namespace(" (demo)");
        if secrets::get_encryption_password().is_none() {
            secrets::set_encryption_password(Some(demo::PASSWORD))?;
        }
    }
    let default_host = cfg.default_host_id();
    secrets::migrate_single_token(&default_host);
    let ledger = ledger::Ledger::open(paths.data_dir.clone(), &default_host)?;
    let paths_base = paths.base.clone();
    let config_market_prices = cfg.costs.market_prices;
    log::info!("CSV ledger: {}", paths.data_dir.display());
    let app = Arc::new(app::App {
        config: RwLock::new(cfg),
        paths,
        ledger,
        status: RwLock::new(app::Status::default()),
        sync_now: tokio::sync::Notify::new(),
        rt: runtime.handle().clone(),
        live: live::Live::new(),
        prices: RwLock::new(market::book_with_market(&paths_base, config_market_prices)),
        demo,
        host_live: RwLock::new(Default::default()),
        host_tokens: RwLock::new(Default::default()),
        proxy_restart: tokio::sync::Notify::new(),
        check_upstream: tokio::sync::Notify::new(),
        crypto: crypto::Crypto::new(),
        records: records::Records::new(),
        hardware: RwLock::new(hardware::Fleet::load(&paths_base)),
        activity: live::Activity::new(),
        alerts: budgets::Alerts::new(),
        retrieval: retrieval::Index::new(),
        chat_client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()?,
        shutting_down: std::sync::atomic::AtomicBool::new(false),
        cache: cache::PromptCache::new(),
        commentary: commentary::State::new(),
        chains: chain::Chains::new(),
        portal_auth: portal_auth::PortalAuth::new(&paths_base),
        api: api::ApiState::new(),
        autopilot: autopilot::Autopilot::new(),
        summaries: summaries::Summaries::new(),
        host_status: hoststatus::HostStatus::new(),
    });
    app.load_host_tokens();
    app.configure_crypto();
    if let Some(p) = app.crypto.problem() {
        log::warn!("encryption: {p}");
    }
    {
        let _guard = runtime.enter();
        records::start(app.clone());
        if demo {
            demo::seed_captures(&app);
        }
    }

    runtime.spawn(proxy::run(app.clone()));
    runtime.spawn(portal::serve(app.clone(), portal_listener));
    runtime.spawn(api::serve(app.clone()));
    runtime.spawn(autopilot::run(app.clone()));
    runtime.spawn(summaries::run(app.clone()));
    runtime.spawn(hoststatus::run(app.clone()));
    runtime.spawn(s3sync::run(app.clone()));
    runtime.spawn(budgets::run(app.clone()));
    runtime.spawn(cache::run(app.clone()));
    runtime.spawn(market::run(app.clone()));

    if demo {
        runtime.spawn(demo::fake_lmstudios());
        runtime.spawn(demo::traffic());
        println!(
            "\nToktail demo: open {} (Ctrl-C to stop)\n",
            app.portal_link("")
        );
    }
    if (first_run || demo) && !headless {
        let _ = open_url(&app.portal_link(""));
    }

    if headless {
        runtime.block_on(async {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("install SIGTERM handler");
                tokio::select! { _=tokio::signal::ctrl_c()=>{},_=term.recv()=>{} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            log::info!("shutting down");
            records::shutdown(&app).await;
            s3sync::final_sync(&app).await;
        });
        return Ok(());
    }
    #[cfg(feature = "desktop")]
    tray::run(app);
    #[cfg(not(feature = "desktop"))]
    Ok(())
}

/// `Toktail open [#/page] [--print]`: a one-time signed-in link to the
/// running Toktail's portal, opened in the browser (or printed).
fn open_portal(args: &[String]) -> Result<()> {
    let (cfg, paths, _) = config::load()?;
    let to = args
        .iter()
        .find(|a| a.starts_with("#/"))
        .map(String::as_str)
        .unwrap_or("");
    let link = portal_auth::login_link(&paths.base, cfg.portal.port, to)
        .context("could not write a sign-in token to Toktail's folder")?;
    let addr = format!("127.0.0.1:{}", cfg.portal.port);
    if !already_running(&addr) {
        eprintln!("Toktail isn't running on {addr}; the link works once it is (for 2 minutes).");
    }
    if args.iter().any(|a| a == "--print") {
        println!("{link}");
        return Ok(());
    }
    open_url(&link).context("could not open the browser")?;
    Ok(())
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
    buf.contains("\"toktail\"")
}

fn open_url(url: &str) -> std::io::Result<std::process::Child> {
    platform::open(url)
}

/// On Windows, print to the terminal Toktail was started from (for
/// `--help`, `fingerprint`, `--set-s3-secret`, `--headless`). Elsewhere a
/// no-op.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
