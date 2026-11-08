#!/usr/bin/env bash
# Build glyphcam for Windows and produce dist/glyphcam-<label>-windows-x64-setup.exe
#   GLYPHCAM_CHANNEL=stable|nightly|dev   (default dev)
#   GLYPHCAM_LABEL=0.1.0|nightly          (default: crate version)
#   MAKENSIS=path/to/makensis             (default: makensis on PATH)
set -euo pipefail
cd "$(dirname "$0")/.."
export GLYPHCAM_CHANNEL="${GLYPHCAM_CHANNEL:-dev}"
export GLYPHCAM_BUILD_SHA="${GLYPHCAM_BUILD_SHA:-$(git rev-parse --short=8 HEAD)}"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
LABEL="${GLYPHCAM_LABEL:-$VERSION}"
MAKENSIS="${MAKENSIS:-makensis}"

cargo build --release --locked -p glyphcam -p glyphcam-vcam-dshow -p glyphcam-vcam-mf
cargo build --release --locked -p glyphcam-vcam-dshow --target i686-pc-windows-msvc

rm -rf dist/stage && mkdir -p dist/stage
cp target/release/glyphcam.exe dist/stage/
cp target/release/glyphcam_vcam.dll dist/stage/glyphcam-vcam-x64.dll
cp target/i686-pc-windows-msvc/release/glyphcam_vcam.dll dist/stage/glyphcam-vcam-x86.dll
cp target/release/glyphcam_mfcam.dll dist/stage/glyphcam-mfcam.dll
cp LICENSE dist/stage/LICENSE.txt
cp THIRD-PARTY.md dist/stage/THIRD-PARTY.txt

OUT="dist/glyphcam-${LABEL}-windows-x64-setup.exe"
winpath() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
"$MAKENSIS" -V2 \
  -DVERSION="$VERSION" \
  -DBUILDTAG="${VERSION}-${GLYPHCAM_BUILD_SHA}" \
  -DSRCDIR="$(winpath "$PWD/dist/stage")" \
  -DOUTFILE="$(winpath "$PWD/$OUT")" \
  packaging/windows/installer.nsi
echo "built $OUT"
