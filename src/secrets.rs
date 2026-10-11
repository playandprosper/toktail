// SPDX-License-Identifier: Apache-2.0
//! The S3 secret access key lives in the macOS Keychain, never in config.toml.
//! `TOKTAIL_S3_SECRET_KEY` in the environment overrides it (useful for testing).

use anyhow::Result;

pub const KEYCHAIN_SERVICE: &str = "Toktail S3";
/// Where the first release (TokenLedger) kept the key; read as a fallback.
const LEGACY_KEYCHAIN_SERVICE: &str = "TokenLedger S3";
pub const ENV_SECRET: &str = "TOKTAIL_S3_SECRET_KEY";

pub fn get_s3_secret(access_key_id: &str) -> Option<String> {
    if let Ok(s) = std::env::var(ENV_SECRET) {
        if !s.is_empty() {
            return Some(s);
        }
    }
    keychain_get(KEYCHAIN_SERVICE, access_key_id)
        .or_else(|| keychain_get(LEGACY_KEYCHAIN_SERVICE, access_key_id))
}

#[cfg(target_os = "macos")]
fn keychain_get(service: &str, account: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(service, account)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

#[cfg(not(target_os = "macos"))]
fn keychain_get(_service: &str, _account: &str) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub fn set_s3_secret(account: &str, secret: &str) -> Result<()> {
    let _ = security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, account);
    security_framework::passwords::set_generic_password(
        KEYCHAIN_SERVICE,
        account,
        secret.as_bytes(),
    )?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set_s3_secret(_account: &str, _secret: &str) -> Result<()> {
    anyhow::bail!("the Keychain is only available on macOS; set {ENV_SECRET} instead")
}

/// Interactive `--set-s3-secret`: reads the key from the terminal without echo.
pub fn prompt_and_store(access_key_id: &str) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    if access_key_id.trim().is_empty() {
        anyhow::bail!("set [s3] access_key_id in config.toml first");
    }
    let tty = std::io::stdin().is_terminal();
    if tty {
        print!("S3 secret access key for {access_key_id}: ");
        std::io::stdout().flush()?;
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if tty {
        let _ = std::process::Command::new("stty").arg("echo").status();
        println!();
    }
    read?;
    let secret = line.trim();
    if secret.is_empty() {
        anyhow::bail!("no secret entered");
    }
    set_s3_secret(access_key_id.trim(), secret)?;
    println!("Saved to the Keychain under \"{KEYCHAIN_SERVICE}\" / {access_key_id}.");
    Ok(())
}
