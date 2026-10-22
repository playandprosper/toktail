// SPDX-License-Identifier: Apache-2.0
//! Signing in to the portal.
//!
//! The portal listens on 127.0.0.1 only and checks the Host and Origin
//! headers, which keeps web pages out. It does not keep out programs on this
//! computer, and that includes coding agents in Docker containers: Docker
//! Desktop forwards `host.docker.internal` to this computer's loopback, and
//! a program can send any Host header it likes. So every page and API call
//! also needs a sign-in cookie that only a browser you opened holds.
//!
//! How a browser gets the cookie:
//!
//! 1. Toktail (the menu bar, the first run) or `Toktail open` writes a
//!    one-time login token into `<Toktail folder>/portal-login/`, a folder
//!    only you can read, and opens `/login?t=<token>`.
//! 2. `/login` checks the token (it must be under two minutes old and is
//!    deleted on use) and answers with an `HttpOnly`, `SameSite=Strict`
//!    cookie good for 30 days, then sends the browser to the page.
//!
//! A container never sees the folder, so it can't make a token, and it never
//! sees the cookie. Only SHA-256 hashes of tokens and cookies are written
//! down. A few read-only endpoints stay open without the cookie because
//! tools rely on them and they show counts, never prompts: `/api/ping`
//! (is Toktail running), `/api/status` (`make status`: Pip's mood and
//! tokens/sec) and `/metrics` (Prometheus).

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The cookie's name.
pub const COOKIE: &str = "toktail_portal";
/// A login link works for this long, once.
const LOGIN_TTL_SECS: u64 = 120;
/// A signed-in browser stays signed in for this long.
const SESSION_SECS: u64 = 30 * 24 * 3600;
/// Paths that answer without signing in (read-only counts, no prompts).
const OPEN_PATHS: [&str; 4] = ["/api/ping", "/api/status", "/metrics", "/login"];

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn random_token() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    format!("{}{}", a.simple(), b.simple())
}

/// Writes a file only this user can read.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// Where one-time login tokens are dropped (by the app or `Toktail open`).
pub fn login_dir(base: &Path) -> PathBuf {
    base.join("portal-login")
}

/// Makes a one-time login token and returns it. Any process that can write
/// to Toktail's folder can do this; the running Toktail redeems it.
pub fn issue_login(base: &Path) -> std::io::Result<String> {
    let token = random_token();
    let path = login_dir(base).join(sha_hex(&token));
    write_private(&path, now().to_string().as_bytes())?;
    Ok(token)
}

/// The portal address, signed in, opened at `to` (`#/settings`, or empty).
pub fn login_link(base: &Path, port: u16, to: &str) -> std::io::Result<String> {
    let token = issue_login(base)?;
    let mut url = format!("http://127.0.0.1:{port}/login?t={token}");
    let to = to.trim();
    if to.starts_with("#/") {
        url.push_str("&to=");
        url.push_str(&url::form_urlencoded::byte_serialize(to.as_bytes()).collect::<String>());
    }
    Ok(url)
}

/// Is this path served without signing in?
pub fn is_open_path(path: &str) -> bool {
    OPEN_PATHS.contains(&path)
}

/// Signed-in browsers, by the SHA-256 of their cookie.
pub struct PortalAuth {
    base: PathBuf,
    sessions: Mutex<HashMap<String, u64>>,
}

impl PortalAuth {
    pub fn new(base: &Path) -> PortalAuth {
        let path = base.join("portal-sessions.json");
        let mut sessions: HashMap<String, u64> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let t = now();
        sessions.retain(|_, exp| *exp > t);
        PortalAuth {
            base: base.to_path_buf(),
            sessions: Mutex::new(sessions),
        }
    }

    fn sessions_file(&self) -> PathBuf {
        self.base.join("portal-sessions.json")
    }

    fn save(&self, sessions: &HashMap<String, u64>) {
        let bytes = serde_json::to_vec(sessions).unwrap_or_default();
        if let Err(e) = write_private(&self.sessions_file(), &bytes) {
            log::warn!("could not save portal sign-ins: {e}");
        }
    }

    /// A signed-in link for this running Toktail, opened at `to`.
    pub fn link(&self, port: u16, to: &str) -> String {
        login_link(&self.base, port, to).unwrap_or_else(|e| {
            log::warn!("could not make a sign-in link: {e}");
            format!("http://127.0.0.1:{port}/")
        })
    }

    /// Uses up a one-time login token. True when it was valid.
    pub fn redeem(&self, token: &str) -> bool {
        let token = token.trim();
        if token.len() != 64 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        let dir = login_dir(&self.base);
        let path = dir.join(sha_hex(token));
        let made = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let _ = std::fs::remove_file(&path);
        // Tidy up tokens nobody used.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let t = now();
            for e in rd.flatten() {
                let stale = std::fs::read_to_string(e.path())
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .is_none_or(|m| t.saturating_sub(m) > LOGIN_TTL_SECS);
                if stale {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        made.is_some_and(|m| now().saturating_sub(m) <= LOGIN_TTL_SECS)
    }

    /// Starts a signed-in browser session; returns the cookie value.
    pub fn start_session(&self) -> String {
        let token = random_token();
        let mut s = self.sessions.lock();
        let t = now();
        s.retain(|_, exp| *exp > t);
        s.insert(sha_hex(&token), t + SESSION_SECS);
        self.save(&s);
        token
    }

    /// The `Set-Cookie` value for a new session.
    pub fn cookie_header(value: &str) -> String {
        format!("{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_SECS}")
    }

    /// Does this Cookie header carry a valid sign-in?
    pub fn check(&self, cookie_header: Option<&str>) -> bool {
        let Some(value) = cookie_header.and_then(cookie_value) else {
            return false;
        };
        let h = sha_hex(value);
        self.sessions.lock().get(&h).is_some_and(|exp| *exp > now())
    }

    /// Signs every browser out (Settings → Sign out everywhere).
    pub fn sign_out_all(&self) {
        let mut s = self.sessions.lock();
        s.clear();
        self.save(&s);
    }
}

/// Our cookie's value from a `Cookie:` header.
fn cookie_value(header: &str) -> Option<&str> {
    header
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// What a browser without the cookie sees at `/`.
pub const SIGNED_OUT_HTML: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Toktail</title>
<style>
:root{--bg:#f7f5f2;--fg:#1d1b19;--muted:#6b645c;--card:#fff;--line:#e4dfd8}
@media (prefers-color-scheme:dark){:root{--bg:#141210;--fg:#f1ece6;--muted:#a59d93;--card:#1d1a17;--line:#2f2a25}}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
main{max-width:30rem;margin:1rem;padding:2rem;background:var(--card);border:1px solid var(--line);border-radius:14px}
h1{font-size:1.3rem;margin:0 0 .5rem}p{color:var(--muted);margin:.5rem 0}code{font:14px ui-monospace,Menlo,monospace;color:var(--fg)}
</style></head>
<body><main>
<h1>Open Toktail from the menu bar</h1>
<p>This browser isn't signed in to Toktail. Choose <b>Open Toktail</b> in the menu bar (the system tray on Windows and Linux), or run <code>Toktail open</code>, and this page opens signed in.</p>
<p>Toktail asks for this so that programs on this computer, coding agents in containers included, can't read your prompts or change your settings.</p>
</main></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("toktail-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn login_tokens_work_once_and_only_fresh() {
        let base = dir();
        let auth = PortalAuth::new(&base);
        let t = issue_login(&base).unwrap();
        assert!(auth.redeem(&t));
        assert!(!auth.redeem(&t), "a login token works once");
        assert!(!auth.redeem("zz"), "garbage is refused");
        // An old token is refused.
        let old = random_token();
        write_private(
            &login_dir(&base).join(sha_hex(&old)),
            (now() - LOGIN_TTL_SECS - 5).to_string().as_bytes(),
        )
        .unwrap();
        assert!(!auth.redeem(&old));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sessions_check_survive_restart_and_sign_out() {
        let base = dir();
        let auth = PortalAuth::new(&base);
        let c = auth.start_session();
        let header = format!("theme=dark; {COOKIE}={c}; other=1");
        assert!(auth.check(Some(&header)));
        assert!(!auth.check(Some("toktail_portal=nope")));
        assert!(!auth.check(None));
        // A restart reads the hashes back.
        let again = PortalAuth::new(&base);
        assert!(again.check(Some(&header)));
        // Only hashes are on disk.
        let on_disk = std::fs::read_to_string(base.join("portal-sessions.json")).unwrap();
        assert!(!on_disk.contains(&c));
        again.sign_out_all();
        assert!(!again.check(Some(&header)));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn links_carry_the_page() {
        let base = dir();
        let l = login_link(&base, 8765, "#/settings/api").unwrap();
        assert!(l.starts_with("http://127.0.0.1:8765/login?t="));
        assert!(l.ends_with("&to=%23%2Fsettings%2Fapi"));
        let plain = login_link(&base, 8765, "https://evil.example").unwrap();
        assert!(!plain.contains("to="), "only portal fragments are allowed");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn open_paths() {
        assert!(is_open_path("/api/ping"));
        assert!(is_open_path("/metrics"));
        assert!(!is_open_path("/api/live"), "the live feed names projects");
        assert!(!is_open_path("/api/settings"));
        assert!(!is_open_path("/"));
    }
}
