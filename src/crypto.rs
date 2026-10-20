// SPDX-License-Identifier: Apache-2.0
//! Encryption at rest for saved prompts and responses.
//!
//! Every saved file is gzip-compressed first, then (when encryption is on)
//! sealed with AES-256-GCM:
//!
//! ```text
//! off:       <gzip bytes>                                   (plain .gz content)
//! password:  "TKT1" 0x01 <key fingerprint:8> <nonce:12> <ciphertext+tag>
//! per_file:  "TKT1" 0x02 <key id:16>          <nonce:12> <ciphertext+tag>
//! ```
//!
//! With "password", one key is derived from your password (Argon2id, with a
//! random salt kept in config.toml). With "per_file", every file gets its own
//! random key, kept in a key store: an encrypted keystore file in Toktail's
//! folder (sealed with the password key), 1Password (through the `op` CLI) or
//! HashiCorp Vault (KV v2). The portal decrypts transparently.

use crate::config::EncryptionConfig;
use crate::secrets;
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use parking_lot::{Mutex, RwLock};
pub use rand_like::random_bytes;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"TKT1";
const MODE_PASSWORD: u8 = 1;
const MODE_PER_FILE: u8 = 2;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

mod rand_like {
    use aes_gcm::aead::rand_core::RngCore;
    use aes_gcm::aead::OsRng;
    pub fn random_bytes<const N: usize>() -> [u8; N] {
        let mut b = [0u8; N];
        OsRng.fill_bytes(&mut b);
        b
    }
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

pub fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(data)
        .read_to_end(&mut out)
        .context("not gzip data")?;
    Ok(out)
}

/// Argon2id(password, salt) -> 32-byte key.
pub fn derive_key(password: &str, salt_hex: &str) -> Result<[u8; 32]> {
    let salt = hex::decode(salt_hex).unwrap_or_else(|_| salt_hex.as_bytes().to_vec());
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|e| anyhow!("deriving the key: {e}"))?;
    Ok(key)
}

pub fn fingerprint(key: &[u8; 32]) -> [u8; 8] {
    let d = Sha256::digest(key);
    let mut fp = [0u8; 8];
    fp.copy_from_slice(&d[..8]);
    fp
}

fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(&nonce, Payload { msg: plain, aad })
        .map_err(|_| anyhow!("encryption failed"))?;
    Ok((nonce.to_vec(), ct))
}

fn open(key: &[u8; 32], aad: &[u8], nonce: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| {
            anyhow!("the file can't be decrypted with this key (wrong password, or it was changed)")
        })
}

// ------------------------------------------------------------ key stores

/// Where per-file keys live.
pub trait KeyStore: Send + Sync {
    fn put(&self, id: &[u8; 16], key: &[u8; 32]) -> Result<()>;
    fn get(&self, id: &[u8; 16]) -> Result<[u8; 32]>;
    fn delete(&self, id: &[u8; 16]) -> Result<()>;
    fn describe(&self) -> String;
    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

fn key_from_b64(s: &str) -> Result<[u8; 32]> {
    let v = B64.decode(s.trim()).context("bad key encoding")?;
    v.try_into().map_err(|_| anyhow!("bad key length"))
}

/// An append-only keystore file in Toktail's folder. Each line is one
/// AES-GCM-sealed entry ("<id>,<key>" or "-,<id>" to forget), sealed with the
/// key derived from your password, so the file is useless without it.
pub struct FileKeyStore {
    path: PathBuf,
    master: [u8; 32],
    dirty: Mutex<bool>,
    journal: Mutex<()>,
    keys: Mutex<HashMap<[u8; 16], [u8; 32]>>,
}

impl FileKeyStore {
    pub fn open(path: PathBuf, master: [u8; 32]) -> Result<FileKeyStore> {
        let mut keys = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for (n, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let raw = B64
                    .decode(line.trim())
                    .with_context(|| format!("keystore line {}", n + 1))?;
                if raw.len() < 13 {
                    continue;
                }
                let plain = open(&master, b"toktail-keystore", &raw[..12], &raw[12..])
                    .with_context(|| format!("keystore line {}", n + 1))?;
                let plain = String::from_utf8_lossy(&plain).to_string();
                let (a, b) = plain.split_once(',').unwrap_or(("", ""));
                if a == "-" {
                    if let Ok(Some(id)) = hex::decode(b).map(|v| <[u8; 16]>::try_from(v).ok()) {
                        keys.remove(&id);
                    }
                } else if let (Ok(id), Ok(k)) = (hex::decode(a), key_from_b64(b)) {
                    if let Ok(id) = <[u8; 16]>::try_from(id) {
                        keys.insert(id, k);
                    }
                }
            }
        }
        Ok(FileKeyStore {
            path,
            master,
            keys: Mutex::new(keys),
            dirty: Mutex::new(false),
            journal: Mutex::new(()),
        })
    }

    fn append(&self, entry: &str) -> Result<()> {
        let _guard = self.journal.lock();
        let (nonce, ct) = seal(&self.master, b"toktail-keystore", entry.as_bytes())?;
        let mut raw = nonce;
        raw.extend(ct);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{}", B64.encode(raw))?;
        *self.dirty.lock() = true;
        Ok(())
    }
}

impl KeyStore for FileKeyStore {
    fn flush(&self) -> Result<()> {
        let _guard = self.journal.lock();
        if *self.dirty.lock() {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&self.path)?
                .sync_data()?;
            *self.dirty.lock() = false;
        }
        Ok(())
    }
    fn put(&self, id: &[u8; 16], key: &[u8; 32]) -> Result<()> {
        self.append(&format!("{},{}", hex::encode(id), B64.encode(key)))?;
        self.keys.lock().insert(*id, *key);
        Ok(())
    }
    fn get(&self, id: &[u8; 16]) -> Result<[u8; 32]> {
        self.keys
            .lock()
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("no key for this file in the keystore"))
    }
    fn delete(&self, id: &[u8; 16]) -> Result<()> {
        self.append(&format!("-,{}", hex::encode(id)))?;
        self.keys.lock().remove(id);
        Ok(())
    }
    fn describe(&self) -> String {
        format!("encrypted keystore ({} keys)", self.keys.lock().len())
    }
}

/// 1Password, through the `op` command-line tool. A service-account token
/// (saved in the Keychain) is used when set; otherwise `op` asks the
/// 1Password app, which may prompt for Touch ID.
pub struct OnePasswordStore {
    vault: String,
    token: Option<String>,
    cache: Mutex<HashMap<[u8; 16], [u8; 32]>>,
}

impl OnePasswordStore {
    pub fn new(vault: &str, token: Option<String>) -> OnePasswordStore {
        OnePasswordStore {
            vault: vault.to_string(),
            token,
            cache: Mutex::new(HashMap::new()),
        }
    }
    fn op(&self, args: &[&str]) -> Result<String> {
        let mut c = std::process::Command::new(op_path());
        c.args(args);
        if let Some(t) = &self.token {
            c.env("OP_SERVICE_ACCOUNT_TOKEN", t);
        }
        let out = c
            .output()
            .context("couldn't run the 1Password CLI (op). Is it installed?")?;
        if !out.status.success() {
            bail!("1Password: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
    fn title(id: &[u8; 16]) -> String {
        format!("toktail-{}", hex::encode(id))
    }
    pub fn check(&self) -> Result<String> {
        self.op(&["vault", "get", &self.vault, "--format", "json"])
            .map(|_| format!("1Password vault \"{}\" is reachable", self.vault))
    }
}

fn op_path() -> String {
    for p in ["/opt/homebrew/bin/op", "/usr/local/bin/op"] {
        if Path::new(p).exists() {
            return p.into();
        }
    }
    "op".into()
}

impl KeyStore for OnePasswordStore {
    fn put(&self, id: &[u8; 16], key: &[u8; 32]) -> Result<()> {
        let pw = format!("password={}", B64.encode(key));
        self.op(&[
            "item",
            "create",
            "--vault",
            &self.vault,
            "--category",
            "password",
            "--title",
            &Self::title(id),
            "--tags",
            "toktail",
            &pw,
        ])?;
        self.cache.lock().insert(*id, *key);
        Ok(())
    }
    fn get(&self, id: &[u8; 16]) -> Result<[u8; 32]> {
        if let Some(k) = self.cache.lock().get(id) {
            return Ok(*k);
        }
        let v = self.op(&[
            "read",
            &format!("op://{}/{}/password", self.vault, Self::title(id)),
        ])?;
        let k = key_from_b64(&v)?;
        self.cache.lock().insert(*id, k);
        Ok(k)
    }
    fn delete(&self, id: &[u8; 16]) -> Result<()> {
        self.cache.lock().remove(id);
        self.op(&["item", "delete", &Self::title(id), "--vault", &self.vault])
            .map(|_| ())
    }
    fn describe(&self) -> String {
        format!("1Password vault \"{}\"", self.vault)
    }
}

/// HashiCorp Vault, KV version 2: one secret per file at <mount>/<path>/<id>.
pub struct VaultStore {
    addr: String,
    mount: String,
    path: String,
    namespace: String,
    token: String,
    http: reqwest::blocking::Client,
    cache: Mutex<HashMap<[u8; 16], [u8; 32]>>,
}

impl VaultStore {
    pub fn new(cfg: &EncryptionConfig, token: String) -> VaultStore {
        VaultStore {
            addr: cfg.vault_addr.trim().trim_end_matches('/').to_string(),
            mount: cfg.vault_mount.trim().trim_matches('/').to_string(),
            path: cfg.vault_path.trim().trim_matches('/').to_string(),
            namespace: cfg.vault_namespace.trim().to_string(),
            token,
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(HashMap::new()),
        }
    }
    fn url(&self, kind: &str, id: &str) -> String {
        format!(
            "{}/v1/{}/{}/{}/{}",
            self.addr, self.mount, kind, self.path, id
        )
    }
    fn req(&self, m: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        let mut r = self
            .http
            .request(m, url)
            .header("X-Vault-Token", &self.token);
        if !self.namespace.is_empty() {
            r = r.header("X-Vault-Namespace", &self.namespace);
        }
        r
    }
    pub fn check(&self) -> Result<String> {
        let r = self
            .req(
                reqwest::Method::GET,
                &format!("{}/v1/auth/token/lookup-self", self.addr),
            )
            .send()?;
        if r.status().is_success() {
            Ok(format!("Vault at {} accepted the token", self.addr))
        } else {
            bail!("Vault answered {}", r.status())
        }
    }
}

impl KeyStore for VaultStore {
    fn put(&self, id: &[u8; 16], key: &[u8; 32]) -> Result<()> {
        let body = serde_json::json!({ "data": { "key": B64.encode(key) } });
        let r = self
            .req(reqwest::Method::POST, &self.url("data", &hex::encode(id)))
            .json_body(&body)
            .send()?;
        if !r.status().is_success() {
            bail!("Vault answered {} when saving a key", r.status());
        }
        self.cache.lock().insert(*id, *key);
        Ok(())
    }
    fn get(&self, id: &[u8; 16]) -> Result<[u8; 32]> {
        if let Some(k) = self.cache.lock().get(id) {
            return Ok(*k);
        }
        let r = self
            .req(reqwest::Method::GET, &self.url("data", &hex::encode(id)))
            .send()?;
        if !r.status().is_success() {
            bail!("Vault answered {} when reading a key", r.status());
        }
        let v: serde_json::Value = serde_json::from_slice(&r.bytes()?)?;
        let k = key_from_b64(
            v.pointer("/data/data/key")
                .and_then(|k| k.as_str())
                .unwrap_or(""),
        )?;
        self.cache.lock().insert(*id, k);
        Ok(k)
    }
    fn delete(&self, id: &[u8; 16]) -> Result<()> {
        self.cache.lock().remove(id);
        let r = self
            .req(
                reqwest::Method::DELETE,
                &self.url("metadata", &hex::encode(id)),
            )
            .send()?;
        if !r.status().is_success() && r.status().as_u16() != 404 {
            bail!("Vault answered {} when deleting a key", r.status());
        }
        Ok(())
    }
    fn describe(&self) -> String {
        format!("Vault at {}", self.addr)
    }
}

trait JsonBody {
    fn json_body(self, v: &serde_json::Value) -> Self;
}
impl JsonBody for reqwest::blocking::RequestBuilder {
    fn json_body(self, v: &serde_json::Value) -> Self {
        self.header("content-type", "application/json")
            .body(v.to_string())
    }
}

// ------------------------------------------------------------ the crypto service

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Off,
    Password,
    PerFile,
}

impl Mode {
    pub fn from_config(s: &str) -> Mode {
        match s {
            "password" => Mode::Password,
            "per_file" => Mode::PerFile,
            _ => Mode::Off,
        }
    }
}

/// Current encryption state: what mode is on and whether keys are available.
pub struct Crypto {
    state: RwLock<State>,
}

struct State {
    mode: Mode,
    master: Option<[u8; 32]>,
    keystore: Option<Box<dyn KeyStore>>,
    /// Why files can't be encrypted right now, if they can't.
    problem: Option<String>,
}

impl Crypto {
    pub fn new() -> Crypto {
        Crypto {
            state: RwLock::new(State {
                mode: Mode::Off,
                master: None,
                keystore: None,
                problem: None,
            }),
        }
    }

    /// (Re)loads keys from settings. Returns the key fingerprint to remember
    /// when none was recorded yet.
    pub fn configure(&self, base: &Path, cfg: &EncryptionConfig) -> Option<String> {
        let mode = Mode::from_config(&cfg.mode);
        let mut st = State {
            mode: mode.clone(),
            master: None,
            keystore: None,
            problem: None,
        };
        let mut new_check = None;
        // Keys are loaded whenever they're available, even when the current
        // mode doesn't need them, so files saved under an earlier mode stay
        // readable. Only what the current mode needs counts as a problem.
        let needs_password =
            mode == Mode::Password || (mode == Mode::PerFile && cfg.keystore == "file");
        match secrets::get_encryption_password() {
            None if needs_password => {
                st.problem = Some("Set an encryption password in Settings.".into())
            }
            None => {}
            Some(pw) => match derive_key(&pw, &cfg.salt) {
                Ok(k) => {
                    let fp = hex::encode(fingerprint(&k));
                    if !cfg.key_check.is_empty() && cfg.key_check != fp {
                        if needs_password {
                            st.problem = Some("The saved password doesn't match the one your files were encrypted with.".into());
                        }
                    } else {
                        if cfg.key_check.is_empty() && needs_password {
                            new_check = Some(fp);
                        }
                        st.master = Some(k);
                    }
                }
                Err(e) if needs_password => st.problem = Some(format!("{e:#}")),
                Err(_) => {}
            },
        }
        let wants_keystore = mode == Mode::PerFile;
        let could_read = match cfg.keystore.as_str() {
            "1password" => secrets::get_named_secret("1password-token").is_some(),
            "vault" => secrets::get_named_secret("vault-token").is_some(),
            _ => st.master.is_some() && base.join("keystore.tkt").exists(),
        };
        if (wants_keystore && st.problem.is_none()) || (!wants_keystore && could_read) {
            let ks: Result<Box<dyn KeyStore>> = match cfg.keystore.as_str() {
                "1password" => Ok(Box::new(OnePasswordStore::new(
                    &cfg.onepassword_vault,
                    secrets::get_named_secret("1password-token"),
                ))),
                "vault" => match secrets::get_named_secret("vault-token") {
                    Some(t) => Ok(Box::new(VaultStore::new(cfg, t))),
                    None => Err(anyhow!("Set a Vault token in Settings.")),
                },
                _ => match st.master {
                    Some(m) => FileKeyStore::open(base.join("keystore.tkt"), m)
                        .map(|k| Box::new(k) as Box<dyn KeyStore>),
                    None => Err(anyhow!("Set an encryption password in Settings.")),
                },
            };
            match ks {
                Ok(k) => st.keystore = Some(k),
                Err(e) if wants_keystore => st.problem = Some(format!("{e:#}")),
                Err(e) => log::warn!("key store (for older files): {e:#}"),
            }
        }
        *self.state.write() = st;
        new_check
    }

    /// None when files can be written; otherwise why not.
    pub fn problem(&self) -> Option<String> {
        self.state.read().problem.clone()
    }

    pub fn describe(&self) -> String {
        let st = self.state.read();
        match (&st.mode, &st.problem) {
            (_, Some(p)) => p.clone(),
            (Mode::Off, _) => "Files are compressed, not encrypted.".into(),
            (Mode::Password, _) => "Files are encrypted with the key from your password.".into(),
            (Mode::PerFile, _) => format!(
                "Every file has its own key, kept in the {}.",
                st.keystore
                    .as_ref()
                    .map(|k| k.describe())
                    .unwrap_or_default()
            ),
        }
    }

    /// gzip, then encrypt per the current mode.
    pub fn seal(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let gz = gzip(plain);
        let st = self.state.read();
        if let Some(p) = &st.problem {
            bail!("{p}");
        }
        match st.mode {
            Mode::Off => Ok(gz),
            Mode::Password => {
                let k = st.master.ok_or_else(|| anyhow!("no key"))?;
                let fp = fingerprint(&k);
                let mut header = MAGIC.to_vec();
                header.push(MODE_PASSWORD);
                header.extend_from_slice(&fp);
                let (nonce, ct) = seal(&k, &header, &gz)?;
                header.extend(nonce);
                header.extend(ct);
                Ok(header)
            }
            Mode::PerFile => {
                let ks = st
                    .keystore
                    .as_ref()
                    .ok_or_else(|| anyhow!("no key store"))?;
                let id: [u8; 16] = random_bytes();
                let key: [u8; 32] = random_bytes();
                ks.put(&id, &key)?;
                let mut header = MAGIC.to_vec();
                header.push(MODE_PER_FILE);
                header.extend_from_slice(&id);
                let (nonce, ct) = seal(&key, &header, &gz)?;
                header.extend(nonce);
                header.extend(ct);
                Ok(header)
            }
        }
    }

    /// Decrypts (if needed) and decompresses a saved file.
    pub fn open_file(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 5 || &data[..4] != MAGIC {
            return gunzip(data).or_else(|_| Ok(data.to_vec()));
        }
        let st = self.state.read();
        let gz = match data[4] {
            MODE_PASSWORD => {
                if data.len() < 5 + 8 + 12 + 16 {
                    bail!("file is too short");
                }
                let k = st.master.ok_or_else(|| {
                    anyhow!("Set the encryption password in Settings to read this file.")
                })?;
                if fingerprint(&k)[..] != data[5..13] {
                    bail!("This file was encrypted with a different password.");
                }
                open(&k, &data[..13], &data[13..25], &data[25..])?
            }
            MODE_PER_FILE => {
                if data.len() < 5 + 16 + 12 + 16 {
                    bail!("file is too short");
                }
                let ks = st
                    .keystore
                    .as_ref()
                    .ok_or_else(|| anyhow!("Its key store isn't set up in Settings."))?;
                let id: [u8; 16] = data[5..21].try_into().unwrap();
                let key = ks.get(&id)?;
                open(&key, &data[..21], &data[21..33], &data[33..])?
            }
            m => bail!("unknown file format {m}"),
        };
        gunzip(&gz)
    }

    pub fn flush(&self) -> Result<()> {
        if let Some(ks) = self.state.read().keystore.as_ref() {
            ks.flush()?;
        }
        Ok(())
    }

    pub fn key_id(data: &[u8]) -> String {
        if data.len() >= 21 && &data[..4] == MAGIC && data[4] == MODE_PER_FILE {
            hex::encode(&data[5..21])
        } else {
            String::new()
        }
    }

    pub fn forget_id(&self, id: &str) {
        if let Ok(bytes) = hex::decode(id) {
            if let Ok(id) = <[u8; 16]>::try_from(bytes) {
                if let Some(ks) = self.state.read().keystore.as_ref() {
                    let _ = ks.delete(&id);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn fixture_per_file(&self, path: PathBuf) {
        *self.state.write() = State {
            mode: Mode::PerFile,
            master: Some([9; 32]),
            keystore: Some(Box::new(FileKeyStore::open(path, [9; 32]).unwrap())),
            problem: None,
        };
    }

    /// Forgets a per-file key (after its file is deleted).
    pub fn forget(&self, data_head: &[u8]) {
        if data_head.len() >= 21 && &data_head[..4] == MAGIC && data_head[4] == MODE_PER_FILE {
            let id: [u8; 16] = data_head[5..21].try_into().unwrap();
            if let Some(ks) = self.state.read().keystore.as_ref() {
                if let Err(e) = ks.delete(&id) {
                    log::warn!("couldn't remove a file key: {e:#}");
                }
            }
        }
    }

    /// Checks a key store's settings without changing anything.
    pub fn test_keystore(cfg: &EncryptionConfig) -> Result<String> {
        match cfg.keystore.as_str() {
            "1password" => OnePasswordStore::new(
                &cfg.onepassword_vault,
                secrets::get_named_secret("1password-token"),
            )
            .check(),
            "vault" => VaultStore::new(
                cfg,
                secrets::get_named_secret("vault-token")
                    .ok_or_else(|| anyhow!("Set a Vault token first."))?,
            )
            .check(),
            _ => Ok(
                "The keystore file lives in Toktail's folder, sealed with your password.".into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_roundtrip() {
        let d = b"hello hello hello hello";
        assert_eq!(gunzip(&gzip(d)).unwrap(), d);
    }

    #[test]
    fn password_and_per_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tl-crypto-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let master = derive_key("correct horse", "00112233445566778899aabbccddeeff").unwrap();
        // Password mode, set up by hand (configure() reads the Keychain).
        let c = Crypto::new();
        *c.state.write() = State {
            mode: Mode::Password,
            master: Some(master),
            keystore: None,
            problem: None,
        };
        let sealed = c.seal(b"{\"messages\":[]}").unwrap();
        assert_eq!(&sealed[..4], MAGIC);
        assert_eq!(c.open_file(&sealed).unwrap(), b"{\"messages\":[]}");
        // Tampering is caught.
        let mut bad = sealed.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(c.open_file(&bad).is_err());

        // Per-file keys in the encrypted keystore, which survives a reopen.
        let ks = FileKeyStore::open(dir.join("keystore.tkt"), master).unwrap();
        *c.state.write() = State {
            mode: Mode::PerFile,
            master: Some(master),
            keystore: Some(Box::new(ks)),
            problem: None,
        };
        let a = c.seal(b"first").unwrap();
        let b = c.seal(b"second").unwrap();
        assert_ne!(a[5..21], b[5..21], "each file has its own key id");
        let ks2 = FileKeyStore::open(dir.join("keystore.tkt"), master).unwrap();
        *c.state.write() = State {
            mode: Mode::PerFile,
            master: Some(master),
            keystore: Some(Box::new(ks2)),
            problem: None,
        };
        assert_eq!(c.open_file(&a).unwrap(), b"first");
        c.forget(&a);
        assert!(c.open_file(&a).is_err());
        assert_eq!(c.open_file(&b).unwrap(), b"second");
        // The keystore file holds no readable keys.
        let text = std::fs::read_to_string(dir.join("keystore.tkt")).unwrap();
        assert!(!text.contains(&hex::encode(&a[5..21])));
        // Plain gzip files (encryption off) still open.
        assert_eq!(c.open_file(&gzip(b"plain")).unwrap(), b"plain");
        std::fs::remove_dir_all(dir).ok();
    }
}
