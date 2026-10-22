#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Builds dist/Toktail.app on a Mac.
#
#   scripts/build-app.sh               this Mac's architecture
#   scripts/build-app.sh --universal   Apple Silicon + Intel in one app (needs both Rust targets)
#   scripts/build-app.sh --install     also quit any running copy, copy it to /Applications and start it
#
# Runs on macOS 11 Big Sur and later, so older Intel Macs can watch an
# AI-capable machine over the network.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-app.sh builds a macOS .app; run it on a Mac. On Windows use scripts/build-windows.ps1, on Linux make build." >&2
  exit 1
fi
command -v cargo >/dev/null || { echo "Rust is not installed: https://rustup.rs" >&2; exit 1; }

UNIVERSAL=0
INSTALL=0
for a in "$@"; do
  case "$a" in
    --universal) UNIVERSAL=1 ;;
    --install) INSTALL=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

export MACOSX_DEPLOYMENT_TARGET=11.0
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP=dist/Toktail.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [[ $UNIVERSAL == 1 ]]; then
  echo "==> Building Toktail $VERSION for Apple Silicon and Intel"
  if command -v rustup >/dev/null; then rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null; fi
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  lipo -create -output "$APP/Contents/MacOS/Toktail" \
    target/aarch64-apple-darwin/release/Toktail target/x86_64-apple-darwin/release/Toktail
  lipo -info "$APP/Contents/MacOS/Toktail"
else
  echo "==> Building Toktail $VERSION for $(uname -m)"
  cargo build --release
  cp target/release/Toktail "$APP/Contents/MacOS/Toktail"
fi
# macOS wants plain numbers in CFBundleVersion: 0.0.3-beta.1 ships as 0.0.3.
sed "s/__VERSION__/${VERSION%%-*}/g" packaging/Info.plist > "$APP/Contents/Info.plist"

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

if [[ $INSTALL == 1 ]]; then
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

