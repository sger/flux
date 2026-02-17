#!/bin/bash
# Run the student grade analyzer example
# This script sets up the correct module root for multi-module resolution

set -e

cd "$(dirname "$0")/.."

echo "Running Student Grade Analyzer..."
echo "================================="
echo ""

cargo run -- --root examples examples/advanced/grade_analyzer.flx
