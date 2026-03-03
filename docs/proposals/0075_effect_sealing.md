- Feature Name: Effect Sealing
- Start Date: 2026-03-02
- Proposal PR:
- Flux Issue:

# Proposal 0075: Effect Sealing

## Summary

Add a `sealing` call-site expression that restricts which effects a callee is
allowed to perform, enforced at compile time by the existing effect-row constraint
solver. A sealed call grants the callee exactly the effects named in the seal —
any effect not in the seal is provably absent in the call, producing a compile
error if the callee's declared row requires it.

This turns Flux's effect rows from type-level documentation into type-level
**capability grants**: the caller decides what the callee is allowed to do, not
just what it declares.

```flux
-- Normal call: callee inherits all ambient effects
fetch_data(url)

-- Sealed call: callee may only use Network; Database is forbidden
fetch_data(url) sealing { Network }

-- Algebraic restriction: grant everything the caller has, minus FileSystem
process(payload) sealing (ambient - FileSystem)
```

## Motivation

### The documentation problem

Flux effect rows tell you what a function *does*. They do not let you control
what a function is *allowed to do*. This is fine for code you wrote, but
insufficient for:

- Third-party libraries you import.
- Callbacks passed into higher-order functions.
- Isolated subsystems that should not cross resource boundaries.

Without sealing, an `analytics` library that you believe only prints to the
console could also write to your database, and the type system would not catch
it. You get a promise from the author, not a guarantee from the compiler.

### The testing problem

Testing effectful code in every pure FP language today requires writing and
maintaining **mocks** — manually-authored stand-ins for effectful interfaces.
Mocks drift from the real interface and catch errors only when a test happens to
exercise the mismatch.

Effect sealing makes a richer model possible: rather than mocking, you **capture**
all effect performances as a typed value that can be asserted against. The
capture shape is derived from the effect declaration itself, so a signature change
is a compile error in the test, not a silent regression.

### Specific use cases

**1. Third-party code sandboxing**

```flux
import Analytics

-- If Analytics.track internally performs Database — compile error here.
-- The type system gives a guarantee, not a promise.
Analytics.track(event) sealing { Console }
```

**2. Effect capture for tests (no mocks)**

```flux
effect Database {
    query:  (String) -> List<Row>
    insert: (String, Row) -> Unit
}

fn save_user(user: User) -> Unit with Database + Audit { ... }

test "save_user emits correct effects" {
    let events = capture { Database + Audit } { save_user(test_user) }
    assert events == [
        Database.insert("users", test_user),
        Audit.log("created: " ++ test_user.name)
    ]
}
```

`events` is a typed list of `Database | Audit` variants. Asserting the wrong
operation name or wrong argument shape is a compile error.

**3. Least-privilege subsystem design**

```flux
fn run_report(report_id: Int) -> Report with Database + FileSystem {
    let data   = load_data(report_id)          -- full ambient: Database + FileSystem
    let result = compute(data) sealing { }     -- pure: provably no effects
    save_pdf(result) sealing { FileSystem }    -- only FileSystem; Database sealed out
    result
}
```

This is a compile-time proof that `compute` has no effects, even if the ambient
context has them. It is not just documentation — the seal is checked.

**4. Algebraic capability composition**

```flux
let read_only  = Network + FileSystem.read
let write_cap  = FileSystem.write + Database.write
let minimal    = read_only - Network              -- subtract using existing row algebra

process_report(data) sealing read_only
save_results(output) sealing write_cap
```

Effect sets can be named, composed, and subtracted using the existing `Add` /
`Subtract` algebra of `EffectExpr`. No new algebra is introduced.

## Guide-level explanation

### What sealing means

A `sealing` expression wraps a single call with a capability grant. The grant is
an `EffectExpr` — the same syntax as a `with` annotation. The solver then
verifies two things:

1. Every effect that the callee's declared row requires is present in the seal.
2. No effect that the seal excludes appears in the callee's row.

```flux
fn do_work() -> Unit with IO + Database { ... }

fn caller() -> Unit with IO + Database + Network {
    do_work() sealing { IO + Database }   -- ok: seal covers do_work's row exactly
    do_work() sealing { IO }              -- error: Database required but not granted
    do_work() sealing { IO + Database + Network }  -- ok: seal is a superset
}
```

### The empty seal — proving purity

Sealing with an empty set `{}` asserts that the callee is pure in this call
context, even when the caller is not:

```flux
fn caller() -> Unit with IO {
    let x = pure_fn(42) sealing { }   -- type-checked: pure_fn must have no effects
    ...
}
```

If `pure_fn` acquires an effect annotation later, this call site becomes a
compile error immediately. The seal is a **regression guard** on purity.

### Row-variable seals

The seal expression can include row variables and arithmetic, inheriting the
full `EffectExpr` grammar:

```flux
-- Grant only the row variable's effects (whatever e resolves to at this call site)
f(x) sealing { |e }

-- Grant the ambient minus a specific capability
g(y) sealing (ambient - Database)
```

`ambient` is a built-in keyword referring to the enclosing function's declared
effect row. `ambient - Database` computes the subtraction at compile time using
the existing row normaliser.

### Effect capture (`capture`)

`capture` is the sealing companion for testing. It wraps a block, intercepts
all effect performances for the named effects, and returns them as a typed list
rather than executing real handlers:

```flux
let events: List<Database | Audit> =
    capture { Database + Audit } {
        save_user(test_user)
    }
```

The element type `Database | Audit` is a compiler-generated sum type whose
constructors mirror each operation in the effect declarations. Asserting
`Database.insert("wrong_table", ...)` is a constructor mismatch — a compile
error.

### Error messages

When a seal does not cover a required effect:

```
error[E430]: sealed call missing required effect `Database`
  --> src/users.flx:14:5
   |
14 |     save_user(user) sealing { IO }
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ seal grants: IO
   |                                    required by: save_user -> with IO + Database
   |
   = hint: add `Database` to the seal, or remove the sealing restriction
```

When a callee performs an effect that the seal excludes:

```
error[E431]: callee performs `Network` which is excluded by the seal
  --> src/analytics.flx:7:3
   |
 7 |     Analytics.track(event) sealing { Console }
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ seal excludes: Network
   |                                                performed by: Analytics.track -> with Console + Network
   |
   = hint: either add `Network` to the seal or remove the Network call from Analytics.track
```

## Reference-level explanation

### New AST node

Add a `SealedCall` expression variant:

```rust
// src/syntax/expression.rs
pub enum Expression {
    // ... existing variants ...

    /// A call with a compile-time capability restriction.
    /// `expr sealing { effect_row }`
    SealedCall {
        call:   Box<Expression>,
        seal:   Vec<EffectExpr>,     // same grammar as `with` annotations
        span:   Span,
    },

    /// Capture all effect performances for the named effects as a typed list.
    /// `capture { effect_row } { block }`
    EffectCapture {
        effects: Vec<EffectExpr>,
        body:    Box<Expression>,
        span:    Span,
    },
}
```

### Parser

The `sealing` keyword is a postfix modifier on any call expression:

```
sealed_call := call_expr "sealing" "{" effect_row "}"
             | call_expr "sealing" "(" effect_expr ")"  -- for arithmetic forms
effect_row  := (effect_atom ("+" effect_atom)* ("-" effect_atom)* ("|" IDENT)?)?
```

`ambient` inside a seal expression is resolved at parse time to the enclosing
function's declared `with` clause. If no `with` clause is present, `ambient`
is the empty row.

### Constraint solver integration

`SealedCall` is resolved during PASS 2 of compilation. For a call
`f(args) sealing S` where `f` has declared effect row `R`:

1. **Coverage check**: Emit `Subset(R, S)` — every atom in `R` must be in `S`.
   Uses the existing `RowConstraint::Subset` path in
   `src/bytecode/compiler/effect_rows.rs`.

2. **Exclusion check**: For every atom `a` in the callee's ambient context that
   is NOT in `S`, emit `Absent(R, a)` — `a` must not appear in `R`.
   Uses the existing `RowConstraint::Absent` path, including deferred
   evaluation for multi-var scenarios.

3. **Row variable restriction**: If `S` contains row variables, they are linked
   to the callee's row variables via `RowConstraint::Eq`. If `S` is a closed
   row and `R` has an open tail `|e`, `e` is constrained to be a subset of `S`
   via `Subset`.

No new constraint types are required. The two existing constraint kinds
(`Subset` and `Absent`) fully express sealing semantics.

### Effect capture implementation

`EffectCapture` compiles to a synthetic handler block at the bytecode level.
For each operation `op` in each named effect `E`, the compiler generates a
handler arm that appends `E.op(args...)` to an accumulator list and resumes
with a neutral value. The resulting list is the value of the `capture` block.

```rust
// Conceptual lowering:
//   capture { Database } { save_user(user) }
// becomes:
//   let __log = []
//   save_user(user) handle Database {
//       query(resume, sql) -> resume([], __log.append(Database.query(sql)))
//       insert(resume, tbl, row) -> resume((), __log.append(Database.insert(tbl, row)))
//   }
//   __log
```

The `Database.query` and `Database.insert` constructors are compiler-generated
from the effect declaration. Their types are verified against the effect
signature at the capture site.

### New error codes

| Code | Title | Trigger |
|------|-------|---------|
| E430 | `sealed_call_missing_effect` | Seal does not cover a required effect in callee's row |
| E431 | `sealed_call_excluded_effect` | Callee declares or performs an effect excluded by the seal |
| E432 | `capture_effect_not_declared` | `capture` names an effect that has no declaration in scope |

These follow the existing diagnostic infrastructure in
`src/diagnostics/registry.rs` and `src/diagnostics/compiler_errors.rs`.

### Interaction with `|e` row variables (Proposal 0064)

A sealed call to a row-polymorphic function unifies the row variable with the
seal. If `f` has `with IO | e` and the call is `f(x) sealing { IO }`, then
`e` is solved to the empty row. If the call is `f(x) sealing { IO + Database }`,
`e` is solved to `{Database}`. This is standard row unification — no new
machinery.

### Interaction with gradual typing

Calls without a `sealing` clause behave exactly as today — the ambient effect
context is inherited and no new checks are added. Sealing is opt-in. Untyped
code paths (using `Any`-typed functions) that lack effect annotations are not
subject to seal checks, preserving gradual typing semantics.

### Interaction with JIT backend

`SealedCall` desugars to a regular call plus constraint-check opcodes before
PASS 2 emits bytecode. The JIT path (`src/jit/compiler.rs`) sees the same
desugared call and is unaffected structurally. The exclusion-check is a
compile-time-only artefact — no runtime overhead is introduced.

## Drawbacks

1. **New keyword `sealing`** — one reserved identifier added to the grammar.
   `ambient` as a contextual keyword inside seal expressions must not conflict
   with user-defined identifiers named `ambient`.

2. **Seal drift** — if a callee's effect row grows (e.g., a library update adds
   `Network`), sealed call sites that exclude `Network` become compile errors.
   This is intentional (the seal is a regression guard) but may be surprising
   during library upgrades.

3. **Empty-seal purity proofs are fragile** — `f(x) sealing { }` breaks
   the moment any effectful operation is added to `f`. Authors relying on this
   for purity proofs must evolve seals together with implementations.

4. **`ambient` keyword complexity** — the contextual `ambient` form requires the
   compiler to track the enclosing function's declared row through expression
   compilation. This is straightforward given the existing
   `Compiler.current_function_effects` state but adds one more piece of context
   to thread.

## Rationale and alternatives

### Why not library-level mocking?

Library mocks require manually maintaining a parallel interface. As the effect
declaration evolves, mocks must be updated separately. Effect capture derives
the capture type directly from the declaration — they cannot drift.

### Why not handler-based sandboxing?

Handlers already let you intercept effects. The difference is scope: handlers
must be written by the callee's author or by someone who controls the call site's
lexical scope. `sealing` is a **caller-side** capability restriction. The callee
does not need to cooperate.

### Why not a separate capability type?

Some languages (e.g., Wasm component model, capability-safe object systems)
implement capabilities as runtime tokens that must be passed explicitly. This is
expressive but requires threading capability values through every interface.
Sealing uses the effect row that is already being inferred — no new values are
passed. The cost is zero at runtime; the check is entirely compile-time.

### Why `sealing` and not `with` or `restricting`?

`with` is already used for effect annotations on function signatures. Using it
at call sites would create syntactic ambiguity. `sealing` makes the
restriction intent explicit and searchable. Alternative candidates (`restricting`,
`cap`, `grant`) were considered; `sealing` best conveys that you are closing an
open set.

### What is the impact of not doing this?

Effect rows remain documentation. Users can read what a function does but cannot
enforce what it is allowed to do. Third-party library sandboxing is impossible
at the type level. Testing requires hand-written mocks. The effect system has
friction (annotations, row variables, the solver) without the enforcement payoff
that justifies that friction.

## Prior art

**Koka** — Row-polymorphic algebraic effects, the closest prior work. Koka
handlers are lexically scoped and anonymous; there is no mechanism for a caller
to restrict a callee's effects from outside the callee's body. Koka's `mask`
operation hides effects from the callee's perspective, but it does not grant or
verify at the call site.

**Wasm Component Model / WASI** — Capability-based access control at the
module boundary. Capabilities are runtime tokens, not types. No static
verification. No row arithmetic.

**Pony** — Capability-secure object references (`iso`, `trn`, `ref`, `val`,
`box`, `tag`). Capabilities apply to data aliasing and mutation, not to effect
profiles. No algebraic composition.

**Clean** — Uniqueness types enforce single-use of certain resources. The
system is richer than monadic IO but does not support row arithmetic or
caller-side restriction.

**Granule** — A research language with graded types that tracks resource use
quantitatively. Closer to linear types than effect rows; does not have
call-site restriction syntax.

**Frank** — A functional language with ports and commands. Effect handling is
deeply integrated into pattern matching. No caller-side sealing.

None of the above combine (a) algebraic row arithmetic, (b) caller-side
restriction at the call site, and (c) compile-time verification via an existing
constraint solver. The combination is specific to Flux's current architecture.

## Unresolved questions

1. **`ambient` keyword scope** — Should `ambient` be a reserved keyword globally,
   or only contextual inside `sealing { ... }` expressions? A contextual keyword
   avoids polluting the namespace but adds parser complexity.

2. **Seal on non-call expressions** — Should `sealing` be allowed on any
   expression that introduces effects (e.g., a block), or strictly on call
   expressions? Generalising to blocks would cover `do`-style sequences but
   complicates the grammar.

3. **Seal subtyping direction** — Currently, a seal that is a strict *superset*
   of the callee's row is permitted (the callee simply does not use the extra
   granted effects). Should overly-permissive seals emit a warning (`W202:
   seal_grants_unused_effect`) to encourage least-privilege by default?

4. **Capture neutral values** — When `capture` intercepts an effect operation,
   it resumes with a neutral value (e.g., `Unit` for `insert`, `[]` for `query`).
   The neutral value must be type-compatible with the callee's expected return.
   Complex return types (e.g., `Result<Row, Error>`) need a policy for the
   default neutral — possibly `Ok(default_value)` for `Result`. This policy
   needs formalisation.

5. **Error code range** — E430–E432 are proposed. Confirm they do not conflict
   with any codes reserved by Proposals 0060 or 0063 before registration.

## Future possibilities

**Named capability profiles** — Allow users to define reusable seals as
module-level constants:

```flux
cap ReadOnly = Network + FileSystem.read
cap WriteOnly = FileSystem.write + Database.write
```

These would be valid seal expressions wherever an `EffectExpr` is accepted.

**`sealing` on import declarations** — Restrict an entire imported module to a
capability profile at the import boundary:

```flux
import Analytics sealing { Console }
-- All calls into Analytics in this module are implicitly sealed to Console.
```

This is the strongest form of third-party sandboxing: set once at the import
site rather than at every call site.

**Effect capability inference** — Rather than requiring explicit seals, the
compiler could infer the minimal seal for each call site (the exact effects
the callee uses) and warn when the callee acquires new effects that are not
present in the caller's ambient context. This moves toward automatic
least-privilege without requiring any annotation.

**Integration with Proposal 0026 (Concurrency)** — Sealed calls give a natural
mechanism to constrain what effects concurrent tasks are allowed to perform.
`spawn(f) sealing { Pure }` would be a compile-time proof that the forked
computation has no shared-state effects, enabling safe parallelism without locks.

**Property-based testing over effect sequences** — Combine `capture` with
a generator framework to assert invariants over all possible effect orderings,
not just a single trace. This is "QuickCheck for effect protocols."
