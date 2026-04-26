- Feature Name: Pattern Exhaustiveness and Redundancy Checking
- Start Date: 2026-04-20
- Status: Implemented (2026-04-22)
- Proposal PR:
- Flux Issue:
- Depends on: [0152](0152_named_fields_for_data_types.md) where applicable for named constructor patterns, current HM/Core pipeline

# Proposal 0166: Pattern Exhaustiveness and Redundancy Checking

## Implementation Summary

Landed in a five-step incremental rollout:

1. **Matrix coverage module** — `src/ast/type_infer/pattern_coverage.rs`: Maranget-style `is_useful` + `missing_witnesses` with normalized `Pat`/`Ctor`/`TyShape` types; 13 unit tests.
2. **Adapter + warning-mode wiring** — `src/ast/type_infer/pattern_coverage_adapter.rs`: AST `Pattern` → `Pat` and `InferType` → `TyShape` translation for Bool / Option / Either / List / Tuple / ADT. Initially gated behind `FLUX_COVERAGE_WARN=1`.
3. **Default-on warnings + ADT field-type threading** — `AdtResolver::lookup_adt` returns full `(name, Vec<TypeExpr>)` so nested ADT exhaustiveness is precise; cycle-guard for recursive ADTs via a `visiting` stack; promoted from env-var-gated to default-on.
4. **Tuple heuristic relaxation** — `src/compiler/expression.rs` `GeneralCoverageDomain::Tuple` accepts a full positional destructure (every slot wildcard/identifier) as structurally exhaustive. `Flow.List` stdlib drops 6 defensive `_ ->` arms.
5. **Decommission + promotion** — Deleted ~755 lines of ad-hoc checks from `src/compiler/expression.rs` (`check_match_exhaustiveness`, `check_adt_match_exhaustiveness`, `check_general_match_exhaustiveness`, `check_nested_constructor_exhaustiveness`, `collect_unreachable_arm_warnings`, `infer_general_match_domain`, `domain_from_*`, `pattern_subsumes`, `literals_equal`, `is_irrefutable_pattern`, the `GeneralCoverageDomain` enum). Promoted the matrix checker's non-exhaustive diagnostic from Warning → **E015 Error** (reusing the existing code). Redundant-arm diagnostic remains a Warning. Orphaned `E083 ADT_NON_EXHAUSTIVE_MATCH`, `W202 UNREACHABLE_PATTERN_ARM`, and the `guarded_wildcard_non_exhaustive` helper were removed.

**Files**
- Checker: `src/ast/type_infer/pattern_coverage.rs`
- Adapter: `src/ast/type_infer/pattern_coverage_adapter.rs`
- Wire-up: `src/ast/type_infer/expression/control_flow.rs` (`experimental_coverage_check` + helpers)
- Incidental fix: `src/compiler/passes/finalization.rs` — warnings no longer cascade as module-skip errors.

**Coverage domains handled**
- Bool (true/false)
- Option<T> (None/Some)
- Either<L, R> (Left/Right)
- List<T> (Nil/Cons, recursion-safe)
- Tuple (single constructor, arity-aware)
- User ADTs including named-field patterns (normalized positionally)
- Nested constructors (e.g. `Some(Circle(r))` on `Option<Shape>`)
- Literal heads on opaque (Int/String) domains — correctly flagged as never-exhaustive without `_`
- Recursive ADTs terminate via `visiting` cycle-guard

**Guards**: conservative — a guarded arm contributes no structural coverage, matching the proposal's v1 scope.

**User-visible error codes**
- `error[E015] Non-Exhaustive Match` — single unified code (previously split across E015 and E083)
- `warning: Redundant Match Arm` — unreachable arm, stays a warning

## Summary
[summary]: #summary

Add a real static coverage checker for `match` expressions so Flux reports non-exhaustive matches and redundant arms instead of relying on a trailing wildcard or falling through to runtime behavior. The checker will be type-directed, constructor-aware, and integrated into the maintained HM/Core pipeline. Scope includes algebraic data types, tuples, literals, list forms already modeled in the syntax, and named-constructor patterns where the constructor set is known.

## Motivation
[motivation]: #motivation

### Today: exhaustiveness is effectively a stub

Flux currently has the machinery to parse and lower rich patterns:

- ADT constructor patterns
- tuple patterns
- `Some` / `None`
- `Left` / `Right`
- cons / empty-list forms
- named-constructor patterns

But the exhaustiveness story is still shallow. The current behavior is close to:

- a trailing wildcard or identifier binder is accepted as “good enough”
- missing constructors are not reliably flagged
- dead arms are not reported

This leaves a major correctness hole in an otherwise strong static type system:

- adding a new variant to an ADT can silently invalidate existing matches
- rearranging arms can make code unreachable without warning
- the user only discovers some failures dynamically, despite having a typed `match`

### Why this matters now

Flux is already strong enough in other parts of its type system that pattern matching stands out as the largest remaining static gap:

- rank-1 HM inference is real, not toy-level
- type classes and dictionary elaboration already exist
- row-polymorphic effects are implemented
- `Dynamic`/`Any` have been removed from maintained Core paths

That means the absence of real exhaustiveness checking is not a minor nicety. It is the biggest correctness hole still visible at the source-language level.

### Concrete failures this proposal should prevent

#### Missing ADT constructor

```flux
data Shape =
    Circle(Float)
  | Rect(Float, Float)

fn area(s) {
    match s {
        Circle(r) -> 3.14 * r * r
    }
}
```

This must fail statically because `Rect` is uncovered.

#### Redundant arm

```flux
fn f(x) {
    match x {
        _ -> 1
        0 -> 2
    }
}
```

The `0 -> 2` arm is dead and should be diagnosed.

#### List shape hole

```flux
fn head_or_zero(xs) {
    match xs {
        [x | _] -> x
    }
}
```

This must be reported as non-exhaustive because `[]` is uncovered.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The user model

After this proposal:

- every `match` is checked against the scrutinee type
- if some cases are missing, Flux reports a non-exhaustive pattern error
- if an arm can never be reached because earlier arms already cover it, Flux reports a redundant arm diagnostic

The mental model is simple:

- a `match` must cover all values of the scrutinee type unless you intentionally write a wildcard arm
- arm order matters
- a later arm that cannot add coverage is a mistake

### Example: ADT exhaustiveness

```flux
data MaybeInt =
    No
  | Yes(Int)

fn unwrap_or_zero(x) {
    match x {
        Yes(n) -> n
    }
}
```

Flux should report something like:

```text
error[E4xx]: Non-Exhaustive Match

This match does not cover all constructors of `MaybeInt`.

  missing patterns:
    No
```

### Example: redundant arm

```flux
fn classify(x) {
    match x {
        _ -> "anything"
        1 -> "one"
    }
}
```

Flux should report:

```text
warning[E4xx]: Redundant Match Arm

This arm can never be reached because previous arms already cover all remaining cases.
```

### Example: partial wildcard remains allowed

```flux
fn area(s) {
    match s {
        Circle(r) -> 3.14 * r * r
        _ -> 0.0
    }
}
```

This is exhaustive and should compile.

### How users should think about it

Users should treat `match` as a checked case split over a known type, not as a chain of ad hoc runtime tests.

That improves:

- refactoring safety
- ADT evolution safety
- readability
- trust in static typing

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

## Scope

The checker covers source-level `Expression::Match` and reasons over the HM-inferred scrutinee type plus the normalized pattern matrix.

Initial supported coverage domains:

- booleans
- `Option`
- `Either`
- tuples
- lists via `EmptyList` and `Cons`
- ADT constructors where the constructor set is known
- literals for finite domains where representation is already explicit (`Bool`, `None`, unit-like constructors)

The checker does **not** attempt full theorem-proving over guards.

Guards are treated conservatively:

- a guarded arm does not fully cover its structural pattern unless an unguarded equivalent also exists

## Checker architecture

Introduce a dedicated pattern coverage module, for example:

- `src/ast/type_infer/pattern_coverage.rs`

or:

- `src/compiler/pattern_coverage.rs`

The checker should operate after HM inference has produced scrutinee types and before maintained lowering proceeds far enough that source-pattern structure is lost.

The core internal model should include:

- a normalized pattern representation
- a notion of pattern constructor head
- a matrix of arms
- a recursive usefulness / missing-pattern computation

This should be implemented as a standard matrix-based coverage checker:

- `is_useful(matrix, vector)` for redundancy
- `missing(matrix, scrutinee_ty)` or equivalent witness generation for non-exhaustiveness

## Integration point

Run the checker as part of the maintained semantic validation flow, not as an AST-only side path.

The diagnostics should be emitted before Core lowering but after enough HM information is available to know:

- the scrutinee type
- the constructor family
- tuple arity
- whether the match is over `Option`, `Either`, list, or a user ADT

## Diagnostics

Introduce two dedicated diagnostics:

- `E4xx Non-Exhaustive Match`
- `W2xx` or `E4xx Redundant Match Arm`

Final codes should align with the existing registry conventions.

Non-exhaustive diagnostics should include:

- the scrutinee type when known
- one or more witness patterns that are missing

Redundant diagnostics should point to:

- the first unreachable arm
- the reason it is covered by prior arms

## Named-constructor patterns

Named-constructor patterns should be normalized to the same positional constructor space already used by lowering. Coverage works over constructors, not field labels.

Example:

```flux
Point { x, y }
```

and

```flux
Point(a, b)
```

must map to the same constructor head for coverage purposes.

## Guards

Guarded arms remain conservative in v1.

Example:

```flux
match x {
    Some(n) if n > 0 -> ...
}
```

This does not exhaust `Some(_)`; it only contributes partial structural coverage.

## Effects on lowering and runtime

No runtime changes are required.

No Core IR changes are required.

This proposal is a semantic-validation improvement:

- same source
- same Core representation
- better rejection and warning before lowering continues

## Drawbacks
[drawbacks]: #drawbacks

- More compiler complexity in the front-end validation layer.
- More diagnostics may initially require fixture churn.
- Guard handling in v1 is intentionally conservative, so some users may want more precision than the first version provides.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why now

This closes the most obvious remaining correctness hole in Flux source typing without requiring architectural change.

### Why a dedicated proposal

Pattern coverage is large enough to deserve its own acceptance criteria and diagnostics work. It should not be hidden inside a general “typing improvements” bucket.

### Why not leave it to runtime

That would contradict the rest of Flux’s direction. `match` over typed ADTs should be statically meaningful, not just syntactic sugar over dynamic branching.

### Why not a warning only

Non-exhaustiveness is a correctness bug, not merely style. Redundancy can be a warning; missing coverage should be an error.

## Prior art
[prior-art]: #prior-art

Most typed ML-family languages perform constructor-based exhaustiveness and redundancy checking for pattern matching. The design space is well understood:

- matrix-based coverage checking
- constructor splitting by scrutinee type
- conservative handling of guards

Flux should follow that family of approaches while remaining aligned with its current AST/HM/Core pipeline.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Final diagnostic codes for non-exhaustive and redundant arms.
- Whether redundant arms should be warnings or hard errors in strict mode.
- How many missing-pattern witnesses should be shown by default.
- Whether finite literal coverage beyond `Bool` should be included in v1.

## Future possibilities
[future-possibilities]: #future-possibilities

- More precise guard reasoning.
- Coverage-aware IDE hints and quick fixes.
- Constructor-evolution guidance when an ADT gains a new variant.
- Coverage checking for effect-handler arm sets if Flux eventually treats them as exhaustiveness domains.
