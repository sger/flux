# Effect Row System (Internal Reference)

> **Proposals:** [0042](../proposals/implemented/0042_effect_rows_and_constraints.md) · [0049](../proposals/implemented/0049_effect_rows_completeness.md) · [0064](../proposals/implemented/0064_effect_row_variables.md)
> **Guide:** [Chapter 10 — Effects and Purity](../guide/10_effects_and_purity.md) · [Chapter 11 — HOF and Effect Polymorphism](../guide/11_hof_effect_polymorphism.md)

This document is the canonical implementation reference for the Flux effect row system as of v0.0.4.

---

## 1. Surface Syntax

Effect annotations appear in `with` clauses on function signatures:

```
effect_annotation := "with" effect_row
effect_row         := effect_atom ("+" effect_atom)* ("-" effect_atom)* ("|" IDENT)?
effect_atom        := IDENT
```

The `|` introduces a **row variable tail** — the open part of the row. Row variables must be lowercase identifiers and always appear last.

| Example | Meaning |
|---------|---------|
| `with IO` | Closed row containing `IO` |
| `with IO, Time` | Closed row containing `IO` and `Time` |
| `with \|e` | Open row — `e` stands for any set of effects |
| `with IO \| e` | Open row — `IO` plus whatever `e` resolves to |
| `with IO + State - Console` | Row extension/subtraction |

Parser: `src/syntax/parser/helpers.rs` — `parse_effect_expr()`.

---

## 2. AST Representation (`EffectExpr`)

File: `src/syntax/effect_expr.rs`

```rust
pub enum EffectExpr {
    Named { name: Identifier, span: Span },    // concrete: IO, State, Time
    Add { left, right, span },                 // IO + State
    Subtract { left, right, span },            // IO - Console
    RowVar { name: Identifier, span: Span },   // |e  (open row tail)
}
```

Key methods:
- `row_var() -> Option<Identifier>` — extracts the tail variable, if any.
- `is_open() -> bool` — true if the expression contains a `RowVar`.

`RowVar` is a first-class AST node since proposal 0064. The legacy pattern of detecting row variables via `is_effect_variable` (checking if an identifier was lowercase) has been removed.

---

## 3. HM-Level Representation (`InferEffectRow`)

File: `src/types/infer_effect_row.rs`

```rust
pub struct InferEffectRow {
    pub concrete: HashSet<Identifier>,  // concrete effect atoms
    pub tail: Option<TypeVarId>,        // open row tail variable, if any
}
```

Construction:
- `InferEffectRow::closed(concrete)` — no tail, fixed effect set.
- `InferEffectRow::open(concrete, tail)` — tail variable can be unified.
- `InferEffectRow::from_effect_expr(expr, interner)` — lowering from AST.

Substitution: `apply_substitution(subst)` follows the tail chain transitively, collecting concrete atoms and merging them until the tail resolves to a concrete row or remains as a free variable.

Unification follows standard row-polymorphism rules:
- Two closed rows must have equal concrete sets.
- Closed + open: the tail unifies with the difference `(closed \ open_concrete)`.
- Two open rows: cross-differences are unified with the respective tails.

---

## 4. Constraint Solver

File: `src/bytecode/compiler/effect_rows.rs`

The solver uses a worklist algorithm over four constraint types:

| Constraint | Semantics | Emitted by |
|-----------|-----------|------------|
| `Eq(row1, row2)` | Rows must be equal; links vars bidirectionally | Callback effect matching |
| `Subset(row1, row2)` | `row1 ⊆ row2`; binds missing atoms to vars or emits E422 | Callback subset checks |
| `Absent(row, atom)` | `atom ∉ row`; deferred until after `resolve_links` | Subtraction expressions |
| `Link(var, row)` | Internal; var is bound to row | Var unification |

**Deferred `Absent` evaluation** — `Absent` constraints are accumulated during the worklist pass and re-evaluated after all `Link` bindings are resolved. This ensures correct results when multiple arguments share a row variable and later arguments bind it to effects that earlier subtraction constraints must exclude.

Entry point: `solve_row_constraints(constraints) -> Vec<Diagnostic>`.

**Deterministic diagnostics**: symbols are sorted by ID before emitting multi-effect error messages, ensuring stable output across runs.

---

## 5. Error Codes

| Code | Name | Trigger |
|------|------|---------|
| `E400` | MISSING EFFECT | Call or operation requires an ambient effect not in scope |
| `E419` | UNRESOLVED ROW VAR (SINGLE) | One row variable remains free after solving |
| `E420` | UNRESOLVED ROW VAR (MULTI) | Multiple row variables remain ambiguous |
| `E421` | INVALID EFFECT SUBTRACTION | Concrete atom subtracted that is not in the row |
| `E422` | UNSATISFIED EFFECT SUBSET | Required subset not satisfied by provided row |

---

## 6. Solver Completeness Matrix

| Row form | Completeness | Diagnostic |
|----------|-------------|------------|
| Concrete atom equivalence (`with IO, Time` == `with Time, IO`) | complete | no diagnostic |
| Higher-order propagation via `\|e` row vars | complete | E400 / E419 / E420 |
| Deterministic first-failure for multi-missing obligations | complete | E400 (first missing) |
| Strict unresolved boundary safeguards | complete (strict path) | E425 |
| Row subtraction via surface normalization (`with A + B - B`) | complete | E400 for missing residuals |
| General absence/subset solving | complete | E421 / E422 |

---

## 7. Effect Rows on Type Class Methods (Proposal 0151, Phase 4)

> **Proposal:** [0151](../proposals/0151_module_scoped_type_classes.md) — Phase 4 (Effects on instance methods)

Type class methods participate in the effect row system. Two features were added in Phase 4:

### 7.1 Effect Floors on Class Methods

A class method can declare an effect floor — a minimum effect row that all instances must satisfy:

```flux
class Logger<h> {
    fn log(hnd: h, msg: String) -> Unit with Console   // floor = {Console}
}
```

Instance methods must declare an effect row that is a **superset** of the class method's floor:

| Instance effect row | Floor `{Console}` | Result |
|--------------------|--------------------|--------|
| `with Console` | `{Console} ⊇ {Console}` | OK |
| `with Console, AuditLog` | `{Console, AuditLog} ⊇ {Console}` | OK |
| *(no `with`)* | `{} ⊇ {Console}` | **E452** |
| `with AuditLog` | `{AuditLog} ⊇ {Console}` | **E452** |

The E452 walker validates this at compile time during Phase 4a. It walks all instance declarations and checks that each instance method's declared effects contain every effect from the corresponding class method's floor.

A class method with no `with` clause imposes no floor — instances are free to add any effects or remain pure.

### 7.2 Effect Row Propagation Through Type-Directed Dispatch

When `try_resolve_class_call` resolves a class method call to a specific instance, the resolved instance's effect row flows into the caller's ambient row. This means:

```flux
class Comparable<a> {
    fn same(x: a, y: a) -> Bool
}

instance Comparable<Int> {
    fn same(x, y) { x == y }           // pure — row = {}
}

instance Comparable<UserId> {
    fn same(a, b) with AuditLog {       // effectful — row = {AuditLog}
        perform AuditLog.record("comparing users")
        match (a, b) { (Id(x), Id(y)) -> x == y }
    }
}
```

At call sites:
- `Comparable.same(1, 2)` resolves to `Comparable<Int>` → row `{}` → caller needs no effects
- `Comparable.same(id1, id2)` resolves to `Comparable<UserId>` → row `{AuditLog}` → caller must have `AuditLog` in its ambient row

This is the key interaction between type classes and the effect system: the same class method name produces different effect requirements depending on which instance the type-directed dispatch selects.

### 7.3 Row-Polymorphic Class Methods

Class methods can use row variables (`|e`) just like regular functions:

```flux
class Foldable<f> {
    fn fold<a, b>(xs: f<a>, init: b, step: (b, a) -> b with |e) -> b with |e
}
```

The row variable `e` is instantiated freshly at each call site and unifies with the callback's actual effects. The instance body itself may be pure — the row variable is bound at the call site, not at the instance.

### 7.4 Implementation Files

- `src/syntax/parser/statement.rs` — `parse_class_method()` and `parse_instance_method()` call `parse_effect_list()` for the `with` clause
- `src/syntax/type_class.rs` — `ClassMethod.effects` and `InstanceMethod.effects` AST fields
- `src/types/class_dispatch.rs` — Phase 1b mangling pass forwards effects to mangled function signatures
- `src/bytecode/compiler/statement.rs` — E452 walker validates effect floor satisfaction
- `src/core/lower_ast/mod.rs` — `try_resolve_class_call()` propagates the resolved instance's row to the caller

---

## 8. Base Function Row Signatures

All 77 base functions with higher-order parameters declare `row_params` in their `BaseHmSignature`. For example:

```rust
// map: forall e. (Any, (Any -> Any with |e)) -> Any with |e
Id::Map => sig_with_row_params(
    vec![],   // no type params
    vec!["e"],  // row param e
    vec![t_any(), t_fun(vec![t_any()], t_any(), row(vec![], Some("e")))],
    t_any(),
    row(vec![], Some("e")),
),
```

This makes `map(xs, io_fn)` propagate the `IO` effect of `io_fn` through `map` to the call site.

See `docs/internals/base_hm_signatures.md` for the full signature reference.

---

## 9. Fixture Evidence

**Passing (constraint correctness):**
- `100_effect_row_order_equivalence_ok.flx` — concrete ordering is non-semantic
- `163_effect_row_multi_atom_closure_ok.flx` — multi-atom row solving
- `164_effect_row_subtract_resolved_ok.flx` — subtraction with resolved var
- `165_effect_row_subset_ok.flx` — subset constraint satisfied
- `104_effect_row_absent_ordering_linked_ok.flx` — deferred absent with shared var

**Failing (diagnostic correctness):**
- `194_effect_row_multi_missing_deterministic_e400.flx` — E400 deterministic first-missing
- `195_effect_row_invalid_subtract_e421.flx` — E421 concrete invalid subtraction
- `196_effect_row_subtract_unresolved_single_e419.flx` — E419 single unresolved var
- `197_effect_row_subtract_unresolved_multi_e420.flx` — E420 multi ambiguous vars
- `198_effect_row_subset_unsatisfied_e422.flx` — E422 unsatisfied subset
- `199_effect_row_subset_ordered_missing_e422.flx` — E422 ordered missing
- `200_effect_row_absent_ordering_linked_violation_e421.flx` — E421 deferred absent violation

---

## 10. Contributor Rules

When changing effect row semantics:

1. Update this document.
2. Add/update fixtures in `examples/type_system/` and `examples/type_system/failing/`.
3. If `EffectExpr` or `InferEffectRow` changes, update the constraint solver and `from_effect_expr`.
4. Run:
   ```bash
   cargo test --test type_inference_tests
   cargo test --test compiler_rules_tests
   cargo test --all --all-features purity_vm_jit_parity_snapshots
   ```
5. Verify VM/JIT parity for any new diagnostic codes.
