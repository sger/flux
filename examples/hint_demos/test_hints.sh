#!/bin/bash

# Test script to demonstrate the hint positioning feature
# Run this to see multi-location error messages in action

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FLUX_BIN="${FLUX_BIN:-cargo run --quiet --}"

echo "=========================================="
echo "Hint Positioning Feature Demo"
echo "=========================================="
echo ""

echo "1. Testing duplicate_variable.flx"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/duplicate_variable.flx" 2>&1 || true
echo ""
echo ""

echo "2. Testing function_arg_mismatch.flx"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/function_arg_mismatch.flx" 2>&1 || true
echo ""
echo ""

echo "3. Testing type_mismatch.flx"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/type_mismatch.flx" 2>&1 || true
echo ""
echo ""

echo "4. Testing shadowing_ok.flx (should succeed)"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/shadowing_ok.flx" 2>&1 && echo "✓ Compiled successfully!" || echo "✗ Unexpected error"
echo ""
echo ""

echo "5. Testing unknown_operator.flx"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/unknown_operator.flx" 2>&1 || true
echo ""
echo ""

echo "6. Testing invalid_lambda.flx (categorized hints)"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/invalid_lambda.flx" 2>&1 || true
echo ""
echo ""

echo "7. Testing inline_suggestion_demo.flx (inline suggestions)"
echo "------------------------------------------"
$FLUX_BIN "$SCRIPT_DIR/inline_suggestion_demo.flx" 2>&1 || true
echo ""
echo ""

echo "=========================================="
echo "Demo Complete!"
echo "=========================================="
echo ""
echo "Notice how errors now show:"
echo "  • The error location (where the problem occurs)"
echo "  • Related locations (where relevant code was defined)"
echo "  • Descriptive labels for each location"
echo "  • Context-aware hints"
echo ""
echo "This makes debugging much easier! 🎉"
