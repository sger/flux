# Lexer Benchmarking

This guide documents how to run and compare Flux lexer performance benchmarks.

## Quick Start

From repository root:

```bash
# one-time setup
cargo install rust-script

# baseline
cargo bench --bench lexer_bench
rm -rf baseline_criterion
cp -r target/criterion baseline_criterion

# run again after code changes
cargo bench --bench lexer_bench

# generate comparison table + PERF_REPORT.md
rust-script scripts/bench_report.rs
```

## Prerequisites

- `criterion` is present in `Cargo.toml` dev dependencies.
- Benchmark target exists in `Cargo.toml`:
  - `[[bench]]`
  - `name = "lexer_bench"`
  - `harness = false`
- Benchmark file exists: `benches/lexer_bench.rs`

## 1) Run Lexer Benchmarks

Run Criterion for the lexer benchmark target:

```bash
cargo bench --bench lexer_bench
```

Criterion output is written to:

```text
target/criterion
```

## 2) Create a Baseline Snapshot

After a baseline run, snapshot the Criterion directory:

```bash
rm -rf baseline_criterion
cp -r target/criterion baseline_criterion
```

## 3) Run Current Benchmarks

After your code changes:

```bash
cargo bench --bench lexer_bench
```

## 4) Generate Before/After Report

Compare baseline vs current:

```bash
rust-script scripts/bench_report.rs
```

The script prints:
- benchmark name
- baseline mean (ms)
- current mean (ms)
- percent change
- baseline/current bytes per second

It also writes an auto-filled report file:

```text
PERF_REPORT.md
```

## 5) Log Results

Save raw bench output and parsed report:

```bash
mkdir -p perf_logs
ts=$(date +%Y%m%d-%H%M%S)

cargo bench --bench lexer_bench 2>&1 | tee perf_logs/bench-${ts}.log
rust-script scripts/bench_report.rs 2>&1 | tee perf_logs/report-${ts}.txt
cp -r target/criterion perf_logs/criterion-${ts}
```

## Troubleshooting

### `rust-script` not found

Install and run from repository root:

```bash
cargo install rust-script
cd /path/to/flux
rust-script scripts/bench_report.rs
```

### Baseline path mismatch

If your baseline is nested (for example `baseline_criterion/criterion`), set explicit paths:

```bash
PERF_BASELINE_DIR=baseline_criterion/criterion \
PERF_CURRENT_DIR=target/criterion \
rust-script scripts/bench_report.rs
```

### Missing baseline benchmarks

Recreate baseline from the same benchmark set:

```bash
rm -rf baseline_criterion
cargo bench --bench lexer_bench
cp -r target/criterion baseline_criterion
```
