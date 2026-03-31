#!/usr/bin/env bash
set -euo pipefail

# check_parity.sh — Run Flux parity checks across VM and LLVM backends.
#
# Usage:
#   scripts/check_parity.sh                         # default: tests/parity + examples/basics
#   scripts/check_parity.sh tests/parity             # specific directory
#   scripts/check_parity.sh --extended               # extended pre-release suite
#
# Builds both VM and native binaries, then runs parity-check on the corpus.
# Exit 0 if all pass, exit 1 on any mismatch.

CYAN='\033[0;36m'
NC='\033[0m'

VM_TARGET_DIR="target/parity_vm"
NATIVE_TARGET_DIR="target/parity_native"
FLUX="target/debug/flux"

EXTENDED=false
DIRS=()

for arg in "$@"; do
    if [ "$arg" = "--extended" ]; then
        EXTENDED=true
    else
        DIRS+=("$arg")
    fi
done

# Default corpus
if [ ${#DIRS[@]} -eq 0 ]; then
    DIRS=("tests/parity" "examples/basics")
fi

# ── Build ───────────────────────────────────────────────────────────────────

echo -e "${CYAN}Building VM binary...${NC}"
CARGO_TARGET_DIR="$VM_TARGET_DIR" cargo build --quiet || exit 1

echo -e "${CYAN}Building native binary...${NC}"
CARGO_TARGET_DIR="$NATIVE_TARGET_DIR" cargo build --features core_to_llvm --quiet || exit 1

echo -e "${CYAN}Building parity-check runner...${NC}"
cargo build --quiet || exit 1

# ── Minimum gate: vm + llvm ─────────────────────────────────────────────────

FAILED=0

for dir in "${DIRS[@]}"; do
    echo ""
    echo -e "${CYAN}=== Parity: $dir (vm, llvm) ===${NC}"
    if ! "$FLUX" parity-check "$dir" --ways vm,llvm \
        --vm-binary "$VM_TARGET_DIR/debug/flux" \
        --llvm-binary "$NATIVE_TARGET_DIR/debug/flux"; then
        FAILED=1
    fi
done

# ── Extended pre-release gate ───────────────────────────────────────────────

if [ "$EXTENDED" = true ]; then
    EXTENDED_WAYS="vm_cached,vm_strict,llvm_strict"

    for dir in "${DIRS[@]}"; do
        echo ""
        echo -e "${CYAN}=== Parity: $dir ($EXTENDED_WAYS) ===${NC}"
        if ! "$FLUX" parity-check "$dir" --ways "$EXTENDED_WAYS" \
            --vm-binary "$VM_TARGET_DIR/debug/flux" \
            --llvm-binary "$NATIVE_TARGET_DIR/debug/flux"; then
            FAILED=1
        fi
    done
fi

# ── Result ──────────────────────────────────────────────────────────────────

echo ""
if [ "$FAILED" -eq 0 ]; then
    echo -e "${CYAN}Parity checks passed.${NC}"
    exit 0
else
    echo -e "\033[0;31mParity checks failed.\033[0m"
    exit 1
fi
