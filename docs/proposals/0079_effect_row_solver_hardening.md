- Feature Name: Effect Row Solver Hardening
- Start Date: 2026-03-07
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: 0042 (effect rows), 0049 (row completeness), 0064 (row variables)

# Proposal 0079: Effect Row Solver Hardening

## Summary

Fix four correctness and quality issues in the effect-row solver and HM
integration identified during a pre-0.0.4 code review. The fixes are
independent and can be merged in any order. None require surface syntax
changes.

## Motivation

Proposals 0042, 0049, and 0064 brought the effect-row system to a working
state. A targeted code review before the 0.0.4 release found the following
issues:

1. Effect mismatch diagnostics are routed through a dummy `Fun` unification
   path, producing poor error messages that reference `() -> ()` types rather
   than the actual missing effects.
2. The constraint solver accumulates duplicate violations when multiple
   constraints fail for the same shared row variable.
3. `from_effect_exprs` silently discards all but the last row variable when a
   `with` clause contains more than one — a latent incorrectness with no guard
   or error.
4. The solver and `InferEffectRow` have no unit tests. All coverage is through
   integration fixtures, making regression detection slow and fragile.

## Guide-level explanation

### Fix A: Dedicated effect-mismatch diagnostic

Today, when a callee requires an effect that the ambient context does not
provide, `constrain_call_effects` falls through to
`report_effect_mismatch_via_unification`, which wraps both rows in synthetic
`Fun([], Unit, row)` types and calls `unify_with_context`. The resulting
diagnostic reads:

```
error[E300]: type mismatch
  --> my_program.flx:5:3
   |
5  |   do_work()
   |   ^^^^^^^^^ expected `() -> ()`, found `() -[IO]-> ()`
```

The fix replaces this with a dedicated diagnostic path that names the missing
effects directly:

```
error[E400]: missing effect
  --> my_program.flx:5:3
   |
5  |   do_work()
   |   ^^^^^^^^^ requires effect `IO` but the enclosing context is pure
   |
   = hint: annotate the enclosing function with `with IO`
```

The existing `E400` family is used. No new error codes are introduced.

### Fix B: Violation deduplication in `RowSolution`

`solve_row_constraints` pushes a `RowConstraintViolation` for every failing
constraint. When ten call-site `Eq` constraints all fail for the same shared
row variable `e`, ten identical `UnresolvedVars { vars: [e] }` entries appear
in `violations`. Callers (in `mod.rs`) must deduplicate before emitting
diagnostics, but currently do not.

The fix deduplicates violations inside `solve_row_constraints` before
returning `RowSolution`, so callers always get a clean list.

### Fix C: Guard against multiple row variables

`InferEffectRow::from_effect_exprs` documents that it keeps only the last row
variable seen:

```rust
// Current behavior keeps the last seen row-var as the tail.
// Multiple row-vars in one list are not merged here.
tail = Some(mapped);
```

In valid Flux, a single `with` clause should never contain two row variables
(e.g. `with IO | e | f` is undefined). The fix adds an explicit guard that
returns an `Err` or emits a diagnostic when more than one distinct row variable
appears in one effect expression list, preventing silent data loss.

### Fix D: Solver and `InferEffectRow` unit tests

Add a `#[cfg(test)]` block to `src/bytecode/compiler/effect_rows.rs` covering:

- `Eq` constraint links vars and binds atoms to both sides
- `Contains` emits `UnsatisfiedSubset` for closed rows with missing atom
- `Absent` deferred evaluation fires after `resolve_links`
- `Subset` emits `UnsatisfiedSubset` when left atoms are not in right
- `resolve_links` propagates bindings transitively through the link graph
- Multi-constraint scenario with shared row variable

Add to `src/types/infer_effect_row.rs`:

- `from_effect_exprs` with concrete effects only (closed row)
- `from_effect_exprs` with one row variable (open row)
- `from_effect_exprs` reuses same `TypeVarId` for repeated row variable name
- `concrete_mut` round-trip

## Reference-level explanation

### Fix A: `constrain_call_effects` diagnostic path

**Current** (`src/ast/type_infer/effects.rs`):

```rust
fn report_effect_mismatch_via_unification(
    &mut self,
    callee: InferEffectRow,
    ambient: InferEffectRow,
    span: Span,
) {
    // Wraps rows in dummy Fun types — produces poor "() -> ()" message
    let actual_effect_ty = InferType::Fun(vec![], Box::new(InferType::Con(TypeConstructor::Unit)), callee);
    let expected_effect_ty = InferType::Fun(vec![], Box::new(InferType::Con(TypeConstructor::Unit)), ambient);
    let _ = self.unify_with_context(&expected_effect_ty, &actual_effect_ty, span, ReportContext::Plain);
}
```

**Replacement**:

```rust
fn report_effect_mismatch(
    &mut self,
    callee: &InferEffectRow,
    ambient: &InferEffectRow,
    span: Span,
) {
    // Compute the first missing effect deterministically (sorted by symbol ID).
    let mut missing: Vec<Identifier> = callee
        .concrete()
        .iter()
        .filter(|e| !ambient.concrete().contains(e))
        .copied()
        .collect();
    missing.sort_by_key(|s| s.as_u32());

    let first_missing = match missing.first() {
        Some(e) => self.interner.resolve(*e).to_string(),
        None => return, // no concrete missing effects; row-tail mismatch only
    };

    let hint = if ambient.tail().is_none() {
        format!("annotate the enclosing function with `with {first_missing}`")
    } else {
        format!("the row variable in the ambient context must include `{first_missing}`")
    };

    self.errors.push(
        Diagnostic::make_error_dynamic(
            "E400",
            "MISSING EFFECT",
            crate::diagnostics::ErrorType::Compiler,
            format!("requires effect `{first_missing}` but the enclosing context does not provide it"),
            Some(hint),
            self.file_path.clone(),
            span,
        )
        .with_primary_label(span, format!("requires `{first_missing}`")),
    );
}
```

### Fix B: Deduplication in `solve_row_constraints`

In `src/bytecode/compiler/effect_rows.rs`, before constructing `RowSolution`:

```rust
// Deduplicate violations: same variant + same payload = same violation.
state.violations.sort_by_key(|v| match v {
    RowConstraintViolation::InvalidSubtract { atom } => (0u8, atom.as_u32(), 0),
    RowConstraintViolation::UnresolvedVars { vars } => (1u8, vars.first().map_or(0, |s| s.as_u32()), 0),
    RowConstraintViolation::UnsatisfiedSubset { missing } => (2u8, missing.first().map_or(0, |s| s.as_u32()), 0),
});
state.violations.dedup_by(|a, b| std::mem::discriminant(a) == std::mem::discriminant(b));
```

### Fix C: Guard in `from_effect_exprs`

In `src/types/infer_effect_row.rs`:

```rust
pub fn from_effect_exprs(
    effects: &[EffectExpr],
    row_var_env: &mut HashMap<Identifier, TypeVarId>,
    row_var_counter: &mut u32,
) -> Result<Self, MultipleRowVarError> {
    let mut concrete = HashSet::new();
    let mut tail: Option<TypeVarId> = None;
    let mut tail_name: Option<Identifier> = None;

    for effect in effects {
        concrete.extend(effect.normalized_concrete_names());
        if let Some(row_var) = effect.row_var() {
            if let Some(existing_name) = tail_name {
                if existing_name != row_var {
                    return Err(MultipleRowVarError { first: existing_name, second: row_var });
                }
            }
            tail_name = Some(row_var);
            let mapped = *row_var_env.entry(row_var).or_insert_with(|| {
                let next = *row_var_counter;
                *row_var_counter += 1;
                next
            });
            tail = Some(mapped);
        }
    }

    Ok(match tail {
        Some(v) => Self::open_from_symbols(concrete, v),
        None => Self::closed_from_symbols(concrete),
    })
}
```

Callers that currently use `from_effect_exprs` are updated to handle the
`Result`. In practice, the parser prevents two distinct row variables in one
`with` clause, so `Err` is unreachable from well-formed AST — but the guard
makes the invariant explicit and testable.

### Fix D: Unit tests

`src/bytecode/compiler/effect_rows.rs` — new `#[cfg(test)]` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::symbol::Symbol;

    fn sym(n: u32) -> Symbol { Symbol::new(n) }

    fn row(atoms: &[u32], vars: &[u32]) -> EffectRow {
        EffectRow {
            atoms: atoms.iter().copied().map(sym).collect(),
            vars: vars.iter().copied().map(sym).collect(),
        }
    }

    #[test]
    fn eq_binds_atoms_to_both_var_sets() {
        let constraints = vec![RowConstraint::Eq(row(&[10], &[1]), row(&[20], &[2]))];
        let sol = solve_row_constraints(&constraints);
        assert!(sol.bindings[&sym(1)].contains(&sym(10)));
        assert!(sol.bindings[&sym(1)].contains(&sym(20)));
        assert!(sol.bindings[&sym(2)].contains(&sym(10)));
        assert!(sol.bindings[&sym(2)].contains(&sym(20)));
    }

    #[test]
    fn contains_emits_violation_for_closed_row_missing_atom() {
        let constraints = vec![RowConstraint::Contains(row(&[10], &[]), sym(20))];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
        assert!(matches!(
            sol.violations[0],
            RowConstraintViolation::UnsatisfiedSubset { .. }
        ));
    }

    #[test]
    fn absent_deferred_fires_after_resolve_links() {
        // var 1 is bound to atom 10 by Eq; Absent(row with var 1, 10) should fail.
        let constraints = vec![
            RowConstraint::Eq(row(&[10], &[1]), row(&[], &[])),
            RowConstraint::Absent(row(&[], &[1]), sym(10)),
        ];
        let sol = solve_row_constraints(&constraints);
        assert!(!sol.violations.is_empty());
    }

    #[test]
    fn subset_emits_violation_for_missing_atoms() {
        let constraints = vec![RowConstraint::Subset(row(&[10, 20], &[]), row(&[10], &[]))];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
        assert!(matches!(
            sol.violations[0],
            RowConstraintViolation::UnsatisfiedSubset { ref missing } if missing.contains(&sym(20))
        ));
    }

    #[test]
    fn resolve_links_propagates_transitively() {
        // var 1 linked to var 2 via Eq; var 2 bound to atom 30 via Contains.
        let constraints = vec![
            RowConstraint::Eq(row(&[], &[1]), row(&[], &[2])),
            RowConstraint::Contains(row(&[], &[2]), sym(30)),
        ];
        let sol = solve_row_constraints(&constraints);
        assert!(sol.bindings.get(&sym(1)).is_some_and(|b| b.contains(&sym(30))));
    }

    #[test]
    fn violations_are_deduplicated() {
        // Three identical Subset failures for the same missing atom.
        let constraints = vec![
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
            RowConstraint::Subset(row(&[10], &[]), row(&[], &[])),
        ];
        let sol = solve_row_constraints(&constraints);
        assert_eq!(sol.violations.len(), 1);
    }
}
```

`src/types/infer_effect_row.rs` — additions to existing `#[cfg(test)]`:

```rust
#[test]
fn from_effect_exprs_closed_row_collects_concrete_names() {
    // test that Named effects become concrete and tail is None
}

#[test]
fn from_effect_exprs_open_row_sets_tail() {
    // test that RowVar produces open row with correct tail id
}

#[test]
fn from_effect_exprs_same_var_name_reuses_id() {
    // test that row_var_env is consulted: same symbol → same TypeVarId
}
```

## Drawbacks

- Fix C changes the return type of `from_effect_exprs` from `Self` to
  `Result<Self, ...>`. All call sites must be updated (currently 4 sites
  in `src/ast/type_infer/`). This is mechanical but touches core inference code.
- Fix A removes the `report_effect_mismatch_via_unification` helper entirely,
  which may surface other call sites that relied on it for row-tail-only
  mismatches (not yet identified).

## Rationale and alternatives

**Fix A alternative: keep the `Fun` wrapper but improve the message.**
The unification error formatter could detect when both Fun types have empty
params and Unit return and emit a special effect-row message. This avoids
changing the diagnostic path but adds complexity to the formatter. Rejected:
the dedicated path is cleaner and produces better hints.

**Fix B alternative: deduplicate at the call site.**
The compiler's `mod.rs` already iterates violations to emit diagnostics.
Deduplication there is equally correct but spreads responsibility. Preferred:
keep `RowSolution` self-contained.

**Fix C alternative: log a warning and keep the last var.**
Silently keeping the last var was the original behavior. A warning rather than
an error would be less breaking. Rejected: the parser already prevents two
distinct row vars syntactically, so `Err` should be unreachable in practice —
making it a hard error is the right signal.

## Prior art

- Proposal 0042 — established `RowConstraint` and `RowSolution`.
- Proposal 0049 — locked diagnostic codes and deferred `Absent` evaluation.
- Proposal 0064 — introduced `EffectExpr::RowVar` and `from_effect_exprs`.

## Unresolved questions

1. Should Fix A emit one diagnostic per missing effect, or one diagnostic
   listing all missing effects? Current proposal: one diagnostic for the
   first (sorted) missing effect, consistent with how `E400` is used elsewhere.
2. Fix C changes the `from_effect_exprs` signature. Should the `MultipleRowVarError`
   be a dedicated type or an existing `DiagnosticError`? Recommendation: a
   dedicated lightweight struct — callers that hit it in tests or fuzzing will
   want to inspect the two conflicting names.

## Future possibilities

- Once Fix A is in place, `E419` (unresolved single var) and `E420` (ambiguous
  multi-var) diagnostics can be routed through the same dedicated path for
  consistent messaging.
- Fix D's solver unit tests become the foundation for property-based tests
  (using `proptest`) that randomly generate constraint sets and verify solver
  invariants.
- `Extend` and `Subtract` solver constraints (currently `#[allow(dead_code)]`)
  can be wired to call sites in a follow-up proposal once the solver is
  well-tested.
