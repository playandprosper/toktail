# Upgrading to Toktail 0.0.3

Toktail's version numbers start over at 0.0.3. It picks up from 0.6 (and,
through 0.6's own migration, from 0.4 and the 0.5 preview). Your CSV history,
settings, saved prompts and secrets carry over on their own; this page says
what changes on disk and how to check that it went well.

## Before you start

Quit Toktail, then copy Toktail's folder somewhere safe:

| | Folder |
|---|---|
| macOS | `~/Library/Application Support/Toktail` |
| Windows | `%APPDATA%\Toktail` |
| Linux | `~/.local/share/Toktail` |

Nothing below needs the copy, but it costs nothing and makes every step
reversible.

## From 0.6

**CSV files gain nine columns.** 0.6 wrote 23 columns; 0.0.3 writes 32
(`token_id`, `cache_read_tokens`, `cache_write_tokens`, `cache_hit`,
`session_id`, `chain_node`, `prefix_tokens`, `agent`, `client`). On first
start each monthly file with the old header is copied to
`tokens-YYYY-MM.csv.pre-v0.0.3.bak` and rewritten with the new header.
Existing rows keep every value; the new columns start empty. The backup is
made once and never overwritten. Delete the `.bak` files when you're happy.
(Files still carrying the 0.4 header are backed up as `.pre-v0.5.bak` and
migrated the same way.)

**Saved prompts become session chains.** Twenty seconds after launch,
Toktail folds every saved prompt blob into a chain in the background, oldest
first: each blob is rewritten as only the bytes that are new over the prompt
it grew from, plus a reference to that prompt by path and checksum. Every
rewrite is verified to unfold back to the original before the original is
replaced, and the original is copied to `records/.pre-v0.0.3/blobs/…` first.
The Prompts → Sessions page shows the migration's progress and, when it's
done, a **Free space** button that removes the originals. Until then
everything stays readable twice over. Records open the same as before during
and after the migration.

**Hosts get a type.** Every existing host is an **LM Studio** host, exactly
as before: the stored API token replaces whatever your apps send. New host
types (Anthropic, OpenAI, another OpenAI-compatible API) handle keys
differently; see the README's *Public API hosts and keys*.

**A secret for token ids.** The first time a request carries a credential,
Toktail creates an install secret in your Keychain / Credential Manager /
secret service (service "Toktail", item `install-secret`) and uses it to make the one-way
`token_id` values in the CSV. Nothing to do; if you remove the secret, new
ids simply won't match old ones.

**Pip is in 3D.** The portal's Pip is now the same three.js fox as the
landing page. Without WebGL, or with reduced motion, the SVG fox is used.

**Prices.** Market prices are checked once a day from LiteLLM and OpenRouter
(no key needed) and kept under `market/` in Toktail's folder; the comparison
uses them, dated, from the day they were fetched. The built-in `pricing.json`
and your override still apply to days before the first snapshot. Turn it off
in Settings → Cost comparison.

## From the 0.0.2 preview

Builds that said 0.0.2 were previews of this release, so nothing on disk
changes. Their backups keep their names: `tokens-YYYY-MM.csv.pre-v0.0.2.bak`
files (delete them when you're happy) and `records/.pre-v0.0.2/`, which the
Sessions page counts and **Free space** removes along with any newer
originals. The portal opens in your browser again (the preview's own window
is gone), and the Right now card can now cancel, reroute and join requests
in flight (Settings → In flight).

## The portal needs a sign-in now

Programs on your computer, coding agents in Docker containers included, could
reach the portal's API by faking the Host header. The portal now opens only
for browsers you open from Toktail: **Open Toktail** in the menu bar (or the
system tray), `make open`, or `Toktail open`. A bookmark or typed address
shows a page saying so; open it once from the menu bar and that browser stays
signed in for 30 days. Scripts that called the portal's JSON should move to
the public API (Settings → API & MCP, with a key). `/metrics`, `/api/ping`
and the new `/api/status` still answer without signing in; `make status`
uses `/api/status`.

## New settings

All optional; all editable in Settings. The `[api]`, `[autopilot]` and
`[summaries]` sections are new in this release; the README's Configuration
section lists every key.

```toml
[cache]
enabled = false          # answer identical requests from disk
ttl_hours = 72
max_mb = 2048

[flights]
join_duplicates = true       # an identical retry gets the running request's answer
abandoned_grace_secs = 180   # keep a request the agent hung up on, for its retry

[commentary]
enabled = true           # Pip's one-liners about what your agents are doing
every_minutes = 5

[market]
enabled = true           # daily LiteLLM / OpenRouter price check
next_source = "litellm"  # alternates on its own

[costs]
cache_aware = false      # price repeated prompt prefixes at the cached-input rate
market_prices = true     # use the daily market prices

[api]
enabled = true           # REST /api/v1 and MCP /mcp on their own port
port = 8766
allow_network = false

[autopilot]
roc_state = ""           # empty = ~/.local/roc/state.json

[summaries]
enabled = true           # Pip's one-line summary of every prompt
bubbles = true

[[hosts]]
# ...as before, plus:
kind = "lmstudio"        # "lmstudio", "anthropic", "openai" or "other"
inject_key = true        # public hosts: add the stored key for apps on this computer
# gateway_keys = [...]   # issued in Settings → Hosts; only hashes are stored
```

## Per platform

**macOS.** The first start of a new build may ask for Keychain access again:
choose **Always Allow**.

**Linux.** `make install` builds with the tray icon when GTK and
libayatana-appindicator are installed, and headless otherwise. `make service` installs a
systemd user service for machines with no desktop.

## Check that it worked

From the source folder:

```sh
make test        # Rust tests: chains fold and unfold byte for byte, keys, cache, migration
make lint        # fmt and clippy
make check-ui    # every portal script parses and is served
make demo        # sample data on http://127.0.0.1:8799, your files untouched
```

Then open Toktail and compare the dashboard's all-time total with the one you
had before upgrading: it should match to the token. Open any saved prompt
from before the upgrade and download the original: it's the same bytes. On
Prompts → Sessions, wait for "Migration done".

## Going back

Quit Toktail, restore the folder you copied, and reinstall 0.6. If you didn't
copy the folder: rename each `tokens-YYYY-MM.csv.pre-v0.0.3.bak` back to
`tokens-YYYY-MM.csv`, and copy `records/.pre-v0.0.3/blobs/` over
`records/blobs/` (only if you haven't pressed **Free space**). Requests made
while 0.0.3 ran are only in the new files, and prompts saved by 0.0.3 are
chains that 0.6 can't open.
