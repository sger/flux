# AoC Cross-Language Benchmarking

This guide now uses the maintained benchmark suite instead of separate AoC comparison programs.

## Recommended Order (Compare then Profile)

Use this exact sequence when tuning VM performance:

1. Build release binaries:

```bash
cargo build --release --bin flux
```

2. Verify the benchmark implementations return the same answer:

```bash
./target/release/flux benchmarks/flux/cfold.flx
./target/release/cfold_rust
python3 benchmarks/python/cfold.py
```

Expected output for all three: `10426 10426`.

3. Run cross-language comparison:

```bash
cargo xtask bench cfold --runs 30 --warmup 5
```

4. Profile the VM on the benchmark workload:

```bash
cargo xtask bench cfold --profile vm
```

samply records the run and opens it in the Firefox Profiler. Invert the call tree to
see the hottest functions first.

Why this order:
- Comparison tells you if Flux got faster/slower vs Rust/Python.
- The profile then tells you where VM time is still being spent.

## Rules for Fair Comparison

- Use the same benchmark workload.
- Use the same algorithm and output format.
- Measure on the same machine and power mode.
- Run enough samples (`30+`) and use medians.
- Keep benchmark commands focused on runtime (avoid extra logging).

## Script

Use:

```bash
cargo xtask bench --help
```

## Recommended Setup

1. Build native implementations first (if needed).
2. Run benchmark with Flux + other languages.
3. Record median and relative speed.

## Example Commands

### `cfold`

```bash
cargo xtask bench cfold --runs 30 --warmup 3
```

### `binarytrees`

```bash
cargo xtask bench binarytrees --runs 30 --warmup 3
```

## Optional: Compile + Run vs Run-only

- Run-only: benchmark already-built binaries/scripts.
- Compile + run: benchmark commands that include build step.

For AoC language/runtime quality, prioritize run-only first.

## Report Template

```text
Task: cfold
Workload: <smoke/full>
Machine: <cpu/ram/os>
Date: <yyyy-mm-dd>

Flux median: <ms>
Rust median: <ms>
Python median: <ms>
Haskell median: <ms>
OCaml median: <ms>

Relative to Flux:
Rust: <x.x>x
Python: <x.x>x
Haskell: <x.x>x
OCaml: <x.x>x

Notes:
- <algorithm parity confirmed?>
- <any stack-safety/runtime differences?>
```
