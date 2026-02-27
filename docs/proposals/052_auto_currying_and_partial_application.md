# Proposal 052: Auto-Currying and Placeholder Partial Application

**Status:** Draft  
**Date:** 2026-02-26  
**Depends on:** `032_type_system_with_effects.md`, `043_pure_flux_checklist.md`, `046_typed_ast_hm_architecture.md`, `027_language_syntax_specification.md`

---

## 1. Status/Header Metadata

This proposal is execution-grade and implementation-oriented. It supersedes the high-level currying vision text in `025_pure_fp_language_vision.md` for delivery decisions, while keeping `025` as a language-direction document.
Roadmap note: this is a stage-2 (post-0.0.4) track under
`054_0_0_4_hm_adt_exhaustiveness_critical_path.md`.

---

## 2. Motivation and Current Pain

Flux currently requires manual lambdas for partial application:

```flux
let inc = \x -> add(1, x)
```

This creates avoidable verbosity in higher-order code and makes point-free/pipe-heavy style less ergonomic than intended. The missing behavior affects:

1. core HOF usage (`map`, `filter`, wrappers),
2. module-qualified function APIs,
3. effect-polymorphic callbacks (`with e`),
4. consistency between language syntax vision and implemented semantics.

Goal: make partial application first-class, deterministic, and type/effect-safe across all callable kinds.

---

## 3. Semantics (Normative)

### 3.1 Callable kinds in scope

The rules in this proposal apply uniformly to:

1. user-defined named functions,
2. lambdas/function expressions,
3. module-qualified functions,
4. builtins/base functions,
5. primops (through callable wrappers where required by lowering/runtime path).

### 3.2 Arity model

Each callable has fixed declared arity `N` at compile time where known.

### 3.3 Call normalization

For `f(a1, ..., am)` where `m <= N`:

1. Create an argument template of length `N`.
2. Fill the first `m` slots with provided call arguments (including placeholders).
3. Fill remaining slots (`m+1..N`) with implicit holes.

Normalization result:

1. No holes remain: execute call immediately.
2. One or more holes remain: return partial callable waiting for the unresolved slots.

### 3.4 Fill order

When invoking a partial callable with new args, holes are filled strictly left-to-right by template slot index.

### 3.5 Over-application

If `m > N` and arity is known in typed-known paths, emit compile-time arity diagnostic (new currying-specific diagnostic class).

Non-typed/dynamic fallback keeps current permissive behavior policy only where callable arity is genuinely unknown at compile time.

### 3.6 Equivalences

The implementation must preserve these equivalences:

1. `add(1)` == `\x -> add(1, x)`
2. `div(_, 2)` == `\x -> div(x, 2)`
3. `f(1)(2)` == `f(1, 2)` when arity/placeholder layout aligns.

---

## 4. Placeholder Semantics (Normative)

### 4.1 Valid position

`_` as placeholder is valid only in function-call argument position.

Allowed:

```flux
div(_, 2)
replace(_, "x", "y")
f(1, _, 3)
```

Not allowed:

```flux
let bad = _
```

### 4.2 Placeholder behavior

`_` reserves a hole in that slot. The hole becomes a parameter of the returned partial callable.

### 4.3 Mixed placeholders and omission

Both are supported together:

- `f(1, _, 3)` with arity 4 yields holes at slots 2 and 4.
- `f(_, _)` is valid if arity is at least 2.

### 4.4 Pattern wildcard compatibility

Pattern wildcard `_` behavior is unchanged. Placeholder `_` in call args is a distinct parser/AST form.

---

## 5. Type System/HM Integration

### 5.1 Type transformation

For `f : (A, B, C) -> R`:

1. `f(a)` => `(B, C) -> R` (after unification of `a` with `A`)
2. `f(_, b)` => `(A, C) -> R` (after unification of `b` with `B`)

### 5.2 Placeholder typing

Each placeholder introduces a fresh HM variable constrained by the corresponding parameter slot type.

### 5.3 Error policy

Typed mismatch remains `E300` (no new mismatch code for type conflict).

### 5.4 Generic instantiation

For polymorphic callables, instantiate scheme per call site before partial-type reduction; returned partial callable carries instantiated residual function type.

---

## 6. Effects Integration (`with e`)

### 6.1 Effect preservation

Partial callables preserve the effect row of their source callable.

Example:

- if `f : (A, B) -> R with IO`, then `f(a)` has type `(B) -> R with IO`.

### 6.2 Effect polymorphism

`with e` remains polymorphic through partials:

- wrapping and reapplying a partially applied callback must preserve row constraints and propagation behavior.

### 6.3 No runtime effect redesign

This proposal does not change effect runtime model; it only extends callable application semantics while reusing current compile-time effect enforcement.

---

## 7. Parser/AST Changes

### 7.1 Grammar extension

Extend call argument grammar to include explicit placeholder token in argument position.

### 7.2 AST shape

Represent placeholder as dedicated AST node/enum variant for call arguments (not `Identifier("_")`).

### 7.3 Parsing guardrails

1. `_` outside call argument position is parser/validation error.
2. Pattern wildcard `_` stays in pattern grammar unchanged.

### 7.4 Formatter/display note

AST display may normalize formatting, but placeholder-presence must round-trip semantically.

---

## 8. Bytecode/Runtime Model

Define one partial-call runtime representation, conceptually:

```rust
Value::Partial {
    callee,
    arity,
    filled_slots,
}
```

Where:

1. `filled_slots` tracks concrete values vs holes,
2. invocation fills holes left-to-right,
3. if all slots filled => call executes,
4. otherwise => returns next `Value::Partial`.

Builtins/base functions and primops participate via same callable abstraction contract at call boundary (no user-visible distinction).

---

## 9. Diagnostics Contract

### 9.1 Existing codes unchanged

1. Typed unification mismatch: `E300`
2. Effect boundary mismatch: `E400` family
3. Runtime boundary mismatch: `E055`

### 9.2 New currying-specific diagnostics

Add dedicated diagnostics (new codes to be assigned via registry update):

1. placeholder outside call arguments,
2. arity overflow in typed-known path,
3. invalid placeholder context/layout (if parser accepts then semantic validation rejects).

Each must include:

1. stable title,
2. single primary label at fault span,
3. actionable hint.

---

## 10. Examples (Pass/Fail)

### 10.1 Pass examples

```flux
fn add(x: Int, y: Int) -> Int { x + y }
let inc = add(1)
let v: Int = inc(5)
```

```flux
fn divi(x: Int, y: Int) -> Int { x / y }
let half = divi(_, 2)
let a: Int = half(10)
```

```flux
fn apply_twice(f: ((Int) -> Int with e), x: Int) -> Int with e { f(f(x)) }
fn log_inc(x: Int) -> Int with IO { print(to_string(x)); x + 1 }
fn main() with IO {
  let g = log_inc(_)
  let out: Int = apply_twice(g, 1)
  print(to_string(out))
}
```

```flux
import TypeSystem.HmGenericModule as M
let id_int = M.id(_)
let x: Int = id_int(7)
```

### 10.2 Fail examples

```flux
let bad = _            // invalid: placeholder outside call arg
```

```flux
fn add(x: Int, y: Int) -> Int { x + y }
let z = add(1, 2, 3)   // arity overflow
```

```flux
fn add(x: Int, y: Int) -> Int { x + y }
let inc = add("x")     // E300
```

---

## 11. Test Matrix + Parity Requirements

### 11.1 Fixture additions

Add dedicated fixtures under:

1. `examples/type_system/` (pass)
2. `examples/type_system/failing/` (fail)

Coverage must include:

1. left-partial application (`add(1)`),
2. placeholder partial (`div(_, 2)`),
3. mixed placeholder+omission (`f(1, _, 3)`),
4. module-qualified partial (`M.id(_)`),
5. effect-polymorphic partial callback (`with e`),
6. invalid placeholder placement,
7. typed-known arity overflow,
8. typed mismatch via partial-call result.

### 11.2 Compiler tests

1. parser tests for placeholder legality,
2. HM inference tests for residual function typing,
3. call lowering tests for partial callable materialization,
4. diagnostics tests for new currying-specific errors.

### 11.3 VM/JIT parity

Add parity fixtures into curated diagnostics parity suite. Tuple parity remains:

- code + title + primary label.

---

## 12. Rollout Plan (Phased)

### Phase A: Syntax + AST

1. parse placeholder in call args,
2. represent placeholder as dedicated AST form,
3. reject placeholder outside call args.

### Phase B: Runtime partial callable representation

1. introduce partial callable value representation,
2. implement hole-filling and re-invocation semantics.

### Phase C: Compiler lowering + call normalization

1. normalize calls into template/hole model,
2. lower full calls vs partial returns.

### Phase D: HM/effect integration

1. infer residual function types,
2. preserve effect rows and `with e` propagation.

### Phase E: Diagnostics hardening

1. implement new currying-specific diagnostics,
2. ensure `E300`/`E400`/`E055` boundaries unchanged.

### Phase F: Fixtures, parity, docs

1. add pass/fail fixtures and tests,
2. update parity snapshots intentionally,
3. update READMEs and guide references.

---

## 13. Risks and Mitigations

1. Risk: ambiguity between wildcard and placeholder `_`.
   - Mitigation: distinct parser contexts + AST forms.
2. Risk: arity behavior drift for builtins/primops.
   - Mitigation: single callable normalization path and dedicated lowering tests.
3. Risk: HM regressions for generic partial calls.
   - Mitigation: targeted generic fixtures and strict inference tests.
4. Risk: parity drift in diagnostics.
   - Mitigation: add focused parity snapshots for currying failure cases.

---

## 14. Acceptance Criteria

1. `add(1)` and `div(_, 2)` compile and execute as specified.
2. All callable kinds use one currying/partial model.
3. HM infers residual function types deterministically.
4. Effect rows are preserved through partial application.
5. VM/JIT parity passes for new diagnostic fixtures.
6. Existing type/effect strict suites remain green.

---

## 15. Explicit Assumptions/Defaults

1. `025` remains vision-level; this proposal is implementation contract.
2. Placeholder support is in initial scope (not deferred).
3. Over-application is compile-time error in typed-known paths.
4. No higher-rank polymorphism is introduced.
5. No runtime effect semantic redesign is included.
