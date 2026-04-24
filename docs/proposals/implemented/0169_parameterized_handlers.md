- Feature Name: Parameterized Effect Handlers
- Start Date: 2026-04-22
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: [0084](0084_aether_memory_model.md) (Aether memory model), [0086](0086_backend_neutral_core_ir.md) (Backend-neutral Core IR)
- Related: [0161](0161_effect_system_decomposition_and_capabilities.md) (Effect decomposition), [0162](0162_unified_effect_handler_runtime.md) (Unified handler runtime), [0165](0165_io_primop_migration_to_effect_handlers.md) (effectful primops through handlers), [0170](0170_polymorphic_effect_operations.md) (polymorphic effect operations)

# Proposal 0169: Parameterized Effect Handlers

## Summary
[summary]: #summary

Introduce **parameterized handlers**: postfix effect handlers that carry a
per-invocation value threaded through each `resume`. This gives Flux a way
to express `State`, `Reader`, `Counter`, `Writer`, and similar effect
patterns without mutable bindings in handler bodies.

The parameter is not a mutable cell. It is an ordinary Flux value stored in
the active handler frame. Each handled operation receives the current
parameter as the final arm argument. The arm must call `resume(result,
next_state)` to resume the continuation and establish the parameter value
seen by the next operation handled by that same handler invocation.

Scope: **surface syntax, typing, Core lowering, CFG/VM handler runtime, and
LIR/native handler lowering.** This is not a syntax-only feature. It extends
the existing `Perform` / `Handle` semantic path with one explicit handler
parameter and one explicit next-parameter value on `resume`.

The current effect system remains unchanged otherwise:

- effects are declared with existing `effect Name { op: Args -> Ret }`
  syntax;
- operations are performed with existing `perform Effect.operation(args)`;
- handlers remain postfix expressions: `expr handle Effect { ... }`;
- builtin operational effects from `Flow.Effects` / `Flow.Primops`
  continue to use the 0165 default-handler path.

## Motivation
[motivation]: #motivation

### Today: Flux has no way to express stateful handlers

Flux now has handleable effect operations, default handlers for operational
prelude effects, row-polymorphic operation schemes, and sealing. What it
still lacks is a way for a user-written handler to remember a value across
multiple operations.

Today, handlers are effectively read-only. A handler can intercept an
operation and resume with a value, but it cannot carry an accumulator,
environment, log, or state value from one handled operation to the next.
Programmers who want this pattern must thread state through ordinary
function arguments instead of using the effect abstraction.

That undercuts the point of user-defined effects. `State`, `Reader`,
`Writer`, and counters are the standard small examples for algebraic
effects, but Flux can only express the stateless subset cleanly.

### Why not mutable bindings inside arms

The tempting alternative is mutation inside the handler body:

```flux
// NOT PROPOSED.
work() handle State {
    get(resume)        -> resume(cell)
    set(resume, value) -> { cell := value; resume(()) }
}
```

This path is rejected.

- It introduces mutable binding identity into a language whose optimizer and
  Aether model assume immutable values.
- It makes multi-shot or copied continuations ambiguous because branches
  would share one mutable cell.
- It forces backend and ownership passes to reason about write barriers or
  mutable frame slots.
- It duplicates what effect handlers already model: control flow made
  explicit as data.

Parameterized handlers keep the state transition explicit at the resume
site. The handler does not mutate a cell; it passes the next state alongside
the resumed value.

## Guide-level Explanation
[guide-level-explanation]: #guide-level-explanation

A parameterized handler is written by adding an initial parameter after the
effect name in the existing postfix `handle` syntax:

```flux
effect State {
    get: () -> Int,
    set: Int -> ()
}

fn program() -> Int with State {
    perform State.set(10)
    let x = perform State.get()
    perform State.set(x + 5)
    perform State.get()
}

fn main() with IO {
    let result = program() handle State(0) {
        get(resume, state) -> resume(state, state),
        set(resume, value, state) -> resume((), value)
    }

    println(result) // prints 15 through the default Console handler
}
```

The current handler syntax is:

```flux
expr handle Effect {
    operation(resume, op_arg_1, op_arg_2) -> resume(result)
}
```

This proposal adds only the parameterized form:

```flux
expr handle Effect(initial_state) {
    operation(resume, op_arg_1, op_arg_2, state) -> resume(result, next_state)
}
```

For v1, the rule is intentionally strict:

- if a handler has `Effect(initial_state)`, every arm must bind the trailing
  `state` parameter;
- every call to the arm's `resume` binder must pass exactly two arguments:
  the operation result and the next state;
- preserving state is explicit: use `resume(result, state)`;
- non-parameterized handlers keep the existing one-argument `resume(result)`
  behavior.

The strict rule avoids overloaded `resume` arity and avoids silent bugs where
a forgotten second argument accidentally preserves stale state.

### Reader

Reader is just a parameterized handler that passes the same state back every
time:

```flux
effect Config {
    ask: () -> String
}

fn describe() -> String with Config {
    perform Config.ask()
}

fn main() with IO {
    let result = describe() handle Config("flux-server") {
        ask(resume, env) -> resume(env, env)
    }

    println(result)
}
```

### Counter

```flux
effect Counter {
    tick: () -> Int
}

fn count_work() -> Int with Counter {
    let a = perform Counter.tick()
    let b = perform Counter.tick()
    let c = perform Counter.tick()
    a + b + c
}

fn main() with IO {
    let total = count_work() handle Counter(0) {
        tick(resume, n) -> resume(n, n + 1)
    }

    println(total) // 0 + 1 + 2
}
```

### Interaction With Builtin Effects

Parameterized handlers are for user-written handlers. They do not change the
0165 builtin path where calls such as `println`, `read_file`, and
`clock_now` route through `perform Console.println`, `perform
FileSystem.read_file`, and `perform Clock.clock_now` before reaching default
handlers.

Users may still intercept those operational effects with ordinary handlers:

```flux
fn main() {
    println("hidden") handle Console {
        println(resume, _value) -> resume(())
        print(resume, _value) -> resume(())
    }
}
```

If a future example needs stateful interception of a builtin effect, the same
parameterized syntax applies:

```flux
fn main() {
    println("a")
    println("b")
} handle Console(0) {
    println(resume, _value, n) -> resume((), n + 1),
    print(resume, _value, n) -> resume((), n)
}
```

## Current Syntax and Semantics

0169 extends the current syntax instead of replacing it.

### Existing Forms That Stay Valid

Current Flux effect syntax remains:

```flux
effect Logger {
    log: String -> ()
}

fn emit(msg: String) -> () with Logger {
    perform Logger.log(msg)
}

fn run() {
    emit("seen") handle Logger {
        log(resume, _msg) -> resume(())
    }
}
```

0169 adds only this new postfix handler form:

```flux
emit("seen") handle Logger(0) {
    log(resume, _msg, count) -> resume((), count + 1)
}
```

No new `perform` syntax is introduced. No effect declaration syntax changes.
No new `with` annotation syntax is introduced.

### Concrete Effects, Not Aliases

`handle` targets a concrete effect with operations. It does not target an
effect-row alias.

Valid:

```flux
program() handle Console(0) {
    println(resume, _value, n) -> resume((), n + 1),
    print(resume, _value, n) -> resume((), n)
}
```

Invalid:

```flux
program() handle IO(0) {
    // IO is an alias row, not one concrete operation namespace.
}
```

Aliases such as `IO = <Console | FileSystem | Stdin>` still work in `with`
annotations and sealing rows, but a handler must name the operation namespace
whose arms it implements.

### Handler Completeness Still Applies

Parameterized handlers use the same completeness rule as ordinary handlers.
If an effect declares multiple operations, the handler must provide every
operation arm unless the current implementation already has an explicit
partial-handler rule for that effect.

For `Console`, that means a stateful handler must cover both `print` and
`println`:

```flux
program() handle Console(0) {
    print(resume, _value, n) -> resume((), n),
    println(resume, _value, n) -> resume((), n + 1)
}
```

The extra state parameter does not change operation coverage.

### Effect Rows and Strict Annotations

The handled body still requires the handled effect before the handler
discharges it:

```flux
fn program() -> Int with State {
    perform State.get()
}

fn main() with IO {
    let x = program() handle State(0) {
        get(resume, state) -> resume(state, state),
        set(resume, value, _state) -> resume((), value)
    }
    println(x)
}
```

`program` still has `with State`; the handler at the call site removes
`State` from the surrounding expression's required row. Strict-mode checks
for helper functions remain unchanged. Entry-point default handlers for
`Console`, `FileSystem`, `Stdin`, and `Clock` still apply only to those
builtin operational effects.

### Initializer Evaluation Order

The initializer is evaluated before the parameterized handler is installed.
Its effects are checked in the surrounding ambient row and are not handled by
the handler being created.

```flux
program() handle State(make_initial_state()) {
    get(resume, state) -> resume(state, state),
    set(resume, value, _state) -> resume((), value)
}
```

This handler does not handle effects performed by `make_initial_state()`.

If the initializer is effectful:

```flux
program() handle State(read_initial_state()) {
    get(resume, state) -> resume(state, state),
    set(resume, value, _state) -> resume((), value)
}
```

then `read_initial_state()` requires its own effects in the surrounding row.
This matches ordinary evaluation order and avoids a self-referential handler
whose parameter depends on operations handled by the handler being installed.

### Sealing

Sealing composes with parameterized handlers because sealing checks the
actual effects of the sealed expression after local handlers discharge their
effects.

```flux
let result = (
    program() handle State(0) {
        get(resume, state) -> resume(state, state),
        set(resume, value, _state) -> resume((), value)
    }
) sealing { Console }
```

The `State` effect is handled before the sealed expression is checked, so the
sealed row does not need to include `State`. If a handler arm calls `println`,
the sealed expression must allow `Console`.

### Polymorphic Operations

Parameterized handlers do not change operation scheme instantiation from
0170. A polymorphic operation such as `Console.println: a -> Unit` is still
instantiated at each `perform` site. The handler arm is checked with the same
operation parameter types the current handler implementation would use, plus
one trailing state binder.

For handlers that ignore the polymorphic value, the state transition remains
ordinary:

```flux
program() handle Console(0) {
    print(resume, _value, n) -> resume((), n),
    println(resume, _value, n) -> resume((), n + 1)
}
```

0169 does not add higher-rank handler arms or generic state parameters. The
state type is one monomorphic type per handler expression.

### Nested Same-effect Handlers

As with ordinary handlers, the innermost matching handler receives the
operation.

```flux
outer() handle State(100) {
    get(resume, outer_state) -> resume(outer_state, outer_state),
    set(resume, value, _outer_state) -> resume((), value)
}
```

If `outer()` installs another `State` handler inside its body, operations
inside the nested handler update the nested parameter, not the outer one.
The outer parameter is visible again only after control leaves the nested
handler.

### Handler Arm Fallthrough

Parameterized handler arms should follow the current handler rule for arms
that do not call `resume`. If the language permits exception-style arms that
return directly, the current parameter is dropped with the abandoned
continuation. If the current implementation rejects non-resumptive arms in a
given path, parameterized handlers inherit that restriction.

The first implementation should not add new non-resumptive behavior just to
support state.

## Reference-level Explanation
[reference-level-explanation]: #reference-level-explanation

### Syntax

Flux currently parses handlers as postfix expressions. This proposal extends
that postfix form:

```text
HandleExpr       ::= Expr 'handle' EffectName HandlerInit? '{' HandlerArmList '}'
HandlerInit      ::= '(' Expression ')'
HandlerArm       ::= Operation '(' ResumeBinder HandlerParamList? ')' '->' Expression
ResumeBinder     ::= Identifier
HandlerParamList ::= (',' Identifier)*
```

Parsing does not try to decide which final identifier is the state binder.
The parser records:

- whether `HandlerInit` is present;
- each arm's existing `resume` binder;
- each arm's ordered parameter list.

Semantic validation uses the effect operation signature to split the arm
parameters:

- for `op: (A1, ..., An) -> R`, a non-parameterized arm must have `N`
  operation parameters after `resume`;
- a parameterized arm must have `N + 1` parameters after `resume`;
- the final parameter in a parameterized arm is the state binder.

This keeps the parser independent of effect signatures and gives better
diagnostics because the compiler can report both the declared operation
arity and the handler arm arity.

### Typing

For an operation:

```text
op: (A1, ..., An) -> R
```

and a parameterized handler:

```flux
expr handle Eff(init) { ... }
```

the type checker infers:

- `init: P`;
- each operation argument binder has its declared type `A1 ... An`;
- the trailing state binder has type `P`;
- `resume` has exactly the function type `R -> P -> T`, where `T` is the
  handled expression result type.

The handler expression has the same result type as the handled expression.
The handled effect is discharged from the body effect row exactly like an
ordinary handler; the initializer's effects are checked in the surrounding
ambient row before the handler is installed.

For v1, `resume(result)` is invalid inside a parameterized handler. The
diagnostic should say that parameterized handlers require
`resume(result, next_state)` and suggest `resume(result, state)` when the
current state binder is available.

### Core IR

Core keeps `Perform` and `Handle` as the semantic mechanism. `CoreExpr::Handle`
gains an optional parameter expression:

```rust
CoreExpr::Handle {
    body: Box<CoreExpr>,
    effect: Identifier,
    handlers: Vec<CoreHandler>,
    parameter: Option<Box<CoreExpr>>,
    span: Span,
}
```

`CoreHandler` gains an optional state binder:

```rust
pub struct CoreHandler {
    pub operation: Identifier,
    pub resume: CoreBinder,
    pub resume_ty: Option<CoreType>,
    pub params: Vec<CoreBinder>,
    pub param_types: Vec<CoreType>,
    pub state: Option<CoreBinder>,
    pub state_ty: Option<CoreType>,
    pub body: Box<CoreExpr>,
    pub span: Span,
}
```

The invariant is:

- `parameter.is_none()` iff every handler has `state.is_none()`;
- `parameter.is_some()` iff every handler has `state.is_some()`;
- `state_ty` is the inferred type of the parameter expression.

Core/Aether passes that walk handlers must treat `state` as a local binder
for scoping, free-variable collection, display, ANF, evidence, inlining,
drop/dup insertion, and reuse analysis.

### Runtime and Backend Semantics

Parameterized handlers extend the existing handler frame with an optional
parameter value. The runtime behavior is:

1. Evaluate the `parameter` expression before installing the handler frame.
2. Store the value in the active handler frame.
3. When a matching `perform` reaches that frame, invoke the handler arm with:
   `resume`, operation arguments, and the current parameter value.
4. `resume(result, next_state)` updates the active handler frame's parameter
   before transferring `result` back to the perform site.
5. Leaving the handler drops the current parameter value like any other
   frame-owned value.

This must be implemented through the maintained semantic path:

- AST/type inference validates and annotates the parameterized handler.
- AST -> Core lowers to `CoreExpr::Handle { parameter: Some(...) }`.
- Core/Aether preserve the parameter and state binder.
- Core -> CFG/bytecode and Core -> LIR/native lower the extended handler
  frame semantics.
- No AST fallback, no second semantic IR, and no backend-only workaround.

### Non-tail and Multi-shot Semantics

For zero-resume arms, the current parameter is dropped when the handler exits
or the branch aborts normally.

For multi-shot continuations, each `resume(result, next_state)` establishes
the parameter for that resumed branch. The proposal intentionally models the
parameter as continuation state, not as a shared mutable cell.

If the existing VM/native multi-shot handler behavior differs, 0169 should
ship only the single-shot/tail-resumptive subset first and track multi-shot
parameterized handlers as a follow-up. The semantics above are still the
target language rule.

## Diagnostics

Add a dedicated diagnostic for parameterized handler shape errors. It should
cover:

- `handle Eff(init)` arm missing the trailing state binder;
- non-parameterized `handle Eff` arm with too many parameters;
- `resume(result)` inside a parameterized arm;
- `resume(result, next_state)` inside a non-parameterized arm;
- mismatched state type across arms.

The message should name the effect, operation, expected operation parameter
count, whether a state binder is required, and the inferred state type when
available.

## Implementation Phases

### Phase 0 — Parser and AST Shape

Status: implemented.

Add the syntax surface without assigning semantics yet.

- Parse `expr handle Eff(init) { ... }` as the existing postfix `handle`
  expression plus an optional initializer expression.
- Add `parameter: Option<Box<Expression>>` to the AST handle expression.
- Keep each arm's parameters as the existing ordered list after `resume`;
  do not make the parser decide which parameter is state.
- Preserve all existing `expr handle Eff { ... }` behavior.
- Add parser/operator-registry compatibility tests for the new postfix shape.

Exit gate:

- Parameterized handler syntax parses into AST.
- Existing handler parser tests remain green.

### Phase 1 — Static Semantics and HM

Status: implemented.

Make parameterized handlers type-check, but do not depend on backend runtime
support yet.

- During effect-handler validation, use the effect operation signature to
  split arm parameters.
- For non-parameterized handlers, require exactly the operation arity after
  `resume`.
- For parameterized handlers, require operation arity plus one trailing state
  binder.
- Infer the initializer type `P` and assign `P` to every state binder in the
  handler.
- Type `resume` in parameterized arms as `R -> P -> T`, where `R` is the
  operation result and `T` is the handled expression result.
- Reject `resume(result)` inside parameterized arms.
- Reject `resume(result, next_state)` inside non-parameterized arms.
- Check initializer effects in the surrounding ambient row before the handler
  discharges the handled effect.
- Add dedicated diagnostics for missing state binders, wrong arm arity, wrong
  resume arity, and state type mismatch.

Exit gate:

- Positive static fixtures for `State` and `Reader` type-check.
- Negative static fixtures reject missing state binders and wrong resume
  arity.
- Existing strict effect annotation tests keep their current behavior.

### Phase 2 — Core Lowering and Core Passes

Status: implemented.

Carry the new semantic shape through the canonical IR.

- Add `parameter: Option<Box<CoreExpr>>` to `CoreExpr::Handle`.
- Add `state: Option<CoreBinder>` and `state_ty: Option<CoreType>` to
  `CoreHandler`.
- Lower the AST initializer into the Core handle parameter.
- Lower the semantic trailing state parameter into `CoreHandler::state`.
- Update binder resolution so the state binder is in scope only inside its
  handler arm body.
- Update Core display, free-variable collection, ANF, evidence, inlining,
  linting, specialization helpers, and all generic Core walkers.
- Update Aether walkers, borrow/reuse/drop analysis, and display paths to
  preserve the parameter and treat the state binder as an ordinary local
  binder.

Exit gate:

- `--dump-core` shows the parameter expression and state binder.
- Existing Core/Aether snapshot churn is limited to intentional handler-shape
  changes.
- No backend path falls back to AST execution.

### Phase 3 — CFG and VM Runtime

Status: implemented for the maintained CFG/VM path.

Implement parameterized handler execution on the maintained VM path.

- Extend CFG handle lowering with the optional parameter expression and state
  binder.
- Evaluate the initializer before installing the handler frame.
- Store the current parameter value in the active handler frame.
- Pass `resume`, operation arguments, and current state into the selected
  handler arm.
- Implement `resume(result, next_state)` so it updates the frame parameter
  before transferring `result` back to the perform site.
- Drop the current parameter when the handler exits or an abandoned
  continuation is discarded.

Exit gate:

- VM fixtures pass for single-shot `State<Int>` and `Reader<String>`.
- Nested same-effect handler fixture proves the innermost parameter is updated.
- Non-parameterized handler fixtures remain green.

### Phase 4 — LIR and Native Runtime

Status: implemented for the LLVM/native yield path.

Mirror the VM semantics in the native backend.

- Lower Core parameterized handlers through LIR without introducing a second
  semantic IR.
- Pass the current parameter into native handler closures.
- Update native resume/yield handling so `resume(result, next_state)` installs
  the next state for that resumed branch.
- Preserve the same drop/dup behavior as the VM path.

Exit gate:

- VM/native parity is green for single-shot parameterized `State` and
  `Reader`.
- Existing native handler parity fixtures keep their previous status.

### Phase 5 — Hardening and Edge Cases

Status: implemented for the documented single-shot VM/native paths, with
multi-shot and direct-body native continuation edges tracked as follow-up.

Add coverage around the semantic boundaries most likely to regress.

- Stateful interception of builtin `Console` handlers still composes with
  0165 default handlers.
- `handle IO(0)` and other alias-row handlers are rejected with a clear
  diagnostic.
- Sealing after a parameterized handler sees the discharged effect row.
- Effectful initializer expressions require their own ambient effects.
- Handler arm fallthrough follows the existing ordinary-handler rule.
- Multi-shot parameterized handlers are either proven green across VM/native
  or explicitly tracked as a follow-up.
- Direct-body native parameterized Console interception
  (`println(...) handle Console(init) { ... }`) is tracked as a native
  continuation edge; the covered operational Console fixture exercises the
  same interception through a helper call and passes parity.

Exit gate:

- `cargo test --test compiler_rules_tests`
- `cargo test --test ir_pipeline_tests`
- `cargo test --test static_typing_contract_tests`
- `cargo test --test test_runner_cli`
- `cargo run -- parity-check tests/parity`
- `cargo test --all --all-features`

## Exit Criteria
[exit-criteria]: #exit-criteria

Proposal 0169 ships when:

- `expr handle Eff(init) { op(resume, ..., state) -> ... }` parses and
  type-checks with current Flux postfix handler syntax.
- `resume(result, next_state)` updates the handler parameter for subsequent
  operations in the same handler invocation.
- `resume(result)` is rejected inside parameterized handler arms with a
  dedicated diagnostic.
- Ordinary `expr handle Eff { op(resume, ...) -> resume(result) }` handlers
  keep their current behavior.
- A `tests/parity/effect_state_parameterized.flx` fixture exercises a
  `State<Int>` pattern.
- A `tests/parity/effect_reader_parameterized.flx` fixture exercises a
  `Reader<String>` pattern.
- A negative fixture rejects missing state binders and wrong resume arity.
- A `--dump-core` regression shows the parameter expression and state binder
  in the Core handler.
- Existing 0165 builtin effect interception/default-handler fixtures remain
  green.
- VM/native parity is green for the single-shot fixtures included in this
  proposal, or unsupported multi-shot cases are explicitly tracked outside
  the exit criteria.

## Drawbacks
[drawbacks]: #drawbacks

- The feature touches multiple compiler layers: parser, HM inference, Core,
  Aether walking, CFG/VM lowering, and LIR/native lowering.
- Handler arm arity diagnostics become slightly more subtle because the
  compiler must distinguish operation arguments from the trailing state
  binder.
- The strict v1 rule requires `resume(result, state)` even for Reader-style
  handlers that preserve state. This is more verbose, but it avoids an
  overloaded resume binder and makes state transitions visible.

## Rationale and Alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why not implicit `resume(result)` state preservation?** It is ergonomic
  but easy to get wrong. In v1, preserving state is explicit:
  `resume(result, state)`.
- **Why not parse a special state separator such as
  `op(resume, args...; state)`?** That remains a good future cleanup if arm
  diagnostics stay confusing. This proposal starts with the smallest syntax
  extension compatible with current handler arms.
- **Why not mutable handler-local variables?** They break Flux's purity and
  ownership assumptions and make multi-shot semantics unclear.
- **Why one parameter?** A tuple or record handles multiple fields without
  more syntax.
- **Why not use `Flow.Primops`?** Parameterized handlers are a language
  feature for user-defined operations. `Flow.Primops` is only the intrinsic
  implementation layer for builtin operational effects.

## Remaining Follow-Up

- Multi-shot parameterized handler parity, if VM/native behavior still
  differs after the single-shot implementation.
- Direct-body native continuation resumption for parameterized builtin
  handlers. The current green Console parity fixture routes the captured
  operation through a helper function; the direct expression form still needs
  native continuation hardening.
- Optional sugar for explicit state preservation if examples become noisy.
- Optional parser syntax with a state separator if semantic arity diagnostics
  are not enough.
- Potential specialization of common `State<Int>` / `Reader<T>` shapes after
  the baseline semantics are correct.

## Future Possibilities
[future-possibilities]: #future-possibilities

- **Polymorphic parameter types.** A future `effect State<s>` form could make
  state effects generic at the effect declaration level.
- **Shared state across multiple effects.** A later proposal could handle
  grouped handlers that share one parameter across effects.
- **Unboxed parameter slots.** Once parameterized handlers are stable,
  monomorphic state such as `Int` or `Float` could use specialized backend
  slots instead of boxed values.
