#!/usr/bin/env bash
# Prints KEY=value lines for $GITHUB_ENV: release channel, label and short SHA.
set -euo pipefail
SHA="${GITHUB_SHA:-$(git rev-parse HEAD)}"
echo "GLYPHCAM_BUILD_SHA=${SHA:0:8}"
if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
  TAG="${GITHUB_REF#refs/tags/}"
  VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
  if [[ "${TAG#v}" != "$VERSION" ]]; then
    echo "tag $TAG does not match Cargo.toml version $VERSION" >&2
    exit 1
  fi
  echo "GLYPHCAM_CHANNEL=stable"
  echo "GLYPHCAM_LABEL=${VERSION}"
else
  echo "GLYPHCAM_CHANNEL=nightly"
  echo "GLYPHCAM_LABEL=nightly"
fi
