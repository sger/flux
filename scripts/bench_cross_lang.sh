#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench_cross_lang.sh [options]

Options:
  --input <path>            Input file path (default: examples/io/aoc_day1.txt)
  --runs <n>                Number of benchmark runs (default: 30)
  --warmup <n>              Warmup runs (default: 3)
  --native                  Use direct release binaries (Flux/Rust) and no shell
  --flux-cmd <cmd>          Flux command (default provided)
  --rust-cmd <cmd>          Rust command
  --python-cmd <cmd>        Python command
  --node-cmd <cmd>          Node command
  --name-prefix <label>     Prefix for benchmark names (default: aoc_day1)
  --no-shell                Use raw commands (no shell expansion)
  -h, --help                Show this help

Examples:
  scripts/bench_cross_lang.sh
  scripts/bench_cross_lang.sh --native
  scripts/bench_cross_lang.sh --runs 50 --warmup 5
  scripts/bench_cross_lang.sh \
    --rust-cmd './target/release/day1_rust examples/io/aoc_day1.txt' \
    --python-cmd 'python3 benchmarks/aoc/day1.py examples/io/aoc_day1.txt' \
    --node-cmd 'node benchmarks/aoc/day1.mjs examples/io/aoc_day1.txt'

Notes:
  - Requires `hyperfine` (https://github.com/sharkdp/hyperfine).
  - Commands should print only the final answer for cleaner timings.
USAGE
}

INPUT="examples/io/aoc_day1.txt"
RUNS=30
WARMUP=3
NAME_PREFIX="aoc_day1"
SHELL_MODE=1
NATIVE_MODE=0

DEFAULT_FLUX_CARGO_CMD="cargo run --release --bin flux -- examples/io/aoc_day1.flx"
FLUX_CMD="$DEFAULT_FLUX_CARGO_CMD"
RUST_CMD=""
PYTHON_CMD=""
NODE_CMD=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --input)
      INPUT="$2"
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
    --native)
      NATIVE_MODE=1
      SHELL_MODE=0
      shift
      ;;
    --flux-cmd)
      FLUX_CMD="$2"
      shift 2
      ;;
    --rust-cmd)
      RUST_CMD="$2"
      shift 2
      ;;
    --python-cmd)
      PYTHON_CMD="$2"
      shift 2
      ;;
    --node-cmd)
      NODE_CMD="$2"
      shift 2
      ;;
    --name-prefix)
      NAME_PREFIX="$2"
      shift 2
      ;;
    --no-shell)
      SHELL_MODE=0
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
  echo "Install: brew install hyperfine" >&2
  exit 1
fi

if [[ ! -f "$INPUT" ]]; then
  echo "Error: input file not found: $INPUT" >&2
  exit 1
fi

DEFAULT_FLUX_NATIVE_CMD="./target/release/flux examples/io/aoc_day1.flx"
DEFAULT_RUST_NATIVE_CMD="./target/release/aoc_day1_rust $INPUT"
DEFAULT_PYTHON_CMD="python3 benchmarks/aoc/day1.py $INPUT"

if [[ "$NATIVE_MODE" -eq 1 ]]; then
  if [[ "$FLUX_CMD" == "$DEFAULT_FLUX_CARGO_CMD" ]]; then
    FLUX_CMD="$DEFAULT_FLUX_NATIVE_CMD"
  fi
  if [[ -z "$RUST_CMD" ]]; then
    RUST_CMD="$DEFAULT_RUST_NATIVE_CMD"
  fi
  if [[ -z "$PYTHON_CMD" ]]; then
    PYTHON_CMD="$DEFAULT_PYTHON_CMD"
  fi
else
  if [[ "$FLUX_CMD" == "$DEFAULT_FLUX_CARGO_CMD" ]]; then
    FLUX_CMD="$DEFAULT_FLUX_CARGO_CMD"
  fi
fi

if [[ "$NATIVE_MODE" -eq 1 ]]; then
  if [[ "$FLUX_CMD" == "$DEFAULT_FLUX_NATIVE_CMD" && ! -x "./target/release/flux" ]]; then
    echo "Error: missing ./target/release/flux for --native mode." >&2
    echo "Build first: cargo build --release --bin flux --bin aoc_day1_rust" >&2
    exit 1
  fi
  if [[ "$RUST_CMD" == "$DEFAULT_RUST_NATIVE_CMD" && ! -x "./target/release/aoc_day1_rust" ]]; then
    echo "Error: missing ./target/release/aoc_day1_rust for --native mode." >&2
    echo "Build first: cargo build --release --bin flux --bin aoc_day1_rust" >&2
    exit 1
  fi
fi

names=()
commands=()

add_case() {
  local name="$1"
  local cmd="$2"
  if [[ -n "$cmd" ]]; then
    names+=("$name")
    commands+=("$cmd")
  fi
}

add_case "$NAME_PREFIX/flux" "$FLUX_CMD"
add_case "$NAME_PREFIX/rust" "$RUST_CMD"
add_case "$NAME_PREFIX/python" "$PYTHON_CMD"
add_case "$NAME_PREFIX/node" "$NODE_CMD"

if [[ ${#commands[@]} -lt 2 ]]; then
  echo "Error: provide at least two languages/commands to compare." >&2
  echo "Current configured commands:" >&2
  printf "  %s\n" "${names[@]}"
  exit 1
fi

echo "Input: $INPUT"
echo "Runs: $RUNS, Warmup: $WARMUP"
echo "Cases:"
for i in "${!names[@]}"; do
  echo "  - ${names[$i]} => ${commands[$i]}"
done
echo

hf_cmd=(hyperfine --warmup "$WARMUP" --runs "$RUNS")
if [[ "$SHELL_MODE" -eq 1 ]]; then
  hf_cmd+=(--shell zsh)
else
  hf_cmd+=(--shell none)
fi

for n in "${names[@]}"; do
  hf_cmd+=(--command-name "$n")
done
for c in "${commands[@]}"; do
  hf_cmd+=("$c")
done

"${hf_cmd[@]}"
