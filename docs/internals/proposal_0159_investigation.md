# Proposal 0159 — Module Phase A predeclaration investigation

## Problem

Adding a module-level Phase A function predeclaration loop to `infer_module`
([src/ast/type_infer/statement.rs:194](../../src/ast/type_infer/statement.rs))
causes `test_mode_flow_list_module_fixture_passes` to fail with a spurious
`E300` at the call site `sort_by(["bb", "a", "ccc"], fn(s) { string_len(s) })`.

## Reproduction

Prepend to `infer_module` before the existing per-statement loop:

```rust
for stmt in &body.statements {
    if let Statement::Function { name, span, .. } = stmt {
        let v = self.env.alloc_infer_type_var();
        self.env.bind_with_span(*name, Scheme::mono(v), Some(*span));
    }
}
```

Then `cargo test --test test_runner_cli test_mode_flow_list_module_fixture_passes`
fails. Instrumenting `self.module_member_schemes` after each statement shows:

```
sort_by forall=[277] ty=(List<_>, (_) -> _) -> List<_>
```

Four underscores but only one quantified var: `a` and `b` of
`sort_by<a, b: Ord>(xs: List<a>, f: (a) -> b) -> List<a>` collapsed into the
same type variable.

## Root cause (H3 from the plan)

In `lib/Flow/List.flx`, `sort` (line 195) and `sort_by` (line 199) both call
the unannotated helper `merge_sort_by` (defined later at line 512):

```flux
public fn sort<a: Ord>(xs: List<a>) -> List<a> {
    merge_sort_by(xs, fn(x) { x })
}
public fn sort_by<a, b: Ord>(xs: List<a>, f: (a) -> b) -> List<a> {
    merge_sort_by(xs, f)
}
fn merge_sort_by(xs, key_fn) { ... }
```

With module Phase A, `merge_sort_by` is pre-bound to `Scheme::mono(v_msb)` —
**one** fresh variable with empty `forall`. Instantiating a `Scheme::mono`
returns the same variable at every call site (no fresh allocation).

Call-site sequence:
1. `sort` body calls `merge_sort_by`. Lookup → `Var(v_msb)`. Unify with
   `Fun([List<a_sort>, (a_sort) -> a_sort], ret, eff)`. Binds
   `v_msb := Fun([List<a_sort>, (a_sort) -> a_sort], …)`.
2. `sort_by` body calls `merge_sort_by`. Lookup → `Var(v_msb)` which resolves
   through the substitution to `Fun([List<a_sort>, (a_sort) -> a_sort], …)`.
   Unify with `Fun([List<a_sb>, (a_sb) -> b_sb], …)`. This unifies
   `a_sort = a_sb`, `a_sort = b_sb`, therefore **`a_sb = b_sb`**. Sort_by's
   two declared type parameters collapse into one.

## The unification that collapses `a` and `b`

`src/types/unify.rs` `bind_var_with_ctx` — when unifying two `Fun` types of
equal arity, `unify_many` runs pairwise over the parameter list. The call-site
shape `(_, (_) -> b_sb)` vs the already-bound shape `(_, (a_sort) -> a_sort)`
forces `a_sort ≡ b_sb` at the second position's argument slot.

## Fix direction (for Commit 2)

`Scheme::mono(fresh_var)` pre-declaration with empty `forall` is **unsafe for
module-level shared helpers**. The correct shapes:

1. **Annotated functions** (complete signature: all params + return type):
   pre-bind with the declared polymorphic scheme via
   `declared_fn_scheme(...)`. Each call site gets a fresh instantiation, so
   distinct callers remain independent. This is the Commit 3 helper, but
   Commit 2 needs it as a prerequisite — promote it.

2. **Unannotated functions**: do **not** add a Phase A entry. Forward
   references to unannotated module-level helpers are not supported (matches
   status quo — `lib/Flow/List.flx` currently orders `merge_sort_by` later
   and `sort_by` calls it through fallback-var recovery, but the test
   passes because the fallback does not leak into the published scheme).

In other words: Commit 2's module Phase A must be annotation-gated. The
helper `declared_fn_scheme` moves earlier than originally planned.

## Note on top-level `infer_program`

Top-level `infer_program` uses the same `Scheme::mono(fresh_var)` predecl
(statement.rs:17-22). The bug does not surface at top level because no
maintained top-level file has two distinct polymorphic callers of a shared
unannotated helper. The same annotation-gated predecl should eventually
apply there too (follow-up work, not Commit 2).
