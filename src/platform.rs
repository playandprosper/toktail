// SPDX-License-Identifier: Apache-2.0
//! The few things that differ between macOS, Windows and Linux: opening a
//! page or a folder, the folder picker, notifications, and where secrets go.

use std::process::Command;

/// A command that doesn't flash a console window on Windows.
pub fn quiet(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Opens a URL, a folder or a file with the system's default app.
pub fn open(target: &str) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        Command::new("open").arg(target).spawn()
    } else if cfg!(windows) {
        if target.starts_with("http://") || target.starts_with("https://") {
            // `start` needs an empty title argument before the target.
            quiet("cmd").args(["/C", "start", "", target]).spawn()
        } else {
            Command::new("explorer").arg(target).spawn()
        }
    } else {
        Command::new("xdg-open").arg(target).spawn()
    }
}

/// Opens a text file in a text editor.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
pub fn open_text(path: &str) -> std::io::Result<std::process::Child> {
    if cfg!(target_os = "macos") {
        Command::new("open").args(["-t", path]).spawn()
    } else if cfg!(windows) {
        Command::new("notepad").arg(path).spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    }
}

/// Where secrets are kept, in words for the Settings page.
pub fn secret_store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "secrets folder (readable only by you)"
    }
}

/// The system's folder picker. `Ok(None)` when it was cancelled, `Err` when
/// there is no picker here (the page then asks for a typed path).
pub async fn pick_folder(prompt: &str) -> Result<Option<String>, String> {
    let out = if cfg!(target_os = "macos") {
        tokio::process::Command::new("osascript")
            .args([
                "-e",
                &format!(
                    "POSIX path of (choose folder with prompt \"{}\")",
                    prompt.replace('"', "'")
                ),
            ])
            .output()
            .await
    } else if cfg!(windows) {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = '{}'; $d.ShowNewFolderButton = $true; if ($d.ShowDialog() -eq 'OK') {{ $d.SelectedPath }}",
            prompt.replace('\'', "''")
        );
        let mut c = tokio::process::Command::new("powershell");
        c.args(["-NoProfile", "-STA", "-Command", &script]);
        #[cfg(windows)]
        c.creation_flags(0x0800_0000);
        c.output().await
    } else {
        tokio::process::Command::new("zenity")
            .args(["--file-selection", "--directory", "--title", prompt])
            .output()
            .await
    };
    match out {
        Ok(o) if o.status.success() => {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let p = if p.len() > 1 {
                p.trim_end_matches(['/', '\\']).to_string()
            } else {
                p
            };
            Ok((!p.is_empty()).then_some(p))
        }
        Ok(_) => Ok(None),
        Err(e) => Err(format!(
            "No folder picker here ({e}). Type the folder path instead."
        )),
    }
}

/// A desktop notification. Best effort: failures are only logged.
pub fn notify(title: &str, message: &str) {
    let r = if cfg!(target_os = "macos") {
        // AppleScript strings: escape backslashes and quotes.
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        Command::new("osascript")
            .args([
                "-e",
                &format!(
                    "display notification \"{}\" with title \"Toktail\" subtitle \"{}\"",
                    q(message),
                    q(title)
                ),
            ])
            .spawn()
    } else if cfg!(windows) {
        // A balloon from a short-lived tray icon: works on Windows 10 and 11
        // with nothing to install.
        let q = |s: &str| s.replace('\'', "''");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
             $n = New-Object System.Windows.Forms.NotifyIcon; $n.Icon = [System.Drawing.SystemIcons]::Information; \
             $n.BalloonTipTitle = 'Toktail: {}'; $n.BalloonTipText = '{}'; $n.Visible = $true; \
             $n.ShowBalloonTip(8000); Start-Sleep -Seconds 9; $n.Dispose()",
            q(title),
            q(message)
        );
        quiet("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
            .spawn()
    } else {
        Command::new("notify-send")
            .args(["--app-name=Toktail", &format!("Toktail: {title}"), message])
            .spawn()
    };
    if let Err(e) = r {
        log::debug!("notification not shown: {e}");
    }
}
