#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'USAGE'
Usage:
  scripts/run_examples.sh <path-under-examples> [flux args...]
  scripts/run_examples.sh <folder-under-examples>/ [flux args...]
  scripts/run_examples.sh --all [flux args...]

Examples:
  scripts/run_examples.sh basics/print.flx
  scripts/run_examples.sh ModuleGraph/ --no-cache
  scripts/run_examples.sh ModuleGraph/module_graph_main.flx --no-cache --trace

Flux flags (common):
  --no-cache
  --trace
  --verbose
  --leak-detector
  --roots-only
  --root <path>  (extra root, can be repeated)
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
    if ! scripts/run_examples.sh "$example" "$@"; then
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
      if ! scripts/run_examples.sh "$file" "$@"; then
        record_failure "$file"
      fi
    done < <(
      rg --files -g '*.flx' "examples/$example" | sed 's#^examples/##'
    )
  else
    while IFS= read -r file; do
      echo
      echo "==> examples/$file"
      if ! scripts/run_examples.sh "$file" "$@"; then
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
  local -a cmd
  local output status stack_trace_detected

  cmd=(cargo run --)

  for root in "${roots[@]}"; do
    cmd+=(--root "$root")
  done

  cmd+=("examples/$example")

  # Forward any extra flags to flux.
  if [[ $# -gt 0 ]]; then
    cmd+=("$@")
  fi

  echo
  echo "---- [vm] examples/$example"
  output=$("${cmd[@]}" 2>&1) && status=0 || status=$?
  stack_trace_detected=0

  echo "$output"

  if echo "$output" | grep -qi "stack overflow"; then
    echo "Error: Stack overflow detected, stopping execution" >&2
    exit 1
  fi

  if echo "$output" | grep -q "Stack trace:"; then
    stack_trace_detected=1
  fi

  if [[ $stack_trace_detected -eq 1 ]]; then
    echo "Error: Stack trace detected, stopping execution" >&2
    exit 1
  fi

  return $status
}

run_once "$@"
