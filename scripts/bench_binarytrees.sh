#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/bench_binarytrees.sh [options]

Options:
  --runs <n>                Number of benchmark runs (default: 20)
  --warmup <n>              Warmup runs (default: 2)
  --full                    Run the aligned full baseline with n = 21
  --report-file <path>      Write a Markdown report (default: reports/BINARYTREES_REPORT.md)
  --no-readme               Do not sync the latest table into benchmarks/README.md
  --flux-cmd <cmd>          Flux VM command
  --flux-jit-cmd <cmd>      Flux JIT command
  --rust-cmd <cmd>          Rust command
  --python-cmd <cmd>        Python command
  --haskell-cmd <cmd>       Haskell command
  --no-flux-jit             Disable Flux JIT benchmark case
  --no-shell                Use raw commands (no shell expansion)
  -h, --help                Show this help

Examples:
  cargo build --release --bin binarytrees_rust
  ghc -O2 benchmarks/haskell/binarytrees.hs -o target/release/binarytrees_hs
  scripts/bench_binarytrees.sh
  scripts/bench_binarytrees.sh --full --runs 3 --warmup 1
  scripts/bench_binarytrees.sh --runs 30 --warmup 5
  scripts/bench_binarytrees.sh --report-file reports/binarytrees_2026-03-09.md
  scripts/bench_binarytrees.sh --haskell-cmd 'runghc benchmarks/haskell/binarytrees.hs'

Notes:
  - By default all benchmarked implementations use the smaller smoke workload with `n = 8`.
  - Flux uses benchmarks/flux/binarytrees_smoke.flx.
  - Rust/Python/Haskell are passed `8` explicitly to keep the workload aligned.
  - Use --flux-cmd/--flux-jit-cmd plus matching --rust-cmd/--python-cmd/--haskell-cmd for the full n=21 run.
  - For a fair Haskell comparison, prefer a compiled binary over `runghc`.
  - Requires `hyperfine`.
USAGE
}

RUNS=20
WARMUP=2
SHELL_MODE=1
ENABLE_FLUX_JIT=1
REPORT_FILE="reports/BINARYTREES_REPORT.md"
UPDATE_README=1
README_PATH="benchmarks/README.md"
FULL_MODE=0

FLUX_CMD="cargo run --release --bin flux -- benchmarks/flux/binarytrees_smoke.flx"
FLUX_JIT_CMD="cargo run --release --features jit --bin flux -- benchmarks/flux/binarytrees_smoke.flx --jit"
PYTHON_CMD="python3 benchmarks/python/binarytrees.py 8"
HASKELL_CMD="./target/release/binarytrees_hs 8"
RUST_CMD="./target/release/binarytrees_rust 8"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs)
      RUNS="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    --full)
      FULL_MODE=1
      shift
      ;;
    --report-file)
      REPORT_FILE="$2"
      shift 2
      ;;
    --no-readme)
      UPDATE_README=0
      shift
      ;;
    --flux-cmd)
      FLUX_CMD="$2"
      shift 2
      ;;
    --flux-jit-cmd)
      FLUX_JIT_CMD="$2"
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
    --haskell-cmd)
      HASKELL_CMD="$2"
      shift 2
      ;;
    --no-flux-jit)
      ENABLE_FLUX_JIT=0
      shift
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

if [[ "$FULL_MODE" -eq 1 ]]; then
  FLUX_CMD="./target/release/flux benchmarks/flux/binarytrees.flx"
  FLUX_JIT_CMD="./target/release/flux benchmarks/flux/binarytrees.flx --jit"
  RUST_CMD="./target/release/binarytrees_rust 21"
  PYTHON_CMD="python3 benchmarks/python/binarytrees.py 21"
  HASKELL_CMD="./target/release/binarytrees_hs 21"
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "Error: hyperfine not found." >&2
  echo "Install: brew install hyperfine" >&2
  exit 1
fi

if [[ "$RUST_CMD" == "./target/release/binarytrees_rust 8" && ! -x "./target/release/binarytrees_rust" ]]; then
  echo "Error: missing ./target/release/binarytrees_rust" >&2
  echo "Build it with: cargo build --release --bin binarytrees_rust" >&2
  exit 1
fi

if [[ "$HASKELL_CMD" == "./target/release/binarytrees_hs" && ! -x "./target/release/binarytrees_hs" ]]; then
  echo "Error: missing ./target/release/binarytrees_hs" >&2
  echo "Build it with: ghc -O2 benchmarks/haskell/binarytrees.hs -o target/release/binarytrees_hs" >&2
  exit 1
fi

names=("binarytrees/flux" "binarytrees/rust" "binarytrees/python" "binarytrees/haskell")
commands=("$FLUX_CMD" "$RUST_CMD" "$PYTHON_CMD" "$HASKELL_CMD")

if [[ "$ENABLE_FLUX_JIT" -eq 1 ]]; then
  names=("binarytrees/flux" "binarytrees/flux-jit" "binarytrees/rust" "binarytrees/python" "binarytrees/haskell")
  commands=("$FLUX_CMD" "$FLUX_JIT_CMD" "$RUST_CMD" "$PYTHON_CMD" "$HASKELL_CMD")
fi

echo "Runs: $RUNS, Warmup: $WARMUP"
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

for name in "${names[@]}"; do
  hf_cmd+=(--command-name "$name")
done
for command in "${commands[@]}"; do
  hf_cmd+=("$command")
done
hf_cmd+=(--export-markdown "$REPORT_FILE")

"${hf_cmd[@]}"

timestamp="$(date -u +"%Y-%m-%d %H:%M:%S UTC")"
tmp_report="$(mktemp "${TMPDIR:-/tmp}/binarytrees_report.XXXXXX.md")"
{
  echo "# Binary Trees Benchmark Report"
  echo
  echo "- Generated: $timestamp"
  echo "- Runs: $RUNS"
  echo "- Warmup: $WARMUP"
  echo "- Full baseline: $(if [[ "$FULL_MODE" -eq 1 ]]; then echo yes; else echo no; fi)"
  echo
  cat "$REPORT_FILE"
} > "$tmp_report"
mv "$tmp_report" "$REPORT_FILE"
echo
echo "Wrote report: $REPORT_FILE"

if [[ "$UPDATE_README" -eq 1 ]]; then
  tmp_readme="$(mktemp "${TMPDIR:-/tmp}/binarytrees_readme.XXXXXX.md")"
  awk -v report="$REPORT_FILE" '
    BEGIN {
      in_block = 0
    }
    /<!-- binarytrees-report:start -->/ {
      print
      while ((getline line < report) > 0) {
        print line
      }
      close(report)
      in_block = 1
      next
    }
    /<!-- binarytrees-report:end -->/ {
      in_block = 0
      print
      next
    }
    !in_block {
      print
    }
  ' "$README_PATH" > "$tmp_readme"
  mv "$tmp_readme" "$README_PATH"
  echo "Updated README: $README_PATH"
fi
