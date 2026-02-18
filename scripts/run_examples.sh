#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/run_examples.sh [--mode vm|jit|both] <path-under-examples> [flux args...]
  scripts/run_examples.sh [--mode vm|jit|both] <folder-under-examples>/ [flux args...]
  scripts/run_examples.sh [--mode vm|jit|both] --all [flux args...]

Examples:
  scripts/run_examples.sh basics/print.flx
  scripts/run_examples.sh --mode jit basics/print.flx
  scripts/run_examples.sh ModuleGraph/ --no-cache
  scripts/run_examples.sh ModuleGraph/module_graph_main.flx --no-cache --trace

Flux flags (common):
  --no-cache
  --trace
  --verbose
  --leak-detector
  --roots-only
  --root <path>  (extra root, can be repeated)

Runner flags:
  --mode vm|jit|both  (default: both)
USAGE
}

list_examples() {
  if command -v rg >/dev/null 2>&1; then
    rg --files -g '*.flx' examples
  else
    find examples -type f -name '*.flx' | sort
  fi \
    | sed 's#^examples/##' \
    | grep -v '^Debug/' \
    | grep -v '^errors/' \
    | grep -v '_error\.flx$' \
    | grep -v '_invalid\.flx$' \
    | grep -v '/[A-Z].*\.flx$'
}

if [[ $# -lt 1 || "$1" == "-h" || "$1" == "--help" ]]; then
  show_help
  echo
  echo "Available examples:"
  list_examples
  exit 0
fi

mode="both"
if [[ "$1" == "--mode" ]]; then
  if [[ $# -lt 2 ]]; then
    echo "Error: --mode requires vm, jit, or both" >&2
    exit 1
  fi
  mode="$2"
  shift 2
fi

case "$mode" in
  vm|jit|both) ;;
  *)
    echo "Error: invalid --mode '$mode' (expected vm|jit|both)" >&2
    exit 1
    ;;
esac

failures=()

record_failure() {
  failures+=("$1")
}

finish_batch() {
  if [[ ${#failures[@]} -eq 0 ]]; then
    return 0
  fi

  echo
  echo "Failed examples (${#failures[@]}):" >&2
  for item in "${failures[@]}"; do
    echo "  - $item" >&2
  done
  exit 1
}

run_all() {
  while IFS= read -r example; do
    [[ -z "$example" ]] && continue
    echo
    echo "==> examples/$example"
    if ! scripts/run_examples.sh --mode "$mode" "$example" "$@"; then
      record_failure "$example"
    fi
  done < <(list_examples)

  finish_batch
}

if [[ "$1" == "--all" ]]; then
  shift
  run_all "$@"
  exit 0
fi

example="$1"
shift

if [[ -d "examples/$example" ]]; then
  if command -v rg >/dev/null 2>&1; then
    while IFS= read -r file; do
      echo
      echo "==> examples/$file"
      if ! scripts/run_examples.sh --mode "$mode" "$file" "$@"; then
        record_failure "$file"
      fi
    done < <(
      rg --files -g '*.flx' "examples/$example" | sed 's#^examples/##'
    )
  else
    while IFS= read -r file; do
      echo
      echo "==> examples/$file"
      if ! scripts/run_examples.sh --mode "$mode" "$file" "$@"; then
        record_failure "$file"
      fi
    done < <(
      find "examples/$example" -type f -name '*.flx' | sort | sed 's#^examples/##'
    )
  fi
  finish_batch
fi

if [[ ! -f "examples/$example" ]]; then
  echo "Error: examples/$example not found" >&2
  exit 1
fi

roots=(
  "examples"
  "examples/Modules"
  "examples/ModuleGraph"
  "examples/roots/root_a"
  "examples/roots/root_b"
)

run_once() {
  local run_mode="$1"
  shift
  local -a cmd
  local output status stack_trace_detected
  local has_jit_flag=0
  local has_no_cache_flag=0

  if [[ "$run_mode" == "jit" ]]; then
    cmd=(cargo run --features jit --)
  else
    cmd=(cargo run --)
  fi

  for root in "${roots[@]}"; do
    cmd+=(--root "$root")
  done

  cmd+=("examples/$example")

  for arg in "$@"; do
    if [[ "$arg" == "--jit" ]]; then
      has_jit_flag=1
    fi
    if [[ "$arg" == "--no-cache" ]]; then
      has_no_cache_flag=1
    fi
  done

  # Forward any extra flags to flux.
  if [[ $# -gt 0 ]]; then
    cmd+=("$@")
  fi

  # JIT pass should force JIT mode for coverage.
  if [[ "$run_mode" == "jit" && $has_jit_flag -eq 0 ]]; then
    cmd+=(--jit)
  fi
  if [[ "$run_mode" == "jit" && $has_no_cache_flag -eq 0 ]]; then
    cmd+=(--no-cache)
  fi

  echo
  echo "---- [$run_mode] examples/$example"
  output=$("${cmd[@]}" 2>&1) && status=0 || status=$?
  stack_trace_detected=0

  echo "$output"

  if echo "$output" | grep -qi "stack overflow"; then
    echo "Error: Stack overflow detected in $run_mode mode, stopping execution" >&2
    exit 1
  fi

  if echo "$output" | grep -q "Stack trace:"; then
    stack_trace_detected=1
  fi

  if [[ $stack_trace_detected -eq 1 ]]; then
    echo "Error: Stack trace detected in $run_mode mode, stopping execution" >&2
    exit 1
  fi

  return $status
}

if [[ "$mode" == "vm" ]]; then
  run_once vm "$@"
  exit $?
fi

if [[ "$mode" == "jit" ]]; then
  run_once jit "$@"
  exit $?
fi

run_once vm "$@" || exit $?
run_once jit "$@" || exit $?
