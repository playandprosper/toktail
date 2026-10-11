# Contributing to Toktail

Thanks for helping Pip count tokens.

## Getting set up

```sh
git clone https://github.com/playandprosper/toktail
cd toktail
make demo        # sample data and pretend traffic
make test
```

`--headless` runs the proxy, the portal and the S3 mirror without the menu
bar, which is handy on Linux or in CI. Open http://127.0.0.1:8765 for the
portal. `TOKTAIL_HOME` keeps your development data away from your real CSV.

## Ground rules

- The CSV is the source of truth. Anything the app shows must be computable
  from the CSV files alone.
- Never write prompt or response text anywhere: not to the CSV, the logs or
  the bucket. Fingerprints only.
- The portal stays local. No CDNs, no web fonts, no analytics; everything it
  needs is embedded in the binary.
- Respect `prefers-reduced-motion`. Pip can sit still.
- Before opening a pull request: `make fmt lint test`.

## Prices

`assets/pricing.json` holds the public API list prices used by the cost
comparison. When a provider changes prices, update the numbers, the `as_of`
date and the `source` link in the same pull request.

## Pip in the menu bar

The menu bar frames in `assets/tray/` are generated. Edit
`scripts/brand/tray_frames.py` and run it to regenerate them.

## License

By contributing you agree that your contributions are licensed under the
Apache License 2.0.
