#!/usr/bin/env bash
# Quick VM/JIT/LLVM parity check — run after any compiler change.
# Usage: scripts/release/check_parity.sh [directory...]
# Default: examples/basics examples/advanced examples/functions examples/patterns
#          examples/tail_call examples/perf examples/primop tests/flux
#
# The LLVM pass runs automatically when the binary was built with --features llvm.
# On CI without LLVM installed, the LLVM pass is skipped silently.
set -euo pipefail

dirs=("${@:-examples/basics examples/advanced examples/functions examples/patterns examples/tail_call examples/perf examples/primop tests/flux}")
exceptions_file="$(dirname "$0")/check_parity_exceptions.tsv"

# Build once (incremental — fast if only source changed)
cargo build --all-features --quiet 2>/dev/null

# Detect LLVM support: check if the binary accepts --llvm flag
has_llvm=false
if target/debug/flux --llvm /dev/null 2>&1 | grep -qv "unknown.*llvm\|unrecognized.*llvm"; then
  has_llvm=true
fi

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

lookup_fixture_roots() {
  local fixture="$1"
  case "$fixture" in
    examples/aoc/2024/*)
      printf '%s\n' "lib" "examples/aoc/2024"
      return 0
      ;;
  esac

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
llvm_pass=0
llvm_fail=0
failures=""

for dir in ${dirs[@]}; do
  [[ -d "$dir" ]] || continue
  for f in "$dir"/*.flx; do
    [[ -f "$f" ]] || continue

    mode="$(lookup_parity_mode "$f" || true)"
    mode="${mode:-exact}"
    if [[ "$mode" == "skip" ]]; then
      skip=$((skip + 1))
      echo -e "  \033[33m-\033[0m $f (skipped)"
      continue
    fi
    extra_args=()
    root_args=()
    if [[ "$mode" == strict* ]]; then
      extra_args+=("--strict")
    fi
    while IFS= read -r root; do
      [[ -n "${root// }" ]] || continue
      root_args+=("--root" "$root")
    done < <(lookup_fixture_roots "$f" || true)

    extra_args_str=""
    if ((${#extra_args[@]} > 0)); then
      extra_args_str="${extra_args[*]} "
    fi
    root_args_str=""
    if ((${#root_args[@]} > 0)); then
      root_args_str="${root_args[*]} "
    fi

    vm_cmd="target/debug/flux ${extra_args_str}${root_args_str}--no-cache $f"
    jit_cmd="target/debug/flux ${extra_args_str}${root_args_str}--no-cache $f --jit"
    vm_cargo_cmd="cargo run -- ${extra_args_str}${root_args_str}--no-cache $f"
    jit_cargo_cmd="cargo run --features jit -- ${extra_args_str}${root_args_str}--no-cache $f --jit"

    if ((${#extra_args[@]} > 0)) || ((${#root_args[@]} > 0)); then
      vm_out=$(NO_COLOR=1 target/debug/flux "${extra_args[@]:+${extra_args[@]}}" "${root_args[@]:+${root_args[@]}}" --no-cache "$f" 2>&1) || true
      vm_rc=${PIPESTATUS[0]:-$?}
      jit_out=$(NO_COLOR=1 target/debug/flux "${extra_args[@]:+${extra_args[@]}}" "${root_args[@]:+${root_args[@]}}" --no-cache "$f" --jit 2>&1) || true
      jit_rc=${PIPESTATUS[0]:-$?}
    else
      vm_out=$(NO_COLOR=1 target/debug/flux --no-cache "$f" 2>&1) || true
      vm_rc=${PIPESTATUS[0]:-$?}
      jit_out=$(NO_COLOR=1 target/debug/flux --no-cache "$f" --jit 2>&1) || true
      jit_rc=${PIPESTATUS[0]:-$?}
    fi
    # Collect LLVM output if available
    llvm_out=""
    llvm_rc=0
    llvm_cmd=""
    if [[ "$has_llvm" == true ]]; then
      llvm_cmd="target/debug/flux ${extra_args_str}${root_args_str}--no-cache $f --llvm"
      if ((${#extra_args[@]} > 0)) || ((${#root_args[@]} > 0)); then
        llvm_out=$(NO_COLOR=1 target/debug/flux "${extra_args[@]:+${extra_args[@]}}" "${root_args[@]:+${root_args[@]}}" --no-cache "$f" --llvm 2>&1) || true
        llvm_rc=${PIPESTATUS[0]:-$?}
      else
        llvm_out=$(NO_COLOR=1 target/debug/flux --no-cache "$f" --llvm 2>&1) || true
        llvm_rc=${PIPESTATUS[0]:-$?}
      fi
    fi

    vm_cmp="$vm_out"
    jit_cmp="$jit_out"
    llvm_cmp="$llvm_out"
    if [[ "$mode" != "exact" ]]; then
      vm_cmp=$(printf '%s\n' "$vm_out" | normalize_output "$mode")
      jit_cmp=$(printf '%s\n' "$jit_out" | normalize_output "$mode")
      if [[ "$has_llvm" == true ]]; then
        llvm_cmp=$(printf '%s\n' "$llvm_out" | normalize_output "$mode")
      fi
    fi

    # Check VM vs JIT parity
    jit_ok=true
    if [[ "$vm_rc" -ne "$jit_rc" ]]; then
      fail=$((fail + 1))
      jit_ok=false
      failures="${failures}\n  \033[31m✗\033[0m $f  exit: vm=$vm_rc jit=$jit_rc"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      VM run:  $vm_cargo_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
      failures="${failures}\n      JIT run: $jit_cargo_cmd"
    elif [[ "$vm_rc" -eq 0 && "$vm_cmp" != "$jit_cmp" ]]; then
      fail=$((fail + 1))
      jit_ok=false
      failures="${failures}\n  \033[31m✗\033[0m $f  output differs (VM vs JIT)"
      failures="${failures}\n      Compare mode: ${mode/exact/exact raw output}"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      VM run:  $vm_cargo_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
      failures="${failures}\n      JIT run: $jit_cargo_cmd"
      failures="${failures}\n      VM out:  $(echo "$vm_out" | head -1)"
      failures="${failures}\n      JIT out: $(echo "$jit_out" | head -1)"
    fi

    # Check VM vs LLVM parity (if LLVM available)
    llvm_ok=true
    if [[ "$has_llvm" == true && "$vm_rc" -eq 0 ]]; then
      if [[ "$vm_rc" -ne "$llvm_rc" ]]; then
        llvm_fail=$((llvm_fail + 1))
        llvm_ok=false
        failures="${failures}\n  \033[31m✗\033[0m $f  exit: vm=$vm_rc llvm=$llvm_rc"
        failures="${failures}\n      LLVM cmd: $llvm_cmd"
      elif [[ "$vm_cmp" != "$llvm_cmp" ]]; then
        llvm_fail=$((llvm_fail + 1))
        llvm_ok=false
        failures="${failures}\n  \033[31m✗\033[0m $f  output differs (VM vs LLVM)"
        failures="${failures}\n      LLVM cmd: $llvm_cmd"
        failures="${failures}\n      VM out:  $(echo "$vm_out" | head -1)"
        failures="${failures}\n      LLVM out: $(echo "$llvm_out" | head -1)"
      else
        llvm_pass=$((llvm_pass + 1))
      fi
    fi

    if [[ "$jit_ok" == true ]]; then
      pass=$((pass + 1))
      label=""
      if [[ "$has_llvm" == true && "$llvm_ok" == true && "$vm_rc" -eq 0 ]]; then
        label=" +llvm"
      fi
      if [[ "$mode" == "exact" ]]; then
        echo -e "  \033[32m✓\033[0m $f${label}"
      else
        echo -e "  \033[32m✓\033[0m $f (normalized: $mode)${label}"
      fi
    fi
  done
done

total=$((pass + fail))
llvm_total=$((llvm_pass + llvm_fail))
echo ""
if [[ "$fail" -eq 0 ]]; then
  if [[ "$skip" -eq 0 ]]; then
    echo -e "\033[32m✓ All $total examples match between VM and JIT\033[0m"
  else
    echo -e "\033[32m✓ All $total checked examples match between VM and JIT\033[0m"
    echo -e "\033[33m- Skipped $skip examples\033[0m"
  fi
else
  echo -e "\033[31m✗ $fail/$total VM/JIT parity failures:\033[0m"
  echo -e "$failures"
fi

if [[ "$has_llvm" == true ]]; then
  if [[ "$llvm_fail" -eq 0 ]]; then
    echo -e "\033[32m✓ All $llvm_total examples match between VM and LLVM\033[0m"
  else
    echo -e "\033[31m✗ $llvm_fail/$llvm_total VM/LLVM parity failures\033[0m"
  fi
else
  echo -e "\033[33m- LLVM parity skipped (not compiled with --features llvm)\033[0m"
fi

if [[ "$fail" -ne 0 || "$llvm_fail" -ne 0 ]]; then
  echo ""
  exit 1
fi
