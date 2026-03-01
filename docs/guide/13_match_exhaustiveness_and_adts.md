# Chapter 13 — Match Exhaustiveness and ADTs

> Examples: [`examples/guide_type_system/10_adt_type_sugar_and_match_ok.flx`](../../examples/guide_type_system/10_adt_type_sugar_and_match_ok.flx), [`11_match_non_exhaustive_fail.flx`](../../examples/guide_type_system/11_match_non_exhaustive_fail.flx)

## Learning Goals

- Use ADT declaration sugar: `type Name = Ctor(...) | ...`.
- Write exhaustive matches.
- Understand `E015` vs `E083`.

## Core Concepts

- `type ... = ... | ...` desugars to existing `data` semantics.
- General non-ADT non-exhaustive matches use `E015`.
- ADT constructor-space non-exhaustive matches use `E083`.
- Guarded wildcard arms (`_ if ...`) are not unconditional catch-alls.

## ADT sugar example

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/10_adt_type_sugar_and_match_ok.flx
cargo run --features jit -- --no-cache examples/guide_type_system/10_adt_type_sugar_and_match_ok.flx --jit
```

## Non-exhaustive example

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/11_match_non_exhaustive_fail.flx
```

Expected diagnostic class:

- `E015` (`NON-EXHAUSTIVE MATCH`)

## Next

Continue to [Chapter 14 — Real-World Pipeline Walkthrough](14_real_world_pipeline_walkthrough.md).
