# Signature-Directed Checking and Skolemisation

This document describes how Flux's bidirectional type checker and rigid
(skolem) type variables work. It is the internals companion to
[Proposal 0159](../proposals/implemented/0159_signature_directed_checking_and_skolemisation.md),
which this work implements.

## Mental model

> Inference **discovers** types. Checking **verifies** them against an
> explicit signature.

Two mutually-recursive entry points in `src/ast/type_infer/expression/`:

| Entry point | Role |
|---|---|
| `infer_expression(expr) -> InferType` | Algorithm W — produces a type by walking the expression. Used when no expected type is available (unannotated code, operator arms). |
| `check_expression(expr, expected) -> InferType` | Bidirectional checking — pushes `expected` into sub-expressions. Mismatches report at the offending sub-expression rather than the enclosing annotation. |

Check mode only fires in a handful of well-defined sites:

- Typed-`let` bindings (`src/ast/type_infer/statement.rs::infer_let_binding`).
- Function bodies with a declared return type (internal to
  `infer_function_declaration`).
- Fixed-arity and higher-order call arguments
  (`src/ast/type_infer/expression/calls.rs`).

Everywhere else, inference stays in W mode.

## Specialised check rules

`check_expression` dispatches on the expression shape. Specialised rules
live in `src/ast/type_infer/expression/checked.rs`:

| Expression | Rule sketch |
|---|---|
| `If` | Each branch is checked against `expected`. |
| `Match` | Each arm body is checked against `expected`. Non-propagatable arm bodies preserve the `MatchArm` ReportContext so match-arm diagnostics keep their specialised framing. |
| `DoBlock` | The block's value expression is checked. Non-value statements infer as normal. |
| `Lambda` | When `expected = Fun(params, ret, eff)`, lambda parameters bind to `params`; body is checked against `ret`. Arity mismatch falls back to `infer + unify`. |
| `Tuple / ListLiteral / ArrayLiteral / Hash / Cons` | Structural destructuring — each element / pair is checked against the corresponding component type. |
| `Some / Left / Right` | Inner value is checked against the matching component of the expected `App(Option/Either, _)`. |
| default | `let actual = infer_expression(expr); unify_reporting(expected, actual)`. |

Rules never overwrite `self.expr_types` for the checked expression — a
prior `infer_expression` pass has already recorded the authoritative
inferred type, and overwriting it with `expected` would mask the actual
shape from downstream consumers (codegen, validation).

## Skolems (rigid type variables)

When entering a function body with declared type parameters, each type
parameter is marked as a **skolem** for the duration of body inference.

Representation:

- `InferCtx::skolem_vars: HashSet<TypeVarId>` — active skolems.
- `InferCtx::skolem_names: HashMap<TypeVarId, Identifier>` — source-level
  parameter names, used to render E305.
- `mark_skolem(v, name)` / `unmark_skolems(vs)` helpers in
  `src/ast/type_infer/mod.rs`.

Marking happens in `infer_function_declaration` via
`mark_signature_skolems`; unmarking happens before
`finalize_and_bind_function_scheme` generalises the scheme — so
downstream use-sites see ordinary flexible variables after scheme
instantiation.

### Inline rejection in `unify_core`

Rigidity is enforced at the unification boundary, not post-hoc:

- `unify_core` signature takes `skolems: &HashSet<TypeVarId>` (threaded
  through `unify_many`, `unify_fun_types`, `bind_var_with_ctx`).
- `bind_var_with_ctx` checks `skolems.contains(&v)` before binding.
  A skolem may resolve to itself transitively through `ctx_subst`
  (accepted silently); anything else returns
  `UnifyErrorKind::RigidBind`.
- A **flip rule** at the top of `unify_core` handles
  `(Var(skol), Var(flex))` by swapping to bind `flex → Var(skol)`
  instead of the illegal reverse.
- External callers of `unify` / `unify_with_span` pass an empty skolem
  set; their behaviour is unchanged.

### The diagnostic: E305 RIGID TYPE VARIABLE ESCAPE

`build_rigid_bind_diagnostic` in
`src/ast/type_infer/unification.rs` renders E305 using the declared
parameter name from `skolem_names` (fallback: synthetic `t{id}`). The
`seen_error_keys` dedup guard bypasses its concreteness check for
`RigidBind` errors, because the expected side is necessarily a type
variable.

## Recursive pre-binding

Algorithm W pre-declares each top-level function with
`Scheme::mono(fresh_var)` so later definitions can forward-reference
earlier ones. For polymorphic recursion this is not enough — every
recursive call would share that single fresh variable, collapsing
distinct instantiations.

`declared_fn_scheme` in `src/ast/type_infer/function.rs` constructs a
full polymorphic `Scheme` from a **complete** explicit signature (every
parameter annotated + return type annotated). Used in:

- `infer_program` — top-level Phase A.
- `infer_module` — **annotation-gated** Phase A. Unannotated module
  helpers are *not* pre-declared, because `Scheme::mono(fresh_var)` with
  empty `forall` would cross-pollinate type parameters across distinct
  callers. See
  [proposal_0159_investigation.md](proposal_0159_investigation.md) for
  the worked example (`sort_by` + `merge_sort_by` in `Flow.List`).

Each recursive call-site instantiates the declared scheme at fresh
types, preserving polymorphism.

## Cross-pass TypeVarId safety

A preloaded module scheme (e.g. `Flow.Array.zip` loaded into the env
when compiling `Flow.List`) carries `TypeVarId`s from the pass that
produced it. Those IDs can coincidentally collide with local IDs in the
current pass. The flip rule above makes such collisions observable by
binding flexible local vars to those foreign skolem IDs.

`advance_counter_past_preloaded_schemes` in
`src/ast/type_infer/mod.rs` bumps the env counter past any TypeVarId
used in preloaded schemes during `InferCtx::new`, so freshly-allocated
vars in the current pass cannot collide with IDs baked into preloaded
scheme bodies.

## Call-site check mode

In `src/ast/type_infer/expression/calls.rs`:

- `infer_call_fixed_arity_path` — for propagatable arguments with a
  fully concrete expected parameter type, `check_expression(arg,
  expected)` runs *before* the canonical per-arg `unify_core` +
  `call_arg_type_mismatch` emission. Per-sub-expression diagnostics
  survive thanks to `hm_span_strictly_narrower` in
  `src/compiler/mod.rs::suppress_overlapping_hm_diagnostics`,
  which keeps HM diagnostics whose span is strictly contained in the
  compiler's whole-value boundary span.
- `infer_call_higher_order_path` — for lambda arguments, runs
  `check_expression` when `lambda_param_types_concrete(expected)`.
  Non-lambda arguments `unify_silent` against the expected type so
  later propagatable args see resolved callee type vars without
  shadowing downstream effect-row diagnostics like E400.

The `lambda_param_types_concrete` / `expected.is_concrete()` gates
prevent premature binding of callee type vars via argument bodies in
cases that were previously soft-failing (see `test_fold_type_error`:
`fold(42, 0, fn(a, x) { a + x })` must compile even though the first
argument is nonsense).

## Snapshot acceptance rule

When landing changes that touch this subsystem:

- Diagnostic **code** must not regress — no E300 becoming vaguer.
- Diagnostic **span** must be narrower-or-equal compared to the prior
  accepted snapshot — never wider.
- Additive per-sub-expression diagnostics (new E300s at precise spans
  that weren't previously surfaced) are acceptable if they don't
  duplicate the canonical.
- Core IR dumps may churn from TypeVar ID renumbering (counter bump)
  or from lambda params gaining concrete types in Core IR — both are
  benign.

Review each `cargo insta pending-snapshots` diff by hand before
`cargo insta accept`.

## File map

Production code:

- [src/ast/type_infer/expression/checked.rs](../../src/ast/type_infer/expression/checked.rs) — check dispatcher + specialised rules.
- [src/ast/type_infer/expression/calls.rs](../../src/ast/type_infer/expression/calls.rs) — call-site propagation.
- [src/ast/type_infer/function.rs](../../src/ast/type_infer/function.rs) — skolem marking, `declared_fn_scheme`.
- [src/ast/type_infer/statement.rs](../../src/ast/type_infer/statement.rs) — typed-`let` wiring, annotation-gated module Phase A.
- [src/ast/type_infer/mod.rs](../../src/ast/type_infer/mod.rs) — `InferCtx` skolem state, `advance_counter_past_preloaded_schemes`.
- [src/ast/type_infer/unification.rs](../../src/ast/type_infer/unification.rs) — E305 diagnostic builder.
- [src/types/unify.rs](../../src/types/unify.rs) — threaded skolem parameter, flip rule, rigid-bind rejection.
- [src/types/unify_error.rs](../../src/types/unify_error.rs) — `UnifyErrorKind::RigidBind`.
- [src/diagnostics/compiler_errors.rs](../../src/diagnostics/compiler_errors.rs) + [src/diagnostics/registry.rs](../../src/diagnostics/registry.rs) — E305 registration.
- [src/compiler/mod.rs](../../src/compiler/mod.rs) — `hm_span_strictly_narrower`.

Fixtures:

- [examples/compiler_errors/rigid_var_escape_e305.flx](../../examples/compiler_errors/rigid_var_escape_e305.flx)
- [examples/compiler_errors/invalid_type_annotation_e303.flx](../../examples/compiler_errors/invalid_type_annotation_e303.flx)
- [examples/compiler_errors/invalid_effect_row_e304.flx](../../examples/compiler_errors/invalid_effect_row_e304.flx)
