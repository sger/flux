- Feature Name: Effect Row Variables (`|e`) in EffectExpr
- Start Date: 2026-03-01
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0064: Effect Row Variables (`|e`) in EffectExpr

## Summary

Extend Flux's `EffectExpr` with explicit row-variable tail syntax (`|e`) so that
higher-order functions like `map`, `filter`, and `fold` can be polymorphic over their
callers' effect context. This is the minimum change needed to make those functions
preserve the effects of their function arguments without hardcoding any specific
effect set.

This proposal builds directly on 0042 (effect rows) and 0049 (row completeness) and is
required before any actor or concurrency effects can compose correctly with user code.

### Implementation status (2026-03-01)

- `EffectExpr::RowVar` is implemented and threaded through parser/compiler/HM paths.
- `with` clauses require explicit row-tail syntax (`|e`) for row variables.
- Legacy implicit forms such as `with e + IO` are rejected with migration guidance.
- HM function-effect unification now supports open rows (`|e`) in addition to closed rows.
- Strict public function-typed boundaries are runtime-enforced via function contracts.

### Current state (what 0042 + 0049 already deliver)

Row variables already work at the compiler level. The parser treats lowercase identifiers
in `with` clauses as regular `EffectExpr::Named` nodes; the compiler distinguishes
concrete effects from row variables via `is_effect_variable`. The constraint solver in
`src/bytecode/compiler/effect_rows.rs` handles:

- **`Eq`**: row equality (unifies atoms + links vars)
- **`Contains`**: atom membership (binds vars or emits `UnsatisfiedSubset`)
- **`Absent`**: deferred absence checking (proves an atom is NOT in a row)
- **`Subset`**: set inclusion (emits `UnsatisfiedSubset` for closed rows, binds vars otherwise)
- **`Extend` / `Subtract`**: reserved solver-level constraints (not yet emitted by callers)

Solver features: worklist algorithm, variable linking, deferred absent evaluation after
`resolve_links`, deterministic diagnostics via sorted symbol IDs.

Error codes: E400 (missing effect), E419 (unresolved single var), E420 (ambiguous multi var),
E421 (invalid subtraction), E422 (unsatisfied subset).

**What is NOT yet done:**

1. Full stdlib migration to explicit `|e` row-tail style everywhere

## Motivation

Without explicit row-variable syntax, every higher-order function in Flux must either:

1. Declare every effect the callback might use — impossible to predict at definition time.
2. Omit effect annotations and accept `Any`-effect fallback — defeats purity checking.

The problem is concrete today:

```flux
-- This function cannot be correctly typed without row variables.
-- If f has IO effect, map must also declare IO.
-- If f has State effect, map must declare State.
-- With a fixed annotation, map is artificially restricted.
fn map(xs: List<a>, f: fn(a) -> b) -> List<b> { ... }
```

The correct type for `map` is:

```
map : (List<a>, fn(a) -> <e> b) -> <e> List<b>
```

where `e` is a row variable meaning "whatever effects `f` has, `map` propagates them."

This is the standard Koka/Eff row-polymorphism solution, and it is required for the
`Actor` effect (proposal 0065) to compose with existing stdlib functions.

## Guide-level explanation

### New surface syntax

A row variable tail is written `|e` inside a `with` annotation:

```flux
-- f is polymorphic over its callback's effects.
-- Any effect e that g has, f also has.
fn f(g: fn(Int) -> Int with IO | e) -> Int with IO | e {
    g(42)
}

-- map preserves the effects of its callback
fn map(xs: List<a>, f: fn(a) -> b with e) -> List<b> with e {
    -- ...
}

-- A pure caller: e = empty row, so map is also pure here
let doubled = map([1, 2, 3], \x -> x * 2)

-- An IO caller: e = IO, so map has IO effect here
let printed = map([1, 2, 3], \x -> do {
    print(x)
    x * 2
})
```

### Difference from current implicit variables

Today, row variables like `e` in `with e + IO - Console` are parsed as plain `Named`
identifiers and classified at compile time by `is_effect_variable`. This works for
single-function signatures but has limitations:

1. **No syntactic distinction** between a concrete effect name and a row variable. A typo
   like `with Io` is silently treated as a concrete effect, not flagged as an undefined
   row variable.
2. **No parser-level validation** that row variables appear in tail position. `with IO + e`
   and `with e + IO` are both valid, but only the former is semantically meaningful as
   "IO plus whatever e is."
3. **No HM integration** — the HM inference pass ignores effect annotations entirely. Row
   variables are not unified across call sites; all checking is deferred to PASS 2.

The `|e` syntax addresses all three by making row variables a first-class AST concept.

### Effect row variable rules

1. A row variable `e` stands for any (possibly empty) set of effects.
2. `with IO | e` means "this function has IO plus all effects in e."
3. Open-row-only signatures are encoded explicitly as `with |e`.
4. Row variables are unified during HM inference — if `f` is called with a callback that
   has `IO`, the row variable `e` in `f`'s signature unifies with `IO`.
5. Row variables in `public fn` API positions require explicit annotation (`E425`-extended).
6. A function without a row variable tail is *effect-monomorphic* — it has exactly the
   declared effects, no more.

### Error messages

When a function with row variable `e` is called with a callback whose effect cannot be
propagated (e.g., the call site is a pure context), the compiler emits a diagnostic:

```
error[E419]: effect row mismatch
  --> examples/my_program.flx:12:5
   |
12 |     map(xs, effectful_fn)
   |     ^^^^^^^^^^^^^^^^^^^^^ this call propagates `IO` through row variable `e`
   |
   = note: `map` has signature: (List<a>, fn(a) -> <e> b) -> <e> List<b>
   = note: caller context is pure (no `with` annotation)
   = hint: annotate the enclosing function with `with IO` to allow this call
```

## Reference-level explanation

### Phase 1: AST + Parser (no solver changes needed)

#### Changes to `EffectExpr` (src/syntax/effect_expr.rs)

Add a `RowVar` variant to `EffectExpr`. No `Open` variant is needed — the existing `Add`
tree structure already composes concrete effects with a tail variable:

```rust
pub enum EffectExpr {
    /// A named concrete effect: `IO`, `State`, `Actor`
    Named { name: Identifier, span: Span },

    /// Row addition: `IO + State`
    Add { left: Box<EffectExpr>, right: Box<EffectExpr>, span: Span },

    /// Row subtraction: `IO + State - Console`
    Subtract { left: Box<EffectExpr>, right: Box<EffectExpr>, span: Span },

    /// NEW: A row variable tail: the `e` in `IO | e`
    /// Represents an open row — any additional effects can appear here.
    RowVar { name: Identifier, span: Span },
}
```

The `RowVar` variant replaces the current convention of parsing row variables as `Named`
and detecting them later via `is_effect_variable`. `EffectRow::from_effect_expr` maps
`RowVar` to `vars` instead of relying on the callback.

New methods on `EffectExpr`:

```rust
impl EffectExpr {
    /// Returns the row variable tail, if any.
    pub fn row_var(&self) -> Option<Identifier> {
        match self {
            EffectExpr::RowVar { name, .. } => Some(*name),
            EffectExpr::Add { right, .. } | EffectExpr::Subtract { left: right, .. } => {
                right.row_var()
            }
            _ => None,
        }
    }

    /// True if this expression contains a row variable (open row).
    pub fn is_open(&self) -> bool {
        self.row_var().is_some()
    }
}
```

#### Parser changes (src/syntax/parser/helpers.rs)

The `with` clause parser recognizes `|` as a row-variable separator:

```
-- Grammar extension:
effect_annotation := "with" effect_row
effect_row         := effect_atom ("+" effect_atom)* ("-" effect_atom)* ("|" IDENT)?
effect_atom        := IDENT
```

```rust
// In parse_effect_expr(), after parsing atoms and operators:
if self.current_token_is(TokenType::Pipe) {
    self.advance();
    let var_name = self.expect_identifier("row variable name")?;
    let span = self.current_span();
    let tail = EffectExpr::RowVar { name: var_name, span };
    // Fold the tail into the existing expression tree via Add
    return Ok(EffectExpr::Add {
        left: Box::new(current_expr),
        right: Box::new(tail),
        span,
    });
}
```

#### Backward compatibility

Implicit row-variable syntax is intentionally rejected. Forms like `with e + IO` must be
rewritten to explicit row-tail form (`with IO | e`), and open-row-only signatures use
`with |e`.

#### Constraint solver impact

None. `EffectRow::from_effect_expr` already separates atoms and vars. The only change is
that `RowVar` is classified directly as a var without needing the `is_var` callback:

```rust
EffectExpr::RowVar { name, .. } => {
    let mut row = Self::default();
    row.vars.insert(*name);
    row
}
```

### Phase 2: HM inference integration

Row variables are represented as fresh type variables in the HM inference context:

```rust
// In InferCtx:
// When a function has `with IO | e` (an open effect row):
// - Allocate a fresh InferType::Var for the row variable `e`
// - Store the mapping: row_var_name -> InferType::Var(id) in the ctx
// - When two effect rows are unified, unify their row variable parts

fn instantiate_effect_row(&mut self, effects: &[EffectExpr]) -> EffectRowType {
    let mut concrete = Vec::new();
    let mut tail = None;
    for effect in effects {
        match effect.row_var() {
            Some(var) => { tail = Some(self.fresh_var()); }
            None => { concrete.extend(effect.concrete_names()); }
        }
    }
    match tail {
        Some(var) => EffectRowType::Open { concrete, tail: var },
        None => EffectRowType::Closed(concrete),
    }
}
```

Unification of effect rows follows standard row-polymorphism rules:

```rust
fn unify_effect_rows(&mut self, r1: &EffectRowType, r2: &EffectRowType, span: Span) -> UnifyResult {
    match (r1, r2) {
        (EffectRowType::Closed(a), EffectRowType::Closed(b)) => {
            // Must be equal sets
            if a != b { return Err(EffectRowMismatch { expected: a, found: b, span }); }
            Ok(())
        }
        (EffectRowType::Closed(concrete), EffectRowType::Open { concrete: oc, tail }) |
        (EffectRowType::Open { concrete: oc, tail }, EffectRowType::Closed(concrete)) => {
            // The tail unifies with (concrete \ oc).
            let diff: Vec<_> = concrete.iter().filter(|e| !oc.contains(e)).collect();
            self.unify_row_var(*tail, diff, span)
        }
        (EffectRowType::Open { concrete: ca, tail: ta },
         EffectRowType::Open { concrete: cb, tail: tb }) => {
            // Row difference: (cb \ ca) → ta, (ca \ cb) → tb
            let diff_a: Vec<_> = cb.iter().filter(|e| !ca.contains(e)).collect();
            let diff_b: Vec<_> = ca.iter().filter(|e| !cb.contains(e)).collect();
            self.unify_row_var(*ta, diff_a, span)?;
            self.unify_row_var(*tb, diff_b, span)?;
            Ok(())
        }
    }
}
```

### Phase 3: Strict mode enforcement

In `--strict` mode, `public fn` parameters of callback type must have explicit effect
annotations (including row variables if the function is higher-order). Unannotated callback
parameters in public APIs emit `E425`.

### New error codes

No new top-level codes are introduced. Row-variable unification failures use the existing
`E419`–`E422` family with extended message text to identify the row variable involved.

### Existing solver infrastructure (0042 + 0049)

The constraint solver already handles everything needed for call-site validation:

| Constraint | Solver behavior | Emitted by |
|---|---|---|
| `Eq(row1, row2)` | Link vars, bind atoms bidirectionally | Callback effect matching |
| `Contains(row, atom)` | Bind atom to vars or emit `UnsatisfiedSubset` | (reserved) |
| `Absent(row, atom)` | Deferred; check bindings post-resolution | Subtraction expressions |
| `Subset(row1, row2)` | Check atoms, bind missing to vars or emit `UnsatisfiedSubset` | Callback subset check |
| `Extend` / `Subtract` | Reduce to `Eq` | (reserved) |

Deferred absent evaluation ensures correct results when multiple arguments share a row
variable and later arguments bind it to effects that earlier subtraction constraints
must exclude.

## Drawbacks

- Row variable syntax (`|e`) is new surface syntax. Existing programs are unaffected
  (the tail is optional), but it extends the learning surface.
- Open-row unification in Phase 2 adds complexity to HM inference. However, the compiler's
  constraint solver already handles the runtime semantics, so HM integration is for
  improved diagnostics and IDE support, not correctness.
- The `|` token is already used for cons-list patterns (`[h | t]`). The context (inside
  a `with` clause after effect names) is unambiguous, but users may find it confusing
  initially.

## Rationale and alternatives

**Why `|e` syntax?** Consistent with Koka, Eff, and the academic literature on row
polymorphism. The `|` visually separates the concrete effects from the polymorphic tail,
making it clear where the "open" part begins.

**Why not keep the current implicit approach?** The implicit approach (lowercase identifiers
detected by `is_effect_variable`) works for the current test suite but has no syntactic
distinction from typos, no parser-level validation, and cannot be represented in the AST
for tooling (formatters, LSP, documentation generators).

**Alternative: implicit row variables.** The compiler could infer a row variable for every
higher-order function automatically. This is simpler for users but loses the explicit
contract at the API level and makes diagnostics harder to explain.

**Alternative: `+Any` instead of `|e`.** Using `with IO + Any` to mean "IO plus anything"
is simpler but loses the ability to name the variable and reason about it across function
boundaries. `|e` preserves composability.

**Impact of not doing this:** Higher-order functions remain effect-monomorphic. The Actor
effect cannot propagate through `map`, `filter`, or any stdlib combinator, severely
limiting composability. This is a blocker for proposal 0065.

## Prior art

- **Koka** (Leijen, 2017) — the canonical reference for row-polymorphic effect types.
  Uses identical `<e>` syntax in function types.
- **Eff** (Bauer & Pretnar, 2015) — row-based algebraic effects with similar polymorphism.
- **Links** (Cooper et al.) — row-polymorphic effects in a web programming context.
- **Proposal 0042** — the Flux row constraint solver foundation (implemented).
- **Proposal 0049** — row completeness hardening: `Absent`, `Subset`, deterministic
  diagnostics, deferred evaluation (implemented).

## Unresolved questions

1. **Row variable naming:** Should row variables be any lowercase identifier, or should
   they use a reserved convention (e.g., single lowercase letter)? Koka allows any
   lowercase identifier. Current Flux behavior already uses any identifier and classifies
   via `is_effect_variable`. **Decision: any lowercase identifier**, consistent with type
   parameters in generics.

2. **Quantification:** Does `with e` on a top-level `public fn` require the caller to
   supply a concrete row, or is it always universally quantified?
   **Decision: universally quantified** (∀e. ...).

3. **Row variable scope:** Are row variables scoped to one function signature or can they
   appear across multiple parameters? **Decision: one signature** — a row variable binds
   across all parameters and the return type of a single function. This is already the
   behavior of the constraint solver (shared `e` across callback parameters links them).

4. **Phase 1 vs Phase 2 ordering:** Can Phase 1 (AST + parser) ship independently?
   **Yes.** The `RowVar` AST variant slots directly into the existing `EffectRow`
   machinery. HM integration (Phase 2) can follow separately.

## Future possibilities

- **Absence constraints in syntax** (`with IO | e \ Console`): explicit syntax for "e
  does not contain Console". The solver already supports `Absent`; this would surface it
  in the grammar.
- **Named effect rows** (`type HttpRow = IO + Network`): type aliases for common effect
  combinations. Natural extension once row variables are stable.
- **Effect inference for top-level functions**: infer the minimal effect row automatically,
  display it in error messages and documentation. Builds on Phase 2's unification
  infrastructure.
- **Negative-cycle detection**: `with IO - IO | e` is contradictory. The solver catches
  this via `InvalidSubtract`, but a dedicated diagnostic explaining the contradiction
  would improve the user experience.
