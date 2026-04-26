- Feature Name: Auto-Currying and Placeholder Partial Application
- Start Date: 2026-02-26
- Last Updated: 2026-04-24
- Status: Not Implemented
- Proposal PR:
- Flux Issue:
- Closes: Lane D of [0063 True FP Completion Program](0063_true_fp_completion_program.md) (together with already-implemented [0145 Type Classes](0145_type_classes.md))

# Proposal 0052: Auto-Currying and Placeholder Partial Application

## Summary
[summary]: #summary

Add two complementary forms of partial application to Flux:

1. **Under-application** of a fixed-arity callable `f(a1, ..., am)` where `m < N`
   returns a closure waiting for the remaining `N - m` arguments.
2. **Placeholder application** of `f(a1, _, a3)` returns a closure over the
   holes in left-to-right order.

Both forms lower to ordinary lambdas during AST-to-Core transformation. No
runtime representation changes, no new value kind, no effect-system changes.

## Motivation
[motivation]: #motivation

Flux today forces a manual lambda for every partial application:

```flux
let inc = \x -> add(1, x)
let evens = filter(\x -> eq(mod(x, 2), 0), xs)
```

The same code with this proposal:

```flux
let inc  = add(1, _)
let evens = filter(eq(mod(_, 2), 0), xs)
```

This is the single largest remaining ergonomic gap in Flux's FP surface after
type classes landed. It is called out directly by the Okasaki-style stdlib work
(persistent structures are consumed through pipelines of partially-applied
folds/maps) and is the last open feature in Lane D of 0063.

## Design goals
[design-goals]: #design-goals

1. **Desugar to lambdas.** No runtime semantics change. A partial application
   is exactly `\x1 ... xk -> f(..., x1, ..., xk, ...)` with the holes filled.
2. **Uniform across callable kinds.** User fns, lambdas, module-qualified fns,
   Base functions, primops, and class-method dispatch all behave identically.
3. **Principal typing.** Partial application preserves the principal type and
   effect row of `f`. Effects are carried by the *returned closure's arrow*,
   not performed at the partial-application site.
4. **No ambiguity with pattern wildcard `_`.** The parser disambiguates by
   context: in a call-argument position `_` is a placeholder; elsewhere it
   remains the pattern/ignore wildcard.
5. **No new diagnostics regressions.** Under-application currently errors as
   "wrong arity"; after this proposal it type-checks. Tests must be updated,
   not deleted — the error is replaced with a successful inference.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Under-application

```flux
fn add(x: Int, y: Int) -> Int { x + y }

let inc: Int -> Int = add(1)        // waiting for y
let three = inc(2)                  // 3
```

### Placeholder application

```flux
fn sub(x: Int, y: Int) -> Int { x - y }

let from_10: Int -> Int = sub(10, _)    // holds x = 10, waits for y
let minus_5: Int -> Int = sub(_, 5)     // waits for x, holds y = 5
```

### Multiple placeholders

Holes are filled strictly **left-to-right** in the order they appear:

```flux
fn triple(a: Int, b: Int, c: Int) -> Int { a + b + c }

let f = triple(_, 10, _)       // f : (Int, Int) -> Int
f(1, 2) == 1 + 10 + 2          // 13
```

### With effects

```flux
fn log(prefix: String, msg: String) with IO -> () { println(prefix ++ msg) }

let warn = log("[warn] ", _)   // warn : String -> () with IO
warn("disk full")              // performs IO at the call of warn, not at log("[warn] ", _)
```

The effect is on the *returned closure's arrow*. Building the partial
application is pure.

### What is explicitly not in scope

- **No point-free composition operator.** This proposal does not add `(.)` or
  `<<`. Partial application gives 90% of point-free value without it.
- **No right-section syntax.** `(+1)` / `(1+)` style sugar is not added;
  placeholders subsume the use case uniformly: `add(_, 1)` / `add(1, _)`.
- **No variadic / rest-argument support.** All callables have fixed compile-
  time arity in this proposal.
- **No new effect behavior.** Partial application does not perform, install,
  or discharge any effect.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Arity model

Every callable `f` has a compile-time declared arity `N ≥ 0`:

| Callable kind | Arity source |
|---|---|
| Named `fn` | parameter list |
| Lambda `\x y -> ...` | parameter list |
| Module-qualified `M.f` | parameter list of the definition |
| Base / stdlib fn | parameter list |
| Primop | fixed in `CorePrimOp` dispatch table |
| Class method (monomorphized) | parameter list of resolved instance |
| Class method (polymorphic dict-passing) | parameter list after dict insertion |

Variadic callables do not exist in Flux and are out of scope.

### Call normalization

For a call `f(a1, ..., am)` where `f` has declared arity `N`:

1. Build a **template** of length `N`, initially all holes.
2. Fill template positions `1..m` in order with `a1..am`. Each `ai` is either
   a value-expression or a placeholder `_`.
3. Count remaining holes `k`:
   - Positions `m+1..N` (unsupplied tail) — always holes.
   - Any explicit `_` placeholders among `a1..am`.
4. If `k == 0`: emit a direct call `App(f, [a1..am])` as today.
5. If `k > 0`: emit a synthesized lambda
   ```
   \$h1 ... $hk -> App(f, [ template with $hi substituted left-to-right ])
   ```
   where each `$hi` is a fresh symbol gensym'd by the desugar pass.
6. If `m > N`: error `E469` (over-application).

### Parser and AST

Add one new expression node:

```rust
enum Expr {
    ...
    ArgPlaceholder(Span),   // `_` appearing in a call-argument position
}
```

Parser changes (recursive-descent call-args rule only):

- In `parse_call_args`, if the next token is `_`, emit `ArgPlaceholder(span)`.
- Everywhere else, `_` continues to parse as the existing pattern / binding-
  wildcard token. No change.

`ArgPlaceholder` outside a call-argument position is rejected in the desugar
pass with `E470`.

### Desugar pass

A new AST-to-AST pass `desugar_partial_app` runs in [src/ast/](../../src/ast/)
before type inference. For every call expression:

- Compute `N` from the resolved callable (may be deferred to type inference
  for callees whose arity is unknown syntactically — see "Deferred arity"
  below).
- If either (a) `m < N` or (b) any arg is `ArgPlaceholder`: rewrite to the
  synthesized lambda described in "Call normalization".

The rewrite produces ordinary AST — nothing downstream (HM, Core lowering,
Aether, codegen) needs to know partial application existed.

### Deferred arity

When the callee is not syntactically resolved (higher-order arg, dict-passed
method, local `let f = g` where `g` is generic), arity is not known at
desugar time. For those cases:

- If no `_` placeholders appear and `m` matches the inferred function type's
  arity after HM, it is a regular call — no desugar needed.
- If `m` differs or `_` appears, desugar runs as a **post-HM** fix-up: a
  second pass on AST nodes that were flagged `needs_arity` during desugar
  builds the lambda using the inferred arity.

Implementation detail: flag the call node with `PartialAppCandidate` in the
first desugar pass; resolve it in a second sweep after inference. This
avoids threading inference state into the main desugar pass.

### Typing

`f : (T1, T2, ..., TN) -> R with E` applied as `f(a1, _, a3)` where
`a1 : T1` and `a3 : T3` has type:

```
T2 -> R with E
```

The returned arrow carries `E`; the partial-application site itself is pure.

Constraints and row variables propagate normally through the synthesized
lambda — no new inference rules are needed because the lambda is real AST.

### Interaction with type classes

For class methods the arity used is the arity **after dictionary insertion**.
Example:

```flux
class Eq a { fn eq(x: a, y: a) -> Bool }

let same_as_zero = eq(0, _)   // Int -> Bool, dict __dict_Eq_Int resolved
```

The dict argument is inserted by [dict_elaborate](../../src/core/passes/dict_elaborate.rs)
after desugar, so user-visible arity stays at 2 for placeholder purposes.

### Interaction with effects

Building a partial application does not perform. This is enforced by desugar:
the synthesized lambda body contains the call, so the call's effects are on
the lambda's arrow, not on the enclosing expression. No special-case in the
effect solver is required.

### Interaction with Aether

The synthesized lambda captures its fixed arguments by value. Aether sees an
ordinary closure creation and inserts `dup` for each captured value that
outlives the partial-application site. No changes to the Aether pass.

## Diagnostics
[diagnostics]: #diagnostics

Three new codes:

| Code | Title | When |
|---|---|---|
| **E469** | `over-application` | `f(a1..am)` with `m > N` for fixed-arity `f`. Replaces the existing generic arity error for the over-application case. |
| **E470** | `placeholder-outside-call` | `_` used as a placeholder expression outside of a call-argument position (e.g. `let x = _`, `return _`). |
| **E471** | `placeholder-in-variadic-position` | Reserved. Currently unreachable (no variadics); defined so downstream syntax extensions have a slot. |

The existing "arity mismatch — expected N, got m" diagnostic is narrowed to
fire only for `m > N`. The `m < N` case becomes successful inference.

## Implementation plan
[implementation-plan]: #implementation-plan

### Phase A — Parser + AST (small)

- [ ] Add `Expr::ArgPlaceholder(Span)` in [src/ast/mod.rs](../../src/ast/mod.rs).
- [ ] Extend the call-args parser to recognize `_`.
- [ ] Parser tests: `_` inside call args parses as placeholder; outside as wildcard.

### Phase B — Desugar pass

- [ ] Implement `desugar_partial_app` at [src/ast/](../../src/ast/).
- [ ] Run the pass before type inference.
- [ ] Emit `E469` for over-application, `E470` for bare `_`.
- [ ] Snapshot tests comparing desugared AST for representative cases.

### Phase C — Deferred-arity fix-up

- [ ] Flag unresolvable calls as `PartialAppCandidate`.
- [ ] Resolve in a second sweep after HM inference.
- [ ] Test matrix: higher-order args, dict-passed class methods, local
      `let`-bound references.

### Phase D — Diagnostics migration

- [ ] Register `E469`, `E470`, `E471` in [src/diagnostics/registry.rs](../../src/diagnostics/registry.rs).
- [ ] Narrow the existing generic arity diagnostic to `m > N` only.
- [ ] Update snapshots for tests whose `m < N` errors disappear.

### Phase E — Parity + corpus

- [ ] `tests/parity/partial_app/` fixtures: under-application, single
      placeholder, multiple placeholders, placeholder with effects,
      placeholder on class method.
- [ ] Run `parity-check tests/parity/partial_app` on both VM and LLVM.
- [ ] Add an `examples/basics/partial_application.flx` example.

No Aether, Core, CFG, LIR, bytecode, LLVM, or runtime changes are required.

## Drawbacks
[drawbacks]: #drawbacks

1. **`_` disambiguation is context-sensitive.** Parsers that don't track
   context get subtly wrong errors. Mitigated by confining `_`-as-placeholder
   strictly to call-argument positions and rejecting it everywhere else with
   `E470`.
2. **Deferred arity adds a second pass.** Mild complexity cost for the
   compiler, invisible to users. Mitigated by scoping the second pass to
   flagged nodes only.
3. **Lost "arity mismatch on under-application" diagnostic.** Some programs
   that previously errored will silently type-check as partial applications.
   For a typed FP language this is conventionally the right default, but it
   is a user-visible behavior change worth calling out in release notes.
4. **Effect-row surprise risk.** New users may expect `log("x", _)` to
   perform `IO` immediately. Mitigated by the guide chapter's explicit
   "effect is on the returned arrow" paragraph.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why auto-curry + placeholder instead of real currying?

Real left-to-right currying (`add : Int -> Int -> Int`) forces every function
to be syntactically curried, which clashes with Flux's `fn name(x, y)`
surface and with tuple-based ADT constructors. Desugaring preserves the
multi-arg surface and gives the full ergonomic benefit at the call site. This
matches OCaml's decision space (OCaml has real currying; we have a multi-arg
surface with placeholder sugar, and users land in roughly the same place).

### Why not only under-application?

Under-application only covers *trailing* holes. Placeholders are needed for
middle and leading holes (`sub(10, _)` vs. `sub(_, 5)`). Shipping only
under-application pushes users back to manual lambdas for the non-trailing
case, which is the more common one in pipeline code.

### Why not a point-free operator?

Composition operators (`.`, `<<`) interact poorly with multi-arg functions
and effect rows (which composition chain's effects bubble up?). Placeholders
make composition explicit:

```flux
let pipeline = \x -> g(f(x, 1), 2)      // clearer than compose(g(_, 2), f(_, 1))
```

If composition sugar is wanted later, it is a separate proposal.

### Why lower to lambdas instead of a runtime partial-application object?

- Zero runtime cost over what users would write by hand.
- No new value kind, no VM opcode, no LLVM emission path.
- Aether/Perceus reasons about closures already; partial applications inherit
  that reasoning for free.
- Debuggers and traces see ordinary closures, not an opaque partial-app type.

## Prior art
[prior-art]: #prior-art

- **Scala** `_` placeholder syntax — direct inspiration for the middle/leading-
  hole case. Scala's rules on where `_` is valid inform the `E470` scope.
- **OCaml / F# / Elm** auto-currying — inspiration for under-application;
  Flux diverges by keeping a multi-arg surface and desugaring.
- **Clojure** `#(f % %2)` — an alternative syntax for the same goal. Rejected
  as visually noisy and not idiomatic for a Rust-family surface.
- **Haskell sections** `(+1)` — elegant but only covers binary operators.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **`_` in nested call args.** `f(g(_, 1), 2)` — does the `_` bind to the
   *inner* call `g` or the outer `f`? Recommendation: bind to the
   innermost enclosing call, matching Scala. Users wanting outer binding
   write an explicit lambda.
2. **Effect floor on the synthesized lambda.** The returned arrow inherits
   `E` from `f`. Should this be a named method on a class with a `with`
   clause effect floor, does the floor apply to the synthesized arrow too?
   Recommendation: yes — same rule as ordinary lambdas.
3. **Anonymous-field access placeholder.** `_.name` for record-field getters
   is a tempting extension. Out of scope for this proposal; track separately.

## Future possibilities
[future-possibilities]: #future-possibilities

- `_.field` field-accessor shorthand as a follow-up.
- Operator sections (`(+1)`, `(1+)`) as sugar over placeholders.
- `|>` pipeline integration (`xs |> filter(_ > 0) |> map(add(1, _))`) — works
  already once this proposal lands, but worth an example-guide chapter.
