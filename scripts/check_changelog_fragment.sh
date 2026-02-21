#!/usr/bin/env bash
set -euo pipefail

# CI helper:
# Requires at least one changelog fragment file in pull requests.
#
# Usage:
#   scripts/check_changelog_fragment.sh <base-ref>
# Example:
#   scripts/check_changelog_fragment.sh main

BASE_REF="${1:-main}"

if ! git rev-parse --verify "$BASE_REF" >/dev/null 2>&1; then
  git fetch --depth=1 origin "$BASE_REF":"$BASE_REF"
fi

changed_files="$(git diff --name-only "$BASE_REF...HEAD")"

if [ -z "$changed_files" ]; then
  echo "No changed files detected; skipping fragment check."
  exit 0
fi

fragment_count="$(
  {
    printf '%s\n' "$changed_files" \
      | grep -E '^changes/.*\.md$' \
      | grep -Ev '^changes/(README|_template)\.md$' \
      || true
  } | wc -l \
    | tr -d '[:space:]'
)"

if [ "$fragment_count" -gt 0 ]; then
  echo "Changelog fragment check passed ($fragment_count fragment file(s))."
  exit 0
fi

echo "Missing changelog fragment."
echo "Add a file under changes/, for example:"
echo "  changes/$(date +%Y-%m-%d)-short-topic.md"
echo
echo "See changes/README.md for format."
exit 1
