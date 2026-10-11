# TokenLedger

A menu bar app for macOS that counts the tokens your LM Studio host renders, per
model per day. Every request is appended as one row to a CSV file on the host,
and that CSV is the source of truth for everything the app shows. The files are
mirrored to an S3-compatible bucket, and a local-only dashboard lets you explore
the history, filter by model and date range, and export any chart as SVG.

```
 your apps / agents ──► :1234  TokenLedger proxy ──► :1235  LM Studio
                              │
                              ├─ appends one row per request ──► tokens-YYYY-MM.csv  (source of truth)
                              │                                          │
                              │                                          └─ every 15 min ──► S3-compatible bucket
                              ├─ menu bar: today's count, per-model breakdown
                              └─ http://127.0.0.1:8765  dashboard (this Mac only)
```

## How tokens are counted

TokenLedger sits in front of LM Studio as a transparent reverse proxy. Clients
keep using port 1234; LM Studio moves to 1235. Every request is forwarded
unchanged, and on the way back TokenLedger reads the token counts LM Studio
reports:

| Counter | Meaning |
|---|---|
| `input_tokens` | Prompt tokens LM Studio processed (system prompt + history + new turn) |
| `output_tokens` | Tokens LM Studio generated |

Counted endpoints: `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`,
`/v1/responses`, `/v1/messages`, and LM Studio's native `/api/v0/*` and
`/api/v1/chat`. Everything else (model lists, model load/unload) is passed
through and not recorded.

For streamed chat and completion requests, TokenLedger asks LM Studio to append
a usage summary to the stream (`stream_options.include_usage`). If your client
didn't ask for that summary, TokenLedger removes it before it reaches the client,
so clients see exactly what they would have seen without the proxy.

If LM Studio ever returns no usage at all, TokenLedger counts streamed deltas
instead (about one token each). Those rows are marked `usage_source=estimated`
and their input count is 0.

**What is not counted:** chats typed into LM Studio's own chat window. Those
never touch the API server, so no proxy can see them.

## Privacy

The CSV never contains the prompt, the response, client IP addresses, or the
host name. The prompt is stored only as a SHA-256 fingerprint, so you can see
when the same prompt was sent again without being able to read it.

The fingerprint is the SHA-256 of the request's prompt fields (`system`,
`instructions`, `messages`, `prompt`, `input`, whichever are present), serialized
as compact JSON with object keys sorted. To check a fingerprint against a
request you have:

```sh
cat request.json | /Applications/TokenLedger.app/Contents/MacOS/TokenLedger fingerprint
```

A plain hash can't be reversed, but a short or predictable prompt (for example
"hi") can be confirmed by hashing a guess and comparing.

Request bodies are never written to the log either. The dashboard binds to
127.0.0.1 only and rejects requests with any other `Host` header, so web pages
can't reach it through DNS rebinding.

## Install

You need Rust on the Mac Studio (one time):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then build and install the app:

```sh
cd tokenledger
./scripts/build-app.sh --install
```

This builds `dist/TokenLedger.app`, copies it to `/Applications` and starts it.
The bar-chart icon appears in the menu bar and the dashboard opens in your
browser. On first start it also enables **Launch at Login** (toggle it in the
menu).

### Move LM Studio to port 1235

TokenLedger takes port 1234, so LM Studio's server has to listen on 1235. Change
the server port in LM Studio's Developer tab, or start the server from the
terminal (LM Studio remembers the last port used):

```sh
lms server start --port 1235
```

Until the port is free, the menu shows **⚠ Proxy can't use 127.0.0.1:1234**, and
TokenLedger retries every 10 seconds. Your clients don't change anything; they
keep calling `http://localhost:1234/v1`.

If other machines on your network call this host, set `listen = "0.0.0.0:1234"`
in the config. LM Studio can then stay bound to localhost only.

## The menu

- **Today: N out · N in**: today's totals, from the CSV
- **Requests today**
- **Today by model**: output, input and request count for each model
- **Open Portal…**: the dashboard
- **Open Data Folder** / **Edit Config…**
- **Sync to S3 Now**, plus the last sync status
- Proxy status
- **Launch at Login**
- **Quit TokenLedger**: runs a final S3 sync (up to 15 seconds), then quits

Today's output count also shows next to the icon (turn that off with
`show_count_in_menu_bar = false`).

## The dashboard

http://127.0.0.1:8765. It works without the internet; nothing loads from a CDN.

- Date range presets (today, 7, 30, 90 days, month to date, all, custom), a model
  filter, and an output / input toggle that scope everything on the page
- Headline totals: output rendered, input, today so far, average per day,
  requests, generation speed, input-to-output ratio
- **Tokens per day by model**: stacked columns with a 7-day average line
- **Tokens by model**: each model's share of the range
- **Generation speed**: tokens per second per model, per day
- **Cumulative tokens**: running input and output totals
- **When your host works**: weekday × hour heatmap
- **Insights**: week-over-week change, busiest day, unusual days (robust
  z-score), the leading model, fastest and slowest model, month-end projection,
  peak hour, repeated prompts, and data-quality notes
- Tables: daily by model, recent requests, repeated prompts (by fingerprint)
- **Export SVG** on every chart (a standalone file with title, legend and
  colors baked in), **Download this table (CSV)**, and **Download raw rows (CSV)**
  for the filtered range

Colors are fixed per model (ranked by all-time output), so a model keeps its
color when you filter. Dark mode follows macOS, or use the **Theme** button.

## Configuration

`~/Library/Application Support/TokenLedger/config.toml` (created on first run;
**Edit Config…** opens it). Restart TokenLedger after editing.

```toml
[proxy]
listen = "127.0.0.1:1234"           # what clients call
upstream = "http://127.0.0.1:1235"  # where LM Studio listens

[portal]
port = 8765                         # always bound to 127.0.0.1

[storage]
data_dir = ""                       # empty = ~/Library/Application Support/TokenLedger/data

[s3]
enabled = false
endpoint = ""                       # see examples below
region = "auto"
bucket = ""
prefix = "tokenledger/"
path_style = true
access_key_id = ""                  # the secret goes in the Keychain, not here
sync_interval_minutes = 15

[app]
launch_at_login = true
show_count_in_menu_bar = true
```

### S3-compatible mirror

1. Fill in the `[s3]` section and set `enabled = true`:

   | Service | `endpoint` | `region` | `path_style` |
   |---|---|---|---|
   | Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` | `auto` | `true` |
   | MinIO / NAS | `http://nas.local:9000` | `us-east-1` | `true` |
   | Backblaze B2 | `https://s3.<region>.backblazeb2.com` | e.g. `us-west-004` | `false` |
   | Wasabi | `https://s3.<region>.wasabisys.com` | e.g. `us-east-1` | `false` |

2. Store the secret access key in the macOS Keychain (typed without echo):

   ```sh
   /Applications/TokenLedger.app/Contents/MacOS/TokenLedger --set-s3-secret
   ```

3. Restart TokenLedger. The menu shows **S3: synced just now** once it works,
   or the error if it doesn't.

Each sync uploads only the monthly files that changed, to
`<bucket>/<prefix>tokens-YYYY-MM.csv`. The bucket holds a copy; the local CSV
stays the source of truth. To restore, download the files from the bucket into
the data folder and the app picks them up within seconds.

After rebuilding the app, macOS may ask once whether TokenLedger may use the
Keychain item. Choose **Always Allow**. (Each build is ad-hoc signed, so macOS
treats it as a new app.)

## The CSV

One file per month, `tokens-YYYY-MM.csv`, one row per request:

| Column | Example | Notes |
|---|---|---|
| `request_id` | `d375dc0b-…` | random UUID |
| `timestamp` | `2026-10-11T14:03:22.123-07:00` | local time with UTC offset |
| `local_date` | `2026-10-11` | the day the row counts toward (local midnight) |
| `model` | `qwen/qwen3-30b-a3b` | as reported by LM Studio |
| `endpoint` | `/v1/chat/completions` | |
| `streamed` | `true` | |
| `status` | `200` | 499 = the client disconnected mid-stream |
| `input_tokens` | `3412` | |
| `output_tokens` | `918` | |
| `duration_ms` | `11873` | whole request |
| `ttft_ms` | `402` | time to first token (streams) |
| `output_tokens_per_sec` | `80.03` | output ÷ generation time |
| `usage_source` | `reported` | `reported`, `estimated` or `none` |
| `prompt_sha256` | `19e21ad5…` | fingerprint, see Privacy |

You can open the files in Numbers or Excel, or edit them by hand. TokenLedger
re-reads any file whose size or modification time changes, and columns are
matched by name.

## Command line

```
TokenLedger                  menu bar app (what the .app runs)
TokenLedger --headless       proxy, dashboard and S3 only, no menu bar
TokenLedger --set-s3-secret  store the S3 secret key in the Keychain
TokenLedger fingerprint      print prompt_sha256 for a request JSON on stdin
```

`TOKENLEDGER_HOME` overrides the base folder (handy for testing).
`TOKENLEDGER_S3_SECRET_KEY` overrides the Keychain. Logs go to
`~/Library/Logs/TokenLedger/tokenledger.log` when started at login.

## Troubleshooting

- **Clients get "TokenLedger could not reach LM Studio"**: LM Studio's server
  isn't running on the `upstream` port.
- **Proxy warning in the menu**: something else (usually LM Studio) still holds
  port 1234.
- **Opening the app twice** just opens the dashboard of the running copy.
- **Counts look low**: chats in LM Studio's own window aren't counted; only API
  traffic is.

## Development

```sh
cargo test                                         # unit tests
TOKENLEDGER_HOME=/tmp/tl cargo run -- --headless   # run without the menu bar
```

Source layout: `proxy.rs` (capture), `usage.rs` (usage parsing, fingerprint),
`ledger.rs` (CSV read/write and queries), `s3sync.rs`, `portal.rs` +
`assets/index.html` (dashboard), `tray.rs` (menu bar), `login.rs`
(LaunchAgent), `secrets.rs` (Keychain), `config.rs`.
