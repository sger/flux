# Compiler Benchmarking

This guide documents compile-path micro-benchmarking for `Compiler::compile_with_opts`.

## Quick Start

From repository root:

```bash
cargo bench --bench compiler_compile_bench
rm -rf baseline_compiler_criterion
cp -r target/criterion baseline_compiler_criterion

# run again after compiler changes
cargo bench --bench compiler_compile_bench
```

## Benchmark Target

- Benchmark file: `benches/compiler_compile_bench.rs`
- Cargo target:
  - `[[bench]]`
  - `name = "compiler_compile_bench"`
  - `harness = false`

## Measured Paths

`compiler_compile_bench` measures two compile modes for representative pre-parsed programs:

1. `compile_with_opts(program, false, false)`
2. `compile_with_opts(program, false, true)`

This isolates non-optimized compile/analyze-path overhead, including clone behavior.

## Logging and Artifacts

```bash
mkdir -p perf_logs
ts=$(date +%Y%m%d-%H%M%S)

cargo bench --bench compiler_compile_bench 2>&1 | tee perf_logs/compiler-bench-${ts}.log
cp -r target/criterion perf_logs/compiler-criterion-${ts}
```

## Validation Notes

Compiler micro-optimizations in v0.0.4 must preserve diagnostics behavior.
Run parity and release checks after benchmarked changes:

```bash
cargo test --all --all-features purity_vm_jit_parity_snapshots
cargo test --all --all-features --test runtime_vm_jit_parity_release
```
