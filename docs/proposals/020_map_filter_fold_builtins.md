# Proposal 020: `map` / `filter` / `fold` Builtins

**Status:** Proposed
**Priority:** High (Language Ergonomics)
**Created:** 2026-02-11
**Related:** Proposal 016 (Tail-Call Optimization), Proposal 017 (Persistent Collections and GC), Proposal 019 (Zero-Copy Value Passing)

## Overview

Add three higher-order builtins with eager array semantics:

- `map(arr, fn)`
- `filter(arr, pred)`
- `fold(arr, init, fn)`

This unlocks idiomatic functional pipelines immediately with minimal compiler impact because these are runtime builtins, not syntax changes.

## Goals

1. Provide first-class functional collection transforms for arrays.
2. Keep semantics explicit and predictable.
3. Ship with clear error behavior and performance expectations.
4. Keep API future-proof for Proposal 017 migration.

## Non-Goals

1. Lazy iterator/stream semantics.
2. Implicit `reduce` variant without `init`.
3. Pattern-matching-based collection transforms.

## Semantics

### `map(arr, fn)`

- Applies `fn(element)` to each element in order.
- Returns a new array with transformed values.
- Empty input returns `[]`.

### `filter(arr, pred)`

- Applies `pred(element)` to each element in order.
- Keeps elements where predicate result is truthy.
- Returns a new array.
- Empty input returns `[]`.

### `fold(arr, init, fn)`

- Left fold only (`foldl` semantics).
- Applies `fn(acc, element)` in index order.
- Returns the final accumulator.
- `fold([], init, fn) == init`.
- `init` is required (no ambiguous no-init variant).

## Error Contract

- Type errors if:
  - first arg is not an array
  - function arg is not callable (`Closure` or `Builtin`)
- Arity errors if callback receives wrong number of args.
- Runtime errors from callback propagate unchanged.

## Performance Expectations

Baseline acceptance targets for initial release:

1. No asymptotic regressions versus hand-written loops.
2. `map/filter/fold` over 1k and 10k arrays execute within 1.5x-2.0x of equivalent explicit recursive/loop-style baseline programs.
3. No additional deep-clone regressions on shared `Value` variants.

## Compatibility and Future-Proofing

- Current semantics are eager over arrays.
- Builtin contract must not expose internals tied to `Vec` mutability.
- Future Proposal 017 migration may extend support to persistent list/map structures without changing user-facing call shape.

## Test Plan

1. Functional correctness
   - `map([1,2,3], f)` returns expected transformed values
   - `filter([1,2,3], p)` keeps expected subset
   - `fold([1,2,3], 0, f)` returns expected accumulator
2. Edge cases
   - Empty arrays for all three
   - Mixed element types
   - Callback returning nested arrays/hashes/options/either values
3. Error behavior
   - Non-array input
   - Non-callable callback
   - Wrong callback arity
4. Determinism
   - Callback evaluation order is left-to-right and stable
5. Performance checks
   - Benchmarks on 1k/10k sizes for map/filter/fold vs baseline

## Rollout

1. Ship builtins and tests first.
2. Add examples in `examples/advanced/functional_pipeline.flx`.
3. Benchmark and record in `PERF_REPORT.md`.
4. Revisit after Proposal 017 to evaluate extension to persistent collections.
