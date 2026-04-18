- Feature Name: Core Semantic Types and Backend Representation Split Execution
- Start Date: 2026-04-15
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: 0156 (Static Typing Completion Roadmap), 0157 (Explicit Core Types and Runtime Representation Split)

# Proposal 0158: Core Semantic Types and Backend Representation Split Execution

## Summary
[summary]: #summary

Execute the downstream cleanup left open by `0156` using the architecture defined by `0157`:

1. remove semantic `Dynamic` from maintained Core and CFG paths
2. preserve semantic residue explicitly in `core`
3. keep backend lowering runtime-representation-oriented
4. align VM and native lowering around the same Core contract

This proposal does not add a second semantic IR and does not revive `Any`.

## Goals
[goals]: #goals

- `CoreType` must preserve semantic residue explicitly with forms such as:
  - `Var`
  - `Forall`
  - `Abstract`
- `CoreBinder` metadata should be able to carry semantic type information when known.
- `IrType::Dynamic` must be replaced by rep-oriented backend typing.
- generic runtime values remain acceptable as runtime representation via `TaggedRep` / tagged backend values.
- dumps and snapshots must show explicit semantic abstraction in Core instead of fake `Dynamic`.

## Phases
[phases]: #phases

### Phase 1 — Core semantic type model

Touched modules:

- `src/core/mod.rs`
- `src/core/display.rs`
- `src/core/lower_ast/*`

Work:

- remove `CoreType::Dynamic`
- add `CoreType::Var`, `CoreType::Forall`, and `CoreType::Abstract`
- make `CoreType::from_infer` preserve semantic residue instead of erasing it
- keep runtime-representation derivation in `FluxRep`

### Phase 2 — Typed Core semantic plumbing

Touched modules:

- `src/core/mod.rs`
- `src/core/lower_ast/*`
- `src/core/passes/*`
- `src/aether/*`
- `src/core/to_ir/*`

Work:

- add semantic type metadata to lambdas, handlers/resumes, and case joins
- populate those typed semantic nodes where HM data is available
- preserve that metadata through Aether and Core passes

### Phase 3 — Rep-oriented CFG lowering

Touched modules:

- `src/cfg/mod.rs`
- `src/core/to_ir/*`
- `src/cfg/validate.rs`
- `src/cfg/passes.rs`

Work:

- remove `IrType::Dynamic`
- replace it with rep-oriented generic typing (`Tagged`)
- keep concrete backend shapes where they remain useful (`Tuple`, `Hash`, `Function`, `Adt`)
- ensure Core-to-CFG lowering chooses runtime representation, not semantic fallback

### Phase 4 — Native alignment

Touched modules:

- `src/lir/lower.rs`
- `src/lir/mod.rs`
- `src/llvm/emit_llvm.rs`

Work:

- keep LIR/LLVM on `FluxRep`
- ensure native lowering consumes the new Core semantic forms without relying on semantic `Dynamic`
- preserve specialization for concrete param/result reps

### Phase 5 — Validation and dumps

Touched modules:

- `src/core/display.rs`
- `src/cfg/validate.rs`
- `tests/aether_cli_snapshots.rs`
- `tests/ir_pipeline_tests.rs`
- `tests/llvm_codegen_snapshots.rs`
- `tests/backend_representation_runtime_tests.rs`

Work:

- validate that semantic `Dynamic` no longer exists in maintained paths
- update `--dump-core=debug` snapshots to show explicit semantic abstraction
- keep VM/native regression coverage on closures, handlers, dictionaries, and generic value passing

## Current implementation slice
[current-implementation-slice]: #current-implementation-slice

This branch now implements Phases 1-5 of the maintained-path migration:

- `CoreType::Dynamic` is removed in favor of explicit semantic forms
- lambdas, handlers/resumes, and case joins now carry explicit semantic metadata
- `IrType::Dynamic` is removed in favor of `IrType::Tagged`
- maintained Core-to-CFG lowering no longer emits semantic `Dynamic`
- CFG block params can now carry inferred semantic metadata alongside runtime rep typing
- CFG validation now checks rep/semantic consistency for function params, returns, and typed join params
- LIR lowering now derives function result reps from explicit lambda/handler semantic metadata instead of defaulting through function-shaped `def.result_ty` or synthetic `TaggedRep`
- handler-arm closures now record semantically derived native `param_reps` / `result_rep`
- native specialization still keys off concrete `FluxRep` families without depending on semantic fallback
- `--dump-core=debug` snapshots now show explicit polymorphic residue
- CLI dump regressions now assert that polymorphic Core debug output shows explicit type-variable / `forall` residue and does not regress to `Dynamic`

Remaining downstream work is optional tightening and specialization work that does not reintroduce semantic fallback.

## Test plan
[test-plan]: #test-plan

Required suites:

- `cargo test --test ir_pipeline_tests -- --nocapture`
- `cargo test --test aether_cli_snapshots -- --nocapture`
- `cargo test --test llvm_codegen_snapshots -- --nocapture`
- `cargo test --test backend_representation_runtime_tests -- --nocapture`

Required coverage:

- polymorphic/class-method Core dumps
- closure capture lowering
- handler-arm closure lowering
- case/join lowering
- VM/native generic value passing

## Rationale
[rationale]: #rationale

`0156` completed front-end static typing. `0157` explains why backend-facing `Dynamic` conflation is architecturally wrong. `0158` is the execution proposal that keeps Flux on the intended architecture:

- `core` owns semantics
- Aether owns ownership/reuse
- CFG and LIR own runtime representation
