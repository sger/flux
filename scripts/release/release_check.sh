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

# Expanded parity sweep (--compile) across non-error example folders.
# Excluded by design: parser_errors, compiler_errors, runtime_errors,
# type_system/failing, diagnostics_demos (all fail-on-purpose).
PARITY_FOLDERS=(
  advanced aether aoc benchmarks Debug flow functions guide_type_system
  imports io ModuleGraph Modules module_scoped_type_classes namespaces
  optimizations patterns performance primop repros roots runtime_boundaries
  strict_types tail_call tests type_inference
)
for folder in "${PARITY_FOLDERS[@]}"; do
  run_cmd cargo run -- parity-check "examples/$folder" --compile --ways vm,llvm
done
# TODO: type_system/ — 308-file matrix. Add once triage confirms which
# subdirs parity-compile cleanly (type_system/failing/ is fail-on-purpose
# and must be excluded). parity-check currently lacks an --exclude flag;
# until it does, enable this by running only passing subsets.
# run_cmd cargo run -- parity-check examples/type_system --compile --ways vm,llvm

echo
echo "Release preflight passed."
