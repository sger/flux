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
scripts/bench.sh cfold --runs 30 --warmup 5
```

4. Generate VM flamegraph on the benchmark workload:

```bash
scripts/bench_benchmark_flamewatch.sh cfold --skip-build
```

This writes `flamegraph.svg` at the project root and prints the top hot paths.

5. Inspect top runtime hotspots from the generated SVG:

```bash
perl -ne 'while(/<title>([^<]+)<\/title>/g){$t=$1; if($t =~ /^(.*) \(([0-9,]+) samples, ([0-9.]+)%\)$/){$n=$1;$p=$3; if(!defined $m{$n} || $p>$m{$n}){$m{$n}=$p;} }} END { for $k (keys %m){ printf "%.2f\t%s\n", $m{$k}, $k; } }' flamegraph.svg | sort -nr | head -n 20
```

Why this order:
- Comparison tells you if Flux got faster/slower vs Rust/Python.
- Flamegraph then tells you where VM time is still being spent.

## Rules for Fair Comparison

- Use the same benchmark workload.
- Use the same algorithm and output format.
- Measure on the same machine and power mode.
- Run enough samples (`30+`) and use medians.
- Keep benchmark commands focused on runtime (avoid extra logging).

## Script

Use:

```bash
scripts/bench.sh --help
```

## Recommended Setup

1. Build native implementations first (if needed).
2. Run benchmark with Flux + other languages.
3. Record median and relative speed.

## Example Commands

### `cfold`

```bash
scripts/bench.sh cfold --runs 30 --warmup 3
```

### `binarytrees`

```bash
scripts/bench.sh binarytrees --runs 30 --warmup 3
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
