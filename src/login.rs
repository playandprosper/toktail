// SPDX-License-Identifier: Apache-2.0
//! Start Toktail when you log in:
//! - macOS: a per-user LaunchAgent (~/Library/LaunchAgents/com.playandprosper.toktail.plist)
//! - Windows: the per-user Run key (HKCU\Software\Microsoft\Windows\CurrentVersion\Run)
//! - Linux: an XDG autostart entry (~/.config/autostart/toktail.desktop)

use anyhow::{Context, Result};
use std::path::PathBuf;

pub const LABEL: &str = "com.playandprosper.toktail";
/// The first release's agent; removed so the app never starts twice.
const LEGACY_LABEL: &str = "com.tokenledger.app";
#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn agent_path(label: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join("Library/LaunchAgents")
            .join(format!("{label}.plist"))
    })
}

fn autostart_path() -> Option<PathBuf> {
    dirs::config_dir().map(|c| c.join("autostart").join("toktail.desktop"))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Where the log goes when Toktail starts at login.
pub fn log_file() -> Option<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        dirs::home_dir()?.join("Library/Logs/Toktail")
    } else {
        crate::config::base_dir().join("logs")
    };
    Some(dir.join("toktail.log"))
}

fn plist_contents() -> Result<String> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let log = log_file().context("no home directory")?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).ok();
    }
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

fn desktop_entry() -> Result<String> {
    let exe = std::env::current_exe()?.canonicalize()?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Toktail\nComment=Every token leaves a trail.\nExec=\"{}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    ))
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn is_enabled() -> bool {
    if cfg!(target_os = "macos") {
        agent_path(LABEL).is_some_and(|p| p.exists())
    } else if cfg!(windows) {
        windows_run_value().is_some()
    } else {
        autostart_path().is_some_and(|p| p.exists())
    }
}

#[cfg(windows)]
fn windows_run_value() -> Option<String> {
    let out = crate::platform::quiet("reg")
        .args(["query", RUN_KEY, "/v", "Toktail"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(not(windows))]
fn windows_run_value() -> Option<String> {
    None
}

/// Turns starting at login on or off. Rewrites the entry if the app moved.
pub fn set_enabled(enabled: bool) -> Result<()> {
    if cfg!(target_os = "macos") {
        if let Some(old) = agent_path(LEGACY_LABEL).filter(|p| p.exists()) {
            let _ = std::fs::remove_file(&old);
        }
        let path = agent_path(LABEL).context("no home directory")?;
        write_or_remove(&path, enabled, plist_contents)
    } else if cfg!(windows) {
        set_windows(enabled)
    } else {
        let path = autostart_path().context("no config directory")?;
        write_or_remove(&path, enabled, desktop_entry)
    }
}

fn write_or_remove(
    path: &PathBuf,
    enabled: bool,
    contents: impl Fn() -> Result<String>,
) -> Result<()> {
    if enabled {
        let want = contents()?;
        if std::fs::read_to_string(path).ok().as_deref() != Some(want.as_str()) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, want).with_context(|| format!("writing {}", path.display()))?;
            log::info!("launch at login enabled ({})", path.display());
        }
    } else if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        log::info!("launch at login disabled");
    }
    Ok(())
}

#[cfg(windows)]
fn set_windows(enabled: bool) -> Result<()> {
    if enabled {
        let exe = std::env::current_exe()?;
        let value = format!("\"{}\"", exe.display());
        if windows_run_value().is_some_and(|v| v.contains(&value)) {
            return Ok(());
        }
        let ok = crate::platform::quiet("reg")
            .args([
                "add", RUN_KEY, "/v", "Toktail", "/t", "REG_SZ", "/d", &value, "/f",
            ])
            .status()?
            .success();
        anyhow::ensure!(ok, "couldn't add Toktail to the Run key");
        log::info!("launch at login enabled (Run key)");
    } else if windows_run_value().is_some() {
        let _ = crate::platform::quiet("reg")
            .args(["delete", RUN_KEY, "/v", "Toktail", "/f"])
            .status();
        log::info!("launch at login disabled");
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_windows(_enabled: bool) -> Result<()> {
    Ok(())
}
