#!/usr/bin/env bash
set -euo pipefail

manifest="ci/examples_manifest.tsv"
tier=""
category=""
shard_index="1"
shard_total="1"

usage() {
  cat <<USAGE
Usage: $0 --tier <1|2|3> [--category <name>] [--manifest <path>] [--shard-index N --shard-total M]
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --manifest) manifest="$2"; shift 2 ;;
    --tier) tier="$2"; shift 2 ;;
    --category) category="$2"; shift 2 ;;
    --shard-index) shard_index="$2"; shift 2 ;;
    --shard-total) shard_total="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown arg: $1"; usage; exit 2 ;;
  esac
done

if [[ -z "$tier" ]]; then
  echo "--tier is required" >&2
  usage
  exit 2
fi

if [[ ! -f "$manifest" ]]; then
  echo "manifest not found: $manifest" >&2
  exit 2
fi

if [[ ! -x "target/debug/flux" ]]; then
  echo "target/debug/flux not found or not executable. Build first (cargo build --all --all-features)." >&2
  exit 2
fi

index=0
run_count=0
fail_count=0

run_case_mode() {
  local name="$1" path="$2" roots_csv="$3" strict="$4" mode="$5" expect_exit="$6" expect_contains="$7"
  local -a cmd=("target/debug/flux" "--no-cache")
  local -a root_list=()

  if [[ "$strict" == "true" ]]; then
    cmd+=("--strict")
  fi

  if [[ "$roots_csv" != "-" && -n "$roots_csv" ]]; then
    read -r -a root_list <<< "${roots_csv//,/ }"
    for r in "${root_list[@]}"; do
      cmd+=("--root" "$r")
    done
  fi

  cmd+=("$path")

  printf '==> [%s] %s :: ' "$mode" "$name"
  printf '%q ' "${cmd[@]}"
  printf '\n'
  local out
  set +e
  out=$("${cmd[@]}" 2>&1)
  local status=$?
  set -e

  if [[ "$status" -ne "$expect_exit" ]]; then
    echo "FAIL: $name [$mode] exit=$status expected=$expect_exit"
    echo "$out"
    return 1
  fi

  if [[ "$expect_contains" != "-" && -n "$expect_contains" ]]; then
    if ! grep -q "$expect_contains" <<< "$out"; then
      echo "FAIL: $name [$mode] missing expected fragment: $expect_contains"
      echo "$out"
      return 1
    fi
  fi

  return 0
}

while IFS=$'\t' read -r row_tier row_category name path roots strict modes expect_exit expect_contains; do
  [[ -z "${row_tier:-}" ]] && continue
  [[ "${row_tier:0:1}" == "#" ]] && continue

  if [[ "$row_tier" != "$tier" ]]; then
    continue
  fi
  if [[ -n "$category" && "$row_category" != "$category" ]]; then
    continue
  fi

  index=$((index + 1))
  slot=$(( (index - 1) % shard_total + 1 ))
  if [[ "$slot" -ne "$shard_index" ]]; then
    continue
  fi

  run_count=$((run_count + 1))
  case "$modes" in
    vm)
      if ! run_case_mode "$name" "$path" "$roots" "$strict" "vm" "$expect_exit" "$expect_contains"; then
        fail_count=$((fail_count + 1))
      fi
      ;;
    jit)
      if ! run_case_mode "$name" "$path" "$roots" "$strict" "jit" "$expect_exit" "$expect_contains"; then
        fail_count=$((fail_count + 1))
      fi
      ;;
    both)
      if ! run_case_mode "$name" "$path" "$roots" "$strict" "vm" "$expect_exit" "$expect_contains"; then
        fail_count=$((fail_count + 1))
      fi
      if ! run_case_mode "$name" "$path" "$roots" "$strict" "jit" "$expect_exit" "$expect_contains"; then
        fail_count=$((fail_count + 1))
      fi
      ;;
    *)
      echo "Unknown modes '$modes' for case '$name'" >&2
      fail_count=$((fail_count + 1))
      ;;
  esac
done < "$manifest"

echo "Executed $run_count case(s); failures: $fail_count"
if [[ "$run_count" -eq 0 ]]; then
  echo "No cases selected (tier=$tier category=${category:-<all>} shard=$shard_index/$shard_total)." >&2
fi
if [[ "$fail_count" -ne 0 ]]; then
  exit 1
fi
