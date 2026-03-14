# Flux Type System and Effects (Canonical Internal Spec)

> **Related guide chapters:** [Chapter 9 — Type System Basics](../guide/09_type_system_basics.md) · [Chapter 10 — Effects and Purity](../guide/10_effects_and_purity.md) · [Chapter 11 — HOF and Effect Polymorphism](../guide/11_hof_effect_polymorphism.md)

This document is the canonical implementation reference for Flux type/effect behavior as of v0.0.4 work.

Supporting documents:

- Historical semantics proposal: `docs/proposals/0032_type_system_with_effects.md`
- Effect-row constraint solver: `docs/proposals/implemented/0042_effect_rows_and_constraints.md` ✓
- Row-solving completeness: `docs/proposals/implemented/0049_effect_rows_completeness.md` ✓
- Row variable (`|e`) syntax and HM integration: `docs/proposals/implemented/0064_effect_row_variables.md` ✓
- Base function HM signature tightening: `docs/proposals/implemented/0074_base_signature_tightening.md` ✓
- Closure evidence and parity checklist: `docs/proposals/0043_pure_flux_checklist.md`
- Effect row system internal spec: `docs/internals/effect_row_system.md`
- Base HM signature registry: `docs/internals/base_hm_signatures.md`

## 1. Purpose and Scope

Audience: compiler contributors.

This file defines:

- what type/effect behavior is implemented today,
- where it lives in compiler code,
- which diagnostics and fixtures lock behavior,
- what must pass for the v0.0.4 type/effects release gate.

Non-goal: introduce new language semantics in this document.

## 1.1 0.0.4 HM/ADT/Exhaustiveness Baseline

Release-critical track for v0.0.4 is:

- Core HM inference for unannotated typed code paths.
- User-defined nominal generic ADTs (`data T<...> { ... }`).
- Stronger exhaustiveness (top-level constructor coverage + nested constructor-space checks for supported shapes).
- General non-ADT `match` exhaustiveness hardening (`E015`) for Bool/list/sum-like
  spaces with deterministic guarded-arm behavior.

Truth sources for this baseline:

- Compiler paths in `src/bytecode/compiler/mod.rs` and `src/bytecode/compiler/expression.rs`.
- Type core in `src/types/*`.
- Fixtures in `examples/type_system/` and `examples/type_system/failing/`.
- Roadmap orchestration/sequencing in:
  `docs/proposals/implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md`.

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
  - operation argument type/arity mismatch -> `E300`
  - missing ambient effect -> `E400`
- `expr handle Effect { ... }`:
  - unknown effect -> `E405`
  - unknown handler operation -> `E401`
  - missing declared operations -> `E402`
  - handler arm arity/type compatibility mismatches -> `E300`
  - handled effect discharges from enclosing call chain where modeled.

Primary anchors:

- `src/bytecode/compiler/expression.rs`:
  - `compile_perform`
  - `compile_handle`
- `src/bytecode/compiler/mod.rs`:
  - `with_handled_effect`.

### 2.4 Module ADT boundary policy (0.0.4)

- ADTs are first-class inside modules.
- Cross-module callers are expected to use `public fn` factories/accessors to
  construct and consume ADT values.
- Direct module-qualified constructor calls (for example
  `TypeSystem.Foo.Bar(...)`) are not part of the 0.0.4 stable API contract.

### 2.5 Entry-point purity boundary (`main` hybrid policy)

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

### 3.1 Row variable syntax (`|e`) — completed (0064)

Effect row variables are expressed with `|e` tail syntax inside `with` clauses:

```flux
-- row variable only
fn map<a, b>(xs: List<a>, f: fn(a) -> b with |e) -> List<b> with |e { ... }

-- concrete effects + row variable
fn log_map<a, b>(xs: List<a>, f: fn(a) -> b with IO | e) -> List<b> with IO | e { ... }
```

`EffectExpr::RowVar { name, span }` — a first-class AST variant introduced by 0064 (`src/syntax/effect_expr.rs`). The `|` in `with IO | e` separates concrete effects from the open row tail. Only lowercase identifiers are valid row variable names.

**Legacy forms rejected.** Implicit lowercase identifiers as row vars (`with e + IO`) are no longer valid. All row variables must use `|e` tail syntax.

Primary anchors:

- `src/syntax/effect_expr.rs` — `EffectExpr::RowVar`, `is_open()`, `row_var()`
- `src/syntax/parser/helpers.rs` — `parse_effect_expr()` recognises `|` as row-var separator
- `src/types/infer_effect_row.rs` — `InferEffectRow { concrete: HashSet<Identifier>, tail: Option<TypeVarId> }` — open row has a tail var, closed row does not; transitive substitution via `apply_substitution`

### 3.2 Row constraint solver — completed (0042 + 0049)

Effect polymorphism is solved in `src/bytecode/compiler/effect_rows.rs`:

- **`Eq`** — row equality; links vars, binds atoms bidirectionally.
- **`Subset`** — set inclusion; emits `E422` for closed rows that don't satisfy the subset.
- **`Absent`** — deferred absence checking; proves an atom is NOT in a row. Evaluated after `resolve_links`.
- **`Extend` / `Subtract`** — reserved; reduces to `Eq` for currently supported forms.

Solver features: worklist algorithm, variable linking, deferred `Absent` evaluation, deterministic diagnostics via sorted symbol IDs.

Primary anchors:

- `src/bytecode/compiler/effect_rows.rs` — `solve_row_constraints()`, `EffectRow`, constraint types
- `src/bytecode/compiler/expression.rs` — `collect_effect_row_constraints`, `infer_argument_function_effect_row`

### 3.3 Surface forms in use

Current fixtures exercise:

| Syntax | Meaning |
|--------|---------|
| `with IO` | Fixed `IO` effect |
| `with IO, Time` | Fixed `IO` and `Time` |
| `with \|e` | Open row — inherits callback's effects |
| `with IO \| e` | `IO` plus whatever `e` resolves to |
| `with IO + State - Console` | Row extension/subtraction (advanced) |

See:

- `examples/type_system/21_effect_polymorphism_with_e.flx`
- `examples/type_system/30_effect_poly_hof_nested_ok.flx`
- `examples/type_system/33_effect_row_subtract_surface_syntax.flx`
- `examples/type_system/100_effect_row_order_equivalence_ok.flx`

Row-variable fixtures: `examples/type_system/failing/196_effect_row_subtract_unresolved_single_e419.flx` through `200_*`.

See `docs/internals/effect_row_system.md` for the full constraint solver reference.

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

HM pass output contract (0.0.4):

- `TypeEnv` for identifier/scheme lookup.
- HM expression typing map keyed by compiler-assigned expression node IDs
  (`ExprTypeMap`) for typed validation callsites.
- Typed validators consume this HM expression map instead of re-deriving
  expression types in ad hoc callsite walkers.
- Pointer-identity invariant: expression IDs are assigned from expression
  allocation addresses for the specific `Program` instance passed to `compile`.
  All AST transforms must run before HM inference so HM and PASS 2 validation
  observe the same `Program` allocation.
- HM expression precision currently covers member access plus index/tuple-field
  projections:
  - module member access typing includes inline module members and imported
    module public function contracts when typed signatures are available.
  - `tuple.i` resolves to the element type when in bounds.
  - `arr[i]`/`list[i]` resolves to `Option<T>`.
  - `map[k]` resolves to `Option<V>`.
  - unknown/unsupported projection shapes fall back to `Any`.

Diagnostics ordering contract for entry/strict class is intentionally deterministic:

- main signature class (`E410-E412`)
- top-level purity (`E413`, `E414`)
- root discharge (`E406`) when main signature is valid
- strict mode (`E415+`).

## 4a. Type System Architecture — Key Files and Responsibilities

This section maps each conceptual layer of the type system to its source files.

### Type Representation Layer (`src/types/`)

| File | Responsibility |
|------|---------------|
| `infer_type.rs` | `InferType` enum — the HM-internal type AST: `Var(u32)`, `Con(TypeConstructor)`, `App(Box<InferType>, Vec<InferType>)`, `Fun(Vec<InferType>, Box<InferType>, EffectSet)`, `Tuple(Vec<InferType>)` |
| `type_subst.rs` | `TypeSubst` — substitution map with `compose`, `apply_to_type`, `apply_to_scheme` |
| `scheme.rs` | `Scheme` — polymorphic type with `forall` binders; `generalize(env, ty)` and `instantiate(scheme)` |
| `type_env.rs` | `TypeEnv` — scoped identifier-to-scheme map; `TypeExpr ↔ InferType` bridge; `RuntimeType ↔ InferType` bridge |
| `type_constructor.rs` | `TypeConstructor` — built-in constructors (`Int`, `Bool`, `String`, …) and ADT constructors |
| `unify_error.rs` | `unify_with_span` — unification with occurs-check; emits `E300`/`E301` on concrete mismatches |
| `mod.rs` | Re-exports the above |

### HM Inference Engine (`src/ast/type_infer/`)

The engine is a module directory split across eight focused files:

| File | Responsibility |
|------|---------------|
| `mod.rs` | `InferCtx` struct, `infer_program` entry point, shared types |
| `expression.rs` | `infer_expr()`, `bind_pattern()`, `infer_call()` |
| `function.rs` | `infer_fn()`, self-recursive refinement helpers |
| `statement.rs` | `infer_stmt()`, `infer_let()`, `infer_module()`, phase orchestration |
| `adt.rs` | Constructor registration, `instantiate_constructor_parts()` |
| `effects.rs` | Ambient effect stack, `constrain_call_effects()`, `infer_effect_row()` |
| `unification.rs` | `unify_propagate()`, `unify_with_context()`, `join_types()` |
| `display.rs` | `display_infer_type()`, `suggest_type_name()` |

Each subfile uses `use super::*;` to access `InferCtx` and shared types across `impl` blocks.

- **`InferCtx`** — holds the type environment, fresh-variable counter, substitution accumulator, and diagnostics list.
- **`infer_program`** — top-level entry point called by the compiler between PASS 1 and PASS 2. Returns `(TypeEnv, ExprTypeMap, expr_ptr_to_id, diagnostics)`.
- **Statement inference** — prebinds top-level functions to fresh variables (enabling mutual recursion), then infers each statement.
- **Expression inference** — returns `InferType`; records the type in `ExprTypeMap` keyed by pointer-stable `ExprNodeId`.
- **Let-polymorphism** — `generalize` is called only when the binding has explicit `<T>` parameters or is a top-level `let`. Unannotated local `let` bindings remain monomorphic.
- **Recovery** — unification failures return `Any` so inference continues without cascading errors.

### Compiler Integration (`src/bytecode/compiler/`)

| File | Responsibility |
|------|---------------|
| `mod.rs` | `Compiler::compile` — orchestrates PASS 1 → HM → PASS 2; stores `TypeEnv`, `ExprTypeMap`, `expr_ptr_to_id` |
| `hm_expr_typer.rs` | `hm_expr_type_strict_path` — resolves `ExprNodeId` and fetches inferred type; `validate_expr_expected_type_with_policy` — strict policy check |
| `expression.rs` | Call-site effect validation, PrimOp resolution, fastcall allowlist, `ensure_base_call_effect_available` |
| `statement.rs` | Function/let codegen; uses `hm_expr_typer` to validate annotated return types and let bindings |
| `contracts.rs` | `TypeExpr → RuntimeType` conversion at compile time; populates `FunctionContract` |

### Runtime Boundary Layer (`src/runtime/`)

| File | Responsibility |
|------|---------------|
| `runtime_type.rs` | `RuntimeType` enum — the subset of types that can be checked at runtime boundaries: `Int`, `Float`, `Bool`, `String`, `Unit`, `Option<T>`, `List<T>`, `Either<L,R>`, `Array<T>`, `Map<K,V>`, tuples |
| `function_contract.rs` | `FunctionContract { params: Vec<Option<RuntimeType>>, ret: Option<RuntimeType> }` — attached to `CompiledFunction` |
| `vm/function_call.rs` | Checks `FunctionContract` on every call; emits `E055` (compile-time mismatch) or `E1004` (runtime `Any → T` violation) |

### Surface Syntax Layer (`src/syntax/`)

| File | Responsibility |
|------|---------------|
| `type_expr.rs` | `TypeExpr` enum — the AST-level type representation: `Named(String)`, `Tuple(Vec<TypeExpr>)`, `Function(Vec<TypeExpr>, Box<TypeExpr>)`, `Generic(String, Vec<TypeExpr>)` |
| `effect_expr.rs` | `EffectExpr` — `with IO`, `with State<T>`, `with e`, `with IO, e`, row extension/subtraction |

### Separation of Concerns

```
┌─────────────────────────────────────────────────────────────┐
│  Source AST (TypeExpr, EffectExpr)                          │  syntax/
└──────────────────┬──────────────────────────────────────────┘
                   │ parse
                   ▼
┌─────────────────────────────────────────────────────────────┐
│  HM Internal Types (InferType, TypeSubst, Scheme, TypeEnv)  │  types/
│  — used only during compilation, not at runtime             │
└──────────────────┬──────────────────────────────────────────┘
                   │ infer_program
                   ▼
┌─────────────────────────────────────────────────────────────┐
│  Compiler (Compiler::compile, hm_expr_typer, contracts)     │  bytecode/compiler/
│  — validates annotated boundaries against HM output         │
└──────────────────┬──────────────────────────────────────────┘
                   │ contracts.rs: TypeExpr → RuntimeType
                   ▼
┌─────────────────────────────────────────────────────────────┐
│  Runtime Boundary (RuntimeType, FunctionContract)           │  runtime/
│  — enforced on function calls; subset of HM type space      │
└─────────────────────────────────────────────────────────────┘
```

### Effect System Architecture

Effects are validated at two points:

**Compile-time** (authoritative):

- `ensure_base_call_effect_available` — checks `IO`/`Time` base calls have the required ambient effect.
- `collect_effect_row_constraints` + `infer_argument_function_effect_row` — row solver for `with e` propagation.
- `compile_perform` / `compile_handle` — validates `perform`/`handle` operation names and arity against effect declarations.
- `validate_main_entrypoint` + `validate_top_level_effectful_code` — enforces main-boundary and top-level purity policies.

**Runtime** (fallback only):

- Unhandled custom-effect paths that escape static analysis fall through to runtime error paths. These are rare for fully-typed programs.

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
| E419 | UNRESOLVED ROW VAR (SINGLE) | Single effect row variable remains unresolved after constraint solving | provide concrete effect annotation |
| E420 | UNRESOLVED ROW VAR (MULTI) | Multiple effect row variables remain ambiguous | provide concrete effect annotations |
| E421 | INVALID EFFECT SUBTRACTION | Concrete effect subtracted that is not present in the row | remove invalid subtraction |
| E422 | UNSATISFIED EFFECT SUBSET | Required effect subset not satisfied by provided row | add missing effects |
| E423 | STRICT ANY TYPE | `Any` appears in strict-checked API types | replace `Any` with concrete type |
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
| HM + ADT + exhaustiveness hardening | `74`, `75`, `76`, `77`, `78`, `79`, `80`, `81`, `82`, `83`, `84`, `85`, `88`, `89`, `90`, `91` | `64`, `65`, `67`, `68`, `69`, `70`, `71`, `72`, `73`, `74`, `75`, `76`, `77`, `78`, `79`, `81`, `82`, `83`, `84` |
| Entry-point boundary | `27`, `28`, `29` | `38`, `43`, `46`, `47`, `48`, `49`, `50` |
| Strict/public boundary | `58`, `59`, `60`, `61` | `29`, `30`, `51`, `52`, `53`, `54`, `55`, `56`, `57`, `58`, `59` |

Module-qualified fixtures must run with:

- `--root examples/type_system`

## 7. Known Limitations and Non-Goals

Current limitations:

- Advanced row-constraint expressiveness remains limited vs full research-grade systems.
- Strict/public function-typed boundaries are runtime-enforced for closure/jit-closure callback values when contract metadata is present.
- In strict mode, unresolved generic runtime boundary checks are rejected (`E425`) to avoid silent skips.
- HM expression typing uses a strict-path authority for typed validation.
  Typed validators must not use runtime-boundary compatibility inference.
- 0.0.4 HM gate is zero-fallback for typed/inferred validation paths:
  typed/inferred binding and validation must not fall back to runtime-boundary
  compatibility typing when strict HM typing is unresolved.
- Known remaining HM gap: some non-strict module-qualified generic call paths
  can still resolve to unresolved pockets and compile permissively; strict mode
  remains the enforcement path for unresolved boundary cases.
- Effects remain compile-time-first enforcement; runtime effect checks are fallback-oriented.
- Runtime boundary-enforced type subset includes `Int`, `Float`, `Bool`, `String`, `Unit`, `Option<T>`, `List<T>`, `Either<L, R>`, `Array<T>`, `Map<K, V>`, and tuples.
- Some historical proposal text may still include broader/future claims not yet implemented.

Non-goals for this spec:

- concurrency/effect integration model,
- GC design and runtime memory model changes,
- macro/type-level metaprogramming.

Tracked separately:

- `docs/proposals/implemented/0042_effect_rows_and_constraints.md`
- `docs/proposals/0026_concurrency_model.md`
- `docs/proposals/implemented/0045_gc.md`.

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

## 9. Base Function HM Signatures (0074)

All 77 base functions have declarative `BaseHmSignature` entries in `src/runtime/base/helpers.rs` (`signature_for_id`). These are lowered to `Scheme` values via `scheme_for_signature_id` and installed in the `TypeEnv` before HM inference, enabling precise type checking at builtin call sites.

| Helper | Type |
|--------|------|
| `t_any()` | `Any` |
| `t_int()` | `Int` |
| `t_bool()` | `Bool` |
| `t_string()` | `String` |
| `t_array(T)` | `Array<T>` |
| `t_list(T)` | `List<T>` |
| `t_map(K, V)` | `Map<K, V>` |
| `t_tuple(elements)` | `(T1, T2, ...)` |
| `t_option(T)` | `Option<T>` |
| `t_var("a")` | `TypeVar("a")` — polymorphic |
| `t_fun(params, ret, effects)` | `fn(...) -> T with e` |

`sig_with_row_params(type_params, row_params, params, ret, effects)` is the constructor for polymorphic signatures. Row params (`row_params: Vec<&'static str>`) are allocated as fresh `TypeVarId`s and stored in `InferEffectRow::tail`.

Deferred builtins (still `Any`): `abs`, `min`, `max`, `sum`, `product`, `concat`, `list` — require type classes (proposal 0053) or union types for correct overload resolution.

See `docs/internals/base_hm_signatures.md` for the full signature reference.

## 10. Contributor Update Rules

When type/effect semantics change:

1. Update this document (`docs/internals/type_system_effects.md`).
2. Add/update passing and failing fixtures in:
   - `examples/type_system/`
   - `examples/type_system/failing/`
3. Update example READMEs:
   - `examples/type_system/README.md`
   - `examples/type_system/failing/README.md`
4. Re-run VM/JIT targeted checks and parity suite.
5. If a base function signature changes, update `src/runtime/base/helpers.rs` `signature_for_id` and `docs/internals/base_hm_signatures.md`.
6. If effect row solver behavior changes, update `docs/internals/effect_row_system.md` and the fixture matrix there.
7. Update proposal docs only as needed:
   - `043` for checklist/milestone status changes,
   - `032` when historical semantics narrative needs alignment notes.
