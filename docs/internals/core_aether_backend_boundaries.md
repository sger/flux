# Core, Aether, and Backend Boundaries

This note describes the compiler boundary after Proposal 0153 Stage 3.

The short version is:

- `Core` is the only semantic IR.
- `Aether` is a backend-only RC lowering product.
- `CFG` and `LIR` consume `AetherProgram`, not semantic Core with hidden RC nodes.

For the broader architecture, see:
- `docs/internals/compiler_architecture.md`
- `docs/internals/ir_pipeline.md`
- `docs/internals/aether.md`

## Canonical Pipeline

```text
Source
  -> syntax/
  -> Program AST
  -> HM inference
  -> CoreProgram          (semantic only)
  -> AetherProgram        (backend-only ownership/reuse lowering)

VM path:
  -> CFG IR
  -> bytecode
  -> VM runtime

Native path:
  -> LIR
  -> LLVM/native pipeline
```

The important boundary is that `CoreProgram` and `AetherProgram` are now
different products with different responsibilities.

## Core

`src/core/` owns the semantic language IR.

`CoreExpr` now contains only semantic constructs such as:
- variables
- literals
- lambdas
- applications
- lets / letrec / cases
- constructors
- semantic primops
- effects / handlers

`CoreExpr` does not contain RC/backend-only constructs anymore.

In particular, these nodes no longer live in `CoreExpr`:
- `AetherCall`
- `Dup`
- `Drop`
- `Reuse`
- `DropSpecialized`

`run_core_passes*` is therefore semantic-only:
- semantic simplification
- evidence/dictionary lowering
- ANF normalization
- no Aether insertion
- no Aether verification contract

`--dump-core` is the first semantic debugging surface and should stay free of
ownership/reuse nodes.

## Aether

`src/aether/` now owns a real backend-only lowering layer:
- `AetherExpr`
- `AetherAlt`
- `AetherHandler`
- `AetherDef`
- `AetherProgram`

This is not a second semantic IR. It is clean Core plus RC/ownership planning
materialized for maintained RC backends and debugging.

Aether-specific constructs live only here:
- `AetherCall`
- `Dup`
- `Drop`
- `Reuse`
- `DropSpecialized`

The main Aether entrypoint is:
- `lower_core_to_aether_program(...)`

It takes clean semantic Core and produces `AetherProgram`.

`--dump-aether` is the ownership/debugging surface:
- borrow modes
- dup/drop insertion
- reuse
- drop specialization
- FBIP/FIP behavior

## Backend Contract

Maintained RC backends lower from `AetherProgram`:

- VM path:
  - `src/core/to_ir/`
  - `src/cfg/`
  - `src/bytecode/`

- native path:
  - `src/lir/lower.rs`
  - `src/llvm/`

This means backend lowering must not:
- look for Aether nodes in `CoreExpr`
- reinterpret semantic Core calls as ownership nodes
- re-decide semantic meaning from function names

The backend job is:
- consume semantic Core decisions already present in `Core`
- consume ownership/reuse decisions already present in `Aether`
- lower those decisions faithfully into backend IR/runtime behavior

## FBIP and Verification

Aether verification and FBIP reasoning now belong to the Aether side of the
boundary.

That means:
- semantic Core passes do not validate Aether-only structure
- Aether verification checks `AetherExpr`
- Aether-side FBIP analysis can reason about `Reuse`, `Drop`, and
  `DropSpecialized` directly

This preserves the old diagnostic/debugging power without polluting semantic
Core.

## Debugging Workflow

Recommended order:

1. inspect the source fixture
2. inspect `--dump-core`
3. inspect `--dump-aether` if ownership/reuse matters
4. only then inspect backend IR
   - VM: CFG / bytecode / `--trace`
   - native: `--dump-lir`, `--dump-lir-llvm`, `--emit-llvm`

Interpretation:
- wrong in `Core`: semantic/frontend bug
- right in `Core`, wrong in `Aether`: Aether bug
- right in `Core` and `Aether`, wrong later: backend/runtime bug

## Future Backends

This split is intentional so future non-RC backends can consume clean
`CoreProgram` directly and skip `AetherProgram` entirely.

So the model is:
- `Core` is universal semantic IR
- `Aether` is backend-specific lowering for RC backends
- backend IRs remain backend-local implementation layers
