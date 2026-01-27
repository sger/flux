#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLES_DIR="$ROOT_DIR/examples"

if [[ ! -d "$EXAMPLES_DIR" ]]; then
  echo "examples directory not found: $EXAMPLES_DIR" >&2
  exit 1
fi

has_failed=0

for file in "$EXAMPLES_DIR"/*.flx; do
  if [[ ! -f "$file" ]]; then
    continue
  fi
  echo "==> $(basename "$file")"
  if ! cargo run --quiet -- run "$file"; then
    has_failed=1
  fi
  echo
 done

if [[ $has_failed -ne 0 ]]; then
  echo "Some examples failed" >&2
  exit 1
fi
