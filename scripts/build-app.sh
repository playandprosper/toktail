#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Builds dist/Toktail.app. With --install, also quits any running copy,
# copies the app to /Applications and starts it.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-app.sh builds a macOS .app; run it on the Mac." >&2
  exit 1
fi
command -v cargo >/dev/null || { echo "Rust is not installed: https://rustup.rs" >&2; exit 1; }

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
echo "==> Building Toktail $VERSION (release)"
cargo build --release

APP=dist/Toktail.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/Toktail "$APP/Contents/MacOS/Toktail"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"

# App icon: Pip on a midnight squircle, every size macOS asks for.
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size packaging/AppIcon-1024.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double packaging/AppIcon-1024.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

# Ad-hoc signature: enough to run on this Mac. No Apple developer account needed.
codesign --force --sign - --timestamp=none "$APP"
echo "==> Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  for name in Toktail TokenLedger; do
    if pgrep -x "$name" >/dev/null; then
      echo "==> Quitting the running $name"
      pkill -x "$name" || true
      sleep 1
    fi
  done
  # The first release was called TokenLedger; Toktail moves its data over on first start.
  rm -rf /Applications/TokenLedger.app /Applications/Toktail.app
  cp -R "$APP" /Applications/
  echo "==> Installed /Applications/Toktail.app"
  open /Applications/Toktail.app
fi
