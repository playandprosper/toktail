// SPDX-License-Identifier: Apache-2.0
//! Secrets live in the macOS Keychain, never in config.toml: the S3 secret
//! access key, each host's LM Studio API token, the encryption password and
//! key-store credentials. Off macOS (development, CI) they go in files only
//! the current user can read. Environment variables override for testing.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::OnceLock;

pub const KEYCHAIN_SERVICE: &str = "Toktail S3";
/// Where the first release (TokenLedger) kept the key; read as a fallback.
const LEGACY_KEYCHAIN_SERVICE: &str = "TokenLedger S3";
pub const ENV_SECRET: &str = "TOKTAIL_S3_SECRET_KEY";
pub const ENV_LMSTUDIO_TOKEN: &str = "TOKTAIL_LMSTUDIO_TOKEN";
pub const ENV_ENCRYPTION_PASSWORD: &str = "TOKTAIL_ENCRYPTION_PASSWORD";

const LMSTUDIO_SERVICE: &str = "Toktail LM Studio";
const SECRETS_SERVICE: &str = "Toktail";

static BASE: OnceLock<PathBuf> = OnceLock::new();
static SUFFIX: OnceLock<String> = OnceLock::new();

/// Where file-based secrets go off macOS.
pub fn set_base(base: PathBuf) {
    let _ = BASE.set(base);
}

/// Keeps the demo's secrets apart from your real ones.
pub fn set_namespace(suffix: &str) {
    let _ = SUFFIX.set(suffix.to_string());
}

fn service(name: &str) -> String {
    format!("{name}{}", SUFFIX.get().map(String::as_str).unwrap_or(""))
}

// ------------------------------------------------------------ storage backends

#[cfg(target_os = "macos")]
fn store_get(svc: &str, account: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(svc, account)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "macos")]
fn store_set(svc: &str, account: &str, value: Option<&str>) -> Result<()> {
    let _ = security_framework::passwords::delete_generic_password(svc, account);
    if let Some(v) = value {
        security_framework::passwords::set_generic_password(svc, account, v.as_bytes())?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn secret_file(svc: &str, account: &str) -> PathBuf {
    let safe = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    BASE.get()
        .cloned()
        .unwrap_or_else(std::env::temp_dir)
        .join("secrets")
        .join(format!("{}--{}", safe(svc), safe(account)))
}

#[cfg(not(target_os = "macos"))]
fn store_get(svc: &str, account: &str) -> Option<String> {
    std::fs::read_to_string(secret_file(svc, account))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn store_set(svc: &str, account: &str, value: Option<&str>) -> Result<()> {
    let path = secret_file(svc, account);
    match value {
        Some(v) => {
            use std::io::Write;
            #[cfg(unix)]
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            o.mode(0o600);
            o.open(&path)?.write_all(v.as_bytes())?;
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

// ------------------------------------------------------------ S3

pub fn get_s3_secret(access_key_id: &str) -> Option<String> {
    env(ENV_SECRET)
        .or_else(|| store_get(&service(KEYCHAIN_SERVICE), access_key_id))
        .or_else(|| store_get(LEGACY_KEYCHAIN_SERVICE, access_key_id))
}

pub fn set_s3_secret(account: &str, secret: Option<&str>) -> Result<()> {
    store_set(&service(KEYCHAIN_SERVICE), account, secret)
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
    set_s3_secret(access_key_id.trim(), Some(secret))?;
    println!("Saved to the Keychain under \"{KEYCHAIN_SERVICE}\" / {access_key_id}.");
    Ok(())
}

// ------------------------------------------------------------ LM Studio tokens, one per host

pub fn get_host_token(host_id: &str) -> Option<String> {
    if let Some(t) = env(&format!(
        "{ENV_LMSTUDIO_TOKEN}_{}",
        host_id.to_uppercase().replace('-', "_")
    )) {
        return Some(t);
    }
    store_get(&service(LMSTUDIO_SERVICE), &format!("api-token:{host_id}"))
}

pub fn set_host_token(host_id: &str, token: Option<&str>) -> Result<()> {
    store_set(
        &service(LMSTUDIO_SERVICE),
        &format!("api-token:{host_id}"),
        token,
    )
}

/// Toktail 0.3 kept one token; it moves to the first host once.
pub fn migrate_single_token(first_host: &str) {
    let legacy =
        env(ENV_LMSTUDIO_TOKEN).or_else(|| store_get(&service(LMSTUDIO_SERVICE), "api-token"));
    #[cfg(not(target_os = "macos"))]
    let legacy = legacy.or_else(|| {
        BASE.get()
            .and_then(|b| std::fs::read_to_string(b.join("lmstudio-token")).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    });
    if let Some(t) = legacy {
        if get_host_token(first_host).is_none() && set_host_token(first_host, Some(&t)).is_ok() {
            let _ = store_set(&service(LMSTUDIO_SERVICE), "api-token", None);
            log::info!("moved the LM Studio token to host {first_host}");
        }
    }
}

// ------------------------------------------------------------ encryption and key stores

pub fn get_encryption_password() -> Option<String> {
    env(ENV_ENCRYPTION_PASSWORD)
        .or_else(|| store_get(&service(SECRETS_SERVICE), "encryption-password"))
}

pub fn set_encryption_password(pw: Option<&str>) -> Result<()> {
    store_set(&service(SECRETS_SERVICE), "encryption-password", pw)
}

/// "1password-token", "vault-token".
pub fn get_named_secret(name: &str) -> Option<String> {
    store_get(&service(SECRETS_SERVICE), name)
}

pub fn set_named_secret(name: &str, value: Option<&str>) -> Result<()> {
    store_set(&service(SECRETS_SERVICE), name, value)
}

/// "••••abcd": enough to recognise a saved secret without showing it.
pub fn token_hint(token: &str) -> String {
    let n = token.chars().count();
    if n <= 8 {
        "••••".into()
    } else {
        format!("••••{}", token.chars().skip(n - 4).collect::<String>())
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    #[test]
    fn file_secrets_roundtrip() {
        set_base(std::env::temp_dir().join(format!("tl-sec-{}", uuid::Uuid::new_v4())));
        set_host_token("mini-1", Some("abc")).unwrap();
        assert_eq!(get_host_token("mini-1").as_deref(), Some("abc"));
        set_host_token("mini-1", None).unwrap();
        assert!(get_host_token("mini-1").is_none());
        assert_eq!(token_hint("0123456789"), "••••6789");
    }
}
