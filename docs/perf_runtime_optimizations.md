# Runtime Optimization Summary

Date: 2026-02-14
Scope: VM/runtime hot-path tuning driven by AoC Day 1 profiling.

## Goals

- Reduce interpreter overhead in dispatch/call paths.
- Reduce `Value` clone/drop churn.
- Keep cold/error logic out of hot branch layout.

## Implemented Optimizations

### 1) AoC workload parsing optimization

- Pre-parse rotations into integer deltas once (`Lx -> -x`, `Rx -> +x`).
- Removed repeated `substring/trim/parse_int` from inner solve loop.
- Files:
  - `examples/io/aoc_day1.flx`
  - `examples/io/aoc_day1_timed.flx`
  - `examples/io/aoc_day1_profile.flx`

### 2) Stack pre-growth

- Added stack headroom growth helper.
- Used on closure entry/tail-call/invoke call-frame setup.
- Files:
  - `src/runtime/vm/mod.rs`
  - `src/runtime/vm/function_call.rs`

### 3) Value churn reduction

- Added untracked stack helpers:
  - `discard_top()`
  - `pop_untracked()`
  - `pop_pair_untracked()`
- Switched binary/comparison ops to pair pop helper.
- Replaced many consume-only opcode `pop+push` flows with in-place top-of-stack updates.
- Added `Rc::try_unwrap` fast paths for unwrap ops (`Some/Left/Right`).
- Files:
  - `src/runtime/vm/mod.rs`
  - `src/runtime/vm/binary_ops.rs`
  - `src/runtime/vm/comparison_ops.rs`
  - `src/runtime/vm/dispatch.rs`

### 4) Dispatch hot-loop tightening

- Captured current instruction stream once per step and passed into dispatch.
- Reduced repeated `current_frame().instructions()` operand fetch indirection.
- Moved rare/error formatting logic into cold helpers.
- Added `push()` fast path with cold overflow slow path.
- Files:
  - `src/runtime/vm/mod.rs`
  - `src/runtime/vm/dispatch.rs`
  - `src/runtime/vm/dispatch_test.rs`

### 5) Call overhead reduction (`execute_call`)

- Reduced callee/arg movement overhead.
- Added reusable tail-call argument scratch buffer.
- Added builtin fixed-arity extraction fast path (0/1/2/3), preserving generic fallback behavior.
- Files:
  - `src/runtime/vm/mod.rs`
  - `src/runtime/vm/function_call.rs`

## Verification

- VM test suite slice:
  - `cargo test -q runtime::vm -- --nocapture`
- AoC correctness:
  - `cargo run --quiet -- examples/io/aoc_day1.flx` -> `1118`
- Profile benchmark command:
  - `hyperfine -N --warmup 5 --runs 30 './target/release/flux examples/io/aoc_day1_profile.flx'`

## Observed Performance Direction

- Major gain: removing parsing churn from inner loop.
- Subsequent VM/call/dispatch changes gave incremental improvements, with noise depending on system load.
- Best observed profile-workload runs during this pass were in the high-700ms range.

## Recommended Regression Workflow

1. Save baseline after stable run:
   - `scripts/bench_aoc.sh save perf-optimized`
2. Compare after each runtime change:
   - `scripts/bench_aoc.sh compare perf-optimized`
3. Use no-shell/run-only comparisons for cross-language checks:
   - `scripts/bench_cross_lang.sh --native --runs 30 --warmup 5`
