#!/usr/bin/env bash
set -euo pipefail

# Rebuild the [Unreleased] section in CHANGELOG.md from changes/*.md fragments.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

FRAGMENTS=()
while IFS= read -r file; do
  FRAGMENTS+=("$file")
done < <(find changes -maxdepth 1 -type f -name '*.md' \
  ! -name 'README.md' ! -name '_template.md' | sort)

if [ "${#FRAGMENTS[@]}" -eq 0 ]; then
  echo "No changelog fragments found under changes/."
  exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

sections=(Added Changed Fixed Performance Docs Removed Security)
for section in "${sections[@]}"; do
  : > "$tmpdir/$section.txt"
done

extract_section() {
  local section="$1"
  local file="$2"
  awk -v target="### $section" '
    /^### / { in_section = ($0 == target); next }
    in_section && /^- / { print; next }
    in_section && /^[[:space:]]*$/ { next }
    in_section && !/^- / { next }
  ' "$file"
}

for fragment in "${FRAGMENTS[@]}"; do
  for section in "${sections[@]}"; do
    extract_section "$section" "$fragment" >> "$tmpdir/$section.txt"
  done
done

unreleased_body="$tmpdir/unreleased_body.txt"
: > "$unreleased_body"

for section in "${sections[@]}"; do
  if [ -s "$tmpdir/$section.txt" ]; then
    echo "### $section" >> "$unreleased_body"
    awk '!seen[$0]++' "$tmpdir/$section.txt" >> "$unreleased_body"
    echo >> "$unreleased_body"
  fi
done

if [ ! -s "$unreleased_body" ]; then
  {
    echo "### Changed"
    echo "- (none yet)"
    echo
  } > "$unreleased_body"
fi

out="$tmpdir/CHANGELOG.md"
awk -v body="$unreleased_body" '
  BEGIN {
    in_unreleased = 0
    inserted = 0
  }
  /^## \[Unreleased\]/ {
    print
    print ""
    while ((getline line < body) > 0) {
      print line
    }
    close(body)
    print "---"
    print ""
    in_unreleased = 1
    inserted = 1
    next
  }
  in_unreleased {
    if ($0 ~ /^## \[v[0-9]+\.[0-9]+\.[0-9]+\]/) {
      in_unreleased = 0
      print
    }
    next
  }
  { print }
' CHANGELOG.md > "$out"

mv "$out" CHANGELOG.md
echo "Updated CHANGELOG.md [Unreleased] from ${#FRAGMENTS[@]} fragment(s)."
