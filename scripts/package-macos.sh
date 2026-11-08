#!/usr/bin/env bash
# Build a universal (arm64 + x86_64) glyphcam.app, plus
#   dist/glyphcam-<label>-macos-universal.dmg      (for people)
#   dist/glyphcam-<label>-macos-universal.app.zip  (for the in-app updater)
# The app is ad-hoc signed. With a Developer ID, set MACOS_SIGN_IDENTITY.
set -euo pipefail
cd "$(dirname "$0")/.."
export GLYPHCAM_CHANNEL="${GLYPHCAM_CHANNEL:-dev}"
export GLYPHCAM_BUILD_SHA="${GLYPHCAM_BUILD_SHA:-$(git rev-parse --short=8 HEAD)}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
LABEL="${GLYPHCAM_LABEL:-$VERSION}"
IDENTITY="${MACOS_SIGN_IDENTITY:--}"

rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
cargo build --release --locked -p glyphcam --target aarch64-apple-darwin
cargo build --release --locked -p glyphcam --target x86_64-apple-darwin

APP=dist/glyphcam.app
rm -rf "$APP" dist/dmg && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/glyphcam" \
  target/aarch64-apple-darwin/release/glyphcam target/x86_64-apple-darwin/release/glyphcam
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUILD@/$GLYPHCAM_BUILD_SHA/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/macos/glyphcam.icns "$APP/Contents/Resources/glyphcam.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE.txt"
cp THIRD-PARTY.md "$APP/Contents/Resources/THIRD-PARTY.txt"
codesign --force --deep --sign "$IDENTITY" "$APP"
codesign --verify --verbose "$APP"

# Smoke test the bundled binary (both slices on Apple Silicon via Rosetta if present).
"$APP/Contents/MacOS/glyphcam" --selftest
arch -x86_64 "$APP/Contents/MacOS/glyphcam" --selftest || echo "(x86_64 slice not runnable here)"

ditto -c -k --keepParent "$APP" "dist/glyphcam-${LABEL}-macos-universal.app.zip"
mkdir -p dist/dmg && cp -R "$APP" dist/dmg/ && ln -s /Applications dist/dmg/Applications
hdiutil create -volname "glyphcam" -srcfolder dist/dmg -ov -format UDZO "dist/glyphcam-${LABEL}-macos-universal.dmg"
rm -rf dist/dmg
echo "built dist/glyphcam-${LABEL}-macos-universal.{dmg,app.zip}"
