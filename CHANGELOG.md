# Changelog

Toktail's version numbers start over at 0.0.2. Everything before it (0.4,
the 0.5 preview, 0.6) is "pre-0.0.2"; see [UPGRADE.md](UPGRADE.md).

## 0.0.2

### Keys and public APIs
- Hosts have a type: LM Studio (as before), Anthropic, OpenAI or another
  OpenAI-compatible API. Public hosts pass the agent's own key through
  untouched; the CSV gets a one-way HMAC id (`token_id`), never the key.
- Toktail gateway keys (`tkg_…`): stand-ins for a provider key stored in the
  Keychain, hashed at rest, shown once, revocable, per-agent ids in the CSV,
  usable from the network or this computer only.
- The stored key is added only for requests from this computer that carry
  none; requests from other machines without a key are refused. A provider
  key arriving over the LAN is noted once with advice to use a gateway key.
- Saved prompts, responses and logs are scrubbed of key-shaped strings.
- `X-Toktail-Session`, `X-Toktail-Agent`, `X-Toktail-Project`,
  `X-Toktail-Client` and `X-Toktail-Cache` are read and stripped before
  forwarding.
- Anthropic `cache_read_input_tokens` / `cache_creation_input_tokens` and
  OpenAI `cached_tokens` are parsed into `cache_read_tokens` /
  `cache_write_tokens`.
- Public hosts count 401/403 as reachable, never raise "host down", and are
  left out of Pip's model list and the savings comparison.

### Prompt cache
- Identical requests (host, endpoint, canonical body, credential id) are
  answered from encrypted entries on disk; streams replayed as streams.
  TTL 72 h and 2 GB cap by default, both settable; `Cache-Control: no-cache`
  / `X-Toktail-Cache: bypass|refresh`; errors, refusals and cancels never
  cached; hits get marked CSV rows and a dashboard stat, outside budgets,
  live speed and electricity.

### Session chains
- Saved prompt blobs are folded: a `{{toktail:chain …}}` header, a
  `{{toktail:ref path= s3= sha256= bytes=}}` line naming the parent, and only
  the new bytes. Unfolding is byte-exact and hash-verified; a snapshot every
  32 links; per-session Merkle index rebuildable from the files; branches on
  edits; `session_id`, `chain_node` and `prefix_tokens` in the CSV.
- Deleting a record refolds and verifies its children before the file goes
  (struck through, then a puff of smoke in the portal); blobs lost outside
  Toktail are tombstoned and show a `{{toktail:missing …}}` marker.
- Background migration of pre-0.0.2 blobs, originals kept under
  `records/.pre-v0.0.2/` until **Free space**.
- Prompts → Sessions page.

### Prices
- Daily market prices from LiteLLM and OpenRouter in turn (no key; an
  optional OpenRouter key in Settings), dated snapshots applied as per-day
  price changes, provider listings preferred over resellers, `market_ids`
  pins in `pricing.json`, **Check prices now**.
- Cache-aware cloud estimate: repeated prompt prefixes priced at the
  providers' cached-input rate (toggle).

### Pip
- Pip in 3D: the landing page's three.js fox skins the portal rig, with
  live tokens/sec on its tail screen, trot/run/sleep/hop clips, emotes and a
  rendered icon; SVG fallback without WebGL or with reduced motion.
- Commentary: for coding agents (opencode, Claude Code, Codex, goose, roc)
  on LM Studio hosts on your network, Pip asks the same model for one
  whimsical sentence, at most once per interval, and says it in a speech
  bubble; a greeting the first time an agent shows up; **u** / **Space** on
  the dashboard asks right away. Never for public hosts.
- The menu bar's Today line is two lines: the total, then "out · in"
  indented beneath it.

### Portal
- Chat threads: encrypted per-thread files with a sidebar (search, rename,
  pin, delete), each thread its own prompt chain; a Full screen view (just
  the chat over a blurred page; Esc twice to leave).
- A **Yesterday** range button next to Today (dashboard, prompts, palette).
- Live feed tags cache hits and agents; a Cache hits stat.
- Settings: host type and gateway keys, Prompt cache, Pip's commentary and
  window, market prices and the cache-aware estimate.

### Files
- CSV gains `token_id`, `cache_read_tokens`, `cache_write_tokens`,
  `cache_hit`, `session_id`, `chain_node`, `prefix_tokens`, `agent`,
  `client` (backup `tokens-YYYY-MM.csv.pre-v0.0.2.bak`).
- New folders in Toktail's base: `cache/`, `market/`, `threads/`,
  `records/sessions/`, `records/.pre-v0.0.2/`.
- New modules: `tokens.rs`, `cache.rs`, `chain.rs`, `commentary.rs`,
  `market.rs`, `threads.rs`; three.js vendored under
  `assets/vendor/`.
