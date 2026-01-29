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
    scripts/run_examples.sh "$example" "$@"
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
          scripts/run_examples.sh "$file" "$@"
        done
  else
    find "examples/$example" -type f -name '*.flx' | sort \
      | sed 's#^examples/##' \
      | while IFS= read -r file; do
          echo "==> examples/$file"
          scripts/run_examples.sh "$file" "$@"
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

"${cmd[@]}"
