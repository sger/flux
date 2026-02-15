#!/usr/bin/env bash
set -euo pipefail

BENCH_NAME="${BENCH_NAME:-aoc_day1_bench}"
GROUP_PREFIX="${GROUP_PREFIX:-aoc/day1/}"

usage() {
  cat <<'EOF'
Usage:
  scripts/bench_aoc.sh save [baseline] [-- <criterion-args...>]
  scripts/bench_aoc.sh compare [baseline] [-- <criterion-args...>]
  scripts/bench_aoc.sh run [-- <criterion-args...>]

Examples:
  scripts/bench_aoc.sh save main
  scripts/bench_aoc.sh compare main
  scripts/bench_aoc.sh compare main -- --sample-size 50

Environment:
  BENCH_NAME    Criterion bench target (default: aoc_day1_bench)
  GROUP_PREFIX  Benchmark id prefix to summarize (default: aoc/day1/)
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || $# -eq 0 ]]; then
  usage
  exit 0
fi

MODE="$1"
shift

BASELINE="main"
if [[ "$MODE" == "save" || "$MODE" == "compare" ]]; then
  if [[ "${1:-}" != "" && "${1:-}" != "--" ]]; then
    BASELINE="$1"
    shift
  fi
fi

EXTRA_ARGS=()
if [[ "${1:-}" == "--" ]]; then
  shift
  EXTRA_ARGS=("$@")
elif [[ $# -gt 0 ]]; then
  echo "Unexpected arguments: $*" >&2
  usage >&2
  exit 1
fi

CMD=(cargo bench --bench "$BENCH_NAME")
case "$MODE" in
  save)
    CMD+=(-- --save-baseline "$BASELINE")
    ;;
  compare)
    CMD+=(-- --baseline "$BASELINE")
    ;;
  run)
    ;;
  *)
    echo "Unknown mode: $MODE" >&2
    usage >&2
    exit 1
    ;;
esac

if [[ ${#EXTRA_ARGS[@]} -gt 0 ]]; then
  if [[ "$MODE" == "run" ]]; then
    CMD+=(-- "${EXTRA_ARGS[@]}")
  else
    CMD+=("${EXTRA_ARGS[@]}")
  fi
fi

LOG_FILE="$(mktemp "${TMPDIR:-/tmp}/bench_aoc.XXXXXX.log")"
trap 'rm -f "$LOG_FILE"' EXIT

echo "Running: ${CMD[*]}"
"${CMD[@]}" 2>&1 | tee "$LOG_FILE"

echo
echo "Summary (${GROUP_PREFIX}*)"
awk -v prefix="$GROUP_PREFIX" '
  BEGIN {
    n = 0
    in_change = 0
  }
  $0 ~ ("^" prefix) {
    bench = $0
    in_change = 0
    if (!(bench in seen)) {
      seen[bench] = 1
      order[++n] = bench
    }
    next
  }
  /^[[:space:]]+change:/ && bench != "" {
    in_change = 1
    next
  }
  /^[[:space:]]+time:/ && bench != "" && in_change == 0 {
    line = $0
    sub(/^[[:space:]]+time:[[:space:]]*/, "", line)
    times[bench] = line
    next
  }
  /^[[:space:]]+time:/ && bench != "" && in_change == 1 {
    line = $0
    sub(/^[[:space:]]+time:[[:space:]]*/, "", line)
    changes[bench] = line
    next
  }
  /^Found [0-9]+ outliers/ {
    in_change = 0
    next
  }
  END {
    if (n == 0) {
      print "No benchmarks matched prefix:", prefix
      exit 0
    }
    printf "%-38s | %-34s | %s\n", "benchmark", "time", "change"
    printf "%-38s-+-%-34s-+-%s\n", "--------------------------------------", "----------------------------------", "------------------------------"
    for (i = 1; i <= n; i++) {
      b = order[i]
      t = (b in times) ? times[b] : "n/a"
      c = (b in changes) ? changes[b] : "n/a"
      printf "%-38s | %-34s | %s\n", b, t, c
    }
  }
' "$LOG_FILE"
