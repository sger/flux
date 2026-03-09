#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench_binarytrees_flamewatch.sh [options]

Runs a binarytrees-specific performance workflow:
  1) Build release binaries
  2) Flux-only stability benchmark (hyperfine)
  3) Cross-language benchmark (via scripts/bench_binarytrees.sh)
  4) Flamegraph generation
  5) Flamewatch: top aggregated hotspots from flamegraph.svg

Options:
  --full                    Use the full n=21 workload instead of the smoke n=8 workload
  --flux-runs <n>           Flux-only runs (default: 20)
  --flux-warmup <n>         Flux-only warmup (default: 2)
  --runs <n>                Cross-language runs (default: 10)
  --warmup <n>              Cross-language warmup (default: 2)
  --top <n>                 Number of flamewatch rows to print (default: 25)
  --skip-build              Skip release build step
  --skip-flamegraph         Skip cargo flamegraph step
  -h, --help                Show this help

Examples:
  scripts/bench_binarytrees_flamewatch.sh
  scripts/bench_binarytrees_flamewatch.sh --full --runs 3 --warmup 1
USAGE
}

FULL_MODE=0
FLUX_RUNS=20
FLUX_WARMUP=2
RUNS=10
WARMUP=2
TOP_N=25
SKIP_BUILD=0
SKIP_FLAMEGRAPH=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full)
      FULL_MODE=1
      shift
      ;;
    --flux-runs)
      FLUX_RUNS="$2"
      shift 2
      ;;
    --flux-warmup)
      FLUX_WARMUP="$2"
      shift 2
      ;;
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    --top)
      TOP_N="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --skip-flamegraph)
      SKIP_FLAMEGRAPH=1
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      show_help >&2
      exit 1
      ;;
  esac
done

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "Error: hyperfine not found." >&2
  exit 1
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== Build release binaries =="
  cargo build --release --features jit --bin flux --bin binarytrees_rust
  echo
fi

if [[ "$FULL_MODE" -eq 1 ]]; then
  FLUX_PROGRAM="benchmarks/flux/binarytrees.flx"
  FLUX_JIT_PROGRAM="benchmarks/flux/binarytrees.flx --jit"
  PROFILE_PROGRAM="benchmarks/flux/binarytrees.flx"
  EXTRA_BENCH_ARGS=(--full)
else
  FLUX_PROGRAM="benchmarks/flux/binarytrees_smoke.flx"
  FLUX_JIT_PROGRAM="benchmarks/flux/binarytrees_smoke.flx --jit"
  PROFILE_PROGRAM="benchmarks/flux/binarytrees_smoke.flx"
  EXTRA_BENCH_ARGS=()
fi

FLUX_CMD="./target/release/flux $FLUX_PROGRAM"

echo "== Flux-only stability benchmark =="
hyperfine -N --warmup "$FLUX_WARMUP" --runs "$FLUX_RUNS" \
  --command-name "binarytrees/flux-only" \
  "$FLUX_CMD"
echo

echo "== Cross-language benchmark =="
scripts/bench_binarytrees.sh --no-shell --runs "$RUNS" --warmup "$WARMUP" "${EXTRA_BENCH_ARGS[@]}"
echo

if [[ "$SKIP_FLAMEGRAPH" -eq 0 ]]; then
  echo "== Flamegraph generation =="
  CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --reverse --bin flux -- "$PROFILE_PROGRAM"
  echo
fi

if [[ ! -f flamegraph.svg ]]; then
  echo "Error: flamegraph.svg not found. Run flamegraph first." >&2
  exit 1
fi

echo "== Flamewatch (top ${TOP_N}) =="
perl -ne 'while(/<title>([^<]+)<\/title>/g){$t=$1; if($t =~ /^(.*) \(([0-9,]+) samples, ([0-9.]+)%\)$/){$n=$1;$p=$3; if(!defined $m{$n} || $p>$m{$n}){$m{$n}=$p;} }} END { for $k (keys %m){ printf "%.2f\t%s\n", $m{$k}, $k; } }' flamegraph.svg \
  | sort -nr \
  | head -n "$TOP_N"
