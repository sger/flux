#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "== Running debug-focused examples =="
echo

echo "-- match non-exhaustive (should error) --"
cargo run -- run "${ROOT_DIR}/examples/match_non_exhaustive_error.flx" || true
echo

echo "-- match wildcard non-last (should error) --"
cargo run -- run "${ROOT_DIR}/examples/match_wildcard_non_last_error.flx" || true
echo

echo "-- option match (should run) --"
cargo run -- run "${ROOT_DIR}/examples/option_match.flx"
echo

echo "-- ICE demo (requires forced ICE in compiler) --"
echo "Set FLUX_FORCE_ICE=1 and remove cache if needed:"
echo "  rm -rf target/flux"
echo "  FLUX_FORCE_ICE=1 cargo run -- run ${ROOT_DIR}/examples/ice_demo.flx"
