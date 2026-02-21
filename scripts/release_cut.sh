#!/usr/bin/env bash
set -euo pipefail

# Cut a release in CHANGELOG.md from [Unreleased].
#
# Usage:
#   scripts/release_cut.sh v0.0.4

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 vX.Y.Z"
  exit 1
fi

NEW_VERSION="$1"
if ! [[ "$NEW_VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Invalid version '$NEW_VERSION'. Expected format: vX.Y.Z"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

scripts/changelog_from_fragments.sh

if grep -Eq "^## \[$NEW_VERSION\]" CHANGELOG.md; then
  echo "CHANGELOG.md already contains section for $NEW_VERSION"
  exit 1
fi

PREV_VERSION="$(
  awk '
    /^## \[v[0-9]+\.[0-9]+\.[0-9]+\]/ {
      gsub(/^## \[/, "", $0)
      gsub(/\].*$/, "", $0)
      print
      exit
    }
  ' CHANGELOG.md
)"

if [ -z "$PREV_VERSION" ]; then
  echo "Could not detect previous version heading in CHANGELOG.md"
  exit 1
fi

today="$(date +%Y-%m-%d)"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

unreleased_body="$tmpdir/unreleased_body.txt"
awk '
  BEGIN { in_unreleased = 0 }
  /^## \[Unreleased\]/ { in_unreleased = 1; next }
  in_unreleased && /^---$/ { exit }
  in_unreleased { print }
' CHANGELOG.md > "$unreleased_body"

if ! grep -Eq "^- " "$unreleased_body"; then
  echo "[Unreleased] has no bullet entries to release."
  exit 1
fi

new_changelog="$tmpdir/CHANGELOG.new.md"
awk -v body="$unreleased_body" -v ver="$NEW_VERSION" -v d="$today" '
  BEGIN {
    in_unreleased = 0
    inserted_release = 0
  }
  /^## \[Unreleased\]/ {
    print
    print ""
    print "### Changed"
    print "- (none yet)"
    print ""
    print "---"
    print ""
    in_unreleased = 1
    next
  }
  in_unreleased {
    if ($0 ~ /^## \[v[0-9]+\.[0-9]+\.[0-9]+\]/) {
      print "## [" ver "] - " d
      print ""
      while ((getline line < body) > 0) {
        print line
      }
      close(body)
      print "---"
      print ""
      print
      in_unreleased = 0
      inserted_release = 1
    }
    next
  }
  { print }
' CHANGELOG.md > "$new_changelog"

# Update compare links.
links_tmp="$tmpdir/links_updated.md"
awk -v new_ver="$NEW_VERSION" -v prev_ver="$PREV_VERSION" '
  BEGIN {
    have_unreleased = 0
    have_new = 0
  }
  /^\[Unreleased\]: / {
    print "[Unreleased]: https://github.com/sger/flux/compare/" new_ver "...HEAD"
    have_unreleased = 1
    next
  }
  $0 ~ "^\\[" new_ver "\\]: " {
    print "[" new_ver "]: https://github.com/sger/flux/compare/" prev_ver "..." new_ver
    have_new = 1
    next
  }
  { print }
  END {
    if (!have_unreleased) {
      print "[Unreleased]: https://github.com/sger/flux/compare/" new_ver "...HEAD"
    }
    if (!have_new) {
      print "[" new_ver "]: https://github.com/sger/flux/compare/" prev_ver "..." new_ver
    }
  }
' "$new_changelog" > "$links_tmp"

mv "$links_tmp" CHANGELOG.md
echo "Released $NEW_VERSION in CHANGELOG.md (previous: $PREV_VERSION)."
echo "Next steps:"
echo "  1) Review CHANGELOG.md"
echo "  2) Create docs/versions/whats_new_${NEW_VERSION}.md"
echo "  3) Commit and tag: git tag $NEW_VERSION && git push origin $NEW_VERSION"
