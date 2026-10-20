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
- Never write a credential anywhere. An API key exists for the life of one
  request; the CSV gets a one-way id (`tokens::token_id`), saved bodies and
  logs go through `tokens::scrub`. Gateway keys are stored as hashes.
- The model sees the request exactly as the agent sent it. Session chains,
  the prompt cache and Pip's commentary all happen beside the request, never
  inside it; `src/chain.rs` and `src/records.rs` carry the tests that prove
  prompts unfold byte for byte.
- The portal stays local. No CDNs, no web fonts, no analytics; everything it
  needs is embedded in the binary.
- Respect `prefers-reduced-motion`. Pip can sit still.
- Before opening a pull request: `make fmt lint test`.

## Prices

`assets/pricing.json` holds the public API list prices used by the cost
comparison, and is the fallback for days before the first market snapshot.
When a provider changes prices, update the numbers, the `as_of` date and the
`source` link in the same pull request. `market_ids` on a model pins which
LiteLLM / OpenRouter listing it matches when the canonical id isn't enough
(`src/market.rs` has the matcher and its tests; set `TOKTAIL_LITELLM_JSON`
to a downloaded list to test against the real thing).

## Pip in 3D

`assets/pip-model.js` and `assets/pip-clips.js` are the same files as the
landing page's; keep them in step with that repository. `assets/pip3d.js`
skins the SVG rig in `assets/pip.js`, which stays the motion controller.
three.js is vendored under `assets/vendor/` (`npm pack three@0.170.0`,
`build/three.module.min.js` and `examples/jsm/utils/BufferGeometryUtils.js`);
the portal loads no CDN.

## Pip in the menu bar

The menu bar frames in `assets/tray/` are generated. Edit
`scripts/brand/tray_frames.py` and run it to regenerate them.

## License

By contributing you agree that your contributions are licensed under the
Apache License 2.0.

