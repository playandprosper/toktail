#!/usr/bin/env bash
# Builds dist/TokenLedger.app. With --install, also quits any running copy,
# copies the app to /Applications and starts it.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-app.sh builds a macOS .app; run it on the Mac." >&2
  exit 1
fi
command -v cargo >/dev/null || { echo "Rust is not installed: https://rustup.rs" >&2; exit 1; }

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
echo "==> Building TokenLedger $VERSION (release)"
cargo build --release

APP=dist/TokenLedger.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/TokenLedger "$APP/Contents/MacOS/TokenLedger"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"

# Ad-hoc signature: enough to run on this Mac. No Apple developer account needed.
codesign --force --sign - --timestamp=none "$APP"
echo "==> Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  if pgrep -x TokenLedger >/dev/null; then
    echo "==> Quitting the running TokenLedger"
    pkill -x TokenLedger || true
    sleep 1
  fi
  rm -rf /Applications/TokenLedger.app
  cp -R "$APP" /Applications/
  # Pick up the new location in the launch-at-login agent on next start.
  echo "==> Installed /Applications/TokenLedger.app"
  open /Applications/TokenLedger.app
fi
