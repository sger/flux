# Benchmarks

Benchmark sources are organized by language:

- `benchmarks/flux/`
- `benchmarks/haskell/`
- `benchmarks/python/`
- `benchmarks/rust/`

## Binary Trees

Sources:

- `benchmarks/flux/binarytrees.flx`
- `benchmarks/flux/binarytrees_smoke.flx`
- `benchmarks/rust/binarytrees.rs`
- `benchmarks/haskell/binarytrees.hs`
- `benchmarks/python/binarytrees.py`

Run each version directly:

```bash
cargo run --release --bin flux -- benchmarks/flux/binarytrees.flx
cargo run --release --features jit --bin flux -- benchmarks/flux/binarytrees.flx --jit
cargo run --release --bin binarytrees_rust
python3 benchmarks/python/binarytrees.py
runghc benchmarks/haskell/binarytrees.hs
```

For a fairer Haskell comparison, compile it first:

```bash
ghc -O2 benchmarks/haskell/binarytrees.hs -o target/release/binarytrees_hs
./target/release/binarytrees_hs
```

Run the cross-language benchmark with `hyperfine`:

```bash
scripts/bench_binarytrees.sh
```

By default this uses the smaller smoke workload with `n = 8` for every language.
Flux uses `benchmarks/flux/binarytrees_smoke.flx`, and Rust/Python/Haskell are invoked with `8`.
To benchmark the aligned full baseline instead:

```bash
scripts/bench_binarytrees.sh --full
```

Equivalent explicit commands:

```bash
scripts/bench_binarytrees.sh \
  --flux-cmd './target/release/flux benchmarks/flux/binarytrees.flx' \
  --flux-jit-cmd './target/release/flux benchmarks/flux/binarytrees.flx --jit' \
  --rust-cmd './target/release/binarytrees_rust 21' \
  --python-cmd 'python3 benchmarks/python/binarytrees.py 21' \
  --haskell-cmd './target/release/binarytrees_hs 21'
```

Useful options:

```bash
scripts/bench_binarytrees.sh --full --runs 3 --warmup 1
scripts/bench_binarytrees.sh --runs 30 --warmup 5
scripts/bench_binarytrees.sh --no-flux-jit
scripts/bench_binarytrees.sh --report-file reports/binarytrees_$(date +%F).md
scripts/bench_binarytrees.sh --haskell-cmd 'runghc benchmarks/haskell/binarytrees.hs'
```

Profile binarytrees specifically:

```bash
scripts/bench_binarytrees_flamewatch.sh
scripts/bench_binarytrees_flamewatch.sh --full --runs 3 --warmup 1
```

Latest report:

<!-- binarytrees-report:start -->
# Binary Trees Benchmark Report

- Generated: 2026-03-09 17:35:34 UTC
- Runs: 10
- Warmup: 2
- Full baseline: no

| Command | Mean [ms] | Min [ms] | Max [ms] | Relative |
|:---|---:|---:|---:|---:|
| `binarytrees/flux` | 57.6 ± 0.6 | 56.9 | 58.6 | 60.29 ± 16.47 |
| `binarytrees/flux-jit` | 64.8 ± 1.3 | 63.5 | 67.2 | 67.84 ± 18.57 |
| `binarytrees/rust` | 1.0 ± 0.3 | 0.8 | 1.7 | 1.00 |
| `binarytrees/python` | 10.2 ± 0.2 | 10.0 | 10.7 | 10.64 ± 2.91 |
| `binarytrees/haskell` | 10.8 ± 0.1 | 10.7 | 11.0 | 11.31 ± 3.09 |
<!-- binarytrees-report:end -->

## AoC Python Benchmarks

Python AoC comparison scripts now live under:

- `benchmarks/python/aoc/day1.py`
- `benchmarks/python/aoc/day1_part2.py`
- `benchmarks/python/aoc/day2_part1.py`
- `benchmarks/python/aoc/day2_part2.py`

Example:

```bash
python3 benchmarks/python/aoc/day1.py examples/io/aoc_day1.txt
scripts/bench_cross_lang.sh --native
```

## AoC Rust Benchmarks

Rust comparison binaries live under:

- `benchmarks/rust/aoc/day1_rust.rs`
- `benchmarks/rust/aoc/day1_part2_rust.rs`
- `benchmarks/rust/aoc/day2_part1_rust.rs`
- `benchmarks/rust/aoc/day2_part2_rust.rs`
