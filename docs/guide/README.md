# Flux Language Guide

A practical manual for learning Flux, organized from basics to advanced topics. Each chapter links to runnable examples in the [`examples/`](../../examples/) directory.

## Chapters

1. [Getting Started](01_getting_started.md) — Variables, types, arithmetic, strings
2. [Functions and Closures](02_functions_and_closures.md) — Named functions, lambdas, closures, do-blocks, where clauses
3. [Collections](03_collections.md) — Arrays, cons lists, hash maps, tuples
4. [Pattern Matching](04_pattern_matching.md) — match, guards, Option, Either, cons patterns
5. [Higher-Order Functions](05_higher_order_functions.md) — map, filter, fold, zip, sort\_by, find, any, all
6. [Pipe Operator and List Comprehensions](06_pipe_and_comprehensions.md) — `|>` pipelines, `[x | x <- xs]` comprehensions
7. [Modules](07_modules.md) — Declaring modules, imports, aliases, private members
8. [Testing](08_testing.md) — Unit test framework, assert base functions, FTest stdlib
9. [Type System Basics](09_type_system_basics.md) — Typed lets/functions, HM inference basics, `E300`
10. [Effects and Purity](10_effects_and_purity.md) — `with IO`/`with Time`, `perform`/`handle`, top-level policy
11. [HOF and Effect Polymorphism](11_hof_effect_polymorphism.md) — `with e` wrappers and effect propagation
12. [Modules, Public API, and Strict Mode](12_modules_public_api_and_strict.md) — `public fn` boundary and strict diagnostics
13. [Match Exhaustiveness and ADTs](13_match_exhaustiveness_and_adts.md) — `type ... = ... | ...` sugar, `E015`/`E083`
14. [Real-World Pipeline Walkthrough](14_real_world_pipeline_walkthrough.md) — End-to-end modules + effects + typed flow

## Type System + Effects Track (09-14)

Quickstart commands:

```bash
cargo run -- --no-cache examples/guide_type_system/01_typed_let_and_fn.flx
cargo run -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx
cargo run -- --no-cache --root examples/guide_type_system examples/guide_type_system/07_module_public_factories.flx
cargo run -- --no-cache --strict examples/guide_type_system/09_strict_public_api_fail.flx
cargo run -- --no-cache examples/guide_type_system/11_match_non_exhaustive_fail.flx
```

JIT spot-check:

```bash
cargo run --features jit -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx --jit
cargo run --features jit -- --no-cache --root examples/guide_type_system examples/guide_type_system/12_real_pipeline_demo.flx --jit
```

Module note:
- For examples importing `Guide.*`, pass `--root examples/guide_type_system`.
