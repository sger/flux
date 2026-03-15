#!/usr/bin/env bash
# Quick VM/JIT parity check — run after any compiler change.
# Usage: scripts/check_parity.sh [directory...]
# Default: examples/basics examples/advanced examples/functions examples/patterns
#          examples/tail_call examples/perf examples/primop tests/flux
set -euo pipefail

dirs=("${@:-examples/basics examples/advanced examples/functions examples/patterns examples/tail_call examples/perf examples/primop tests/flux}")

# Build once (incremental — fast if only source changed)
cargo build --all-features --quiet 2>/dev/null

pass=0
fail=0
skip=0
failures=""

for dir in ${dirs[@]}; do
  [[ -d "$dir" ]] || continue
  for f in "$dir"/*.flx; do
    [[ -f "$f" ]] || continue

    vm_cmd="cargo run -- --no-cache $f"
    jit_cmd="cargo run --features jit -- $f --jit"

    vm_out=$(target/debug/flux --no-cache "$f" 2>&1) || true
    vm_rc=${PIPESTATUS[0]:-$?}
    jit_out=$(target/debug/flux --no-cache "$f" --jit 2>&1) || true
    jit_rc=${PIPESTATUS[0]:-$?}

    if [[ "$vm_rc" -ne "$jit_rc" ]]; then
      fail=$((fail + 1))
      failures="${failures}\n  \033[31m✗\033[0m $f  exit: vm=$vm_rc jit=$jit_rc"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
    elif [[ "$vm_rc" -eq 0 && "$vm_out" != "$jit_out" ]]; then
      fail=$((fail + 1))
      failures="${failures}\n  \033[31m✗\033[0m $f  stdout differs"
      failures="${failures}\n      VM cmd:  $vm_cmd"
      failures="${failures}\n      JIT cmd: $jit_cmd"
      failures="${failures}\n      VM out:  $(echo "$vm_out" | head -1)"
      failures="${failures}\n      JIT out: $(echo "$jit_out" | head -1)"
    else
      pass=$((pass + 1))
      echo -e "  \033[32m✓\033[0m $f"
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
