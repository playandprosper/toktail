# Changelog

Toktail's version numbers start over at 0.0.3. Everything before it (0.4,
the 0.5 preview, 0.6) is "pre-0.0.3"; see [UPGRADE.md](UPGRADE.md). Builds
labelled 0.0.2 were previews of 0.0.3; upgrading from one changes nothing on
disk.

## 0.0.3

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
- Background migration of pre-0.0.3 blobs, originals kept under
  `records/.pre-v0.0.3/` until **Free space** (which also clears a 0.0.2
  preview's `records/.pre-v0.0.2/`).
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

### Requests in flight
- Cancel any request on the Right now card (two-step button): the model
  server's connection is closed and the agent gets HTTP 499, which agents
  don't retry.
- Reroute a request to another host and a model that host has (the dialog
  lists each host's models live, and the server refuses one it lacks) until
  its first token; the agent that sent it gets that answer on the same
  request, with `X-Toktail-Rerouted` saying where it came from. The agent's
  key is never forwarded to another host.
- Repeated prompts are recognised by their input (messages, system, tools,
  response format, model), not their bytes: agents change `temperature` or
  `max_tokens` between retries. A retry is answered with the original's
  answer, streamed from the first byte, so the model reads a 700K-token
  prompt once instead of once per retry (automatic, or **Use #N's answer**).
- When an agent hangs up, its request keeps running for a grace period (180 s)
  so the retry can join it, then stops; hang-ups are noticed even before the
  model server answers. An answer that finishes before the retry arrives is
  kept for the retry.
- A diagnosis above the list explains a pile-up in plain words: how many
  times the same prompt was sent, which attempt gets the answer, what's
  queued behind it and how many tokens that is, and why (LM Studio reads
  one prompt at a time per model). Buttons: **Answer them with #N's
  answer** (merge copies already running) and **Send the waiting N to
  another machine…**. Rows show their place in line and prompt size.
- A streamed answer ends at its last event (`[DONE]`, `message_stop`,
  `response.completed`) even if the server keeps the connection open.
- Select several to cancel or reroute together; the list holds still under
  the pointer.
- Settings → In flight; `/api/flights/*`.

### Portal
- Chat threads: encrypted per-thread files with a sidebar (search, rename,
  pin, delete), each thread its own prompt chain; a Full screen view (just
  the chat over a blurred page; Esc twice to leave).
- A **Yesterday** range button next to Today (dashboard, prompts, palette).
- Live feed tags cache hits and agents; a Cache hits stat.
- Settings: host type and gateway keys, Prompt cache, Pip's commentary and
  window, market prices and the cache-aware estimate.

### Autopilot (with roc 0.3)
- The **Agents** page: every session roc runs, and for opencode sessions
  (roc publishes their own server on 127.0.0.1 with a per-session password)
  autopilot, which keeps the agent working until you stop it.
- When the session goes quiet it sends your words (Settings → Autopilot): a
  firm "it isn't finished" when the agent says it's done, "continue"
  otherwise, with `{{goal}}`, `{{verifier}}` (a command run in the agent's
  container), `{{turn}}` and `{{done_claims}}`.
- The agent's questions come to you (Agents page, bell, notification) and
  nothing is sent until they're answered; after a time you set, autopilot
  sends your default answer and marks it.
- Stop after this turn, stop now, end the roc session (`roc -kill`); pauses
  on identical replies, stops on failed turns with a diagnosis, or at your
  prompt or token limit; `NEED_HUMAN:` from the agent alerts you.
- A banner on every page while autopilot drives; runs resume after restart.
- Pip lists coding sessions and, when your message asks, starts or stops
  autopilot.

### API and MCP
- REST at `/api/v1` with an OpenAPI 3.1 document, and an MCP server at
  `/mcp` (Streamable HTTP, 2024-11-05 through 2026-07-28) with the same 14
  operations, on port 8766.
- Keys (`tkm_…`, hashed) with `read` and `records` scopes, "this computer"
  or network; Settings → API & MCP with **Connect a client** (Open WebUI,
  OpenAPI tools, Claude Code).
- `read_record` gives a prompt's place in its session chain and, by
  default, only what that turn added; whole bodies go to `records` keys when
  records aren't encrypted, or when you allow it.

### Pip's summaries
- One sentence per prompt to a host on your network ("what's this prompt?"),
  from the same model while it's idle, deduplicated, sealed, searchable on
  the Prompts page, returned by the API and said in Pip's bubble.

### Sign-in, session chains and This computer
- Sign-in: one-time links from the menu bar or `Toktail open` set an
  `HttpOnly`, `SameSite=Strict` cookie; everything but `/api/ping`,
  `/api/status` and `/metrics` needs it.
- Session chains are a flat timeline with link numbers and folded straight
  runs, readable at hundreds or thousands of links (they used to indent
  every link).
- Settings → This computer: Toktail's and the host's memory, CPU, pressure,
  swap and disk, live.
- The intro overlay no longer stays on screen when the intro is off or
  motion is reduced.

### Building
- `VERSION` (kept with `bump`) is the version; `make version` copies it into
  `Cargo.toml` and the build refuses a mismatch; `make beta` bumps the beta.

### Files
- CSV gains `token_id`, `cache_read_tokens`, `cache_write_tokens`,
  `cache_hit`, `session_id`, `chain_node`, `prefix_tokens`, `agent`,
  `client` (backup `tokens-YYYY-MM.csv.pre-v0.0.3.bak`).
- New folders in Toktail's base: `cache/`, `market/`, `threads/`,
  `records/sessions/`, `records/.pre-v0.0.3/`.
- New folders and files: `summaries/`, `portal-login/`,
  `portal-sessions.json`, `autopilot.json` (all readable only by you).
- New modules: `tokens.rs`, `cache.rs`, `chain.rs`, `commentary.rs`,
  `market.rs`, `threads.rs`, `portal_auth.rs`, `api.rs`, `roc.rs`,
  `opencode.rs`, `autopilot.rs`, `portal_agents.rs`, `summaries.rs`,
  `hoststatus.rs`; `assets/agents.js`; three.js vendored under
  `assets/vendor/`.
