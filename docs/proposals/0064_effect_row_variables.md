- Feature Name: Effect Row Variables (`|e`) in EffectExpr
- Start Date: 2026-03-01
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0064: Effect Row Variables (`|e`) in EffectExpr

## Summary
[summary]: #summary

Extend Flux's `EffectExpr` with a row-variable tail (`|e`) so that functions can be
polymorphic over their callers' effect context. This is the minimum change needed to make
higher-order functions like `map`, `filter`, and `fold` preserve the effects of their
function arguments without hardcoding any specific effect set.

This proposal builds directly on 0042 (effect rows) and 0049 (row completeness) and is
required before any actor or concurrency effects can compose correctly with user code.

## Motivation
[motivation]: #motivation

Without row variables, every higher-order function in Flux must either:
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
[guide-level-explanation]: #guide-level-explanation

### New surface syntax

A row variable tail is written `|e` inside a `with` annotation:

```flux
-- f is polymorphic over its callback's effects.
-- Any effect e that g has, f also has.
fn f(g: fn(Int) -> Int with e) -> Int with e {
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

### Effect row variable rules

1. A row variable `e` stands for any (possibly empty) set of effects.
2. `with e` on a function means "this function has all effects in e plus any concrete effects listed."
3. Row variables are unified during HM inference — if `f` is called with a callback that
   has `IO`, the row variable `e` in `f`'s signature unifies with `IO`.
4. Row variables in `public fn` API positions require explicit annotation (`E425`-extended).
5. A function without a row variable tail is *effect-monomorphic* — it has exactly the
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
[reference-level-explanation]: #reference-level-explanation

### Changes to `EffectExpr` (src/syntax/effect_expr.rs)

Add a `RowVar` variant and an `Open` variant to `EffectExpr`:

```rust
// src/syntax/effect_expr.rs

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectExpr {
    /// A named concrete effect: `IO`, `State`, `Actor`
    Named { name: Identifier, span: Span },

    /// Row addition: `IO + State`
    Add { left: Box<EffectExpr>, right: Box<EffectExpr>, span: Span },

    /// Row subtraction: `IO + State - Console`
    Subtract { left: Box<EffectExpr>, right: Box<EffectExpr>, span: Span },

    /// NEW: A row variable tail: the `e` in `IO | e`
    /// Represents an open row — any effects can appear here.
    RowVar { name: Identifier, span: Span },

    /// NEW: An open effect row: `IO + e` or just `e`
    /// Semantically: concrete_effects ∪ row_variable
    Open {
        concrete: Vec<Identifier>,   // e.g., [IO, State]
        tail: Identifier,            // the row variable name, e.g., `e`
        span: Span,
    },
}

impl EffectExpr {
    /// Returns all concrete effect names in this expression,
    /// not including any row variable.
    pub fn concrete_names(&self) -> Vec<Identifier> { ... }

    /// Returns the row variable tail, if any.
    pub fn row_var(&self) -> Option<Identifier> { ... }

    /// True if this expression contains a row variable.
    pub fn is_open(&self) -> bool {
        self.row_var().is_some()
    }
}
```

### Parser changes (src/syntax/parser/statement.rs)

The `with` clause parser must recognize the `|` separator before a row variable:

```
-- Grammar extension:
effect_annotation := "with" effect_row
effect_row         := effect_atom ("+" effect_atom)* ("|" IDENT)?
effect_atom        := IDENT ("<" type_args ">")?
```

```rust
// In parse_effect_annotation():
fn parse_effect_row(&mut self) -> ParseResult<EffectExpr> {
    let mut concrete = vec![self.parse_effect_atom()?];

    while self.current_token_is(TokenType::Plus) {
        self.advance();
        concrete.push(self.parse_effect_atom()?);
    }

    // NEW: row variable tail after `|`
    if self.current_token_is(TokenType::Pipe) {
        self.advance();
        let var_name = self.expect_identifier("row variable name")?;
        let span = self.current_span();
        return Ok(EffectExpr::Open {
            concrete: concrete.into_iter()
                .map(|e| e.into_name())
                .collect(),
            tail: var_name,
            span,
        });
    }

    // Closed row (existing behavior)
    Ok(self.fold_effect_atoms(concrete))
}
```

### HM inference changes (src/ast/type_infer.rs)

Row variables are represented as fresh type variables in the HM inference context:

```rust
// In InferCtx: extend InferType with row variable support

// In infer_function_type():
// When a function has `with e` (an Open effect row):
// - Allocate a fresh InferType::Var for the row variable `e`
// - Store the mapping: row_var_name -> InferType::Var(id) in the ctx
// - When two effect rows are unified, unify their row variable parts

fn instantiate_open_effect(&mut self, effect: &EffectExpr) -> EffectRowType {
    match effect {
        EffectExpr::Open { concrete, tail, .. } => {
            let row_var = self.fresh_var(); // fresh unification variable
            self.row_var_env.insert(*tail, row_var);
            EffectRowType::Open {
                concrete: concrete.clone(),
                tail: row_var,
            }
        }
        _ => EffectRowType::Closed(effect.concrete_names()),
    }
}

// Unification of open rows:
// Open { concrete: [IO], tail: e } ~ Open { concrete: [IO, State], tail: e2 }
// → e = State + e2  (solve by row difference)
fn unify_effect_rows(
    &mut self,
    r1: &EffectRowType,
    r2: &EffectRowType,
    span: Span,
) -> UnifyResult {
    match (r1, r2) {
        (EffectRowType::Closed(a), EffectRowType::Closed(b)) => {
            // Must be equal sets
            if a != b { return Err(EffectRowMismatch { expected: a, found: b, span }); }
            Ok(())
        }
        (EffectRowType::Open { concrete: ca, tail: ta },
         EffectRowType::Open { concrete: cb, tail: tb }) => {
            // The difference (cb \ ca) must unify with ta,
            // and (ca \ cb) must unify with tb.
            let diff_a: Vec<_> = cb.iter().filter(|e| !ca.contains(e)).collect();
            let diff_b: Vec<_> = ca.iter().filter(|e| !cb.contains(e)).collect();
            self.unify_row_var(*ta, diff_a, span)?;
            self.unify_row_var(*tb, diff_b, span)?;
            Ok(())
        }
        (EffectRowType::Closed(concrete), EffectRowType::Open { concrete: oc, tail }) => {
            // Closed row must contain exactly the open row's concrete effects.
            // The tail unifies with (concrete \ oc).
            let diff: Vec<_> = concrete.iter().filter(|e| !oc.contains(e)).collect();
            self.unify_row_var(*tail, diff, span)
        }
        (open, closed) => self.unify_effect_rows(closed, open, span),
    }
}
```

### Compiler validation changes (src/bytecode/compiler/mod.rs)

At call sites in PASS 2, when the callee has an open effect row, the compiler must:
1. Determine the concrete effects of the actual callback argument.
2. Unify them with the row variable to determine the call site's effect obligations.
3. Verify the enclosing function's effect annotation satisfies those obligations.

```rust
// In validate_call_effects():
fn validate_open_row_call(
    &self,
    callee_row: &EffectRowType,
    arg_effects: &[EffectName],
    call_span: Span,
) -> CompileResult<()> {
    if let EffectRowType::Open { concrete, .. } = callee_row {
        let propagated: Vec<_> = arg_effects.iter()
            .filter(|e| !concrete.contains(e))
            .collect();
        // Each propagated effect must appear in the enclosing function's row
        for effect in &propagated {
            if !self.current_function_effects().contains(effect) {
                self.emit_diagnostic(
                    diag_enhanced(E419)
                        .with_span(call_span)
                        .with_message(format!(
                            "effect `{}` propagated through row variable but \
                             enclosing function does not declare it",
                            effect
                        ))
                        .with_hint("add `with {}` to the enclosing function signature", effect)
                );
            }
        }
    }
    Ok(())
}
```

### New error codes

No new top-level codes are introduced. Row-variable unification failures use the existing
`E419`–`E422` family with extended message text to identify the row variable involved.

### Interaction with `--strict` mode

In `--strict` mode, `public fn` parameters of callback type must have explicit effect
annotations (including row variables if the function is higher-order). Unannotated callback
parameters in public APIs emit `E425`.

## Drawbacks
[drawbacks]: #drawbacks

- Row variable syntax (`|e`) is new surface syntax. Existing programs are unaffected
  (the tail is optional), but it extends the learning surface.
- Open-row unification adds a new class of type errors that must be diagnosed clearly.
- The implementation in the HM solver adds complexity to `unify_effect_rows`.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why `|e` syntax?** Consistent with Koka, Eff, and the academic literature on row
polymorphism. The `|` is already used for cons lists in Flux, but the context
(inside a `with` clause after effect names) is unambiguous.

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
[prior-art]: #prior-art

- **Koka** (Leijen, 2017) — the canonical reference for row-polymorphic effect types.
  Uses identical `|e` syntax in function types.
- **Eff** (Bauer & Pretnar, 2015) — row-based algebraic effects with similar polymorphism.
- **Links** (Cooper et al.) — row-polymorphic effects in a web programming context.
- **Proposal 0042** — the Flux row constraint foundation this proposal extends.
- **Proposal 0049** — row completeness hardening this proposal must be consistent with.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should row variables be user-nameable (any identifier) or use a reserved set (`e`, `r`,
   `eff`)? Koka allows any lowercase identifier. Flux should match.
2. Does `with e` on a top-level `public fn` require the caller to supply a concrete row,
   or is it always universally quantified? Decision: universally quantified (∀e. ...).
3. Row variable scope: are row variables scoped to one function signature or can they
   appear across multiple parameters? Decision: one signature, one variable (linear scope).

## Future possibilities
[future-possibilities]: #future-possibilities

- **Absence constraints** (`\e` meaning "e does not contain IO"): useful for enforcing
  pure-callback APIs. Deferred to a follow-up proposal.
- **Named effect rows** (`type HttpRow = IO + Network`): type aliases for common effect
  combinations. Natural extension once row variables are stable.
- **Effect inference for top-level functions**: infer the minimal effect row automatically,
  display it in error messages and documentation. Builds on this proposal's unification
  infrastructure.
