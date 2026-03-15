#!/usr/bin/env bash
# Quick VM/JIT parity check — run after any compiler change.
# Usage: scripts/check_parity.sh [directory...]
# Default: examples/basics examples/advanced examples/functions examples/patterns
#          examples/tail_call examples/perf examples/primop tests/flux
set -euo pipefail

dirs=("${@:-examples/basics examples/advanced examples/functions examples/patterns examples/tail_call examples/perf examples/primop tests/flux}")
exceptions_file="$(dirname "$0")/check_parity_exceptions.tsv"

# Build once (incremental — fast if only source changed)
cargo build --all-features --quiet 2>/dev/null

lookup_parity_mode() {
  local fixture="$1"
  [[ -f "$exceptions_file" ]] || return 1

  while IFS=$'\t' read -r listed_fixture listed_mode; do
    [[ -n "${listed_fixture// }" ]] || continue
    [[ "$listed_fixture" == \#* ]] && continue
    if [[ "$listed_fixture" == "$fixture" ]]; then
      printf '%s\n' "$listed_mode"
      return 0
    fi
  done < "$exceptions_file"

  return 1
}

normalize_output() {
  local mode="$1"
  case "$mode" in
    time)
      sed -E 's/[0-9]{10,}/<TIME>/g'
      ;;
    runtime_no_stack)
      awk '
        /^Stack trace:$/ { exit }
        { print }
      '
      ;;
    *)
      cat
      ;;
  esac
}

pass=0
fail=0
skip=0
failures=""

for dir in ${dirs[@]}; do
  [[ -d "$dir" ]] || continue
  for f in "$dir"/*.flx; do
    [[ -f "$f" ]] || continue

    mode="$(lookup_parity_mode "$f" || true)"
    mode="${mode:-exact}"
    extra_args=()
    if [[ "$mode" == strict* ]]; then
      extra_args+=("--strict")
    fi

    extra_args_str=""
    if ((${#extra_args[@]} > 0)); then
      extra_args_str="${extra_args[*]} "
    fi

    vm_cmd="target/debug/flux ${extra_args_str}--no-cache $f"
    jit_cmd="target/debug/flux ${extra_args_str}--no-cache $f --jit"
    vm_cargo_cmd="cargo run -- ${extra_args_str}--no-cache $f"
    jit_cargo_cmd="cargo run --features jit -- ${extra_args_str}--no-cache $f --jit"

    if ((${#extra_args[@]} > 0)); then
      vm_out=$(NO_COLOR=1 target/debug/flux "${extra_args[@]}" --no-cache "$f" 2>&1) || true
      vm_rc=${PIPESTATUS[0]:-$?}
      jit_out=$(NO_COLOR=1 target/debug/flux "${extra_args[@]}" --no-cache "$f" --jit 2>&1) || true
      jit_rc=${PIPESTATUS[0]:-$?}
    else
      vm_out=$(NO_COLOR=1 target/debug/flux --no-cache "$f" 2>&1) || true
      vm_rc=${PIPESTATUS[0]:-$?}
      jit_out=$(NO_COLOR=1 target/debug/flux --no-cache "$f" --jit 2>&1) || true
      jit_rc=${PIPESTATUS[0]:-$?}
    fi
    vm_cmp="$vm_out"
    jit_cmp="$jit_out"
    if [[ "$mode" != "exact" ]]; then
      vm_cmp=$(printf '%s\n' "$vm_out" | normalize_output "$mode")
      jit_cmp=$(printf '%s\n' "$jit_out" | normalize_output "$mode")
    fi

    if [[ "$vm_rc" -ne "$jit_rc" ]]; then
      fail=$((fail + 1))
      failures="${failures}\n  \033[31m✗\033[0m $f  exit: vm=$vm_rc jit=$jit_rc"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      VM run:  $vm_cargo_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
      failures="${failures}\n      JIT run: $jit_cargo_cmd"
    elif [[ "$vm_rc" -eq 0 && "$vm_cmp" != "$jit_cmp" ]]; then
      fail=$((fail + 1))
      failures="${failures}\n  \033[31m✗\033[0m $f  output differs"
      failures="${failures}\n      Compare mode: ${mode/exact/exact raw output}"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      VM run:  $vm_cargo_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
      failures="${failures}\n      JIT run: $jit_cargo_cmd"
      failures="${failures}\n      VM out:  $(echo "$vm_out" | head -1)"
      failures="${failures}\n      JIT out: $(echo "$jit_out" | head -1)"
    else
      pass=$((pass + 1))
      if [[ "$mode" == "exact" ]]; then
        echo -e "  \033[32m✓\033[0m $f"
      else
        echo -e "  \033[32m✓\033[0m $f (normalized: $mode)"
      fi
    fi
  done
done

total=$((pass + fail))
echo ""
if [[ "$fail" -eq 0 ]]; then
  echo -e "\033[32m✓ All $total examples match between VM and JIT\033[0m"
else
  echo -e "\033[31m✗ $fail/$total parity failures:\033[0m"
  echo -e "$failures"
  echo ""
  exit 1
fi
