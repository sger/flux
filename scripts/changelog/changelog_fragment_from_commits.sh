#!/usr/bin/env bash
set -euo pipefail

# Generate a changelog fragment draft from commit subjects between a base ref and HEAD.
#
# Usage:
#   scripts/changelog/changelog_fragment_from_commits.sh [base-ref] [topic]
# Example:
#   scripts/changelog/changelog_fragment_from_commits.sh main primops

BASE_REF="${1:-main}"
TOPIC="${2:-auto}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if ! git rev-parse --verify "$BASE_REF" >/dev/null 2>&1; then
  git fetch --depth=1 origin "$BASE_REF":"$BASE_REF"
fi

range="$BASE_REF...HEAD"
if ! git merge-base "$BASE_REF" HEAD >/dev/null 2>&1; then
  echo "Warning: no merge base for $BASE_REF...HEAD; using $BASE_REF..HEAD."
  range="$BASE_REF..HEAD"
fi

map_subject() {
  local subject="$1"
  local section="Changed"
  local line="$subject"

  case "$subject" in
  feat:* | feat\(*\):*)
    section="Added"
    ;;
  fix:* | fix\(*\):*)
    section="Fixed"
    ;;
  perf:* | perf\(*\):*)
    section="Performance"
    ;;
  docs:* | docs\(*\):*)
    section="Docs"
    ;;
  *)
    section="Changed"
    ;;
  esac

  line="$(printf '%s' "$line" | sed -E 's/^[a-z]+(\([^)]+\))?:[[:space:]]*//')"
  printf '%s\t- %s\n' "$section" "$line"
}

subjects="$(git log --no-merges --pretty=format:%s "$range")"
if [ -z "$subjects" ]; then
  echo "No commits found in range $range."
  exit 1
fi

today="$(date +%Y-%m-%d)"
outfile="changes/${today}-${TOPIC}.md"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

for section in Added Changed Fixed Performance Docs; do
  : >"$tmpdir/$section.txt"
done

while IFS= read -r subject; do
  [ -z "$subject" ] && continue
  mapped="$(map_subject "$subject")"
  section="${mapped%%$'\t'*}"
  bullet="${mapped#*$'\t'}"
  printf '%s\n' "$bullet" >>"$tmpdir/$section.txt"
done <<<"$subjects"

{
  for section in Added Changed Fixed Performance Docs; do
    if [ -s "$tmpdir/$section.txt" ]; then
      printf '### %s\n' "$section"
      awk '!seen[$0]++' "$tmpdir/$section.txt"
      printf '\n'
    fi
  done
} >"$outfile"

echo "Generated: $outfile"
echo "Review and edit before committing."
