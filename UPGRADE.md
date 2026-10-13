# Upgrading to Toktail 0.6

0.6 picks up from 0.4 (the last finished release) and from the unfinished
0.5 preview. Your CSV history, settings and saved prompts carry over on their
own; this page says what changes on disk and how to check that it went well.

## Before you start

Quit Toktail, then copy Toktail's folder somewhere safe:

| | Folder |
|---|---|
| macOS | `~/Library/Application Support/Toktail` |
| Windows | `%APPDATA%\Toktail` |
| Linux | `~/.local/share/Toktail` |

Nothing below needs the copy, but it costs nothing and makes every step
reversible.

## From 0.4

**CSV files gain seven columns.** 0.4 wrote 16 columns; 0.6 writes 23
(`outcome`, `signal`, `response_chars`, `prompt_blob`, `prompt_key`,
`response_key`, `reference_key`). On first start each monthly file with the
old header is copied to `tokens-YYYY-MM.csv.pre-v0.5.bak` and rewritten with
the new header. Existing rows keep every value; the new columns start empty.
The backup is made once and never overwritten. Delete the `.bak` files when
you're happy.

**Saved prompts share their system prompt and history.** With prompt saving
on, the repeated part of a conversation (system prompt and earlier turns) is
written once to `records/blobs/<first two hex>/<sha256>` and each record
points at it through `prompt_blob`. Records saved by 0.4 stay where they are
and still open on the Prompts page.

**Token counts are never deleted.** When a saved file disappears (retention,
a manual delete, a lifecycle rule on the bucket), the matching CSV row keeps
its counts and only loses its file pointers. 0.4 and the 0.5 preview removed
the whole row, which quietly lowered your totals; 0.6 doesn't.

**New things to look at.** The Models page, the live feed and budgets on the
dashboard, the alerts bell, the ⌘K / Ctrl+K palette, `/metrics`, and Pip's
chat (press `/`, double-click Pip, or **Ask Pip…** in the menu).

## From the 0.5 preview

The preview's CSV layout is the same as 0.6, so no files are rewritten. What
changes:

- **The purple brand is back.** The preview's grey theme is gone. Light and
  dark mode follow the 0.4 palette; nothing to migrate.
- **Pip's chat answers from your CSV.** Totals, comparisons, budgets, costs
  and slowest requests are computed in Toktail and handed to the model, so the
  model never does arithmetic and never answers from an empty context. The
  receipts under each answer show the exact numbers used.
- **Direct mode and the system prompt.** The chat's settings drawer has two
  modes. *Pip* keeps the data tools and lets you replace Pip's persona with
  your own system prompt. *Direct* sends your system prompt and messages
  straight to the model you pick, with nothing added. **Continue in chat** on
  a saved prompt opens it in Direct mode with that record's system prompt.
- **Rows the preview deleted are gone.** If the preview's reconcile removed
  rows after files vanished, 0.6 can't bring them back. If you kept a backup
  of the CSV folder from before the preview, copy those monthly files back
  while Toktail is quit; 0.6 reads them as they are.

## New settings

Both are optional and both can be edited in Settings instead.

```toml
[[budgets]]
id = "studio-day"
name = "Mac Studio, daily"
host = "studio"        # empty = every host
model = ""             # empty = every model
metric = "combined"    # "combined", "input" or "output"
period = "day"         # "day", "week" or "month"
limit = 50000000
notify = true

[alerts]
budgets = true
host_down = true
stampede = false
```

## Per platform

**macOS.** Builds now target macOS 11 or later. `make dist` makes a universal
app that runs natively on Apple Silicon and Intel Macs; `make install` builds
for the Mac you're on. The first start of a new build may ask for Keychain
access again: choose **Always Allow**.

**Windows (new).** Run `scripts\build-windows.ps1 -Install` from PowerShell, or
`make install` from Git Bash. Toktail installs to
`%LOCALAPPDATA%\Programs\Toktail`, adds a Start menu shortcut, starts at login
through the per-user Run key, and keeps secrets in Credential Manager. The
icon sits in the notification area (it may be under the ^ arrow at first).

**Linux.** `make install` builds with the tray icon when GTK and
libayatana-appindicator are installed, and headless otherwise. `make service`
installs a systemd user service for machines with no desktop.

## Check that it worked

From the source folder:

```sh
make test        # Rust tests, including the CSV migration and calendar math
make lint        # fmt and clippy
make check-ui    # every portal script parses and is served
make demo        # sample data on http://127.0.0.1:8799, your files untouched
```

Then open Toktail and compare the dashboard's all-time total with the one you
had before upgrading: it should match to the token. Ask Pip "how many tokens
did I use last month?" and check the receipt against the dashboard.

## Going back

Quit Toktail, restore the folder you copied, and reinstall 0.4. If you didn't
copy the folder, rename each `tokens-YYYY-MM.csv.pre-v0.5.bak` back to
`tokens-YYYY-MM.csv`: those are the files exactly as 0.4 left them. Requests
made while 0.6 ran are only in the new files.
