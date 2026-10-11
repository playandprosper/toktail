<p align="center">
  <img src="docs/banner.png" alt="Toktail: Pip the Token Fox, with the tagline Every token leaves a trail" width="100%">
</p>

# Toktail

*Pronounced "tock-tail". A mix of **token** and `tail -f`.*

Toktail counts every token your LM Studio hosts render, input and output,
per model, per host, per day. It watches up to 17 hosts at once, writes one
CSV row per request, mirrors the files to an S3-compatible bucket, and shows
the history in a local portal you can leave open all day. If you want, it
also keeps the prompts and responses themselves (encrypted), so you can read,
search and re-run them, and it works out what your own hardware saves you
against public API prices over its lifecycle. Pip the Token Fox lives in your
menu bar and in the portal: napping when nothing is happening, munching while
tokens stream, and losing its mind when your models all start talking at once.

<p align="center"><img src="docs/pip-intro.gif" alt="The portal opening: Pip hops out from the middle of the window, each landing lighting the ground red, green or blue, then settles onto the trail at the bottom" width="800"></p>

## Meet Pip

Pip sits between your apps and LM Studio. Every request that passes through,
Pip catches the tokens, counts them, and adds them to the timeline. Its tail
is a real chart: the glowing line on it is your live tokens per second.

In the portal Pip is a living character, not a picture. It's drawn in vector
parts (body, head, two ears, four jointed legs and a flowing tail) in the
style of the mascot artwork, and animated live: a gallop where every paw is
placed on the ground with inverse kinematics, so feet plant instead of
sliding; a tail that streams and sways; blinking, ear twitches and breathing.

| When | Pip | In the menu |
|---|---|---|
| Nothing is streaming | Curls up and naps, eyes closed, breathing slowly | *Nothing to count. Nap time!* |
| Tokens are streaming | Runs along the trail, faster as throughput climbs, tail drawing the live chart | *Nom nom nom… 2,847 tokens/sec!* |
| Four or more requests at once, or three times your usual speed | Fur up, ears perked, tail puffed, flat-out sprint | *Holy tokens! We've got a stampede!* |

Every time one of Pip's paws touches the ground, that spot lights up in pure
red (`#FF0000`), green (`#00FF00`) or blue (`#0000FF`). The lights add up
where they overlap, the way real light does. Click Pip for a fact about your
tokens, or press **Replay the trail** to watch Pip run across your history.

Pip also keeps you company while you read. Rest your pointer on a card and,
after a moment, Pip crouches, leaps up onto that card's top edge (via the
cards in between if it's far), and walks back and forth along it while
tokens flow, always on the side away from your pointer so it never covers
what you're reading. Scroll and Pip rides along with the card; scroll the card
away and Pip hops back down to the trail. It won't move more than every few
seconds, and it naps wherever it is when nothing is streaming.

<p align="center"><img src="docs/pip-platform.gif" alt="Pip leaping from the trail onto the cards of the dashboard and walking along their top edges" width="720"></p>

## What you get

- **Many hosts, one dashboard.** Watch up to 17 LM Studio servers (this Mac,
  a Mac mini in the closet, a remote box with an API token). Each host gets
  its own Toktail port. The dashboard shows every host as a live tile (status,
  tokens per second, today and the last 7 days), and you can look at all of
  them combined or click one (shift-click for several).
- **Combined, input and output.** Totals default to *combined* tokens (input +
  output). One switch changes the charts marked ◆ to output-only or
  input-only; hovering it lights up exactly the charts it affects, and every
  other card says what it always shows.
- **A hero number that fits.** "Your hosts rendered, in the last 7 days," is
  sized to fit anything up to 17 trillion (and beyond), with the words under it.
- **Menu bar.** Pip runs in your menu bar while tokens stream and naps when
  they stop (or stays still, if you turn the animation off). The menu shows
  today's combined count, a per-host and per-model breakdown, S3 and host
  status, and links to the portal.
- **A CSV you own.** One row per request, one file per month, with the host it
  ran on. Every number in the app comes from these files.
- **S3-compatible mirror.** CSV files, and saved prompts and responses if you
  want, go to Cloudflare R2, MinIO, Backblaze B2, Wasabi, AWS or your NAS.
- **Prompts (optional, off by default).** Save each request's prompt and
  response as `YYYY-MM-DD-HH-mm-SS.prompt` / `.response` gzip files in a folder
  you choose, encrypted at rest if you like, uploaded to S3, with rolling
  retention. Browse them, see how each one ranked, annotate, delete, and chat
  with your models from the portal.
- **Savings.** Track each machine (serial number, price, purchase date,
  lifecycle), see its rolling speed, lifetime tokens and a glowing lifecycle
  bar, and what it saved you month by month and year by year. Replace a
  machine when you upgrade and its history stays with it.
- **Cloud cost comparison (optional).** What the same tokens would have cost
  on Claude, GPT, Grok, Gemini, Perplexity, DeepSeek, Mistral and Kimi, against
  what your hosts cost: hardware share plus electricity.
- **The portal.** Four pages (Dashboard, Prompts, Savings, Settings), Pip's
  intro, replay and platforming, insights, SVG export for every chart, CSV
  download for every table, light and dark mode.

| Dark | Light |
|---|---|
| ![Portal in dark mode](docs/dashboard-dark.png) | ![Portal in light mode](docs/dashboard-light.png) |

## Install

Open Terminal and run:

```sh
git clone https://github.com/playandprosper/toktail
cd toktail
make demo
```

`make demo` installs what's missing (Apple's command line tools and Rust, the
first time only), compiles Toktail, and starts it with three pretend hosts
(a Mac Studio, a Mac mini and a MacBook Pro), 14 months of sample history,
the machines behind them, and pretend traffic. Saving prompts is on, with a
random encryption key per file, so the Prompts page fills up as you watch.
Pip appears in your menu bar and the portal opens at http://127.0.0.1:8799.
Your real data and Keychain entries aren't touched. Press Ctrl-C to stop.

When you're ready to count your own tokens:

```sh
make install     # builds Toktail.app, copies it to /Applications and starts it
make settings    # add your LM Studio hosts: address, token, and the port apps call
```

Run `make` on its own to see every command, and `make doctor` to check your
setup. Toktail turns on **Launch at Login** on first start (toggle it in the
menu).

| Command | What it does |
|---|---|
| `make demo` | Sample data and pretend traffic, nothing to set up |
| `make install` | Build, install and start Toktail.app |
| `make settings` | Hosts, saving prompts, encryption, S3 |
| `make prompts` | Open the Prompts page |
| `make savings` | Open the Savings page |
| `make lmstudio` | Optional: move this Mac's LM Studio to 1235 so apps keep 1234 |
| `make open` | Open the portal |
| `make status` | What Pip is doing right now |
| `make logs` | Follow the log |
| `make s3-secret` | Save your S3 secret key in the Keychain |
| `make uninstall` | Remove the app and login item, keep your CSV history |

### Add your hosts

Open **Settings → Hosts** (or click the hosts pill at the top). Toktail starts
with one host, **This Mac**. For each host:

- **LM Studio's address**: where its server runs. This Mac
  (`http://127.0.0.1:1235`) or another machine (`http://studio.local:1234`,
  `https://lm.example.com`).
- **API token**: only if that LM Studio asks for one. Toktail sends it as
  `Authorization: Bearer …` on every request to that host, replacing whatever
  key your apps send. It's saved in your Mac's Keychain, never in a file.
- **Toktail listens on port**: the port your apps call for this host. Every
  host has its own.
- **Let other devices on your network use it**: off keeps the port to this Mac.

**Test connection** checks the address and token before you save. Changes
apply right away: ports open and close without a restart. Then point each app
at its host's `http://localhost:<port>/v1`. Up to 17 hosts.

<p align="center"><img src="docs/settings-hosts.png" alt="Settings: three hosts, each forwarding a local port to its LM Studio, all reachable" width="720"></p>

**Watching a remote LM Studio.** Run Toktail on your laptop, add the machine
that runs LM Studio as a host, and point your laptop's apps at Toktail. Your
requests go laptop → Toktail → remote LM Studio, and Toktail counts the
tokens in between.

**Keeping port 1234 on this Mac.** If your apps already call
`localhost:1234`, move LM Studio to 1235 (`make lmstudio` does it with
LM Studio's `lms` command, or change the port in LM Studio's Developer tab)
and let Toktail listen on 1234.

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

The CSV never contains prompts, responses, client IP addresses or your host
names (hosts appear by the short id you gave them). Each prompt is stored only
as a SHA-256 fingerprint, which lets the portal spot repeated prompts without
being able to read them. If the CSV leaks, the worst that's exposed is model
names, counts, timings and fingerprints.

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
Secrets (API tokens, the S3 key, the encryption password, 1Password and Vault
tokens) live in the Keychain and are never sent back to the page; Settings
only shows whether one is saved.

## Saving prompts and responses

Off by default. Turn it on in **Settings → Saving prompts** and choose a
folder (**Choose…** opens the macOS folder picker). From then on, each counted
request leaves two files, named for the second it finished:

```
<folder>/<host>/2026/10/11/2026-10-11-14-03-22.prompt     the request, exactly as your app sent it
                           2026-10-11-14-03-22.response   the response (for streams: the text, reasoning and usage)
```

Each file is the gzip of the original bytes, encrypted when encryption is on.
(Two requests in the same second get `-02`, `-03`…) The CSV row's `record`
column points at them; the prompt itself never goes in the CSV.

With S3 on, the files are also uploaded to `<prefix>records/…`. Retention is a
rolling window you set separately for this Mac and for S3: for example keep 90
days locally and 3 years in S3, or keep everything forever on your own MinIO.
A local file is only removed once it's safely in S3.

### Encryption

**Settings → Encryption** offers three modes:

| Mode | What happens |
|---|---|
| Off | gzip only |
| One password | Every file is encrypted (AES-256-GCM) with a key derived from your password (Argon2id) |
| Random password for every file | Every file gets its own random key, and Toktail keeps track of the keys in a secrets store |

For per-file keys, pick the secrets store:

1. **An encrypted file in Toktail's folder** (`keystore.tkt`), sealed with your
   password and mirrored to S3 next to the records.
2. **1Password**, one item per key in a vault, through the `op` command-line
   tool (with the 1Password app's sign-in, or a service-account token kept in
   the Keychain).
3. **HashiCorp Vault**, one secret per key in a KV v2 engine (address, mount,
   path, optional namespace, and a token kept in the Keychain).

Decryption is automatic: the Prompts page opens files from this Mac, or from
S3 when the local copy is gone, and decrypts them on this Mac only. Deleting a
record deletes its files here and in S3 and destroys its key. If encryption is
on but the key isn't available (no password yet, Vault unreachable), Toktail
keeps counting but saves nothing rather than write anything unencrypted, and
the Prompts page says why.

<p align="center"><img src="docs/settings.png" alt="Settings: encryption with a random key for every file, kept in an encrypted file, 1Password or HashiCorp Vault" width="720"></p>

### The Prompts page

<p align="center"><img src="docs/prompts.png" alt="The Prompts page: a list of saved records and one record open, with its token counts, percentile ranks, and the prompt revealed" width="820"></p>

- **Browse** every saved record by host, model, date, star or text (titles,
  tags, notes, model, fingerprint). It pages through the CSV on the server, so
  it stays quick with millions of records.
- **Read**: the prompt and the response are blurred until you click each one.
  Next to them: the token counts, speed, time to first token, and where this
  request ranks, as a percentile of every request and of the same model.
- **Annotate**: title, tags, notes and a star, kept sealed in `notes.tkt`.
- **Delete** one or many: the files go (here and in S3); tick a box to also
  remove the rows from the token history.
- **Chat**: a messaging view that talks to any host through its Toktail port,
  so the conversation is counted (and saved) like any other app's. **Continue
  in chat** picks up from any saved record.

<p align="center"><img src="docs/chat.png" alt="Chat with a model through Toktail, with the counted tokens under each answer" width="820"></p>

## Savings and hardware

<p align="center"><img src="docs/savings.png" alt="Savings: total saved, three machines with glowing lifecycle bars, rolling speed and payback" width="820"></p>

Add each machine on the **Savings** page: name, model, serial number, the
price you paid, when you bought it, which host it serves, its power draw, and
its lifecycle (72 months by default; change the default in **Settings →
Hardware**, or per machine, to 96, 144 or whatever you plan). Toktail then
shows, per machine:

- a lifecycle bar that glows at how far along it is,
- what it costs per month while in its lifecycle (after that it's paid off),
- lifetime tokens, rolling speed over 7 days, 30 days and its whole life,
- the cloud value of its tokens, net savings, and how much of its price it
  has paid back.

Below, month by month and year by year: what your tokens would have cost at
the baseline service (and every other compared service), against your
hardware share and electricity, with a running total. When you upgrade, use
**Replace…**: the old machine retires on the new one's purchase date, keeps
its lifetime numbers, and the host's traffic counts toward the new one.

## Cloud cost comparison

Off by default. Turn it on in the portal under **Settings → Cloud cost
comparison**.

<p align="center"><img src="docs/cost-comparison.png" alt="Cost comparison: running locally saved about $300 compared with Claude Sonnet 5.5; the cloud price next to what the hosts cost in hardware share and electricity, every service, and a running total" width="820"></p>

For the selected date range and hosts, Toktail prices every request at each
service's public list price, using that request's own date and size (so
long-prompt tiers and scheduled price changes land on the right requests).
Your side is **what your hosts cost**: each machine's hardware share for those
days (its price spread over its lifecycle) plus electricity (the time your
hosts were actually generating, with overlapping requests counted once, times
the power draw and kWh price). The card shows the two side by side, then every
compared service, a running total, and a link to the full breakdown on the
Savings page.

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
[[hosts]]                           # one block per LM Studio host, up to 17
id = "local"                        # short id used in the CSV and file paths
name = "This Mac"
upstream = "http://127.0.0.1:1235"  # where LM Studio listens
listen = "127.0.0.1:1234"           # what your apps call ("0.0.0.0:…" lets your network in)
enabled = true
# API tokens live in the Keychain; set them in Settings → Hosts.

[portal]
port = 8765                         # always bound to 127.0.0.1
intro_animation = true              # Pip runs out from the middle on open
pip_roams = true                    # Pip runs the trail and hops onto cards

[storage]
data_dir = ""                       # empty = ~/Library/Application Support/Toktail/data

[capture]
enabled = false                     # save prompts and responses
dir = ""                            # empty = ~/Library/Application Support/Toktail/records
s3_upload = true
local_retention_days = 0            # 0 = keep forever
s3_retention_days = 0               # e.g. 1095 for three years

[encryption]
mode = "off"                        # "off", "password" or "per_file"
keystore = "file"                   # "file", "1password" or "vault"
onepassword_vault = "Toktail"
vault_addr = "http://127.0.0.1:8200"
vault_mount = "secret"
vault_path = "toktail"
# salt and key_check are generated; they aren't secret.

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
animate_menu_bar = true             # false keeps Pip still in the menu bar
stampede_tokens_per_sec = 0.0       # 0 = three times your usual speed

[costs]
show_comparison = false
baseline = "claude-sonnet-5.5"
compare = []                        # empty = the default set
electricity_usd_per_kwh = 0.18
watts_while_generating = 250.0      # for hosts without a machine on the Savings page

[hardware]
default_lifecycle_months = 72
```

The machines themselves are in `hardware.json` next to `config.toml`.
Upgrading from 0.3? Your single `[proxy]` section becomes the first host
automatically, and its API token moves with it.

### S3-compatible mirror

1. Fill in **Settings → S3 mirror** (or `[s3]` in config.toml):

   | Service | `endpoint` | `region` | `path_style` |
   |---|---|---|---|
   | Cloudflare R2 | `https://<account-id>.r2.cloudflarestorage.com` | `auto` | `true` |
   | MinIO / NAS | `http://nas.local:9000` | `us-east-1` | `true` |
   | Backblaze B2 | `https://s3.<region>.backblazeb2.com` | e.g. `us-west-004` | `false` |
   | Wasabi | `https://s3.<region>.wasabisys.com` | e.g. `us-east-1` | `false` |

2. Enter the secret access key in Settings (it goes to the Keychain), or run
   `make s3-secret`. **Test bucket** writes, reads and deletes a small object.

3. Turn on **Mirror to S3**. The menu shows **S3: synced just now** when it
   works, or the error when it doesn't. No restart needed.

Each sync uploads only what changed:

```
<prefix>tokens-YYYY-MM.csv               the CSV, one file per month
<prefix>hardware.json                    your machines
<prefix>records/<host>/…/<stamp>.prompt  saved prompts and responses (if on)
<prefix>records/keystore.tkt             sealed per-file keys (if you use the key file)
<prefix>records/notes.tkt                sealed titles, tags and notes
```

The bucket is a copy; your Mac stays the source of truth. To restore on a new
Mac, copy the CSV files into the data folder (Toktail picks them up within
seconds) and the `.tkt` files into Toktail's folder; saved records are read
from S3 on demand.

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
| `host` | `studio` | which host served it |
| `record` | `studio/2026/10/11/2026-10-11-14-03-22` | saved files, if saving was on |

Files from earlier versions get the two new columns added on first start.

Open the files in Numbers or Excel, or edit them by hand. Toktail re-reads any
file whose size or modification time changes, and matches columns by name.

## Command line

```
Toktail                  menu bar app (what the .app runs)
Toktail --demo           sample data and pretend traffic (what make demo runs)
Toktail --headless       proxy, portal and S3 only, no menu bar
Toktail --set-s3-secret  store the S3 secret key in the Keychain
Toktail fingerprint      print prompt_sha256 for a request JSON on stdin
```

`TOKTAIL_HOME` overrides the base folder. `TOKTAIL_S3_SECRET_KEY`,
`TOKTAIL_LMSTUDIO_TOKEN_<HOST ID>` (for example `TOKTAIL_LMSTUDIO_TOKEN_STUDIO`)
and `TOKTAIL_ENCRYPTION_PASSWORD` override the Keychain. Logs go to `~/Library/Logs/Toktail/toktail.log` when Toktail
starts at login.

## Coming from TokenLedger

Toktail's first release was called TokenLedger. On first start, Toktail moves
`~/Library/Application Support/TokenLedger` (config, CSV history, sync state)
to its new folder, removes the old login item, and still reads an S3 secret
saved under the old Keychain name. `build-app.sh --install` removes the old
app.

## Troubleshooting

- **Your apps get "Toktail could not reach LM Studio".** That host's LM
  Studio server isn't running at the address in Settings. **Test connection**
  says why. The host's tile on the dashboard shows the reason too.
- **"LM Studio wants an API token" / "refused the API token".** Add or fix the
  token in Settings.
- **A host shows "Can't listen on…".** Another app has that host's port.
  Pick a different port in Settings → Hosts.
- **Prompts aren't being saved.** The Prompts page says why at the top: usually
  encryption is on but its password or key store isn't set up yet.
- **Opening the app twice** just opens the portal of the running copy.
- **Counts look low.** Chats in LM Studio's own window aren't counted; only
  API traffic is.
- **Pip won't move.** Your Mac is set to reduce motion, or **Let Pip roam**
  is off in Settings. Pip still reacts; it just stays put. To stop the menu bar
  animation only, turn off **Animate Pip in the menu bar**.

## Development

```sh
make test        # unit tests
make lint        # formatting and clippy, as CI runs them
make run         # run from the source folder
TOKTAIL_HOME=/tmp/toktail-dev cargo run -- --headless
```

| Path | What it does |
|---|---|
| `src/proxy.rs` | One proxy per host: forwards traffic to LM Studio (with its token), captures usage, hands bodies to records |
| `src/usage.rs` | Usage parsing, streamed text and prompt fingerprints |
| `src/ledger.rs` | CSV reading, writing and queries |
| `src/records.rs` | Saved prompts and responses: files, S3, notes, retention |
| `src/crypto.rs` | gzip, AES-256-GCM, Argon2id, key stores (file, 1Password, Vault) |
| `src/hardware.rs` | Machines, lifecycles and the savings report |
| `src/live.rs` | Live tokens per second and Pip's mood, combined and per host |
| `src/pricing.rs`, `assets/pricing.json` | Cloud cost comparison |
| `src/s3sync.rs` | S3 mirror |
| `src/portal.rs`, `portal_records.rs`, `portal_settings.rs` | Local web server and API |
| `assets/index.html`, `app.css` | The portal's pages |
| `assets/common.js`, `dashboard.js`, `prompts.js`, `savings.js`, `settings.js`, `main.js` | Page code |
| `assets/stage.js` | Pip's stage: intro, trail, replay, card platforming |
| `assets/pip.js` | Pip's animated rig: drawing, gait and IK, leaps, tail, moods, footstep lights |
| `assets/pip.svg` | The mascot artwork (logo, app icon, banner) |
| `src/tray.rs`, `assets/tray/` | Menu bar and Pip's run cycle |
| `src/demo.rs` | `--demo`: three hosts, sample history, hardware, pretend LM Studios and traffic |
| `src/login.rs`, `src/secrets.rs`, `src/config.rs` | Login item, Keychain, config |

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
