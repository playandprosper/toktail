<p align="center">
  <img src="docs/banner.png" alt="Toktail: Pip the Token Fox, with the tagline Every token leaves a trail" width="100%">
</p>

# Toktail

*Pronounced "tock-tail". A mix of **token** and `tail -f`.*

Toktail counts every token your Mac renders in LM Studio, per model, per day.
It writes one CSV row per request on your host, mirrors the files to an
S3-compatible bucket, and shows the history in a local portal you can play
with. Pip the Token Fox lives in your menu bar and in the portal: napping when
nothing is happening, munching while tokens stream, and losing its mind when
your four models all start talking at once.

<p align="center"><img src="docs/pip-intro.gif" alt="The portal opening: Pip hops out from the middle of the window, each landing lighting the ground red, green or blue, then settles onto the trail at the bottom" width="800"></p>

## Meet Pip

Pip sits between your apps and LM Studio. Every request that passes through,
Pip catches the tokens, counts them, and adds them to the timeline. Its tail
is a real chart: the glowing line on it is your live tokens per second.

| When | Pip | In the menu |
|---|---|---|
| Nothing is streaming | Curls up and naps, tail softly pulsing | *Nothing to count. Nap time!* |
| Tokens are streaming | Hops along the trail, faster as throughput climbs, tail drawing the live chart | *Nom nom nom… 2,847 tokens/sec!* |
| Several requests at once, or three times your usual speed | Ears up, tail puffed, sprinting | *Holy tokens! We've got a stampede!* |

Every time Pip's feet touch the ground, the spot lights up in pure red
(`#FF0000`), green (`#00FF00`) or blue (`#0000FF`). The lights add up where
they overlap, the way real light does.

## What you get

- **Menu bar.** Pip runs in your menu bar while tokens stream and naps when
  they stop. The menu shows today's output and input counts, a per-model
  breakdown, S3 and proxy status, and links to the portal.
- **A CSV you own.** One row per request, one file per month. Every number
  in the app comes from these files, and you can open or edit them anywhere.
- **S3-compatible mirror.** Changed files go to Cloudflare R2, MinIO, Backblaze
  B2, Wasabi, AWS or your NAS every 15 minutes and when you quit.
- **A portal that's fun to look at.**
  - The intro: Pip runs out from the middle of the window when it opens.
  - Daily tokens by model, share by model, speed per model, running totals,
    and a weekday-by-hour heatmap.
  - **Replay the trail**: Pip hops across the daily chart, lighting up each
    day as it adds up your history.
  - Insights Pip worked out: trends, unusual days, repeated prompts and a
    month-end projection.
  - Every chart exports as a standalone SVG. Tables and raw rows download as CSV.
  - Light and dark mode.
- **Cloud cost comparison (optional).** See what the same tokens would have
  cost on Claude, GPT, Grok, Gemini, Perplexity, DeepSeek, Mistral and Kimi,
  next to the electricity your host actually used.

| Dark | Light |
|---|---|
| ![Portal in dark mode](docs/dashboard-dark.png) | ![Portal in light mode](docs/dashboard-light.png) |

## Install

You need Rust on the Mac (one time):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then build and install:

```sh
git clone https://github.com/playandprosper/toktail
cd toktail
./scripts/build-app.sh --install
```

This builds `dist/Toktail.app`, copies it to `/Applications` and starts it.
Pip shows up in the menu bar, and the portal opens in your browser. On first
start Toktail also turns on **Launch at Login** (toggle it in the menu).

### Move LM Studio to port 1235

Toktail takes port 1234, so LM Studio's server moves to 1235. Change the port
in LM Studio's Developer tab, or start the server from the terminal (LM Studio
remembers the last port you used):

```sh
lms server start --port 1235
```

Your apps don't change anything. They keep calling `http://localhost:1234/v1`.
Until the port is free, the menu shows **⚠ Proxy can't use 127.0.0.1:1234** and
Toktail retries every 10 seconds.

If other machines on your network call this host, set `listen = "0.0.0.0:1234"`
in the config. LM Studio can then stay bound to localhost.

## How tokens are counted

Toktail is a transparent reverse proxy in front of LM Studio. Every request is
forwarded unchanged, and on the way back Toktail reads the counts LM Studio
reports:

| Counter | Meaning |
|---|---|
| `input_tokens` | Prompt tokens LM Studio read (system prompt, history, new turn) |
| `output_tokens` | Tokens LM Studio generated |

Counted endpoints: `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`,
`/v1/responses`, `/v1/messages`, and LM Studio's native `/api/v0/*` and
`/api/v1/chat`. Model lists, model loading and everything else pass straight
through and aren't recorded.

For streamed chat and completion requests, Toktail asks LM Studio to add a
usage summary to the stream. If your app didn't ask for that summary itself,
Toktail removes it before the response reaches your app, so nothing changes
from the app's point of view. If LM Studio ever reports no usage, Toktail counts
the streamed pieces instead (about one token each) and marks the row
`estimated`.

**Not counted:** chats typed into LM Studio's own chat window. They never touch
the API server, so no proxy can see them.

## Privacy

The CSV never contains prompts, responses, client IP addresses or the host
name. Each prompt is stored only as a SHA-256 fingerprint, which lets the
portal spot repeated prompts without being able to read them. If the CSV
leaks, the worst that's exposed is model names, counts, timings and
fingerprints.

The fingerprint is the SHA-256 of the request's prompt fields (`system`,
`instructions`, `messages`, `prompt`, `input`, whichever are present), as
compact JSON with keys sorted. To check one against a request you have:

```sh
cat request.json | /Applications/Toktail.app/Contents/MacOS/Toktail fingerprint
```

A hash can't be reversed, but a short, guessable prompt (like "hi") can be
confirmed by hashing a guess and comparing.

Request bodies never go to the log. The portal listens on 127.0.0.1 only,
rejects any other `Host` or `Origin`, and loads nothing from the internet.

## Cloud cost comparison

Off by default. Turn it on in the portal under **Settings → Cloud cost
comparison**.

<p align="center"><img src="docs/cost-comparison.png" alt="Cost comparison: running locally saved about $102 compared with Claude Sonnet 5.5, with bars for each service and a running total" width="820"></p>

For the selected date range, Toktail prices every request at each service's
public list price, using that request's own date and size (so long-prompt
tiers and scheduled price changes land on the right requests). The local side
is electricity: the time your host was actually generating, with overlapping
requests counted once, times the power draw and kWh price you set. You choose
which services to compare and which one savings are measured against. Add what
your Mac cost to see how long it takes to pay for itself.

Built-in prices, per 1 million tokens, as of 2026-10-11:

| Provider | Model | Input | Output | Notes |
|---|---|---|---|---|
| Anthropic | Claude Fable 5.1 | $10 | $50 | |
| Anthropic | Claude Opus 5.5 | $4 | $20 | |
| Anthropic | Claude Sonnet 5.5 | $2 | $10 | Default baseline |
| Anthropic | Claude Haiku 5.5 | $0.10 | $0.50 | $0.50 / $2.50 for prompts over 100K tokens |
| OpenAI | GPT-6 Astra | $10 | $50 | |
| OpenAI | GPT-6.1 Sol | $2 | $10 | |
| OpenAI | GPT-6 Luna | $0.10 | $0.50 | |
| xAI | Grok 4.7 | $2 | $6 | |
| Google | Gemini 3.1 Pro (preview) | $2 | $12 | Prompts up to 200K tokens |
| Google | Gemini 3.8 Flash | $0.75 | $3.75 | $1.50 / $7.50 from 2027-01-01 |
| Google | Gemini 3.5 Flash-Lite | $0.30 | $2.50 | |
| Perplexity | Sonar Pro | $3 | $15 | Plus $6 per 1,000 requests |
| Perplexity | Sonar | $1 | $1 | Plus $5 per 1,000 requests |
| DeepSeek | DeepSeek V4 Pro | $0.66 | $1.98 | Off-peak; peak hours cost 2× |
| DeepSeek | DeepSeek Flash | $0.15 | $0.60 | Off-peak; peak hours cost 2× |
| Mistral | Mistral Large | $0.50 | $1.50 | |
| Moonshot AI | Kimi K3 | $3 | $15 | |

Sources: [Anthropic](https://platform.claude.com/docs/en/about-claude/pricing),
[OpenAI](https://developers.openai.com/api/docs/pricing),
[xAI](https://docs.x.ai/docs/models),
[Google](https://ai.google.dev/gemini-api/docs/pricing),
[Perplexity](https://docs.perplexity.ai/getting-started/pricing),
[DeepSeek](https://api-docs.deepseek.com/quick_start/pricing),
[Mistral](https://mistral.ai/pricing),
[Moonshot AI](https://platform.kimi.ai/docs/pricing/chat).

Treat the numbers as a fair estimate, not a quote:

- They're standard list prices, without prompt caching, batch or volume discounts.
- Your token counts come from your local models' tokenizers. Public models
  count the same text a little differently.
- Embedding requests are left out.

Prices change. To use your own, put a `pricing.json` in Toktail's folder
(same format as [`assets/pricing.json`](assets/pricing.json)); it replaces
the built-in list.

## Configuration

`~/Library/Application Support/Toktail/config.toml` is created on first run.
**Edit Config…** in the menu opens it; restart Toktail after editing by hand.
Changes made in the portal's Settings apply right away.

```toml
[proxy]
listen = "127.0.0.1:1234"           # what your apps call
upstream = "http://127.0.0.1:1235"  # where LM Studio listens

[portal]
port = 8765                         # always bound to 127.0.0.1
intro_animation = true              # Pip runs out from the middle on open
pip_roams = true                    # Pip hops along the trail while tokens stream

[storage]
data_dir = ""                       # empty = ~/Library/Application Support/Toktail/data

[s3]
enabled = false
endpoint = ""
region = "auto"
bucket = ""
prefix = "toktail/"
path_style = true
access_key_id = ""                  # the secret goes in the Keychain, not here
sync_interval_minutes = 15

[app]
launch_at_login = true
show_count_in_menu_bar = true
stampede_tokens_per_sec = 0.0       # 0 = three times your usual speed

[costs]
show_comparison = false
baseline = "claude-sonnet-5.5"
compare = []                        # empty = the default set
electricity_usd_per_kwh = 0.18
watts_while_generating = 250.0
hardware_cost_usd = 0.0             # set it to see a payback estimate
```

### S3-compatible mirror

1. Fill in `[s3]` and set `enabled = true`:

   | Service | `endpoint` | `region` | `path_style` |
   |---|---|---|---|
   | Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` | `auto` | `true` |
   | MinIO / NAS | `http://nas.local:9000` | `us-east-1` | `true` |
   | Backblaze B2 | `https://s3.<region>.backblazeb2.com` | e.g. `us-west-004` | `false` |
   | Wasabi | `https://s3.<region>.wasabisys.com` | e.g. `us-east-1` | `false` |

2. Store the secret access key in the macOS Keychain (typed without echo):

   ```sh
   /Applications/Toktail.app/Contents/MacOS/Toktail --set-s3-secret
   ```

3. Restart Toktail. The menu shows **S3: synced just now** when it works, or the
   error when it doesn't.

Each sync uploads only the monthly files that changed, to
`<bucket>/<prefix>tokens-YYYY-MM.csv`. The bucket is a copy; the local CSV
stays the source of truth. To restore, copy the files from the bucket into the
data folder and Toktail picks them up within seconds.

After you rebuild the app, macOS may ask once whether Toktail can use the
Keychain item. Choose **Always Allow**. (Each build is ad-hoc signed, so macOS
sees it as a new app.)

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
| `status` | `200` | 499 means the client hung up mid-stream |
| `input_tokens` | `3412` | |
| `output_tokens` | `918` | |
| `duration_ms` | `11873` | whole request |
| `ttft_ms` | `402` | time to first token (streams) |
| `output_tokens_per_sec` | `80.03` | output ÷ generation time |
| `usage_source` | `reported` | `reported`, `estimated` or `none` |
| `prompt_sha256` | `19e21ad5…` | fingerprint, see Privacy |

Open the files in Numbers or Excel, or edit them by hand. Toktail re-reads any
file whose size or modification time changes, and matches columns by name.

## Command line

```
Toktail                  menu bar app (what the .app runs)
Toktail --headless       proxy, portal and S3 only, no menu bar
Toktail --set-s3-secret  store the S3 secret key in the Keychain
Toktail fingerprint      print prompt_sha256 for a request JSON on stdin
```

`TOKTAIL_HOME` overrides the base folder and `TOKTAIL_S3_SECRET_KEY` overrides
the Keychain. Logs go to `~/Library/Logs/Toktail/toktail.log` when Toktail
starts at login.

## Coming from TokenLedger

Toktail's first release was called TokenLedger. On first start, Toktail moves
`~/Library/Application Support/TokenLedger` (config, CSV history, sync state)
to its new folder, removes the old login item, and still reads an S3 secret
saved under the old Keychain name. `build-app.sh --install` removes the old
app.

## Troubleshooting

- **Your apps get "Toktail could not reach LM Studio".** LM Studio's server
  isn't running on the `upstream` port.
- **The menu shows a proxy warning.** Something else, usually LM Studio, still
  has port 1234.
- **Opening the app twice** just opens the portal of the running copy.
- **Counts look low.** Chats in LM Studio's own window aren't counted; only
  API traffic is.
- **Pip won't move.** Your Mac is set to reduce motion, or **Hop along the
  trail** is off in Settings. Pip still reacts; it just stays put.

## Development

```sh
cargo test
TOKTAIL_HOME=/tmp/toktail-dev cargo run -- --headless
```

| Path | What it does |
|---|---|
| `src/proxy.rs` | Forwards traffic to LM Studio and captures usage |
| `src/usage.rs` | Usage parsing and prompt fingerprints |
| `src/ledger.rs` | CSV reading, writing and queries |
| `src/live.rs` | Live tokens per second and Pip's mood |
| `src/pricing.rs`, `assets/pricing.json` | Cloud cost comparison |
| `src/s3sync.rs` | S3 mirror |
| `src/portal.rs` | Local web server for the portal |
| `assets/index.html`, `app.css`, `dashboard.js` | Portal page |
| `assets/pip.js`, `assets/pip.svg` | Pip's rig, footstep lights and hops |
| `src/tray.rs`, `assets/tray/` | Menu bar and Pip's run cycle |
| `src/login.rs`, `src/secrets.rs`, `src/config.rs` | Login item, Keychain, config |

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
