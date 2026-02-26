# Chapter 10 — Effects and Purity

> Examples: [`examples/guide_type_system/04_with_io_and_with_time.flx`](../../examples/guide_type_system/04_with_io_and_with_time.flx), [`05_perform_handle_basics.flx`](../../examples/guide_type_system/05_perform_handle_basics.flx)

## Learning Goals

- Understand pure-by-default execution.
- Use `with IO` and `with Time` effect declarations.
- Understand top-level effect policy (`E413`, `E414`).

## Core Concepts

- Typed code is pure unless effects are declared.
- Effectful top-level execution is rejected.
- Effectful execution belongs in `fn main() with ... { ... }`.

## Example: IO + Time in `main`

`04_with_io_and_with_time.flx` shows a `Time` function consumed by an `IO, Time` `main`.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx
cargo run --features jit -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx --jit
```

## Example: `perform` / `handle` basics

`05_perform_handle_basics.flx` performs `Console.print` and discharges it with `handle`.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/05_perform_handle_basics.flx
cargo run --features jit -- --no-cache examples/guide_type_system/05_perform_handle_basics.flx --jit
```

## Failure Pattern to Remember

- Effectful top-level expression without `main`: `E413` and `E414`.
- Missing required effect on caller/callee boundary: `E400` family.

## Next

Continue to [Chapter 11 — HOF and Effect Polymorphism](11_hof_effect_polymorphism.md).
