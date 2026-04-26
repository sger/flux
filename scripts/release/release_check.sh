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
run_cmd cargo run -- parity-check examples/guide --ways vm,llvm,vm_cached,vm_strict,llvm_strict

# Expanded parity sweep (--compile) across release-green example folders.
# Excluded by design: parser_errors, compiler_errors, runtime_errors,
# diagnostics (all fail-on-purpose). Also excluded until release-green under
# forced vm/llvm ways: effects, type_classes, type_system.
PARITY_FOLDERS=(
  aether aoc benchmarks flow functions guide_type_system
  imports io ModuleGraph Modules namespaces
  optimizations patterns performance primop roots runtime_boundaries
  sealing strict_types tail_call tests type_inference
)
for folder in "${PARITY_FOLDERS[@]}"; do
  run_cmd cargo run -- parity-check "examples/$folder" --compile --ways vm,llvm
done

echo
echo "Release preflight passed."
