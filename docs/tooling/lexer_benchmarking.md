# Lexer Benchmarking

This guide documents how to run and compare Flux lexer performance benchmarks.

## Quick Start

From repository root:

```bash
# baseline
cargo bench --bench lexer_bench
rm -rf baseline_criterion
cp -r target/criterion baseline_criterion

# run again after code changes
cargo bench --bench lexer_bench
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

## 4) Compare Before/After Results

Compare the latest Criterion run against the saved baseline by inspecting:

```bash
open target/criterion/report/index.html
open baseline_criterion/report/index.html
```

For a simple CLI summary, compare the `estimates.json` files under:

```text
target/criterion/**/new/estimates.json
baseline_criterion/**/new/estimates.json
```

## 5) Log Results

Save raw bench output and Criterion snapshots:

```bash
mkdir -p perf_logs
ts=$(date +%Y%m%d-%H%M%S)

cargo bench --bench lexer_bench 2>&1 | tee perf_logs/bench-${ts}.log
cp -r target/criterion perf_logs/criterion-${ts}
```

## Troubleshooting

### Baseline path mismatch

If your baseline is nested differently, compare the correct `report/` and `new/estimates.json` paths directly.

### Missing baseline benchmarks

Recreate baseline from the same benchmark set:

```bash
rm -rf baseline_criterion
cargo bench --bench lexer_bench
cp -r target/criterion baseline_criterion
```
