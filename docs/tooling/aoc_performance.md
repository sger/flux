# AoC Performance Guide

This guide defines a repeatable way to track Flux performance while solving Advent of Code.

## Goals

- Keep solutions correct first.
- Measure both developer experience and runtime speed.
- Catch regressions early.

## What to Measure

Track these metrics per day:

1. `compile+run` time
2. `run-only` time
3. input size in bytes
4. result correctness
5. optional GC telemetry (`collections`, `allocations`, `live objects`)

## Commands

### 0) Recommended order for tuning

1. Compare Flux vs Rust/Python first:

```bash
scripts/bench_cross_lang.sh --native --runs 30 --warmup 5 \
  --name-prefix aoc_day1_part2 \
  --flux-cmd './target/release/flux examples/io/aoc_day1_part2.flx' \
  --rust-cmd './target/release/aoc_day1_part2_rust examples/io/aoc_day1.txt' \
  --python-cmd 'python3 benchmarks/aoc/day1_part2.py examples/io/aoc_day1.txt'
```

2. Then profile VM hotspots:

```bash
CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --reverse --bin flux -- examples/io/aoc_day1_part2_profile.flx
```

3. Review `flamegraph.svg` and optimize the top nodes.

### 1) Run correctness check

```bash
cargo run -- examples/io/aoc_day1.flx
```

### 2) Run timed example

```bash
cargo run -- examples/io/aoc_day1_timed.flx
```

### 3) Run benchmark (Criterion)

```bash
cargo bench --bench aoc_day1_bench
```

### 4) GC telemetry (optional)

```bash
cargo run --features gc-telemetry -- examples/io/aoc_day1.flx --gc-telemetry
```

## Baseline Workflow

1. Run `cargo bench --bench aoc_day1_bench`.
2. Save a baseline copy:

```bash
cp -r target/criterion baseline_criterion
```

3. Re-run benchmark after changes.
4. Compare with your existing bench report tooling (`scripts/bench_report.rs`).

## Suggested Targets (starting point)

Tune these based on your machine:

- Day 1 `run-only` p50 under `20 ms`
- Day 1 `compile+run` p50 under `120 ms`
- No regression > `15%` without justification

## Decision Rules

If correctness regresses:
- stop and fix correctness first.

If performance regresses:
- investigate only if regression is > 10-15% and stable across reruns.

If performance improves but code becomes risky:
- prefer maintainable optimizations unless this is a proven hot path.

## Per-Day Tracking Template

```text
Day: 1
Input bytes: <n>
Correct: yes/no
compile+run (ms): <p50>/<p95>
run-only (ms): <p50>/<p95>
GC collections: <n>
Notes: <what changed>
```
