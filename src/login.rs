//! Launch at login via a per-user LaunchAgent
//! (~/Library/LaunchAgents/com.tokenledger.app.plist).

use anyhow::{Context, Result};
use std::path::PathBuf;

pub const LABEL: &str = "com.tokenledger.app";

fn plist_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn plist_contents() -> Result<String> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let home = dirs::home_dir().context("no home directory")?;
    let log_dir = home.join("Library/Logs/TokenLedger");
    std::fs::create_dir_all(&log_dir).ok();
    let log = log_dir.join("tokenledger.log");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        exe = xml_escape(&exe.display().to_string()),
        log = xml_escape(&log.display().to_string()),
    ))
}

pub fn is_enabled() -> bool {
    plist_path().is_some_and(|p| p.exists())
}

/// Writes or removes the LaunchAgent. Rewrites it if the app moved.
pub fn set_enabled(enabled: bool) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let path = plist_path().context("no home directory")?;
    if enabled {
        let want = plist_contents()?;
        if std::fs::read_to_string(&path).ok().as_deref() != Some(want.as_str()) {
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, want).with_context(|| format!("writing {}", path.display()))?;
            log::info!("launch at login enabled ({})", path.display());
        }
    } else if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        log::info!("launch at login disabled");
    }
    Ok(())
}
