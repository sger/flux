# Type Primitives (`src/types/`)

> **Related:** [hm_inference_compiler.md](hm_inference_compiler.md) · [effect_row_system.md](effect_row_system.md) · [type_system_effects.md](type_system_effects.md)

This document is the contributor reference for `src/types/` — the HM type primitives layer. Each module has a single responsibility. Nothing in this layer depends on the compiler, the VM, or the AST beyond the types it defines.

---

## Module Map

| File | Responsibility |
|------|---------------|
| `mod.rs` | Re-exports and the `TypeVarId` type alias |
| `type_constructor.rs` | Concrete type constructors |
| `infer_type.rs` | The HM type AST |
| `infer_effect_row.rs` | Effect rows for function types |
| `type_subst.rs` | Substitution map |
| `unify_error.rs` | Error types for unification failures |
| `unify.rs` | Unification algorithm |
| `scheme.rs` | Polymorphic type schemes |
| `type_env.rs` | Scoped typing environment |

---

## `mod.rs`

Defines `TypeVarId = u32` — the shared identifier type for both type variables and row-tail variables. Re-exports the public surface of every submodule.

---

## `type_constructor.rs`

`TypeConstructor` is the set of concrete, nullary-or-parametric type formers:

```
Int | Float | Bool | String | Unit | Never | Any
List | Array | Map | Option | Either
Adt(Symbol)
```

`Adt(Symbol)` stores an interned symbol ID, not a resolved string. Its `Display` impl renders as `$<id>` (e.g. `$7`) because `Symbol::Display` renders the raw interned ID — this is intentional for diagnostic/debug output, not user-facing type names.

`Any` still exists in the internal type model for legacy and compatibility reasons, but it is not intended as a normal user-facing source-language type.

No logic lives here; the type is a pure data enum.

---

## `infer_type.rs`

`InferType` is the HM-internal type AST used exclusively during compilation:

```rust
pub enum InferType {
    Var(TypeVarId),                              // unification variable
    Con(TypeConstructor),                        // nullary concrete type
    App(TypeConstructor, Vec<InferType>),        // type application
    Fun(Vec<InferType>, Box<InferType>, InferEffectRow),  // function type
    Tuple(Vec<InferType>),                       // tuple type
}
```

Key methods:

| Method | Complexity | Notes |
|--------|-----------|-------|
| `free_vars()` | O(n) | Collects both type vars and row-tail vars into a `HashSet` |
| `free_type_vars()` | O(n) | Type vars only — used to distinguish from row vars in `Scheme::instantiate` |
| `apply_type_subst(subst)` | O(n) | Recursive walk; cycle-safe via `seen_vars` stack |
| `is_concrete()` | O(n), early-exit | Short-circuit via `contains_var()` — no `HashSet` allocation |
| `contains_any()` | O(n), early-exit | Returns `true` if `Any` appears at any depth |

`is_concrete` and `contains_any` both use short-circuit recursive walks to exit on first match, avoiding full traversal of large concrete types.

`InferType` is distinct from:
- `TypeExpr` — the surface-syntax annotation AST in `src/syntax/`
- `RuntimeType` — the VM boundary type in `src/runtime/`

Conversion between these three representations happens in `type_env.rs`.

---

## `infer_effect_row.rs`

`InferEffectRow` represents the effect annotation on a function type:

```rust
pub struct InferEffectRow {
    concrete: HashSet<Identifier>,  // concrete effect names (IO, Time, Console, …)
    tail: Option<TypeVarId>,        // open row tail variable, if any
}
```

A row with `tail = None` is **closed** — its effect set is exactly `concrete`. A row with `tail = Some(v)` is **open** — `v` can be unified with additional effects.

Construction:

| Constructor | Meaning |
|-------------|---------|
| `closed_empty()` | No effects, closed |
| `closed_from_symbols(iter)` | Fixed concrete effects, closed |
| `open_from_symbols(iter, tail)` | Concrete effects plus open tail |
| `from_effect_exprs(exprs, row_var_env, row_var_counter)` | Lowers `EffectExpr` AST nodes; allocates fresh `TypeVarId`s for row variables via `row_var_counter` |

`apply_row_subst(subst)` follows the tail chain transitively — it merges concrete effects and chases row-variable bindings until the tail is either unbound or resolves to a closed row.

`free_row_vars()` returns the `TypeVarId` of the tail variable, if present.

---

## `type_subst.rs`

`TypeSubst` is a lazy-normalized substitution map with two independent namespaces:

```rust
pub struct TypeSubst {
    type_bindings: HashMap<TypeVarId, InferType>,
    row_bindings:  HashMap<TypeVarId, InferEffectRow>,
}
```

**Lazy normalization**: stored values may contain variables that are themselves mapped. Full resolution happens at read time via `apply_type_subst`, which recursively follows chains. The occurs check (enforced by `unify`) guarantees acyclicity, so chain-following always terminates.

Primary API:

| Method | Notes |
|--------|-------|
| `get(v)` | Look up a type binding |
| `insert(v, ty)` | Add a type binding; panics in debug builds on self-reference |
| `get_row(v)` | Look up a row binding |
| `insert_row(v, row)` | Add a row binding |
| `compose(self, other)` | `self ∘ other` — applies `self` to incoming `other` values before merging; left wins on key conflicts |
| `is_empty()` | True only if both namespaces are empty |
| `iter()` / `iter_rows()` | Iterate bindings (unspecified order) |

`compose` drops trivial self-bindings (`?t → ?t`) after merging to prevent no-op entries from participating in substitution chains.

---

## `unify_error.rs`

Error types only — no algorithm.

```rust
pub enum UnifyErrorKind { Mismatch, OccursCheck }

pub struct UnifyErrorDetail {
    pub expected: InferType,
    pub actual: InferType,
}

pub struct UnifyError {
    pub kind: UnifyErrorKind,
    pub detail: UnifyErrorDetail,
    pub span: Span,
}
```

Constructors `mismatch` and `occurs` are `pub(crate)` — they are called only from `unify.rs`.

`effect_row_mismatch` is the public constructor for effect-row failures; it is called from the HM inference engine when effect sets do not unify.

---

## `unify.rs`

The unification algorithm. Module-level `#![allow(clippy::result_large_err, clippy::too_many_arguments)]`.

Public API:

| Function | Description |
|----------|-------------|
| `unify(t1, t2, subst)` | Unify without source location; no diagnostics |
| `unify_with_span(expected, actual, subst, span)` | Unify with location; returns `UnifyError` on failure |
| `unify_core(expected, actual, subst, span, fresh_row_var)` | Full unification including effect-row variables; `fresh_row_var` allocates new row-tail IDs for residual rows |

Private helpers:

| Helper | Role |
|--------|------|
| `resolve_head` | Shallow substitution chase (up to 128 hops) |
| `occurs_in_with_ctx` | Occurs check — prevents `?a → Foo<?a>` |
| `unify_many` | Pairwise unification of two slices |
| `unify_fun_types` | Unifies params, return, and effect rows |
| `resolve_row` | Applies substitution to an `InferEffectRow` |
| `unify_effect_rows` | Dispatches between four row cases |
| `unify_open_with_closed` | Open row unified with closed; binds the tail to the residual concrete set |
| `unify_both_open_rows` | Both open; introduces a fresh residual row variable |
| `unify_row_var` / `bind_var_with_ctx` | Bind a single row/type variable |

**Gradual typing rule**: if either side is `Any`, unification succeeds immediately with no substitution change.

**Error reporting policy** (`unify_with_span`): errors are emitted only when both sides are concrete (no `Var` nodes, no `Any`). This prevents cascading false positives in partially-typed code.

**Effect-row unification cases** (in `unify_effect_rows`):

| Left tail | Right tail | Condition | Result |
|-----------|-----------|-----------|--------|
| None | None | equal concrete sets | success |
| Some(l) | Some(r) | `l == r`, equal sets | success |
| None | None | unequal concrete sets | `E300` mismatch |
| None | Some(r) | — | `unify_open_with_closed` |
| Some(l) | None | — | `unify_open_with_closed` (swapped) |
| Some(l) | Some(r) | `l != r` or unequal sets | `unify_both_open_rows` — fresh residual |

---

## `scheme.rs`

`Scheme` is a polymorphic type: a body `InferType` paired with universally quantified `TypeVarId`s.

```rust
pub struct Scheme {
    pub forall: Vec<TypeVarId>,   // ∀-binders (sorted for determinism)
    pub infer_type: InferType,    // body — may contain Var(v) for v in forall
}
```

`forall` holds both plain type variables and row-tail variables — they share the `TypeVarId` space. `instantiate` uses `free_type_vars()` to distinguish them: variables that appear only as row-tail variables (not as `Var` nodes in the type tree) receive only a row binding, not a spurious type binding.

Key functions:

| Function | Description |
|----------|-------------|
| `Scheme::mono(ty)` | Wraps a monotype (no quantifiers) |
| `Scheme::instantiate(&self, counter)` | Replaces each `forall` variable with a fresh `Var`; returns the instantiated type and the old→new mapping |
| `generalize(ty, env_free_vars)` | Quantifies all free vars in `ty` not free in `env_free_vars`; sorts quantifiers for reproducible output |

`generalize` is the "let-generalization" step of Algorithm W. In Flux, generalization is conservative — it is applied only to explicitly type-parameterized functions and top-level declarations, not to unannotated local `let` bindings (the value restriction).

`TypeEnv::generalize_at_level` is the preferred alternative: it uses per-variable allocation levels to generalize in O(type-size) without scanning the full environment.

---

## `type_env.rs`

`TypeEnv` is the scoped identifier-to-scheme environment. It also hosts type-conversion bridge functions that have no natural home elsewhere.

### Scope and binding

Shadow-stack design: each name maps to a `Vec<TypeBindingEntry>`, with the top entry being the currently visible binding. `scope_markers` records which names were introduced at each level so `leave_scope` can pop exactly those entries in O(names-bound-at-scope) time.

| Method | Description |
|--------|-------------|
| `enter_scope()` / `leave_scope()` | Push/pop a scope level |
| `bind(name, scheme)` | Add a binding in the current scope |
| `bind_with_span(name, scheme, span)` | As above, recording definition location |
| `lookup(name)` | O(1) — top of shadow stack |
| `lookup_span(name)` | O(1) — definition span of visible binding |
| `free_vars()` | Union of free vars across all visible bindings |

### Level-based generalization

`TypeEnv` tracks the allocation level (`u32`) of each type variable so that generalization does not need to scan the full environment:

```
var_levels: HashMap<TypeVarId, u32>
```

`alloc_type_var_id()` records the current `level` for each new variable. `generalize_at_level(ty)` quantifies variables whose recorded level is strictly greater than the current environment level — those are variables that were allocated inside a `let` binding scope and are not constrained by the outer context.

`counter` (type variable allocator) is `pub(crate)` — callers outside the module access it via `alloc_type_var_id` / `alloc_infer_type_var`, or pass `&mut env.counter` directly to functions that allocate row variables.

### Type-conversion bridges

These are static methods (no `&self`) that convert between the three type representations:

| Method | Conversion |
|--------|-----------|
| `infer_type_from_type_expr(expr, type_params, interner)` | `TypeExpr` → `InferType` (clean API; allocates its own `row_var_counter`) |
| `convert_type_expr_rec(expr, type_params, interner, row_var_env, row_var_counter)` | Recursive worker; threads `row_var_env` and `row_var_counter` through for shared row-variable identity within one annotation site |
| `infer_type_from_runtime(runtime_type)` | `RuntimeType` → `InferType` |
| `to_runtime(infer_type, subst)` | `InferType` → `RuntimeType`; unresolved vars and `Fun` collapse to `Any` |

`convert_type_expr_rec` is called by the HM inference engine, the bytecode compiler, and the ADT type-parameter lowering paths. All callers that need row-variable identity sharing across an annotation site call it directly with a shared `row_var_env`.

---

## Invariants

1. **Acyclicity** — `TypeSubst` must never contain a binding `?v → ... ?v ...`. The occurs check in `unify` enforces this before any `insert` call.
2. **Stable `forall` order** — `generalize` and `generalize_at_level` both sort quantifiers so scheme output is reproducible across runs.
3. **Pointer-stable expression IDs** — `TypeEnv` and `TypeSubst` are used by `infer_program`, which assigns expression IDs from pointer addresses. The same `Program` allocation must be used for both HM inference and PASS 2 validation.
4. **No runtime dependencies** — nothing in `src/types/` imports from `src/runtime/`. The `RuntimeType` bridge lives in `type_env.rs` because it is a type-conversion utility, not a runtime concern; it imports `RuntimeType` as a data type only.

---

## Adding a New Type Primitive

To add a new built-in type constructor (e.g. `Result<T, E>`):

1. Add a variant to `TypeConstructor` in `type_constructor.rs`.
2. Add a display arm in `TypeConstructor::fmt`.
3. Add an `App` case in `TypeEnv::convert_type_expr_rec` to recognize the name string.
4. Add cases in `TypeEnv::infer_type_from_runtime` and `TypeEnv::to_runtime` for the `RuntimeType` bridge.
5. Add a test in `type_constructor.rs` for the display, and in `type_env.rs` for the round-trip.
