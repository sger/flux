#!/usr/bin/env bash
# The benchmark harness is `cargo xtask bench` (xtask/src/bench/), which runs
# on Linux, macOS and Windows. This script keeps the old command working.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
exec cargo xtask bench "$@"
