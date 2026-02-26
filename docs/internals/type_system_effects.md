# Flux Type System and Effects (Canonical Internal Spec)

This document is the canonical implementation reference for Flux type/effect behavior as of v0.0.4 work.

Supporting documents:
- Historical semantics proposal: `docs/proposals/032_type_system_with_effects.md`
- Effect-row expansion track: `docs/proposals/042_effect_rows_and_constraints.md`
- Closure evidence and parity checklist: `docs/proposals/043_pure_flux_checklist.md`

## 1. Purpose and Scope

Audience: compiler contributors.

This file defines:
- what type/effect behavior is implemented today,
- where it lives in compiler code,
- which diagnostics and fixtures lock behavior,
- what must pass for the v0.0.4 type/effects release gate.

Non-goal: introduce new language semantics in this document.

## 2. Semantic Model (Implemented)

### 2.1 Typed vs untyped/inferred behavior

- Typed signatures (`fn f(x: T) -> U ...`) are checked statically.
- Unannotated functions participate in inference and effect propagation.
- Strict mode adds extra API-boundary constraints for `public fn`.

Primary anchors:
- `src/bytecode/compiler/mod.rs`:
  - `compile`
  - `infer_unannotated_function_effects`
  - `validate_strict_mode`
- `src/bytecode/compiler/expression.rs`:
  - call-site effect validation and row constraints.

### 2.2 Purity-by-default and ambient effects

- A context can only perform effects present in its ambient set.
- Missing ambient effects are compile-time failures (`E400` family).
- Direct effectful builtins/primops (`print`, `read_file`, `now_ms`, etc.) are checked statically.

Primary anchors:
- `src/bytecode/compiler/expression.rs`:
  - `ensure_base_call_effect_available`
  - `required_effect_for_base_name`
  - `track_effect_alias_for_binding` (alias-aware checks like `let p = print`).
- `src/bytecode/compiler/mod.rs`:
  - `is_effect_available_name`
  - `is_effect_available_symbol`.

### 2.3 Perform/handle semantics

- `perform Effect.op(...)`:
  - unknown effect -> `E403`
  - unknown operation -> `E404`
  - missing ambient effect -> `E400`
- `expr handle Effect { ... }`:
  - unknown effect -> `E405`
  - unknown handler operation -> `E401`
  - missing declared operations -> `E402`
  - handled effect discharges from enclosing call chain where modeled.

Primary anchors:
- `src/bytecode/compiler/expression.rs`:
  - `compile_perform`
  - `compile_handle`
- `src/bytecode/compiler/mod.rs`:
  - `with_handled_effect`.

### 2.4 Entry-point purity boundary (`main` hybrid policy)

- Pure programs may omit `main`.
- Effectful top-level execution is rejected (`E413`).
- If effectful top-level exists without `main`, also emit `E414`.
- `main` signature checks:
  - duplicate -> `E410`
  - parameters -> `E411`
  - invalid return type -> `E412`
- Root disallow rule:
  - residual effects at `main` can include `IO`, `Time`.
  - custom undischarged effects at root -> `E406`.

Primary anchors:
- `src/bytecode/compiler/mod.rs`:
  - `validate_main_entrypoint`
  - `validate_top_level_effectful_code`
  - `validate_main_root_effect_discharge`.

## 3. Effect Row Semantics (Current)

### 3.1 Solver-level `with e`

Effect polymorphism is solved in compiler effect-row constraints (not syntax-only propagation).

Implemented behavior:
- preserves `e` through higher-order wrappers/composition,
- supports row extension and subtraction in current model,
- validates resolved required effects at call boundaries.

Primary anchors:
- `src/bytecode/compiler/expression.rs`:
  - `collect_effect_row_constraints`
  - `infer_argument_function_effect_row`
  - row compatibility checks in call compilation path.
- `src/bytecode/compiler/mod.rs`:
  - effect-variable helpers (`is_effect_variable`).

### 3.2 Surface forms in use

Current fixtures exercise forms such as:
- `with e`
- `with IO, e`
- `with e + IO - Console`

See:
- `examples/type_system/21_effect_polymorphism_with_e.flx`
- `examples/type_system/30_effect_poly_hof_nested_ok.flx`
- `examples/type_system/33_effect_row_subtract_surface_syntax.flx`.

### 3.3 Out of scope (still future track)

- Full Koka-level row polymorphism expressiveness (advanced subtraction/constraint algebra).
- Additional row solver features beyond current implemented subset.

Track in:
- `docs/proposals/042_effect_rows_and_constraints.md`.

## 4. Compiler Pipeline Mapping (Type/Effects)

Current order in `Compiler::compile` (`src/bytecode/compiler/mod.rs`):

1. Reset per-file state.
2. Process base directives/import context.
3. Collect module contracts.
4. Infer unannotated function effects.
5. Collect ADT definitions.
6. Collect effect declarations.
7. Validate entrypoint + top-level purity + root discharge.
8. Validate strict mode constraints.
9. Predeclare functions.
10. HM inference pass.
11. Statement codegen + diagnostics aggregation.

Diagnostics ordering contract for entry/strict class is intentionally deterministic:
- main signature class (`E410-E412`)
- top-level purity (`E413`, `E414`)
- root discharge (`E406`) when main signature is valid
- strict mode (`E415+`).

## 5. Diagnostics Contract Matrix

| Code | Title (current) | Trigger | Expected fix hint |
|---|---|---|---|
| E400 | MISSING EFFECT | Call/operation requires missing ambient effect | Add required `with ...` or handle effect before call |
| E401 | UNKNOWN HANDLE OPERATION | `handle` arm operation not declared by target effect | Use declared operations for that effect |
| E402 | INCOMPLETE HANDLE | handler misses declared operations | Add missing handler arms |
| E403 | UNKNOWN EFFECT | `perform` references undeclared effect | Declare effect or fix name |
| E404 | UNKNOWN EFFECT OPERATION | `perform` op missing from declared effect | Use declared operation |
| E405 | UNKNOWN HANDLER EFFECT | `handle` references undeclared effect | Declare effect or fix name |
| E406 | UNHANDLED ROOT EFFECT | custom effect escapes valid `main` boundary | Discharge with `handle` before return |
| E410 | DUPLICATE MAIN FUNCTION | more than one top-level `main` | keep one valid `main` |
| E411 | INVALID MAIN PARAMETERS | `main` has parameters | remove parameters |
| E412 | INVALID MAIN RETURN TYPE | invalid/non-Unit `main` return | use Unit-compatible return |
| E413 | TOP-LEVEL EFFECT | effectful top-level expression | move effectful execution into `main` |
| E414 | MISSING MAIN FUNCTION | effectful top-level program without `main` | define `fn main()` root handler |
| E415 | MISSING MAIN FUNCTION (STRICT) | strict policy requiring `main` | add `main` |
| E416 | STRICT FUNCTION ANNOTATION REQUIRED | `public fn` params untyped in strict mode | annotate all params |
| E417 | STRICT RETURN ANNOTATION REQUIRED | `public fn` missing return type in strict mode | add `-> Type` |
| E418 | STRICT EFFECT ANNOTATION REQUIRED | effectful `public fn` missing explicit `with` | add `with ...` |
| E423 | STRICT ANY TYPE | `Any` appears in strict-checked API types | replace `Any` with concrete type |
| E424 | STRICT UNSUPPORTED FUNCTION CONTRACT | strict/public boundary uses function-typed runtime contract | use concrete boundary types or keep API internal |
| E425 | STRICT UNRESOLVED BOUNDARY TYPE | strict mode cannot resolve runtime boundary type for generic/unconverted annotation | make boundary type concrete |

Notes:
- Runtime unhandled-effect diagnostics remain fallback only for non-static paths.
- VM/JIT parity is measured as tuple equality: code + title + primary label.

## 6. Fixture Evidence Matrix

Canonical fixture references:
- passing: `examples/type_system/README.md`
- failing: `examples/type_system/failing/README.md`
- parity harness: `tests/purity_vm_jit_parity_snapshots.rs`, `tests/common/purity_parity.rs`.

| Rule area | Passing fixtures | Failing fixtures |
|---|---|---|
| Direct effect checks and propagation | `19`, `20`, `27`, `28` | `15`, `16`, `20`, `31`, `32`, `33`, `34`, `35`, `36`, `37`, `39`, `40`, `41` |
| Perform/handle static correctness | `18`, `22`, `29` | `17`, `18`, `21`, `42`, `43` |
| Effect polymorphism and rows | `21`, `23`, `30`, `31`, `32`, `33` | `19`, `22`, `44`, `45` |
| Entry-point boundary | `27`, `28`, `29` | `38`, `43`, `46`, `47`, `48`, `49`, `50` |
| Strict/public boundary | `58`, `59`, `60`, `61` | `29`, `30`, `51`, `52`, `53`, `54`, `55`, `56`, `57`, `58`, `59` |

Module-qualified fixtures must run with:
- `--root examples/type_system`

## 7. Known Limitations and Non-Goals

Current limitations:
- Advanced row-constraint expressiveness remains limited vs full research-grade systems.
- Function-typed runtime boundary contracts are not enforced yet; strict/public boundary usage is rejected (`E424`).
- In strict mode, unresolved generic runtime boundary checks are rejected (`E425`) to avoid silent skips.
- Effects remain compile-time-first enforcement; runtime effect checks are fallback-oriented.
- Runtime boundary-enforced type subset includes `Int`, `Float`, `Bool`, `String`, `Unit`, `Option<T>`, `List<T>`, `Either<L, R>`, `Array<T>`, `Map<K, V>`, and tuples.
- Some historical proposal text may still include broader/future claims not yet implemented.

Non-goals for this spec:
- concurrency/effect integration model,
- GC design and runtime memory model changes,
- macro/type-level metaprogramming.

Tracked separately:
- `docs/proposals/042_effect_rows_and_constraints.md`
- `docs/proposals/026_concurrency_model.md`
- `docs/proposals/045_gc.md`.

## 8. v0.0.4 Release Gate Checklist (Type + Effects)

Required command pack:

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Targeted fixture validation (VM):

```bash
cargo run -- --no-cache examples/type_system/22_handle_discharges_effect.flx
cargo run -- --no-cache examples/type_system/30_effect_poly_hof_nested_ok.flx
cargo run -- --no-cache examples/type_system/failing/43_main_unhandled_custom_effect.flx
cargo run -- --no-cache --strict examples/type_system/failing/53_strict_public_effectful_missing_with.flx
```

Targeted fixture validation (JIT):

```bash
cargo run --features jit -- --no-cache examples/type_system/22_handle_discharges_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/30_effect_poly_hof_nested_ok.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/43_main_unhandled_custom_effect.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/53_strict_public_effectful_missing_with.flx --jit
```

Acceptance criteria:
- parity suite green,
- no VM/JIT mismatch on purity-critical diagnostics,
- no known fixture regression in A-G matrices from `043`.

Snapshot governance:
- intentional parity changes must update snapshots with review:
  - `INSTA_UPDATE=always cargo test --all --all-features purity_vm_jit_parity_snapshots`

## 9. Contributor Update Rules

When type/effect semantics change:

1. Update this document (`docs/internals/type_system_effects.md`).
2. Add/update passing and failing fixtures in:
   - `examples/type_system/`
   - `examples/type_system/failing/`
3. Update example READMEs:
   - `examples/type_system/README.md`
   - `examples/type_system/failing/README.md`
4. Re-run VM/JIT targeted checks and parity suite.
5. Update proposal docs only as needed:
   - `043` for checklist/milestone status changes,
   - `042` for row-constraint scope changes,
   - `032` when historical semantics narrative needs alignment notes.
