// SPDX-License-Identifier: Apache-2.0
//! Credentials that pass through Toktail, and how they are protected.
//!
//! Toktail sits between a coding agent and a model server, so an agent's API
//! key (Anthropic, OpenAI, a private gateway) can travel through it. The rules:
//!
//! - **The key is never written down.** Not to the CSV, not to a saved
//!   prompt, not to a log line, not to S3. Toktail keeps only a one-way id of
//!   it: `tk_` + HMAC-SHA256(install secret, key), truncated. The id tells
//!   keys apart (for the cache, for records, for the dashboard) and cannot
//!   be turned back into the key, not even with the CSV and the config.
//! - **Gateway keys keep the real key at home.** For a public API host you
//!   can store the provider's key in the Keychain and hand your agents a
//!   Toktail gateway key (`tkg_…`) instead. Toktail swaps it for the real key
//!   on the way out, so the real key never reaches the agent, its container
//!   or your LAN. Only the gateway key's SHA-256 is kept in config.toml.
//! - **No open relay.** A public API host listening on the network only
//!   answers requests that carry a gateway key or a provider key of their
//!   own; Toktail never adds its stored key for a request from another
//!   machine.
//! - **Scrubbing.** Saved prompts and responses, commentary input and error
//!   messages are scrubbed of anything that looks like an API key before
//!   they are written or shown.

use crate::config::HostConfig;
use axum::http::{header, HeaderMap};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

const INSTALL_SECRET_NAME: &str = "install-secret";
static INSTALL_SECRET: OnceLock<[u8; 32]> = OnceLock::new();

/// The per-install HMAC secret, created once and kept in the secret store.
/// With no store at all (tests, a read-only system) a random secret is used
/// for the life of the process, so ids stay consistent within a run.
pub fn install_secret() -> &'static [u8; 32] {
    INSTALL_SECRET.get_or_init(|| {
        if let Some(hex) = crate::secrets::get_named_secret(INSTALL_SECRET_NAME) {
            if let Ok(b) = hex::decode(hex.trim()) {
                if let Ok(arr) = <[u8; 32]>::try_from(b) {
                    return arr;
                }
            }
        }
        let fresh = random32();
        if let Err(e) =
            crate::secrets::set_named_secret(INSTALL_SECRET_NAME, Some(&hex::encode(fresh)))
        {
            log::warn!("could not save the install secret: {e:#}; token ids are per run");
        }
        fresh
    })
}

/// For tests: a fixed secret, so ids are reproducible.
#[cfg(test)]
pub fn set_install_secret_for_tests(s: [u8; 32]) {
    let _ = INSTALL_SECRET.set(s);
}

fn random32() -> [u8; 32] {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(a.as_bytes());
    out[16..].copy_from_slice(b.as_bytes());
    out
}

/// `tk_` + 16 hex characters: a one-way id for a credential. Two different
/// keys get different ids; the same key always gets the same id on this
/// install; nothing can recover the key from the id.
pub fn token_id(credential: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(install_secret()).expect("any key length works");
    mac.update(credential.trim().as_bytes());
    let out = mac.finalize().into_bytes();
    format!("tk_{}", hex::encode(&out[..8]))
}

/// The credential a request carries, in order of preference: Anthropic's
/// `x-api-key`, a bearer token, Google's `x-goog-api-key`.
pub fn client_credential(headers: &HeaderMap) -> Option<Credential> {
    let s = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    if let Some(v) = s("x-api-key") {
        return Some(Credential {
            header: "x-api-key",
            value: v,
        });
    }
    if let Some(v) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = v
            .trim()
            .strip_prefix("Bearer ")
            .or(v.trim().strip_prefix("bearer "))
        {
            if !t.trim().is_empty() {
                return Some(Credential {
                    header: "authorization",
                    value: t.trim().to_string(),
                });
            }
        }
    }
    if let Some(v) = s("x-goog-api-key") {
        return Some(Credential {
            header: "x-goog-api-key",
            value: v,
        });
    }
    None
}

/// A credential as a client sent it. The value is kept only for the life of
/// the request and is never cloned into anything that outlives it.
#[derive(Clone)]
pub struct Credential {
    /// Which header carried it.
    pub header: &'static str,
    pub value: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({}: {})", self.header, token_id(&self.value))
    }
}

impl Credential {
    pub fn id(&self) -> String {
        token_id(&self.value)
    }
    pub fn is_gateway(&self) -> bool {
        self.value.starts_with("tkg_")
    }
}

// ------------------------------------------------------------ gateway keys

/// A Toktail-issued key an agent can use instead of the provider's. Only the
/// hash lives here; the key itself is shown once when it is issued.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayKey {
    /// Short id (`tkg_…`, the first 8 hex of the hash) shown in the portal.
    pub id: String,
    /// SHA-256 of the full key, hex.
    pub hash: String,
    pub label: String,
    pub created: String,
    /// Allow requests from other machines on the network with this key.
    pub lan: bool,
}

impl Default for GatewayKey {
    fn default() -> Self {
        GatewayKey {
            id: String::new(),
            hash: String::new(),
            label: "Agent".into(),
            created: String::new(),
            lan: true,
        }
    }
}

fn sha_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Makes a new gateway key. Returns the key to show once, and the entry to
/// keep in config.
pub fn issue_gateway_key(label: &str) -> (String, GatewayKey) {
    let raw = format!("{}{}", hex::encode(random32()), hex::encode(random32()));
    let plain = format!("tkg_{raw}");
    let hash = sha_hex(&plain);
    let entry = GatewayKey {
        id: format!("tkg_{}", &hash[..8]),
        hash,
        label: label.trim().chars().take(60).collect(),
        created: chrono::Local::now().to_rfc3339(),
        lan: true,
    };
    (plain, entry)
}

/// The gateway key a credential matches, if any, by constant-time hash check.
pub fn match_gateway_key<'a>(host: &'a HostConfig, cred: &Credential) -> Option<&'a GatewayKey> {
    if !cred.is_gateway() {
        return None;
    }
    let h = sha_hex(&cred.value);
    host.gateway_keys.iter().find(|k| ct_eq(&k.hash, &h))
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------------ the network

/// Loopback, private IPv4 (10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10)
/// and private IPv6 (ULA fc00::/7, link-local fe80::/10, ::1). Public
/// addresses are not: Pip never talks to a public model about your work,
/// and a key from another machine is treated as having crossed the LAN.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// True when a URL points at this machine or the local network.
pub fn is_private_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost"
                || d.ends_with(".local")
                || d.ends_with(".lan")
                || d.ends_with(".home")
                || d.ends_with(".internal")
                || d == "host.docker.internal"
        }
        Some(url::Host::Ipv4(ip)) => is_private_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_private_ip(IpAddr::V6(ip)),
        None => false,
    }
}

// ------------------------------------------------------------ scrubbing

/// Replaces anything that looks like an API key with `[redacted tk_…]`, so a
/// key that ended up inside a prompt (an agent echoing its environment, a
/// pasted curl command) never lands in a saved file, a log or a card.
pub fn scrub(text: &str) -> String {
    if !looks_like_it_has_a_key(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((start, len)) = find_key(rest) {
        out.push_str(&rest[..start]);
        let key = &rest[start..start + len];
        out.push_str(&format!("[redacted {}]", token_id(key)));
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    out
}

/// The same for bytes: a body that isn't UTF-8 is left alone.
pub fn scrub_bytes(b: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(b) {
        Ok(s) if looks_like_it_has_a_key(s) => scrub(s).into_bytes(),
        _ => b.to_vec(),
    }
}

fn looks_like_it_has_a_key(s: &str) -> bool {
    s.contains("sk-")
        || s.contains("tkg_")
        || s.contains("Bearer ")
        || s.contains("AKIA")
        || s.contains("ghp_")
        || s.contains("xai-")
        || s.contains("AIza")
}

const PREFIXES: [&str; 7] = ["sk-ant-", "sk-proj-", "sk-", "tkg_", "ghp_", "xai-", "AIza"];

/// Finds the next key-shaped run: a known prefix followed by at least 16
/// characters of key material, or `Bearer <token>`.
fn find_key(s: &str) -> Option<(usize, usize)> {
    let is_key_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut best: Option<(usize, usize)> = None;
    for p in PREFIXES {
        let mut from = 0;
        while let Some(i) = s[from..].find(p) {
            let start = from + i;
            // The prefix must begin a word.
            let word_start = start == 0
                || !s[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            let tail = &s[start + p.len()..];
            let n = tail.chars().take_while(|c| is_key_char(*c)).count();
            if word_start && n >= 16 {
                let len = p.len() + n;
                if best.is_none_or(|b| start < b.0) {
                    best = Some((start, len));
                }
                break;
            }
            from = start + p.len();
        }
    }
    let mut from = 0;
    while let Some(i) = s[from..].find("Bearer ") {
        let start = from + i + "Bearer ".len();
        let n = s[start..].chars().take_while(|c| is_key_char(*c)).count();
        if n >= 16 && !s[start..start + n].starts_with("[redacted") {
            if best.is_none_or(|b| start < b.0) {
                best = Some((start, n));
            }
            break;
        }
        from = start;
    }
    // AWS access key ids: AKIA + 16 upper/digits.
    if let Some(i) = s.find("AKIA") {
        let n = s[i + 4..]
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .count();
        if n >= 16 && best.is_none_or(|b| i < b.0) {
            best = Some((i, 4 + n));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_one_way_and_stable() {
        set_install_secret_for_tests([7; 32]);
        let a = token_id("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789");
        let b = token_id("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789");
        let c = token_id("sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456788");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("tk_") && a.len() == 19, "{a}");
        assert!(!a.contains("abcdef"));
    }

    #[test]
    fn credentials_from_headers() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer sk-x".parse().unwrap());
        assert_eq!(client_credential(&h).unwrap().value, "sk-x");
        h.insert("x-api-key", "ant-key".parse().unwrap());
        let c = client_credential(&h).unwrap();
        assert_eq!((c.header, c.value.as_str()), ("x-api-key", "ant-key"));
        assert!(
            !format!("{c:?}").contains("ant-key"),
            "debug never shows the key"
        );
    }

    #[test]
    fn gateway_keys_match_by_hash_only() {
        let (plain, entry) = issue_gateway_key("roc");
        assert!(plain.starts_with("tkg_") && plain.len() > 60);
        assert!(!serde_json::to_string(&entry).unwrap().contains(&plain[4..]));
        let host = HostConfig {
            gateway_keys: vec![entry.clone()],
            ..Default::default()
        };
        let ok = Credential {
            header: "authorization",
            value: plain.clone(),
        };
        assert_eq!(
            match_gateway_key(&host, &ok).map(|k| &k.id),
            Some(&entry.id)
        );
        let bad = Credential {
            header: "authorization",
            value: format!("{}x", &plain[..plain.len() - 1]),
        };
        assert!(match_gateway_key(&host, &bad).is_none());
    }

    #[test]
    fn private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.20.0.5",
            "192.168.1.9",
            "169.254.3.3",
            "100.70.1.1",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:192.168.0.2",
        ] {
            assert!(is_private_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["172.217.3.4", "8.8.8.8", "2606:4700::1111", "172.32.0.1"] {
            assert!(!is_private_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_private_url("http://studio.local:1234"));
        assert!(is_private_url("http://127.0.0.1:1235"));
        assert!(is_private_url("http://host.docker.internal:1234"));
        assert!(!is_private_url("https://api.anthropic.com"));
        assert!(!is_private_url("http://172.217.3.4"));
    }

    #[test]
    fn scrubbing_keeps_the_text_and_drops_the_key() {
        set_install_secret_for_tests([7; 32]);
        let s = "export ANTHROPIC_API_KEY=sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 and curl -H 'Authorization: Bearer sk-proj-ABCDEFGHIJKLMNOPQRSTUV' plus sk-short and AKIAIOSFODNN7EXAMPLE";
        let out = scrub(s);
        assert!(
            !out.contains("AbCdEfGh")
                && !out.contains("ABCDEFGHIJKLMNOP")
                && !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "{out}"
        );
        assert!(out.contains("[redacted tk_"));
        assert!(out.contains("sk-short"), "a short word is not a key");
        assert_eq!(scrub("nothing to see"), "nothing to see");
        assert_eq!(scrub_bytes(b"\xff\xfe"), b"\xff\xfe");
    }
}
