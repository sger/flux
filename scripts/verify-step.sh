#!/usr/bin/env bash
# Verify one step of a change: lint, the full suite, the evidence sweeps and
# backend parity. Each step's outputs are tagged, so they can be compared with
# the previous step's.
#
# Usage:
#   ./scripts/verify-step.sh <tag>
#
# Outputs go to .git/parked/out/:
#   test_<tag>.txt                      the suite
#   sweep_<tag>/                        every example compiled; errors.txt, summary.txt
#   dump_<tag>__dump_core/              the same under --dump-core
#   dump_<tag>__optimize___dump_core/   the same under --optimize --dump-core
#   parity_<tag>.txt                    vm, vm_cached, vm_strict and llvm compared
#
# The sweep tools are local, not tracked: .git/parked/sweep/, .git/parked/dsweep/
# and the dump sweep's file list, .git/parked/out/files2.txt.

set -u
# Sort file lists the same way in every shell, so runs compare line for line.
export LC_ALL=C

TAG="${1:?usage: ./scripts/verify-step.sh <tag>}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
PARKED="$REPO_ROOT/.git/parked"
OUT="$PARKED/out"

for tool in "$PARKED/sweep/evidence_sweep.sh" "$PARKED/dsweep/sweep.sh" "$OUT/files2.txt"; do
  if [ ! -e "$tool" ]; then
    echo "missing $tool: the sweep tools are local to this checkout" >&2
    exit 1
  fi
done
mkdir -p "$OUT"

strip_ansi() { sed -u $'s/\x1b\\[[0-9;]*m//g'; }

# Run a sweep in the background and print how many of its files are done.
sweep_with_progress() {
  local dir=$1 total=$2
  shift 2
  "$@" &
  local pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    sleep 10
    local done
    done=$(ls "$dir/runs" 2>/dev/null | wc -l | tr -d ' ')
    printf '\r   %s / %s files' "$done" "$total"
  done
  wait "$pid"
  printf '\n'
}

echo "== fmt, clippy"
cargo fmt --check || echo "FMT: run cargo fmt"
cargo clippy --all-targets -- -D warnings 2>&1 | grep -E "^(warning|error)" -A6 | head -20

echo "== suite"
INSTA_FORCE_PASS=1 cargo test --no-fail-fast 2>&1 | tee "$OUT/test_$TAG.txt" \
  | grep --line-buffered -E "^\s+Running |^test result"
echo "tests exit ${PIPESTATUS[0]}"
grep -E "^test result: FAILED" "$OUT/test_$TAG.txt"
find . -name "*.snap.new" -not -path "./target/*"

ALL_FILES=$(find examples tests lib -name '*.flx' | wc -l | tr -d ' ')
DUMP_FILES=$(wc -l < "$OUT/files2.txt" | tr -d ' ')

echo "== normal sweep"
sweep_with_progress "$OUT/sweep_$TAG" "$ALL_FILES" \
  sh -c '"$1" "$2" "$3" > "$4" 2>&1' _ \
  "$PARKED/sweep/evidence_sweep.sh" "$REPO_ROOT" "$OUT/sweep_$TAG" "$OUT/sweep_$TAG.log"

echo "== dump sweep (--dump-core)"
sweep_with_progress "$OUT/dump_${TAG}__dump_core" "$DUMP_FILES" \
  sh -c '"$1" "$2" "$3" "$4" "$5" > "$6" 2>&1' _ \
  "$PARKED/dsweep/sweep.sh" "$REPO_ROOT" "$OUT/dump_${TAG}__dump_core" "$OUT/files2.txt" \
  "--dump-core" "$OUT/dump_${TAG}_a.log"

echo "== dump sweep (--optimize --dump-core)"
sweep_with_progress "$OUT/dump_${TAG}__optimize___dump_core" "$DUMP_FILES" \
  sh -c '"$1" "$2" "$3" "$4" "$5" > "$6" 2>&1' _ \
  "$PARKED/dsweep/sweep.sh" "$REPO_ROOT" "$OUT/dump_${TAG}__optimize___dump_core" \
  "$OUT/files2.txt" "--optimize --dump-core" "$OUT/dump_${TAG}_b.log"

echo "== internal errors"
for dir in "sweep_$TAG" "dump_${TAG}__dump_core" "dump_${TAG}__optimize___dump_core"; do
  count=$(grep -li "internal compiler error" "$OUT/$dir/runs/"* 2>/dev/null | wc -l | tr -d ' ')
  echo "$dir: $count"
done

echo "== parity"
for dir in examples/type_classes examples/generics tests/flux tests/parity; do
  echo "=== $dir"
  ./target/debug/flux parity-check "$dir" --ways vm,vm_cached,vm_strict,llvm 2>&1 \
    | grep --line-buffered -vE "^\s+Compiling|^\s+Finished"
done 2>&1 | tee "$OUT/parity_$TAG.txt" | strip_ansi \
  | grep --line-buffered -E "^===|^PASS|^MISMATCH|^EXPECTED|^SKIP|^Total|^Pass|^Mismatch"

echo "all-done: $TAG"
