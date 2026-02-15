#!/usr/bin/env bash
set -euo pipefail

PROGRAM="${1:-examples/perf/complexity_scaling.flx}"

if [[ ! -f "$PROGRAM" ]]; then
  echo "Error: missing Flux program: $PROGRAM" >&2
  exit 1
fi

echo "== Build release =="
cargo build --release --bin flux
echo

echo "== Run complexity probe =="
./target/release/flux "$PROGRAM"
