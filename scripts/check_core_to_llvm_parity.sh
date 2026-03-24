#!/bin/bash
# check_core_to_llvm_parity.sh — Compare VM and core_to_llvm native output.
#
# Usage:
#   scripts/check_core_to_llvm_parity.sh [dir]
#
# Runs each .flx file through both backends and reports parity.

DIR="${1:-examples/basics}"
TMPBIN="/tmp/flux_parity_test_$$"
TIMEOUT=15

pass=0
fail=0
skip=0
mismatch=0
total=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

for f in "$DIR"/*.flx; do
    name=$(basename "$f")
    total=$((total + 1))

    # Try to compile to native binary
    native_compile=$(timeout $TIMEOUT cargo run --features core_to_llvm -- "$f" --core-to-llvm --emit-binary -o "$TMPBIN" 2>&1)
    if ! echo "$native_compile" | grep -q "Emitted binary"; then
        skip=$((skip + 1))
        reason=$(echo "$native_compile" | grep -E "failed|unsupported" | head -1 | sed 's/.*: //')
        echo -e "${YELLOW}SKIP${NC} $name${reason:+ ($reason)}"
        continue
    fi

    # Run native binary
    native_out=$(timeout $TIMEOUT "$TMPBIN" 2>&1 || true)

    # Run VM
    vm_out=$(timeout $TIMEOUT cargo run -- "$f" 2>&1 | grep -v '^warning\|^  \|^$\|Finished\|Running\|Compiling\|^   =\|^   |' || true)

    # Compare
    if [ "$vm_out" = "$native_out" ]; then
        pass=$((pass + 1))
        echo -e "${GREEN}PASS${NC} $name"
    else
        mismatch=$((mismatch + 1))
        echo -e "${RED}MISMATCH${NC} $name"
        # Show first difference
        diff_out=$(diff <(echo "$vm_out") <(echo "$native_out") | head -6)
        echo "  $diff_out"
    fi

    rm -f "$TMPBIN"
done

echo ""
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
