# AoC Cross-Language Benchmarking

This guide compares Flux against other languages on the same AoC task.

## Recommended Order (Compare then Profile)

Use this exact sequence when tuning VM performance:

1. Build release binaries:

```bash
cargo build --release --bin flux --bin aoc_day1_part2_rust
```

2. Verify all implementations return the same answer:

```bash
./target/release/flux examples/io/aoc_day1_part2.flx
./target/release/aoc_day1_part2_rust examples/io/aoc_day1.txt
python3 benchmarks/aoc/day1_part2.py examples/io/aoc_day1.txt
```

Expected output for all three: `6289`.

3. Run cross-language comparison:

```bash
scripts/bench_cross_lang.sh --native --runs 30 --warmup 5 \
  --name-prefix aoc_day1_part2 \
  --flux-cmd './target/release/flux examples/io/aoc_day1_part2.flx' \
  --rust-cmd './target/release/aoc_day1_part2_rust examples/io/aoc_day1.txt' \
  --python-cmd 'python3 benchmarks/aoc/day1_part2.py examples/io/aoc_day1.txt'
```

4. Generate VM flamegraph on a profile workload (not single-run input):

```bash
CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --reverse --bin flux -- examples/io/aoc_day1_part2_profile.flx
```

This writes `flamegraph.svg` at the project root.

5. Inspect top runtime hotspots from the generated SVG:

```bash
perl -ne 'while(/<title>([^<]+)<\/title>/g){$t=$1; if($t =~ /^(.*) \(([0-9,]+) samples, ([0-9.]+)%\)$/){$n=$1;$p=$3; if(!defined $m{$n} || $p>$m{$n}){$m{$n}=$p;} }} END { for $k (keys %m){ printf "%.2f\t%s\n", $m{$k}, $k; } }' flamegraph.svg | sort -nr | head -n 20
```

Why this order:
- Comparison tells you if Flux got faster/slower vs Rust/Python.
- Flamegraph then tells you where VM time is still being spent.

## Rules for Fair Comparison

- Use the same input file.
- Use the same algorithm and output format.
- Measure on the same machine and power mode.
- Run enough samples (`30+`) and use medians.
- Keep benchmark commands focused on runtime (avoid extra logging).

## Script

Use:

```bash
scripts/bench_cross_lang.sh --help
```

Default input:

- `examples/io/aoc_day1.txt`

## Recommended Setup

1. Build native implementations first (if needed).
2. Run benchmark with Flux + other languages.
3. Record median and relative speed.

## Example Commands

### Flux vs Rust vs Python vs Node

```bash
scripts/bench_cross_lang.sh \
  --runs 30 \
  --warmup 3 \
  --flux-cmd 'cargo run --release -- examples/io/aoc_day1.flx' \
  --rust-cmd './target/release/day1_rust examples/io/aoc_day1.txt' \
  --python-cmd 'python3 benchmarks/aoc/day1.py examples/io/aoc_day1.txt' \
  --node-cmd 'node benchmarks/aoc/day1.mjs examples/io/aoc_day1.txt'
```

### Flux vs Python only

```bash
scripts/bench_cross_lang.sh \
  --runs 50 \
  --flux-cmd 'cargo run --release -- examples/io/aoc_day1.flx' \
  --python-cmd 'python3 benchmarks/aoc/day1.py examples/io/aoc_day1.txt'
```

## Optional: Compile + Run vs Run-only

- Run-only: benchmark already-built binaries/scripts.
- Compile + run: benchmark commands that include build step.

For AoC language/runtime quality, prioritize run-only first.

## Report Template

```text
Task: AoC Day 1
Input bytes: <n>
Machine: <cpu/ram/os>
Date: <yyyy-mm-dd>

Flux median: <ms>
Rust median: <ms>
Python median: <ms>
Node median: <ms>

Relative to Flux:
Rust: <x.x>x
Python: <x.x>x
Node: <x.x>x

Notes:
- <algorithm parity confirmed?>
- <any IO/parsing differences?>
```
