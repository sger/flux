#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

fixtures=(
  "examples/aether/bench_reuse.flx"
  "examples/aether/tree_updates.flx"
  "examples/aether/bench_reuse_enabled.flx"
  "examples/aether/bench_reuse_blocked.flx"
  "examples/aether/queue_workload.flx"
)

backends=("vm" "jit" "llvm")

print_stats_block() {
  local fixture="$1"
  local backend="$2"
  local stats_output

  case "$backend" in
    vm)
      stats_output="$(NO_COLOR=1 target/release/flux --stats "$fixture" 2>&1)"
      ;;
    jit)
      stats_output="$(NO_COLOR=1 target/release/flux --jit --stats "$fixture" 2>&1)"
      ;;
    llvm)
      stats_output="$(NO_COLOR=1 target/release/flux --llvm --stats "$fixture" 2>&1)"
      ;;
  esac

  printf '  [%s]\n' "$backend"
  printf '%s\n' "$stats_output" | awk '
    /^  parse/ || /^  compile/ || /^  execute/ || /^  total/ { print }
  '
}

print_aether_totals() {
  local fixture="$1"
  local dump_output
  dump_output="$(NO_COLOR=1 target/release/flux --dump-aether "$fixture" 2>&1)"

  printf '%s\n' "$dump_output" | awk '
    /── Total ──/ { capture=1; next }
    capture && /^  / { print; lines++; if (lines >= 2) exit }
  '
}

echo "== Phase U Aether workload benchmark =="
echo "This script prints Aether totals separately from runtime timings."
echo "Compare reuse-enabled vs reuse-blocked only within the same backend."
echo ""

cargo build --release --features "jit llvm" --quiet

for fixture in "${fixtures[@]}"; do
  echo "-- $fixture --"
  echo "  Aether totals:"
  print_aether_totals "$fixture" | sed 's/^/    /'
  echo "  Runtime:"
  for backend in "${backends[@]}"; do
    print_stats_block "$fixture" "$backend" | sed 's/^/    /'
  done
  echo ""
done
