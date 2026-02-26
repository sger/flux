# Chapter 14 — Real-World Pipeline Walkthrough

> Example: [`examples/guide_type_system/12_real_pipeline_demo.flx`](../../examples/guide_type_system/12_real_pipeline_demo.flx)

## Learning Goals

- Combine modules, typed APIs, HOFs, and effects in one app.
- Apply strict/public boundary discipline in a realistic flow.
- Debug common failures quickly by diagnostic code.

## Pipeline structure

`12_real_pipeline_demo.flx` composes three modules:

- `Guide.Factories`: ADT-backed factory/accessor API.
- `Guide.Effects`: effectful wrappers and `with e` utility.
- `Guide.StrictApi`: strict-friendly public output functions.

Run (VM):

```bash
cargo run -- --no-cache --root examples/guide_type_system examples/guide_type_system/12_real_pipeline_demo.flx
```

Run (JIT):

```bash
cargo run --features jit -- --no-cache --root examples/guide_type_system examples/guide_type_system/12_real_pipeline_demo.flx --jit
```

## Debugging failures by code

- `E300`: typed/HM mismatch.
- `E400`: effect requirement mismatch.
- `E413`/`E414`: invalid top-level effect usage.
- `E416`/`E417`/`E418`/`E423`: strict public API violations.
- `E015`/`E083`: non-exhaustive match.

## Next

Return to the [Guide Index](README.md).
