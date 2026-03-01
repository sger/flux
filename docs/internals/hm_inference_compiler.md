# HM Inference in the Flux Compiler

> **Related:** [type_system_effects.md](type_system_effects.md) · [Guide Ch. 9 — Type System Basics](../guide/09_type_system_basics.md)

This document explains how Hindley-Milner (HM) inference is wired into Flux compilation today.

Canonical companions:
- `docs/internals/type_system_effects.md` (semantics + diagnostics contract)
- `docs/proposals/0046_typed_ast_hm_architecture.md` (design history)

## Purpose

HM inference in Flux has two jobs:
- infer types for unannotated code and report type errors (`E300`/`E301`),
- provide expression-level type facts used by compiler typed-validation paths.

The implementation is intentionally gradual: unresolved/heterogeneous paths may degrade to `Any` in non-strict contexts.

## Main Entry Points

- HM engine core: `src/ast/type_infer.rs`
- Type primitives and unification:
  - `src/types/infer_type.rs`
  - `src/types/type_subst.rs`
  - `src/types/scheme.rs`
  - `src/types/type_env.rs`
  - `src/types/unify_error.rs`
- Compiler integration:
  - `src/bytecode/compiler/mod.rs`
  - `src/bytecode/compiler/hm_expr_typer.rs`
  - `src/bytecode/compiler/statement.rs`
  - `src/bytecode/compiler/expression.rs`

## Compile Pipeline Placement

Inside `Compiler::compile` (`src/bytecode/compiler/mod.rs`), HM runs after pass-1 function predeclaration and before pass-2 statement codegen.

High-level order:
1. Reset per-file compiler state.
2. Collect contracts/effects/ADTs and run strict/effect validations.
3. Pass 1 symbol predeclaration for module-level functions.
4. HM pass:
   - call `infer_program(...)`,
   - store `type_env`, `hm_expr_types`, `expr_ptr_to_id` on `Compiler`,
   - collect HM diagnostics.
5. Pass 2 codegen and additional diagnostics.
6. Suppress redundant `E055` when equivalent HM `E300` already exists on the same line.
7. Append HM diagnostics and return all errors.

## HM Data Contract Returned to Compiler

`infer_program(...)` returns:
- `TypeEnv` for identifier schemes,
- `ExprTypeMap` (`ExprNodeId -> InferType`) for expression-level types,
- `expr_ptr_to_id` mapping (`Expression` pointer identity -> `ExprNodeId`),
- diagnostics list.

Important invariant:
- HM pass and compiler typed-validation must observe the same `Program` allocation in one compile invocation, because expression IDs are pointer-based.

## Inference Model (Current)

`src/ast/type_infer.rs` uses an Algorithm-W style structure with recovery:

- Program phase:
  - prebind top-level functions to fresh vars (enables mutual recursion),
  - infer each statement.
- Let-polymorphism:
  - infer value type,
  - `generalize` over env-free vars,
  - store `Scheme` in `TypeEnv`,
  - instantiate at use sites.
- Unification:
  - implemented in `src/types/unify_error.rs`,
  - supports vars, constructors, apps, tuples, function types,
  - `Any` unifies with everything,
  - function unification requires effect-set equality.
- Recovery:
  - `unify_reporting` emits diagnostics only when both conflicting sides are concrete and non-`Any`,
  - on failure returns `Any` so inference can continue.

## Expression Precision and Known Fallbacks

Precision that is used downstream by typed validation includes:
- identifiers and call inference,
- tuples/lists/arrays/maps,
- projection typing for tuple field access and index expressions,
- module member scheme lookup for typed module members.

Intentional fallback zones (current behavior):
- branch disagreement (`if`/`match`) joins through `join_types`, which may return `Any`,
- `MemberAccess` on non-module values falls back to `Any`,
- unresolved/unsupported effect-operation signatures in `Perform`/`Handle` still degrade to `Any`.

## Worked Example: How Types Are Inferred

Example program:

```flux
fn id(x) { x }

fn main() -> Unit {
    let n = id(1)
    let s = id("hi")
    let pair = (n, s)
    print(pair)
}
```

### Inference Walkthrough

1. Predeclaration phase:
- `id` and `main` are first bound to fresh variables in the environment so recursive references are legal.

2. Infer `id`:
- Parameter `x` gets fresh type variable, say `t0`.
- Body is `x`, so body type is `t0`.
- Function type becomes `(t0) -> t0`.
- Let-generalization quantifies free vars not in the outer env:
  - `id : forall t0. t0 -> t0`.

3. Infer `let n = id(1)`:
- Instantiate `id` scheme: fresh copy, e.g. `t1 -> t1`.
- Argument `1` is `Int`.
- Unify `t1` with `Int` -> substitution `t1 := Int`.
- Result type for call is `Int`; bind `n : Int`.

4. Infer `let s = id(\"hi\")`:
- Instantiate `id` again: fresh copy, e.g. `t2 -> t2`.
- Argument `\"hi\"` is `String`.
- Unify `t2` with `String`.
- Result type is `String`; bind `s : String`.

5. Infer `let pair = (n, s)`:
- Tuple literal type is `(Int, String)`.
- Bind `pair : (Int, String)`.

6. Compile-time typed checks:
- HM expression map stores inferred type per expression node.
- Strict typed-validation paths consume those HM results via `hm_expr_type_strict_path`.
- If a boundary expression is unresolved (`Any` or free vars) under strict policy, compiler emits `E425`.

### Why this matters

- The same polymorphic function `id` is safely reused at two concrete types in one scope.
- This is the core HM guarantee enabled by `Scheme::generalize` + `Scheme::instantiate`.

### Mismatch variant (what fails)

```flux
fn main() -> Unit {
    let x: Int = id("hi")
}
```

In this case, HM infers `id("hi") : String`, typed binding expects `Int`, and compiler surfaces a type mismatch diagnostic (`E300`/typed-boundary mismatch path depending on context).

## Strict-Path Consumption in Compiler

`src/bytecode/compiler/hm_expr_typer.rs` is the strict consumer for HM expression types:
- `hm_expr_type_strict_path` resolves expression node id and fetches inferred type,
- resolved means: no free vars and no `Any`,
- typed checks call `validate_expr_expected_type(_with_policy)`.

If strict mode requires a resolved type and HM has unresolved output, compiler raises:
- `E425 STRICT UNRESOLVED BOUNDARY TYPE`.

This prevents silent runtime-boundary fallback on strict typed boundaries.

## Diagnostic Sources

Primary HM-related diagnostics:
- `E300 TYPE_UNIFICATION_ERROR` (mismatch),
- `E301 OCCURS_CHECK_FAILURE` (infinite type),
- `E425 STRICT UNRESOLVED BOUNDARY TYPE` (strict boundary cannot be proven from HM result).

Also relevant:
- `E055` typed boundary mismatch may be emitted in compiler passes; some redundant instances are suppressed if HM already reported equivalent mismatch on same line.

## Effects and HM Boundary

Current split of responsibilities:
- HM type unification includes function effect-set compatibility.
- Compiler effect checks enforce ambient-effect availability and effect-row constraints.
- HM expression inference now uses declared effect operation signatures to type `Perform` and `Handle` expressions where signatures are resolvable.

## Test Anchors

Main HM tests:
- `tests/type_inference_tests.rs`

Compiler/HM integration tests:
- `src/bytecode/compiler/compiler_test.rs`
- `tests/compiler_rules_tests.rs`

Parity and fixture coverage:
- `tests/common/purity_parity.rs`
- `tests/purity_vm_jit_parity_snapshots.rs`
- `examples/type_system/failing/`

## Practical Workflow for HM Changes

1. Reproduce with a focused fixture or unit test.
2. Change the smallest HM source-of-truth module (`type_infer.rs` or `src/types/*`).
3. If boundary behavior changes, update strict-path expectations in compiler tests.
4. Run targeted checks first:
   - `cargo test --test type_inference_tests`
   - affected compiler test target(s)
5. If diagnostics text changes, refresh relevant snapshots.

## Type Variable Lifecycle

Understanding how type variables are created, unified, and consumed is essential for debugging inference issues.

### Creation

Fresh type variables are created by `InferCtx::fresh_var()`. Each call returns a unique `Var(u32)` with a monotonically increasing counter. Fresh variables are created for:
- Unannotated function parameters.
- The result type of unannotated `let` bindings.
- Instantiation of polymorphic schemes (each `forall`-bound variable gets a fresh copy).
- Internal inference of collection literals and constructor calls.

### Unification

`unify_with_span(t1, t2, subst, span)` in `src/types/unify_error.rs` attempts to unify two types by extending the current substitution:

1. Apply the current substitution to both sides first.
2. If both sides are the same variable, or one is a variable and the other is not recursive (`occurs_check`), bind the variable.
3. If both sides are `Con(c1)` and `Con(c2)` with `c1 == c2`, unification succeeds.
4. If both sides are `App(f1, args1)` and `App(f2, args2)`, recurse on `(f1, f2)` and `zip(args1, args2)`.
5. If both sides are `Fun(params1, ret1, eff1)` and `Fun(params2, ret2, eff2)`, recurse on each param pair, the return type, and effect sets.
6. If either side is `Any`, succeed (gradual typing escape).
7. Otherwise, emit `E300` — but only when both sides are concrete (non-`Any`, no free vars).

### Application

The current substitution is applied via `TypeSubst::apply_to_type(ty)`. This recursively replaces every `Var(n)` that has a binding in the substitution map. The substitution is in compositional form — `compose(s1, s2)` applies `s1` to all values in `s2` before merging, ensuring transitivity.

### Generalization

`generalize(env, ty)` computes the set of free type variables in `ty` that are **not** free in `env` (the outer type environment). These unconstrained variables are universally quantified in the resulting `Scheme`. This is the core of Hindley-Milner let-polymorphism.

Generalization is conservative in Flux:
- Only explicitly `<T>`-parameterized functions or top-level `fn` declarations are generalized.
- Unannotated local `let` bindings remain monomorphic (the value restriction).

---

## Gradual Typing Integration

Flux is **gradually typed**: `Any` is a first-class type that unifies with everything. This makes the type system safe for incremental adoption.

### Where `Any` enters

| Source | Description |
|--------|-------------|
| Unannotated parameters | When HM can't determine a concrete type (e.g., never used in a typed context) |
| Heterogeneous `if`/`match` branches | `join_types(t1, t2)` returns `Any` when branches have different concrete types |
| Unknown module member | `MemberAccess` on non-module values falls back to `Any` |
| Unresolved `Perform`/`Handle` signatures | Effect operations without resolvable types degrade to `Any` |

### `Any` and error suppression

When either side of a unification is `Any`, the unification succeeds silently. This prevents false positives on partially-typed code. The trade-off: type errors in `Any` regions are not caught statically.

### Strict mode and `Any`

`--strict` enables additional checks that catch `Any` leaking into public API positions:
- `E423`: `Any` appears in a `public fn` parameter or return type.
- `E425`: A strict-path HM lookup returned `Any` or unresolved type variables.

---

## ExprTypeMap and Pointer-Stable IDs

The HM engine records the inferred type for each expression node in `ExprTypeMap`:

```
ExprTypeMap: ExprNodeId → InferType
```

`ExprNodeId` values are assigned from the allocation address of each `Expression` node in the `Program` instance passed to `infer_program`. This is a pointer-identity approach: the same `Program` allocation must be used by both HM and PASS 2 validation.

**Critical invariant**: all AST transforms (desugaring, constant folding, etc.) must complete **before** `infer_program` is called. Post-HM transforms would create new `Expression` allocations with IDs unknown to the `ExprTypeMap`, breaking strict-path lookups.

In `Compiler::compile`, this ordering is guaranteed by running HM after all pre-compilation passes.

---

## Adding a New Typed Construct

When adding a new expression form or statement that needs type-checked behavior:

1. **Inference side** (`src/ast/type_infer.rs`):
   - Add a case in the expression/statement inference dispatch.
   - Produce and record an `InferType` for the new node.
   - Emit `E300` via `unify_reporting` for concrete mismatches.

2. **Compiler consumption side** (`src/bytecode/compiler/`):
   - If the construct has an annotated type, call `hm_expr_type_strict_path` to get the HM-inferred type, then `validate_expr_expected_type_with_policy` to check it.
   - Avoid re-deriving the type in the compiler — always consume the `ExprTypeMap` result.

3. **Add a fixture**:
   - Passing case in `examples/type_system/`.
   - Failing case in `examples/type_system/failing/`.

4. **Run targeted tests**:
   ```bash
   cargo test --test type_inference_tests
   cargo test --test compiler_rules_tests
   ```

---

## Non-Goals / Current Limits

- Not a full redesign of effect-row polymorphism.
- Gradual typing fallback via `Any` remains intentional in non-strict paths.
- Runtime boundary type conversion (`TypeEnv::to_runtime`) is intentionally conservative and may map unresolved/unsupported forms to `Any`.
