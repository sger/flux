# Chapter 12 — Modules, Public API, and Strict Mode

> Examples: [`examples/guide_type_system/07_module_public_factories.flx`](../../examples/guide_type_system/07_module_public_factories.flx), [`08_strict_public_api_ok.flx`](../../examples/guide_type_system/08_strict_public_api_ok.flx), [`09_strict_public_api_fail.flx`](../../examples/guide_type_system/09_strict_public_api_fail.flx)

## Learning Goals

- Use module-qualified public APIs.
- Understand strict checks targeting `public fn` boundaries.
- Treat underscore naming as style-only.

## Core Concepts

- Public boundary semantics are explicit (`public fn`).
- Strict mode enforces annotated public interfaces.
- Private/internal helpers are not the strict API target.

## Module examples

`07_module_public_factories.flx` and `08_strict_public_api_ok.flx` import module APIs from `examples/guide_type_system/Guide`.

Run (VM):

```bash
cargo run -- --no-cache --root examples/guide_type_system examples/guide_type_system/07_module_public_factories.flx
cargo run -- --no-cache --strict --root examples/guide_type_system examples/guide_type_system/08_strict_public_api_ok.flx
```

Run (JIT):

```bash
cargo run --features jit -- --no-cache --root examples/guide_type_system examples/guide_type_system/07_module_public_factories.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/guide_type_system examples/guide_type_system/08_strict_public_api_ok.flx --jit
```

## Strict failure example

`09_strict_public_api_fail.flx` is intentionally missing a public parameter annotation.

Run:

```bash
cargo run -- --no-cache --strict examples/guide_type_system/09_strict_public_api_fail.flx
```

Expected diagnostic class:

- `E416` (`public fn` parameter annotation required)

Related strict diagnostics: `E417`, `E418`, `E423`.

## Next

Continue to [Chapter 13 — Match Exhaustiveness and ADTs](13_match_exhaustiveness_and_adts.md).
