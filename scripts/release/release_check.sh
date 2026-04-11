#!/usr/bin/env bash
set -euo pipefail

# Local release preflight for Flux.
# Mirrors release/CI quality gates before tagging.

run_cmd() {
  echo
  echo "==> $*"
  "$@"
}

run_cmd cargo fmt --all -- --check
run_cmd cargo clippy --all-targets --all-features -- -D warnings
run_cmd cargo test --all --all-features
run_cmd cargo run -- --test examples/tests/array_test.flx
run_cmd cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
run_cmd cargo run -- parity-check examples/basics --ways vm,llvm,vm_cached,vm_strict,llvm_strict

echo
echo "Release preflight passed."
