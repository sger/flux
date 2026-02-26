# Chapter 9 — Type System Basics

> Examples: [`examples/guide_type_system/01_typed_let_and_fn.flx`](../../examples/guide_type_system/01_typed_let_and_fn.flx), [`02_annotations_vs_inference.flx`](../../examples/guide_type_system/02_annotations_vs_inference.flx), [`03_pure_function_boundaries.flx`](../../examples/guide_type_system/03_pure_function_boundaries.flx)

## Learning Goals

- Read and write typed `let` bindings.
- Use typed function signatures with simple inference.
- Understand where compile-time type mismatches surface (`E300`).

## Core Concepts

- Flux supports explicit annotations for bindings and function boundaries.
- HM inference fills many local expression types.
- Mismatches in typed paths are compile-time failures (`E300`).

## Example 1: Typed `let` + typed function

```flux
fn add(x: Int, y: Int) -> Int {
    x + y
}

let total: Int = add(40, 2)
```

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/01_typed_let_and_fn.flx
cargo run --features jit -- --no-cache examples/guide_type_system/01_typed_let_and_fn.flx --jit
```

## Example 2: Annotations vs inference

- `02_annotations_vs_inference.flx` shows inferred and explicit values used together.
- `03_pure_function_boundaries.flx` shows typed tuple/list manipulation without effects.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/02_annotations_vs_inference.flx
cargo run -- --no-cache examples/guide_type_system/03_pure_function_boundaries.flx
```

## Failure Pattern to Remember

When a typed binding cannot unify with the inferred expression type, Flux reports `E300`.

## Next

Continue to [Chapter 10 — Effects and Purity](10_effects_and_purity.md).
