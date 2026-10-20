// SPDX-License-Identifier: Apache-2.0
//! Session chains: saved prompts stored as what's new, plus a reference to
//! the prompt they grew from.
//!
//! A coding agent resends the whole conversation on every turn, so the Nth
//! prompt of a session repeats nearly all of the (N-1)th. Toktail notices
//! and saves only the new bytes. A folded prompt blob looks like this:
//!
//! ```text
//! {{toktail:chain v=2 session="s_…" node="…" parent="…" content="<sha256>" depth=7 prefix=48213 snapshot=false created="…"}}
//! {{toktail:ref path="blobs/3f/3f…e1.prompt" s3="records/blobs/3f/3f…e1.prompt" sha256="3f…e1" bytes="0-48213"}}
//! <the bytes after the shared prefix, verbatim>
//! ```
//!
//! Unfolding puts the referenced byte range of the parent's content in
//! front of the new bytes, so the result is the original request, byte for
//! byte, line breaks and all; the content hash in the header proves it.
//! Every 32nd link is a snapshot (full content, no reference) so no prompt
//! is ever more than 32 hops from solid ground.
//!
//! The proxy never sees any of this: folding happens after the answer has
//! been counted and saved, unfolding only when a person opens a record.
//! The model gets the request exactly as the agent sent it.
//!
//! Each session is a Merkle tree: a node's id is the hash of its parent's
//! id and its own content hash, so a session id plus a head commits to the
//! whole branch. New prompts attach to the node they share the longest
//! prefix with, which is how an edited conversation becomes a branch. The
//! header on every blob carries enough to rebuild the index
//! (`records/sessions/<id>.json`) from the files alone.
//!
//! Erasing a link: the children of a deleted prompt are first refolded
//! against its parent (or made snapshots), verified against their content
//! hashes, and only then does the file go. The index keeps a tombstone so
//! the gap stays visible. A blob that vanished behind Toktail's back
//! (deleted in Finder, expired in S3) is marked missing instead, and its
//! descendants unfold with a `{{toktail:missing …}}` marker where the lost
//! bytes were.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

pub const VERSION: u32 = 2;
/// A snapshot (full content) every this many links.
pub const SNAPSHOT_EVERY: u32 = 32;
/// A parent must share at least this many bytes to be worth referencing.
pub const MIN_PREFIX: usize = 256;
/// Heads kept in memory for attaching, across sessions.
const HEAD_CACHE: usize = 96;
const HEAD_CACHE_BYTES: usize = 96 * 1024 * 1024;

const HEADER_TAG: &[u8] = b"{{toktail:chain ";
const REF_TAG: &[u8] = b"{{toktail:ref ";
const MISSING_TAG: &str = "{{toktail:missing ";

// ------------------------------------------------------------------ format

/// The first line of a folded blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub session: String,
    pub node: String,
    pub parent: Option<String>,
    /// SHA-256 of the unfolded content.
    pub content: String,
    pub depth: u32,
    /// Bytes shared with the parent (0 for a snapshot or root).
    pub prefix: usize,
    pub snapshot: bool,
    pub created: String,
}

/// The second line: where the shared prefix lives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ref {
    /// Path relative to the records folder.
    pub path: String,
    /// The same file's key in the S3 mirror, when one is configured.
    pub s3: Option<String>,
    /// Content hash of the parent (also its blob name).
    pub sha256: String,
    pub from: usize,
    pub to: usize,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Parsed<'a> {
    /// A v1 blob or any other file: the bytes are the content.
    Plain(&'a [u8]),
    Folded {
        header: Header,
        reference: Option<Ref>,
        delta: &'a [u8],
    },
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A node's id: the parent's id and this content, hashed together.
pub fn node_id(parent: Option<&str>, content_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"toktail-node\0");
    h.update(parent.unwrap_or("").as_bytes());
    h.update(b"\0");
    h.update(content_hash.as_bytes());
    hex::encode(h.finalize())
}

pub fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parses `key=value key="quoted value"` pairs.
fn attrs(line: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_string();
        rest = &rest[eq + 1..];
        let value;
        if let Some(r) = rest.strip_prefix('"') {
            let mut v = String::new();
            let mut chars = r.char_indices();
            let mut end = r.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            v.push(match n {
                                'n' => '\n',
                                other => other,
                            });
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => v.push(c),
                }
            }
            value = v;
            rest = r[end.min(r.len())..].trim_start();
        } else {
            let end = rest.find(' ').unwrap_or(rest.len());
            value = rest[..end].to_string();
            rest = rest[end..].trim_start();
        }
        out.insert(key, value);
    }
    out
}

impl Header {
    fn line(&self) -> String {
        let mut s = format!(
            "{{{{toktail:chain v={VERSION} session={} node={} ",
            quote(&self.session),
            quote(&self.node)
        );
        if let Some(p) = &self.parent {
            s.push_str(&format!("parent={} ", quote(p)));
        }
        s.push_str(&format!(
            "content={} depth={} prefix={} snapshot={} created={}}}}}",
            quote(&self.content),
            self.depth,
            self.prefix,
            self.snapshot,
            quote(&self.created)
        ));
        s
    }
    fn parse(line: &str) -> Result<Header> {
        let inner = line
            .strip_prefix("{{toktail:chain ")
            .and_then(|s| s.strip_suffix("}}"))
            .ok_or_else(|| anyhow!("bad chain header"))?;
        let a = attrs(inner);
        let get = |k: &str| {
            a.get(k)
                .cloned()
                .ok_or_else(|| anyhow!("chain header lacks {k}"))
        };
        let v: u32 = get("v")?.parse().context("chain version")?;
        if v > VERSION {
            bail!("this prompt was saved by a newer Toktail (chain v{v})");
        }
        Ok(Header {
            session: get("session")?,
            node: get("node")?,
            parent: a.get("parent").cloned(),
            content: get("content")?,
            depth: get("depth")?.parse().context("chain depth")?,
            prefix: get("prefix")?.parse().context("chain prefix")?,
            snapshot: a.get("snapshot").is_some_and(|s| s == "true"),
            created: a.get("created").cloned().unwrap_or_default(),
        })
    }
}

impl Ref {
    fn line(&self) -> String {
        let mut s = format!("{{{{toktail:ref path={} ", quote(&self.path));
        if let Some(k) = &self.s3 {
            s.push_str(&format!("s3={} ", quote(k)));
        }
        s.push_str(&format!(
            "sha256={} bytes=\"{}-{}\"}}}}",
            quote(&self.sha256),
            self.from,
            self.to
        ));
        s
    }
    fn parse(line: &str) -> Result<Ref> {
        let inner = line
            .strip_prefix("{{toktail:ref ")
            .and_then(|s| s.strip_suffix("}}"))
            .ok_or_else(|| anyhow!("bad chain reference"))?;
        let a = attrs(inner);
        let bytes = a.get("bytes").cloned().unwrap_or_default();
        let (from, to) = bytes
            .split_once('-')
            .and_then(|(f, t)| Some((f.parse().ok()?, t.parse().ok()?)))
            .ok_or_else(|| anyhow!("bad byte range in chain reference"))?;
        let sha = a
            .get("sha256")
            .cloned()
            .ok_or_else(|| anyhow!("chain reference lacks sha256"))?;
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("bad hash in chain reference");
        }
        Ok(Ref {
            path: a.get("path").cloned().unwrap_or_default(),
            s3: a.get("s3").cloned(),
            sha256: sha,
            from,
            to,
        })
    }
    pub fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Builds a blob: header, optional reference, then the bytes after the prefix.
pub fn fold(content: &[u8], header: &Header, reference: Option<&Ref>) -> Vec<u8> {
    let prefix = reference.map_or(0, Ref::len).min(content.len());
    debug_assert_eq!(prefix, header.prefix);
    let mut out = Vec::with_capacity(content.len() - prefix + 512);
    out.extend_from_slice(header.line().as_bytes());
    out.push(b'\n');
    if let Some(r) = reference {
        out.extend_from_slice(r.line().as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(&content[prefix..]);
    out
}

pub fn is_folded(bytes: &[u8]) -> bool {
    bytes.starts_with(HEADER_TAG)
}

pub fn parse(bytes: &[u8]) -> Result<Parsed<'_>> {
    if !is_folded(bytes) {
        return Ok(Parsed::Plain(bytes));
    }
    let nl = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| anyhow!("truncated chain header"))?;
    let header = Header::parse(std::str::from_utf8(&bytes[..nl])?)?;
    let mut rest = &bytes[nl + 1..];
    let mut reference = None;
    if rest.starts_with(REF_TAG) {
        let nl = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| anyhow!("truncated chain reference"))?;
        reference = Some(Ref::parse(std::str::from_utf8(&rest[..nl])?)?);
        rest = &rest[nl + 1..];
    }
    Ok(Parsed::Folded {
        header,
        reference,
        delta: rest,
    })
}

/// What unfolding produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Unfolded {
    pub bytes: Vec<u8>,
    /// Byte ranges of the result that stand in for erased links.
    pub missing: Vec<(usize, usize)>,
    pub depth: u32,
    pub header: Option<Header>,
}

impl Unfolded {
    pub fn intact(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Rebuilds the content of `bytes`, asking `parent_of` for each referenced
/// blob's raw (opened, still folded) bytes; `Ok(None)` means that link is
/// gone. Verifies the result against the header's content hash.
pub fn unfold(
    bytes: &[u8],
    parent_of: &mut dyn FnMut(&Ref) -> Result<Option<Vec<u8>>>,
) -> Result<Unfolded> {
    unfold_at(bytes, parent_of, 0)
}

fn unfold_at(
    bytes: &[u8],
    parent_of: &mut dyn FnMut(&Ref) -> Result<Option<Vec<u8>>>,
    hops: u32,
) -> Result<Unfolded> {
    if hops > SNAPSHOT_EVERY * 2 + 2 {
        bail!("this prompt chain is longer than Toktail allows; it may be damaged");
    }
    match parse(bytes)? {
        Parsed::Plain(b) => Ok(Unfolded {
            bytes: b.to_vec(),
            missing: vec![],
            depth: 0,
            header: None,
        }),
        Parsed::Folded {
            header,
            reference,
            delta,
        } => {
            let mut out = Vec::new();
            let mut missing = Vec::new();
            let mut depth = 0;
            if let Some(r) = reference.as_ref().filter(|r| !r.is_empty()) {
                match parent_of(r)? {
                    Some(raw) => {
                        let parent = unfold_at(&raw, parent_of, hops + 1)?;
                        if parent.bytes.len() < r.to {
                            bail!(
                                "the parent prompt is shorter than this one's reference ({} < {})",
                                parent.bytes.len(),
                                r.to
                            );
                        }
                        out.extend_from_slice(&parent.bytes[r.from..r.to]);
                        missing.extend(
                            parent
                                .missing
                                .into_iter()
                                .filter(|(a, _)| *a < r.to)
                                .map(|(a, b)| (a.saturating_sub(r.from), b.min(r.to) - r.from)),
                        );
                        depth = parent.depth + 1;
                    }
                    None => {
                        let marker = format!(
                            "{MISSING_TAG}bytes=\"{}-{}\" sha256={}}}}}",
                            r.from,
                            r.to,
                            quote(&r.sha256)
                        );
                        out.extend_from_slice(marker.as_bytes());
                        missing.push((0, out.len()));
                        depth = 1;
                    }
                }
            }
            out.extend_from_slice(delta);
            if missing.is_empty() && sha256_hex(&out) != header.content {
                bail!(
                    "this prompt did not unfold to its recorded hash; a link in the chain changed"
                );
            }
            Ok(Unfolded {
                bytes: out,
                missing,
                depth,
                header: Some(header),
            })
        }
    }
}

// ------------------------------------------------------------------ index

/// One prompt in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub node: String,
    pub content: String,
    pub parent: Option<String>,
    /// The record stem of the first request that saved it.
    pub stem: String,
    pub depth: u32,
    pub prefix: usize,
    /// Unfolded size in bytes.
    pub size: usize,
    pub snapshot: bool,
    /// The blob is gone (erased here, or lost behind Toktail's back).
    #[serde(default)]
    pub missing: bool,
    pub created: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub project: String,
    /// Nodes by id.
    pub nodes: BTreeMap<String, Node>,
    /// Nodes nothing attaches to (the branch tips), newest first.
    pub heads: Vec<String>,
}

impl Session {
    pub fn children(&self, node: &str) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.parent.as_deref() == Some(node))
            .collect()
    }
    pub fn by_content(&self, content: &str) -> Option<&Node> {
        self.nodes.values().find(|n| n.content == content)
    }
    fn recompute_heads(&mut self) {
        let mut heads: Vec<&Node> = self
            .nodes
            .values()
            .filter(|n| {
                !n.missing
                    && !self
                        .nodes
                        .values()
                        .any(|c| c.parent.as_deref() == Some(&n.node) && !c.missing)
            })
            .collect();
        heads.sort_by(|a, b| b.created.cmp(&a.created));
        self.heads = heads.into_iter().map(|n| n.node.clone()).collect();
    }
    /// Bytes saved by folding, and what the full copies would have been.
    pub fn savings(&self) -> (usize, usize) {
        let full: usize = self
            .nodes
            .values()
            .filter(|n| !n.missing)
            .map(|n| n.size)
            .sum();
        let stored: usize = self
            .nodes
            .values()
            .filter(|n| !n.missing)
            .map(|n| n.size.saturating_sub(n.prefix))
            .sum();
        (full.saturating_sub(stored), full)
    }
    /// The Merkle root of a head: its own id (each id commits to its parents).
    pub fn root_of(&self, head: &str) -> Option<&Node> {
        let mut cur = self.nodes.get(head)?;
        while let Some(p) = cur.parent.as_deref().and_then(|p| self.nodes.get(p)) {
            cur = p;
        }
        Some(cur)
    }
}

pub fn sessions_dir(root: &Path) -> PathBuf {
    root.join("sessions")
}

fn session_path(root: &Path, id: &str) -> PathBuf {
    sessions_dir(root).join(format!("{id}.json"))
}

pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

pub fn load_session(root: &Path, id: &str) -> Option<Session> {
    if !valid_session_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(session_path(root, id)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_session(root: &Path, s: &Session) -> Result<()> {
    let p = session_path(root, &s.id);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn list_sessions(root: &Path) -> Vec<Session> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sessions_dir(root)) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    if let Ok(s) = serde_json::from_str::<Session>(&t) {
                        out.push(s);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    out
}

pub fn new_session_id() -> String {
    let b: [u8; 8] = crate::crypto::random_bytes();
    format!("s_{}", hex::encode(b))
}

// ------------------------------------------------------------------ attaching

/// A head's unfolded content, kept for prefix matching.
#[derive(Clone)]
struct Head {
    session: String,
    node: String,
    content: String,
    depth: u32,
    bytes: std::sync::Arc<Vec<u8>>,
}

/// In-memory side of the chains: recent heads and the sessions touched
/// lately. Everything here can be rebuilt from the files.
#[derive(Default)]
pub struct Chains {
    heads: parking_lot::Mutex<VecDeque<Head>>,
    sessions: parking_lot::Mutex<HashMap<String, Session>>,
    pub migration: parking_lot::Mutex<Migration>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Migration {
    pub running: bool,
    pub done: bool,
    pub total: usize,
    pub folded: usize,
    pub skipped: usize,
    pub saved_bytes: u64,
    pub error: Option<String>,
}

/// How a new prompt will be stored.
#[derive(Debug, Clone)]
pub struct Plan {
    pub header: Header,
    pub folded: Vec<u8>,
}

impl Chains {
    pub fn new() -> Chains {
        Chains::default()
    }

    fn remember_head(&self, h: Head) {
        let mut heads = self.heads.lock();
        heads.retain(|x| !(x.session == h.session && x.node == h.node));
        heads.push_front(h);
        let mut total: usize = heads.iter().map(|h| h.bytes.len()).sum();
        while heads.len() > HEAD_CACHE || (total > HEAD_CACHE_BYTES && heads.len() > 1) {
            if let Some(old) = heads.pop_back() {
                total -= old.bytes.len();
            }
        }
    }

    pub fn session(&self, root: &Path, id: &str) -> Option<Session> {
        if let Some(s) = self.sessions.lock().get(id) {
            return Some(s.clone());
        }
        let s = load_session(root, id)?;
        self.sessions.lock().insert(id.to_string(), s.clone());
        Some(s)
    }

    fn store_session(&self, root: &Path, s: Session) -> Result<()> {
        save_session(root, &s)?;
        self.sessions.lock().insert(s.id.clone(), s);
        Ok(())
    }

    /// Picks the parent for `content`: the head (of `hint`'s session when
    /// given, else any recent one) sharing the longest prefix.
    fn pick_parent(&self, content: &[u8], hint: Option<&str>) -> Option<Head> {
        let heads = self.heads.lock();
        let mut best: Option<(usize, &Head)> = None;
        for h in heads.iter() {
            if let Some(s) = hint {
                if h.session != s {
                    continue;
                }
            }
            let lcp = common_prefix(content, &h.bytes);
            // Enough shared bytes, and most of the parent (an unrelated prompt
            // with the same system message shares its start but not its body).
            // Ties go to the shorter parent: the one this prompt diverges
            // from least, which is where an edited conversation branches.
            let better = match best {
                None => true,
                Some((b, bh)) => lcp > b || (lcp == b && h.bytes.len() < bh.bytes.len()),
            };
            if lcp >= MIN_PREFIX && lcp * 2 >= h.bytes.len() && better {
                best = Some((lcp, h));
            }
        }
        best.map(|(_, h)| h.clone())
    }

    /// Plans where `content` goes and what gets written. `blob_path` is the
    /// blob's path relative to the records folder; `s3_prefix` the mirror's
    /// key prefix for blobs when S3 is on.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &self,
        root: &Path,
        content: &[u8],
        content_hash: &str,
        session_hint: Option<&str>,
        host: &str,
        agent: &str,
        project: &str,
        s3_key_of: &dyn Fn(&str) -> Option<String>,
        read: Reader,
    ) -> Result<Plan> {
        let hint = session_hint.filter(|s| valid_session_id(s));
        if let Some(h) = hint {
            // Warm the cache with that session's heads if they aren't in it.
            let known = self.heads.lock().iter().any(|x| x.session == h);
            if !known {
                if let Some(s) = self.session(root, h) {
                    for head in s.heads.iter().take(4) {
                        if let Some(n) = s.nodes.get(head) {
                            if let Ok(Some(raw)) = read(&n.content) {
                                if let Ok(u) = unfold(&raw, &mut |r| read(&r.sha256)) {
                                    if u.intact() {
                                        self.remember_head(Head {
                                            session: h.to_string(),
                                            node: n.node.clone(),
                                            content: n.content.clone(),
                                            depth: n.depth,
                                            bytes: std::sync::Arc::new(u.bytes),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let parent = self.pick_parent(content, hint);
        let created = chrono::Local::now().to_rfc3339();
        let (session, header, reference) = match parent {
            Some(p) => {
                let depth = p.depth + 1;
                let snapshot = depth % SNAPSHOT_EVERY == 0;
                let prefix = if snapshot {
                    0
                } else {
                    common_prefix(content, &p.bytes)
                };
                let reference = (!snapshot).then(|| Ref {
                    path: blob_rel_path(&p.content),
                    s3: s3_key_of(&p.content),
                    sha256: p.content.clone(),
                    from: 0,
                    to: prefix,
                });
                let header = Header {
                    session: p.session.clone(),
                    node: node_id(Some(&p.node), content_hash),
                    parent: Some(p.node.clone()),
                    content: content_hash.to_string(),
                    depth,
                    prefix,
                    snapshot,
                    created,
                };
                (p.session.clone(), header, reference)
            }
            None => {
                let session = hint.map(String::from).unwrap_or_else(new_session_id);
                let header = Header {
                    session: session.clone(),
                    node: node_id(None, content_hash),
                    parent: None,
                    content: content_hash.to_string(),
                    depth: 0,
                    prefix: 0,
                    snapshot: true,
                    created,
                };
                (session, header, None)
            }
        };
        let folded = fold(content, &header, reference.as_ref());
        // Record it in the session index now; the caller writes the blob.
        let mut s = self.session(root, &session).unwrap_or_else(|| Session {
            id: session.clone(),
            created: header.created.clone(),
            host: host.to_string(),
            agent: agent.to_string(),
            project: project.to_string(),
            ..Default::default()
        });
        if s.agent.is_empty() {
            s.agent = agent.to_string();
        }
        if s.project.is_empty() {
            s.project = project.to_string();
        }
        s.nodes.insert(
            header.node.clone(),
            Node {
                node: header.node.clone(),
                content: content_hash.to_string(),
                parent: header.parent.clone(),
                stem: String::new(),
                depth: header.depth,
                prefix: header.prefix,
                size: content.len(),
                snapshot: header.snapshot,
                missing: false,
                created: header.created.clone(),
            },
        );
        s.recompute_heads();
        self.store_session(root, s)?;
        self.remember_head(Head {
            session,
            node: header.node.clone(),
            content: content_hash.to_string(),
            depth: header.depth,
            bytes: std::sync::Arc::new(content.to_vec()),
        });
        Ok(Plan { header, folded })
    }

    /// Notes which record first saved a node.
    pub fn set_stem(&self, root: &Path, session: &str, node: &str, stem: &str) {
        if let Some(mut s) = self.session(root, session) {
            if let Some(n) = s.nodes.get_mut(node) {
                if n.stem.is_empty() {
                    n.stem = stem.to_string();
                    let _ = self.store_session(root, s);
                }
            }
        }
    }

    /// Finds the session and node a content hash belongs to.
    pub fn locate(&self, root: &Path, content_hash: &str) -> Option<(Session, Node)> {
        for s in self.sessions.lock().values() {
            if let Some(n) = s.by_content(content_hash) {
                return Some((s.clone(), n.clone()));
            }
        }
        for s in list_sessions(root) {
            if let Some(n) = s.by_content(content_hash).cloned() {
                self.sessions.lock().insert(s.id.clone(), s.clone());
                return Some((s, n));
            }
        }
        None
    }

    /// Erases a link: children are refolded against the grandparent (or made
    /// snapshots), each verified against its content hash, the index gets a
    /// tombstone and new heads. Returns the number of children rewritten.
    /// `open` reads and decrypts a blob; `seal_and_write` stores one.
    pub fn detach(
        &self,
        root: &Path,
        content_hash: &str,
        s3_key_of: &dyn Fn(&str) -> Option<String>,
        read: Reader,
        write_blob: Writer,
    ) -> Result<usize> {
        let Some((mut session, node)) = self.locate(root, content_hash) else {
            return Ok(0);
        };
        let children: Vec<Node> = session.children(&node.node).into_iter().cloned().collect();
        let grand = node
            .parent
            .as_deref()
            .and_then(|p| session.nodes.get(p))
            .filter(|g| !g.missing)
            .cloned();
        let grand_bytes = match &grand {
            Some(g) => match read(&g.content)? {
                Some(raw) => {
                    let u = unfold(&raw, &mut |r| read(&r.sha256))?;
                    u.intact().then_some(u.bytes)
                }
                None => None,
            },
            None => None,
        };
        let mut rewritten = 0;
        for child in &children {
            let Some(raw) = read(&child.content)? else {
                continue;
            };
            let u = unfold(&raw, &mut |r| read(&r.sha256))?;
            if !u.intact() {
                // Already damaged; leave it as it is.
                continue;
            }
            let content = u.bytes;
            let (parent_id, prefix, reference) = match (&grand, &grand_bytes) {
                (Some(g), Some(gb)) => {
                    let lcp = common_prefix(&content, gb);
                    if lcp >= MIN_PREFIX {
                        (
                            Some(g.node.clone()),
                            lcp,
                            Some(Ref {
                                path: blob_rel_path(&g.content),
                                s3: s3_key_of(&g.content),
                                sha256: g.content.clone(),
                                from: 0,
                                to: lcp,
                            }),
                        )
                    } else {
                        (Some(g.node.clone()), 0, None)
                    }
                }
                _ => (None, 0, None),
            };
            let header = Header {
                session: session.id.clone(),
                node: node_id(parent_id.as_deref(), &child.content),
                parent: parent_id.clone(),
                content: child.content.clone(),
                depth: grand.as_ref().map_or(0, |g| g.depth + 1),
                prefix,
                snapshot: reference.is_none(),
                created: child.created.clone(),
            };
            let folded = fold(&content, &header, reference.as_ref());
            // Verify before anything is written.
            let check = unfold(&folded, &mut |r| {
                if r.sha256
                    == header
                        .parent
                        .as_deref()
                        .and_then(|p| session.nodes.get(p))
                        .map(|g| g.content.clone())
                        .unwrap_or_default()
                {
                    Ok(grand_bytes.clone())
                } else {
                    read(&r.sha256)
                }
            })?;
            if check.bytes != content {
                bail!("refolding a child prompt did not reproduce it; nothing was deleted");
            }
            write_blob(&child.content, &folded)?;
            // Recalculate ids down this branch: every descendant's id depends
            // on its parent's.
            let old_id = child.node.clone();
            let new_id = header.node.clone();
            let mut n = session.nodes.remove(&old_id).unwrap();
            n.node = new_id.clone();
            n.parent = parent_id;
            n.depth = header.depth;
            n.prefix = prefix;
            n.snapshot = header.snapshot;
            session.nodes.insert(new_id.clone(), n);
            rename_descendants(
                &mut session,
                &old_id,
                &new_id,
                header.depth,
                read,
                write_blob,
            )?;
            rewritten += 1;
        }
        if let Some(n) = session.nodes.get_mut(&node.node) {
            n.missing = true;
        }
        session.recompute_heads();
        {
            // Cached heads keep their bytes; their ids and depths follow the index.
            let mut heads = self.heads.lock();
            heads.retain(|h| h.content != content_hash);
            for h in heads.iter_mut().filter(|h| h.session == session.id) {
                if let Some(n) = session.by_content(&h.content) {
                    h.node = n.node.clone();
                    h.depth = n.depth;
                }
            }
        }
        self.store_session(root, session)?;
        Ok(rewritten)
    }

    /// A blob disappeared behind Toktail's back: tombstone it.
    pub fn mark_missing(&self, root: &Path, content_hash: &str) {
        if let Some((mut s, n)) = self.locate(root, content_hash) {
            if let Some(x) = s.nodes.get_mut(&n.node) {
                x.missing = true;
            }
            s.recompute_heads();
            self.heads.lock().retain(|h| h.content != content_hash);
            let _ = self.store_session(root, s);
        }
    }
}

/// After a node's id changed, its descendants' ids change too (and their
/// headers must say so), without touching their bytes.
fn rename_descendants(
    session: &mut Session,
    old_parent: &str,
    new_parent: &str,
    parent_depth: u32,
    read: Reader,
    write_blob: Writer,
) -> Result<()> {
    let kids: Vec<String> = session
        .nodes
        .values()
        .filter(|n| n.parent.as_deref() == Some(old_parent))
        .map(|n| n.node.clone())
        .collect();
    for old_id in kids {
        let mut n = session.nodes.remove(&old_id).unwrap();
        let new_id = node_id(Some(new_parent), &n.content);
        n.node = new_id.clone();
        n.parent = Some(new_parent.to_string());
        n.depth = parent_depth + 1;
        if !n.missing {
            if let Some(raw) = read(&n.content)? {
                if let Parsed::Folded {
                    mut header,
                    reference,
                    delta,
                } = parse(&raw)?
                {
                    header.node = new_id.clone();
                    header.parent = Some(new_parent.to_string());
                    header.depth = parent_depth + 1;
                    let mut out = header.line().into_bytes();
                    out.push(b'\n');
                    if let Some(r) = reference {
                        out.extend_from_slice(r.line().as_bytes());
                        out.push(b'\n');
                    }
                    out.extend_from_slice(delta);
                    write_blob(&n.content, &out)?;
                }
            }
        }
        session.nodes.insert(new_id.clone(), n);
        rename_descendants(
            session,
            &old_id,
            &new_id,
            parent_depth + 1,
            read,
            write_blob,
        )?;
    }
    Ok(())
}

pub fn blob_rel_path(hash: &str) -> String {
    format!("blobs/{}/{}.prompt", &hash[..2.min(hash.len())], hash)
}

/// Reads and decrypts a blob by content hash; `Ok(None)` when it's gone.
pub type Reader<'a> = &'a dyn Fn(&str) -> Result<Option<Vec<u8>>>;
/// Seals and writes a blob by content hash.
pub type Writer<'a> = &'a dyn Fn(&str, &[u8]) -> Result<()>;

/// A reader for plain (unencrypted) blobs under `root`, for tests and tools.
#[cfg(test)]
pub fn plain_reader(root: &Path) -> impl Fn(&str) -> Result<Option<Vec<u8>>> + '_ {
    move |hash: &str| {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("bad blob hash");
        }
        match std::fs::read(root.join(blob_rel_path(hash))) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Reads a blob's content in full.
#[cfg(test)]
pub fn read_blob(read: Reader, hash: &str) -> Result<Option<Unfolded>> {
    match read(hash)? {
        Some(raw) => Ok(Some(unfold(&raw, &mut |r| read(&r.sha256))?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("toktail-chain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn write_raw(root: &Path, hash: &str, bytes: &[u8]) {
        let p = root.join(blob_rel_path(hash));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    fn store(root: &Path, content: &[u8], plan: &Plan) {
        write_raw(root, &plan.header.content, &plan.folded);
        assert_eq!(sha256_hex(content), plan.header.content);
    }

    #[test]
    fn fold_and_unfold_are_byte_exact() {
        let parent = b"{\"messages\":[{\"role\":\"system\",\"content\":\"a\\nb\\n\\n  c\"},{\"role\":\"user\",\"content\":\"hi\"}]}\n\n\r\n  ".to_vec();
        let child = {
            let mut c = parent[..parent.len() - 7].to_vec();
            c.extend_from_slice(b",{\"role\":\"assistant\",\"content\":\"hello\"}]}\n\n\r\n  \t");
            c
        };
        let lcp = common_prefix(&parent, &child);
        assert!(lcp > 60 && lcp < parent.len());
        let ph = sha256_hex(&parent);
        let header = Header {
            session: "s_1".into(),
            node: node_id(Some("p"), &sha256_hex(&child)),
            parent: Some("p".into()),
            content: sha256_hex(&child),
            depth: 1,
            prefix: lcp,
            snapshot: false,
            created: "2026-10-15T10:00:00+00:00".into(),
        };
        let r = Ref {
            path: blob_rel_path(&ph),
            s3: Some("records/x".into()),
            sha256: ph.clone(),
            from: 0,
            to: lcp,
        };
        let folded = fold(&child, &header, Some(&r));
        assert!(
            folded.ends_with(&child[lcp..]),
            "only the new bytes are stored"
        );
        assert_eq!(
            folded.len(),
            header.line().len() + 1 + r.line().len() + 1 + child.len() - lcp
        );
        assert!(is_folded(&folded));
        match parse(&folded).unwrap() {
            Parsed::Folded {
                header: h2,
                reference,
                delta,
            } => {
                assert_eq!(h2, header);
                assert_eq!(reference.unwrap(), r);
                assert_eq!(delta, &child[lcp..]);
            }
            _ => panic!("not folded"),
        }
        let mut calls = 0;
        let u = unfold(&folded, &mut |rf| {
            calls += 1;
            assert_eq!(rf.sha256, ph);
            Ok(Some(parent.clone()))
        })
        .unwrap();
        assert_eq!(
            u.bytes, child,
            "line breaks, carriage returns and trailing spaces survive"
        );
        assert!(u.intact() && u.depth == 1 && calls == 1);
        // A tampered delta fails the hash check.
        let mut bad = folded.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(unfold(&bad, &mut |_| Ok(Some(parent.clone()))).is_err());
        // A plain v1 blob is its own content.
        let u = unfold(&parent, &mut |_| unreachable!()).unwrap();
        assert_eq!(u.bytes, parent);
    }

    #[test]
    fn header_quoting_round_trips() {
        let h = Header {
            session: "s_\"q\"".into(),
            node: "n".into(),
            parent: None,
            content: "c".into(),
            depth: 3,
            prefix: 0,
            snapshot: true,
            created: "t\nx".into(),
        };
        assert_eq!(Header::parse(&h.line()).unwrap(), h);
        let r = Ref {
            path: "blobs/aa/aa.prompt".into(),
            s3: None,
            sha256: "a".repeat(64),
            from: 0,
            to: 10,
        };
        assert_eq!(Ref::parse(&r.line()).unwrap(), r);
    }

    #[test]
    fn a_missing_link_is_marked_not_fatal() {
        let child = b"PREFIX-PREFIX-PREFIX tail".to_vec();
        let header = Header {
            session: "s".into(),
            node: "n".into(),
            parent: Some("p".into()),
            content: sha256_hex(&child),
            depth: 1,
            prefix: 20,
            snapshot: false,
            created: String::new(),
        };
        let r = Ref {
            path: "x".into(),
            s3: None,
            sha256: "b".repeat(64),
            from: 0,
            to: 20,
        };
        let folded = fold(&child, &header, Some(&r));
        let u = unfold(&folded, &mut |_| Ok(None)).unwrap();
        assert!(!u.intact());
        let text = String::from_utf8(u.bytes).unwrap();
        assert!(
            text.starts_with("{{toktail:missing bytes=\"0-20\""),
            "{text}"
        );
        assert!(text.ends_with(" tail"));
    }

    fn turn(n: usize) -> Vec<u8> {
        // A growing conversation, like an agent resending history every turn.
        let mut s = String::from("{\"messages\":[{\"role\":\"system\",\"content\":\"You are a careful coding assistant working in /Users/me/proj. Keep answers short.\"}");
        for i in 0..n {
            let filler = "x".repeat(300 + i % 7);
            s.push_str(&format!(",{{\"role\":\"user\",\"content\":\"step {i}: please change file{i}.rs\\n\\nthanks {filler}\"}},{{\"role\":\"assistant\",\"content\":\"done {i}\"}}"));
        }
        s.push_str(",{\"role\":\"user\",\"content\":\"next\"}]}");
        s.into_bytes()
    }

    #[test]
    fn a_session_chains_and_snapshots_every_32() {
        let root = tmp();
        let chains = Chains::new();
        let none = |_: &str| None;
        let rd = plain_reader(&root);
        let mut hashes = Vec::new();
        for n in 1..=70 {
            let c = turn(n);
            let h = sha256_hex(&c);
            let plan = chains
                .plan(
                    &root, &c, &h, None, "studio", "opencode", "proj", &none, &rd,
                )
                .unwrap();
            store(&root, &c, &plan);
            hashes.push((h, plan.header.clone(), plan.folded.len(), c.len()));
        }
        let sess_id = hashes[0].1.session.clone();
        assert!(
            hashes.iter().all(|(_, h, _, _)| h.session == sess_id),
            "one session"
        );
        for (i, (_, h, stored, full)) in hashes.iter().enumerate() {
            assert_eq!(h.depth as usize, i);
            if i % 32 == 0 {
                assert!(h.snapshot && h.prefix == 0 && *stored >= *full);
            } else {
                assert!(!h.snapshot && h.prefix > MIN_PREFIX, "turn {i}");
                // Stored = two header lines + the new bytes, nothing of the prefix.
                assert!(
                    *stored < full - h.prefix + 700,
                    "turn {i}: {stored} of {full}, prefix {}",
                    h.prefix
                );
                if i >= 6 {
                    assert!(*stored < *full / 2, "turn {i}: {stored} of {full}");
                }
            }
        }
        // Every turn unfolds back to its exact bytes, never more than 32 hops.
        for (i, (h, _, _, _)) in hashes.iter().enumerate() {
            let u = read_blob(&rd, h).unwrap().unwrap();
            assert_eq!(u.bytes, turn(i + 1));
            assert!(u.depth <= 32);
        }
        let s = chains.session(&root, &sess_id).unwrap();
        assert_eq!(s.nodes.len(), 70);
        assert_eq!(s.heads.len(), 1);
        let (saved, full) = s.savings();
        assert!(saved * 10 > full * 9, "saved {saved} of {full}");
        // The index rebuilds from a fresh Chains against the same files.
        let again = Chains::new();
        assert_eq!(again.session(&root, &sess_id).unwrap().nodes.len(), 70);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn edits_branch_and_merkle_ids_commit_to_history() {
        let root = tmp();
        let chains = Chains::new();
        let none = |_: &str| None;
        let rd = plain_reader(&root);
        let mut ids = Vec::new();
        for n in 1..=4 {
            let c = turn(n);
            let plan = chains
                .plan(&root, &c, &sha256_hex(&c), None, "", "", "", &none, &rd)
                .unwrap();
            store(&root, &c, &plan);
            ids.push(plan.header);
        }
        // The user edits turn 2's last message and continues: a new branch
        // off turn 2 (the prompt it shares the most with), not the head.
        let mut edited = turn(2);
        let at = edited.len() - 20;
        edited.splice(at..at, b"EDITED ".iter().cloned());
        let plan = chains
            .plan(
                &root,
                &edited,
                &sha256_hex(&edited),
                None,
                "",
                "",
                "",
                &none,
                &rd,
            )
            .unwrap();
        store(&root, &edited, &plan);
        assert_eq!(
            plan.header.parent.as_deref(),
            Some(ids[1].node.as_str()),
            "branches off turn 2, not the head"
        );
        assert_eq!(plan.header.session, ids[0].session);
        let s = chains.session(&root, &ids[0].session).unwrap();
        assert_eq!(s.heads.len(), 2, "two branch tips");
        assert_eq!(s.children(&ids[1].node).len(), 2);
        // Continuing the edited branch attaches to it.
        let mut cont = edited.clone();
        let at = cont.len() - 2;
        cont.splice(at..at, b",{\"role\":\"assistant\",\"content\":\"ok\"},{\"role\":\"user\",\"content\":\"go on\"}".iter().cloned());
        let plan2 = chains
            .plan(
                &root,
                &cont,
                &sha256_hex(&cont),
                None,
                "",
                "",
                "",
                &none,
                &rd,
            )
            .unwrap();
        assert_eq!(
            plan2.header.parent.as_deref(),
            Some(plan.header.node.as_str())
        );
        // Ids: same content under a different parent is a different node.
        assert_ne!(node_id(Some("a"), "c"), node_id(Some("b"), "c"));
        assert_eq!(ids[3].node, node_id(Some(&ids[2].node), &ids[3].content));
        assert_eq!(s.root_of(&ids[3].node).unwrap().node, ids[0].node);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn session_hints_and_unrelated_prompts() {
        let root = tmp();
        let chains = Chains::new();
        let none = |_: &str| None;
        let rd = plain_reader(&root);
        let a = turn(3);
        let pa = chains
            .plan(
                &root,
                &a,
                &sha256_hex(&a),
                Some("roc-abc"),
                "",
                "",
                "",
                &none,
                &rd,
            )
            .unwrap();
        store(&root, &a, &pa);
        assert_eq!(pa.header.session, "roc-abc");
        // Same system prompt, different conversation: shares a start but not enough.
        let mut b = turn(1);
        b.truncate(b.len() - 30);
        b.extend_from_slice(b"\"content\":\"totally different\"}]}");
        let pb = chains
            .plan(&root, &b, &sha256_hex(&b), None, "", "", "", &none, &rd)
            .unwrap();
        assert!(pb.header.parent.is_none() && pb.header.session != "roc-abc");
        // A hinted session only attaches within that session.
        let c = turn(4);
        let pc = chains
            .plan(
                &root,
                &c,
                &sha256_hex(&c),
                Some("other"),
                "",
                "",
                "",
                &none,
                &rd,
            )
            .unwrap();
        assert_eq!(pc.header.session, "other");
        assert!(pc.header.parent.is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn erasing_a_link_rebalances_and_verifies_the_rest() {
        let root = tmp();
        let chains = Chains::new();
        let none = |_: &str| None;
        let rd = plain_reader(&root);
        let mut hs = Vec::new();
        for n in 1..=6 {
            let c = turn(n);
            let plan = chains
                .plan(&root, &c, &sha256_hex(&c), None, "", "", "", &none, &rd)
                .unwrap();
            store(&root, &c, &plan);
            hs.push(plan.header);
        }
        let sess = hs[0].session.clone();
        let writer = |hash: &str, bytes: &[u8]| {
            write_raw(&root, hash, bytes);
            Ok(())
        };
        // Erase turn 3 (depth 2): turn 4 must be refolded against turn 2.
        let n = chains
            .detach(&root, &hs[2].content, &none, &rd, &writer)
            .unwrap();
        assert_eq!(n, 1);
        std::fs::remove_file(root.join(blob_rel_path(&hs[2].content))).unwrap();
        for (i, h) in hs.iter().enumerate() {
            if i == 2 {
                continue;
            }
            let u = read_blob(&rd, &h.content).unwrap().unwrap();
            assert_eq!(u.bytes, turn(i + 1), "turn {} still unfolds exactly", i + 1);
            assert!(u.intact());
        }
        let s = chains.session(&root, &sess).unwrap();
        let gone = s.by_content(&hs[2].content).unwrap();
        assert!(gone.missing, "tombstoned");
        let t4 = s.by_content(&hs[3].content).unwrap();
        assert_eq!(t4.parent.as_deref(), Some(hs[1].node.as_str()));
        assert_eq!(t4.depth, 2);
        assert_eq!(
            t4.node,
            node_id(Some(&hs[1].node), &hs[3].content),
            "ids recalculated down the chain"
        );
        let t6 = s.by_content(&hs[5].content).unwrap();
        assert_eq!(t6.depth, 4);
        assert_eq!(s.heads, vec![t6.node.clone()]);
        // Erase the root: its child becomes a snapshot.
        let n = chains
            .detach(&root, &hs[0].content, &none, &rd, &writer)
            .unwrap();
        assert_eq!(n, 1);
        std::fs::remove_file(root.join(blob_rel_path(&hs[0].content))).unwrap();
        let u = read_blob(&rd, &hs[1].content).unwrap().unwrap();
        assert_eq!(u.bytes, turn(2));
        assert!(u.header.unwrap().snapshot);
        let u = read_blob(&rd, &hs[5].content).unwrap().unwrap();
        assert_eq!(u.bytes, turn(6));
        // The next prompt attaches to the rebalanced head.
        let c = turn(7);
        let plan = chains
            .plan(
                &root,
                &c,
                &sha256_hex(&c),
                Some(&sess),
                "",
                "",
                "",
                &none,
                &rd,
            )
            .unwrap();
        let s = chains.session(&root, &sess).unwrap();
        assert_eq!(
            plan.header.parent.as_deref(),
            Some(s.by_content(&hs[5].content).unwrap().node.as_str())
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
