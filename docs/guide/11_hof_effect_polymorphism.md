# Chapter 11 — HOF and Effect Polymorphism

> Example: [`examples/guide_type_system/06_hof_with_e_compose.flx`](../../examples/guide_type_system/06_hof_with_e_compose.flx)

## Learning Goals

- Write higher-order functions over typed callbacks.
- Use `with e` to preserve callback effects through wrappers.
- Recognize propagation failures (`E400`).

## Core Concepts

- `with e` means the wrapper carries the callback's effect row.
- Pure callback keeps wrapper pure.
- IO callback forces IO effect requirement through call chains.

## Example

`06_hof_with_e_compose.flx` runs `apply_twice` with:

- a pure callback (`plus_one`), and
- an IO callback (`log_inc`) inside `main with IO`.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/06_hof_with_e_compose.flx
cargo run --features jit -- --no-cache examples/guide_type_system/06_hof_with_e_compose.flx --jit
```

## Failure Pattern to Remember

If a caller declares incompatible effects for a polymorphic callback chain, Flux emits `E400`.

## Next

Continue to [Chapter 12 — Modules, Public API, and Strict Mode](12_modules_public_api_and_strict.md).
