#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench_flamewatch.sh [options]

Runs a full performance workflow:
  1) Build release binaries
  2) Flux-only stability benchmark (hyperfine)
  3) Cross-language benchmark (via scripts/bench_cross_lang.sh)
  4) Flamegraph generation
  5) Flamewatch: top aggregated hotspots from flamegraph.svg

Options:
  --input <path>             Input file for Rust/Python tools (default: examples/io/aoc_day1.txt)
  --flux <path>              Flux program for cross-lang benchmark (default: examples/io/aoc_day1_part2.flx)
  --profile <path>           Flux profiling workload for flamegraph (default: examples/io/aoc_day1_part2_profile.flx)
  --rust-bin <name>          Rust binary name (default: aoc_day1_part2_rust)
  --python <path>            Python benchmark script (default: benchmarks/aoc/day1_part2.py)
  --name-prefix <label>      Benchmark name prefix (default: aoc_day1_part2)
  --runs <n>                 Cross-language runs (default: 60)
  --warmup <n>               Cross-language warmup (default: 15)
  --flux-runs <n>            Flux-only runs (default: 60)
  --flux-warmup <n>          Flux-only warmup (default: 15)
  --skip-build               Skip cargo release build step
  --skip-flamegraph          Skip cargo flamegraph step
  --top <n>                  Number of flamewatch rows to print (default: 25)
  -h, --help                 Show this help

Example:
  scripts/bench_flamewatch.sh \
    --input examples/io/aoc_day1.txt \
    --flux examples/io/aoc_day1_part2.flx \
    --profile examples/io/aoc_day1_part2_profile.flx \
    --rust-bin aoc_day1_part2_rust \
    --python benchmarks/aoc/day1_part2.py
USAGE
}

INPUT="examples/io/aoc_day1.txt"
FLUX_PROGRAM="examples/io/aoc_day1_part2.flx"
PROFILE_PROGRAM="examples/io/aoc_day1_part2_profile.flx"
RUST_BIN="aoc_day1_part2_rust"
PYTHON_SCRIPT="benchmarks/aoc/day1_part2.py"
NAME_PREFIX="aoc_day1_part2"
RUNS=60
WARMUP=15
FLUX_RUNS=60
FLUX_WARMUP=15
TOP_N=25
SKIP_BUILD=0
SKIP_FLAMEGRAPH=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --input)
      INPUT="$2"
      shift 2
      ;;
    --flux)
      FLUX_PROGRAM="$2"
      shift 2
      ;;
    --profile)
      PROFILE_PROGRAM="$2"
      shift 2
      ;;
    --rust-bin)
      RUST_BIN="$2"
      shift 2
      ;;
    --python)
      PYTHON_SCRIPT="$2"
      shift 2
      ;;
    --name-prefix)
      NAME_PREFIX="$2"
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
    --flux-runs)
      FLUX_RUNS="$2"
      shift 2
      ;;
    --flux-warmup)
      FLUX_WARMUP="$2"
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
  echo "Error: hyperfine not found. Install with: brew install hyperfine" >&2
  exit 1
fi

for f in "$INPUT" "$FLUX_PROGRAM" "$PROFILE_PROGRAM" "$PYTHON_SCRIPT"; do
  if [[ ! -f "$f" ]]; then
    echo "Error: missing file: $f" >&2
    exit 1
  fi
done

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== Build release binaries =="
  cargo build --release --features jit --bin flux --bin "$RUST_BIN"
  echo
fi

FLUX_CMD="./target/release/flux $FLUX_PROGRAM"
FLUX_JIT_CMD="./target/release/flux $FLUX_PROGRAM --jit"
RUST_CMD="./target/release/$RUST_BIN $INPUT"
PYTHON_CMD="python3 $PYTHON_SCRIPT $INPUT"

echo "== Flux-only stability benchmark =="
hyperfine -N --warmup "$FLUX_WARMUP" --runs "$FLUX_RUNS" \
  --command-name "${NAME_PREFIX}/flux-only" \
  "$FLUX_CMD"
echo

echo "== Cross-language benchmark =="
scripts/bench_cross_lang.sh --native --runs "$RUNS" --warmup "$WARMUP" \
  --input "$INPUT" \
  --name-prefix "$NAME_PREFIX" \
  --flux-cmd "$FLUX_CMD" \
  --flux-jit-cmd "$FLUX_JIT_CMD" \
  --rust-cmd "$RUST_CMD" \
  --python-cmd "$PYTHON_CMD"
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
