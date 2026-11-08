#!/usr/bin/env bash
# Publish CI artifacts to GitHub Releases.
#  - tag push vX.Y.Z  -> public release "glyphcam vX.Y.Z" (pre-release if the tag has a '-')
#  - push to main     -> rolling public pre-release "nightly" (replaced each time)
set -euo pipefail
DIR="${1:-artifacts}"
REPO="${GITHUB_REPOSITORY:?}"
SHA="${GITHUB_SHA:?}"
SHORT="${SHA:0:8}"
mapfile -t FILES < <(find "$DIR" -type f \( -name '*.exe' -o -name '*.dmg' -o -name '*.app.zip' \) | sort)
[ ${#FILES[@]} -ge 3 ] || { echo "expected 3 artifacts, got: ${FILES[*]}"; exit 1; }
printf 'artifact: %s\n' "${FILES[@]}"

if [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
  TAG="${GITHUB_REF#refs/tags/}"
  NOTES=$(mktemp)
  sed -e "s/@TAG@/$TAG/g" -e "s/@VERSION@/${TAG#v}/g" -e "s/@SHA@/$SHORT/g" .github/release-notes.md > "$NOTES"
  PRE=()
  [[ "$TAG" == *-* ]] && PRE=(--prerelease)
  if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
    gh release upload "$TAG" "${FILES[@]}" --clobber --repo "$REPO"
  else
    gh release create "$TAG" "${FILES[@]}" --repo "$REPO" --title "glyphcam $TAG" \
      --notes-file "$NOTES" --verify-tag "${PRE[@]}"
  fi
else
  gh release delete nightly --repo "$REPO" --yes --cleanup-tag >/dev/null 2>&1 || true
  # wait until the old tag is really gone
  for _ in 1 2 3 4 5; do
    gh api "repos/$REPO/git/refs/tags/nightly" >/dev/null 2>&1 || break
    sleep 2
  done
  NOTES=$(mktemp)
  {
    echo "Rolling build of \`main\` at ${SHORT} ($(date -u +%Y-%m-%d))."
    echo
    echo "Install it from glyphcam → Updates, or download below. See the latest stable release for install notes."
    echo
    git log -n 15 --pretty='- %s (%h)' "$SHA" 2>/dev/null || true
  } > "$NOTES"
  gh release create nightly "${FILES[@]}" --repo "$REPO" --target "$SHA" --prerelease \
    --title "Nightly build (${SHORT})" --notes-file "$NOTES"
fi
