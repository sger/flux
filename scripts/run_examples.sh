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
    | sed 's#^examples/##'
}

if [[ $# -lt 1 || "$1" == "-h" || "$1" == "--help" ]]; then
  show_help
  echo
  echo "Available examples:"
  list_examples
  exit 0
fi

run_all() {
  while IFS= read -r example; do
    [[ -z "$example" ]] && continue
    echo "==> examples/$example"
    if ! scripts/run_examples.sh "$example" "$@"; then
      echo "Stopping: example failed" >&2
      exit 1
    fi
  done < <(list_examples)
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
    rg --files -g '*.flx' "examples/$example" \
      | sed 's#^examples/##' \
      | while IFS= read -r file; do
          echo "==> examples/$file"
          if ! scripts/run_examples.sh "$file" "$@"; then
            echo "Stopping: example failed" >&2
            exit 1
          fi
        done
  else
    find "examples/$example" -type f -name '*.flx' | sort \
      | sed 's#^examples/##' \
      | while IFS= read -r file; do
          echo "==> examples/$file"
          if ! scripts/run_examples.sh "$file" "$@"; then
            echo "Stopping: example failed" >&2
            exit 1
          fi
        done
  fi
  exit 0
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

cmd=(cargo run --)
for root in "${roots[@]}"; do
  cmd+=(--root "$root")
done

cmd+=("examples/$example")

# Forward any extra flags to flux.
if [[ $# -gt 0 ]]; then
  cmd+=("$@")
fi

# Run and capture output to check for errors
output=$("${cmd[@]}" 2>&1) && status=0 || status=$?
stack_trace_detected=0

echo "$output"

# Check for stack overflow in output
if echo "$output" | grep -qi "stack overflow"; then
  echo "Error: Stack overflow detected, stopping execution" >&2
  exit 1
fi

# Detect stack trace and log at the end.
if echo "$output" | grep -q "Stack trace:"; then
  stack_trace_detected=1
fi

if [[ $stack_trace_detected -eq 1 ]]; then
  echo "Error: Stack trace detected, stopping execution" >&2
  exit 1
fi

# Exit with the original status
exit $status
