#!/bin/bash
# check_core_to_llvm_parity.sh — Compare VM and core_to_llvm native output.
#
# Usage:
#   scripts/check_core_to_llvm_parity.sh [dir] [--root dir ...]
#
# Runs each .flx file through both backends and reports parity.
# Requires: cargo build --features native

DIR="${1:-examples/basics}"
shift 2>/dev/null || true

# Collect extra args (e.g. --root lib --root examples/aoc/2024)
EXTRA_ARGS=("$@")

TMPBIN="/tmp/flux_parity_test_$$"
FLUX="target/debug/flux"
TIMEOUT=15

if [ ! -f "$FLUX" ]; then
    echo "Build first: cargo build --all --all-features"
    exit 1
fi

pass=0
skip=0
mismatch=0
total=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

for f in "$DIR"/*.flx; do
    [ -f "$f" ] || continue
    name=$(basename "$f")
    total=$((total + 1))

    # Delete any stale bytecode cache
    fxc="${f%.flx}.fxc"
    rm -f "$fxc"

    # Build the command arrays
    vm_cmd=("$FLUX" "$f" --no-cache "${EXTRA_ARGS[@]}")
    native_cmd=("$FLUX" "$f" --native --no-cache "${EXTRA_ARGS[@]}")

    # Print full cargo run commands for easy copy-paste
    vm_cargo="cargo run -- $f --no-cache${EXTRA_ARGS[*]:+ ${EXTRA_ARGS[*]}}"
    native_cargo="cargo run --features native -- $f --native --no-cache${EXTRA_ARGS[*]:+ ${EXTRA_ARGS[*]}}"
    echo -e "${CYAN}vm:${NC}     ${vm_cargo}"
    echo -e "${CYAN}native:${NC} ${native_cargo}"

    # Run VM (capture stdout only; stderr goes to /dev/null)
    vm_out=$(timeout $TIMEOUT "${vm_cmd[@]}" 2>/dev/null || true)

    # Run native (capture stdout; save stderr separately for error detection)
    native_err=$(mktemp)
    native_out=$(timeout $TIMEOUT "${native_cmd[@]}" 2>"$native_err" || true)

    # Check if native compilation failed
    if grep -q "core_to_llvm compilation failed\|unsupported CoreToLlvm" "$native_err"; then
        skip=$((skip + 1))
        reason=$(grep -E "failed|unsupported" "$native_err" | head -1 | sed 's/.*: //')
        echo -e "${YELLOW}SKIP${NC} $name${reason:+ ($reason)}"
        echo ""
        continue
    fi

    # Compare
    if [ "$vm_out" = "$native_out" ]; then
        pass=$((pass + 1))
        echo -e "${GREEN}PASS${NC} $name"
    else
        mismatch=$((mismatch + 1))
        echo -e "${RED}MISMATCH${NC} $name"
        diff_out=$(diff <(echo "$vm_out") <(echo "$native_out") | head -8)
        echo "  $diff_out"
    fi
    echo ""

    rm -f "$TMPBIN" "$native_err"
done

echo "=== Parity Results ==="
echo "Total:    $total"
echo -e "Pass:     ${GREEN}$pass${NC}"
echo -e "Mismatch: ${RED}$mismatch${NC}"
echo -e "Skip:     ${YELLOW}$skip${NC}"

if [ $mismatch -eq 0 ] && [ $pass -gt 0 ]; then
    echo -e "\n${GREEN}All compiled examples match!${NC}"
    exit 0
else
    exit 1
fi
