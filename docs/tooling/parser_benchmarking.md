# Parser Benchmarking

This guide documents how to run and compare Flux parser performance benchmarks.

## Quick Start

From repository root:

```bash
cargo bench --bench parser_bench
rm -rf baseline_parser_criterion
cp -r target/criterion baseline_parser_criterion

# run again after parser changes
cargo bench --bench parser_bench
```

## Benchmark Target

- Benchmark file: `benches/parser_bench.rs`
- Cargo target:
  - `[[bench]]`
  - `name = "parser_bench"`
  - `harness = false`

## Corpora

`parser_bench` includes four representative corpora:

1. declaration-heavy
2. expression/operator-heavy
3. string/interpolation/comment-heavy
4. malformed/recovery-heavy

## Logging and Artifacts

Save benchmark artifacts for before/after comparisons:

```bash
mkdir -p perf_logs
ts=$(date +%Y%m%d-%H%M%S)

cargo bench --bench parser_bench 2>&1 | tee perf_logs/parser-bench-${ts}.log
cp -r target/criterion perf_logs/parser-criterion-${ts}
```

## Validation Notes

- Parser benchmark changes must not alter grammar acceptance.
- Run parser regression suites after optimizations:

```bash
cargo test --test parser_tests
cargo test --test parser_recovery
cargo test --test snapshot_parser
```
