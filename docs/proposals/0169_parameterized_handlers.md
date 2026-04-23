- Feature Name: Parameterized Effect Handlers
- Start Date: 2026-04-22
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0084](implemented/0084_aether_memory_model.md) (Aether memory model), [0086](implemented/0086_backend_neutral_core_ir.md) (Backend-neutral Core IR)
- Related: [0161](implemented/0161_effect_system_decomposition_and_capabilities.md) (Effect decomposition), [0162](0162_unified_effect_handler_runtime.md) (Unified handler runtime)

# Proposal 0169: Parameterized Effect Handlers

## Summary
[summary]: #summary

Introduce **parameterized handlers**: effect handlers that carry a
per-invocation value threaded through each `resume`. This gives Flux a way
to express `State`, `Counter`, `Writer`, and similar stateful effect
patterns without mutable bindings in handler bodies.

The parameter is *not* a mutable cell. It is threaded through the
continuation: `resume(v, s)` hands the resumed computation the value `v`
and sets the handler's parameter to `s` for the next operation on that
handler. Between operations, each handler invocation sees exactly the
parameter value the previous operation returned via `resume`.

Scope: **surface syntax + typing + Core IR lowering.** No runtime rewrite.
Parameterized handlers compile down to the existing handler mechanism with
one extra argument threaded through each resume — the runtime changes
proposed in 0162 apply verbatim.

## Motivation
[motivation]: #motivation

### Today: Flux has no way to express stateful effects

Every effect handler in the current Flux tree is *read-only*. A survey of
all in-tree handlers (`tests/flux/effect_*.flx`, `tests/parity/effect_*.flx`,
`examples/`) reveals zero handlers that carry a value changing across
`perform` calls. This is not because Flux programmers chose pure idioms —
it is because **the language has no syntactic form for it**. The comment
on [`tests/flux/effect_tr_loop.flx:22`](../../tests/flux/effect_tr_loop.flx)
records this explicitly:

> The handler discards the values (no state mutation in Flux closures).

Programmers who want `State` today write one of:

1. A recursive helper that threads the state manually through every call.
2. A `ref`-like heap cell allocated outside the handler — which Flux does
   not have, so this collapses to option 1.
3. Give up on the effect abstraction and pass the state explicitly as an
   argument.

Option 3 is what actually happens. The effect system is under-used because
the patterns it handles best — accumulators, configuration, logging with
context — can only be expressed *statelessly* today.

### Why not mutable bindings inside arms

The obvious bolt-on is imperative assignment inside the handler body:

```flux
// NOT PROPOSED.
handle State {
    get(resume)    -> resume(cell)
    set(v, resume) -> { cell := v; resume(()) }
}
```

This path is rejected for four reasons.

**First, it breaks purity.** Flux's value model is Rc-shared immutable
NaN-boxed values forming DAGs. `cell := v` is the first place in the
language where a binding changes identity after creation. The blast radius
is large — every pass that assumes `Var` expressions are referentially
transparent (const fold, CSE, inlining, specialization) has to learn about
the new exception.

**Second, it breaks multi-shot.** Once 0162 Phase 3 lands and handlers can
resume twice, `set(v, resume)` leaves the cell observably mutated in one
branch and pristine in the other. There is no principled answer to "which
value does the second resume see?" — any answer is a new language rule.

**Third, it breaks Aether.** Perceus-style reference counting on an Rc
graph requires that heap objects never change identity. A mutable handler
cell is either stack-allocated (and then cannot escape the handler, which
makes it useless for most effect patterns) or heap-allocated with mutation
(and then Aether needs write barriers and the borrow-inference pass
re-derives). Both options add substantial compiler surface.

**Fourth, it duplicates effects.** The purpose of effect handlers is
*already* to reify control flow into data. Adding a second, parallel
mechanism (mutable cells) for "thing that changes over time" is a design
smell — the language would have two stories for the same problem.

### What threading buys instead

Threading the state through the continuation keeps every Flux invariant:

- No new mutation. `resume(v, s)` returns from the perform with value `v`
  and replaces the handler's parameter with `s`. The parameter is a normal
  immutable Flux value; "replacing" it is just supplying a new one on the
  next invocation.
- Multi-shot works. Each resume branch passes its own new state. If a
  branch does not call `resume`, the state is simply discarded along with
  the branch.
- Aether is unaffected. The parameter lives in a normal frame slot and
  participates in RC like any other value.
- No new operator. `resume(v, s)` is an ordinary call with two arguments
  at a specific call site; no `:=`, no `ref`, no lvalue grammar.

The feature compiles to existing machinery with one new binder threaded
through the handler's frame — runtime work is proportional to "one extra
local slot per handler invocation," not to "new algorithm."

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

A parameterized handler declares an initial parameter alongside the
`handle`:

```flux
effect State {
    get : () -> Int
    set : (Int) -> ()
}

fn main() with IO {
    let result = do {
        perform State.set(10)
        let x = perform State.get()
        perform State.set(x + 5)
        perform State.get()
    } handle State(0) {
        get(resume, state)    -> resume(state, state),
        set(resume, _, value) -> resume((), value)
    }
    print(to_string(result))   // prints "15"
}
```

### What changed syntactically

1. **`handle Effect(init) { … }`** accepts an initial parameter expression
   between the effect name and the arm block. The parameter is bound at
   `handle` entry to `init`.
2. **Each arm takes an extra trailing parameter** naming the current state.
   The order is `(resume, ...original_op_params, state)`. For an effect
   operation declared `set : (Int) -> ()`, the arm is
   `set(resume, value, state)`.
3. **`resume(v, new_state)` carries two values**: the return value handed
   back to the perform site, and the new state for the next operation on
   this handler. If an arm calls `resume(v)` with one argument, the state
   is left unchanged — the previous parameter value is reused.

### What did not change

- Non-parameterized handlers work exactly as before. The syntax
  `handle Eff { … }` has no initializer and arms take no trailing state.
  This proposal is strictly additive.
- `perform Eff.op(args)` is unchanged. The extra parameter is threaded by
  the handler runtime, not the perform site.
- Effect declarations (`effect State { get : () -> Int; set : (Int) -> () }`)
  are unchanged. An effect does not know whether its handlers are
  parameterized — the same effect can be handled with or without a
  parameter.

### Reader falls out as a read-only slice

A Reader-style handler simply never calls `resume(v, new_state)` with a
second argument:

```flux
effect Config { ask : () -> String }

fn describe() with Config {
    perform Config.ask()
}

fn main() with IO {
    let result = describe() handle Config("flux-server") {
        ask(resume, env) -> resume(env, env)
    }
    print(result)
}
```

The arm threads `env` through unchanged, so every `ask()` sees the same
value. Nothing structurally distinguishes Reader from State except
whether the arm ever updates the parameter.

### Counter and accumulator patterns

```flux
effect Counter { tick : () -> Int }

fn count_work() -> Int with Counter {
    let a = perform Counter.tick()
    let b = perform Counter.tick()
    let c = perform Counter.tick()
    a + b + c        // 0 + 1 + 2 = 3
}

fn main() with IO {
    let total = count_work() handle Counter(0) {
        tick(resume, n) -> resume(n, n + 1)
    }
    print(to_string(total))
}
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Syntax

Grammar extension in [`src/syntax/parser/expression.rs`](../../src/syntax/parser/expression.rs)
(the `parse_handle_expression` path):

```text
HandleExpr  ::= 'handle' EffectName HandlerInit? '{' HandlerArm (',' HandlerArm)* '}'
HandlerInit ::= '(' Expression ')'
HandlerArm  ::= Operation '(' 'resume' (',' Identifier)* StateBinder? ')' '->' Expression
StateBinder ::= ',' Identifier                     -- present iff HandlerInit is present
```

Parser rule: if the handle expression has a `HandlerInit`, every arm must
have a `StateBinder`. If it does not, no arm may have one. Mismatches are
a parse error with a suggestion pointing at the first non-conforming arm.

### Typing

A parameterized handler has two type components:

```
handle Eff(init : P) { arms } :: forall a. (() -> <Eff|e> a) -> <e> a
```

where `P` is the parameter type and flows to each arm's state binder.

The type of `resume` inside an arm for operation `op : (A1, ..., An) -> R`
with handler parameter `P` is:

```
resume : R -> P -> <e> a           -- two-argument form
resume : R -> <e> a                -- one-argument form, parameter preserved
```

Both forms coexist: the solver decides arity at the call site. This is
already how effect handlers work in Flux today — `resume(v)` vs
`resume(v, k)` is resolved by arity — so no new machinery is needed in
type inference.

### Core IR lowering

[`src/core/lower_ast/mod.rs`](../../src/core/lower_ast/mod.rs) — the
`Handle` node gains an optional `parameter` field:

```rust
CoreExpr::Handle {
    body: Box<CoreExpr>,
    effect: Identifier,
    handlers: Vec<CoreHandler>,
    parameter: Option<Box<CoreExpr>>,    // NEW: initial parameter expression
    span: Span,
}
```

Each `CoreHandler` gains an optional state binder:

```rust
struct CoreHandler {
    operation: Identifier,
    resume: CoreBinder,
    params: Vec<CoreBinder>,
    state: Option<CoreBinder>,           // NEW: None for un-parameterized handlers
    body: CoreExpr,
    span: Span,
}
```

Every downstream Core pass that recurses into `Handle` / handler arms
(evidence, ANF, inliner, dead-let, specialize, Aether's reuse-infer,
display) gains one line to pass `state` through.

### Runtime representation

The handler frame on the VM stack ([`src/runtime/handler_frame.rs`](../../src/runtime/handler_frame.rs))
gains one slot:

```rust
pub struct HandlerFrame {
    // existing fields: effect, arms, entry_frame_index, entry_sp, ...
    pub parameter: Option<Value>,        // NEW: None for un-parameterized handlers
}
```

`OpHandle` evaluates the initial expression, pushes the result into
`HandlerFrame::parameter`, and proceeds as today. `OpPerform` that lands
on a parameterized handler pushes the arm closure, the resume closure,
the operation's arguments in order, **and then** the current parameter
value as the last argument before calling the arm.

`resume(v, s)` in an arm updates `HandlerFrame::parameter` to `s` before
transferring control back to the perform site. `resume(v)` with one
argument leaves the parameter unchanged.

### Why this does not conflict with Proposal 0162

Proposal 0162 Phase 1 / Phase 2 specializes tail-resumptive handlers,
including `State` / `Reader` shapes, to direct-call code with no
continuation allocation. Parameterized handlers fit naturally: the
parameter is one extra frame slot threaded through the specialized
call-site, exactly as 0162 Phase 2 envisions. After 0169 lands, 0162
Phase 2's shape-matcher has real shapes in the tree to match.

### Aether interaction

The handler parameter is a normal value with normal refcounting. When
`resume(v, s)` updates the parameter, the old parameter is dropped and
`s` is dup'd — the standard swap sequence. No new Aether rule is
required; the parameter slot is treated as an ordinary frame-local from
the allocator's perspective.

### Non-tail-resumptive semantics

For handlers that resume zero or multiple times, the parameter has
intuitive semantics:

- **Zero resumes** (exception-style): the parameter is dropped when the
  handler returns from the arm without resuming. No ambiguity.
- **Multiple resumes** (search-style, after 0162 Phase 3): each call to
  `resume(v, s)` establishes the parameter value for *that branch's*
  continuation. Branches are independent — there is no shared mutable
  cell to contend over.
- **Conditional resume** (mix of resume and fall-through): the parameter
  persists along resume paths and is dropped along fall-through paths,
  exactly like any other frame local.

This is the main advantage of threading over mutation: the semantics are
derivable from "parameter is an extra argument" with no new rules.

## Exit Criteria
[exit-criteria]: #exit-criteria

Proposal 0169 ships when:

- `handle Eff(init) { arm(resume, ..., state) -> … }` parses and
  type-checks.
- `resume(v, s)` updates the handler parameter for the subsequent
  operation; `resume(v)` leaves it unchanged.
- A `tests/parity/effect_state_parameterized.flx` fixture exercises a
  `State<Int>` counter pattern and passes under both VM and LLVM.
- A `tests/parity/effect_reader_parameterized.flx` fixture exercises a
  `Reader<String>` pattern and passes under both backends.
- `--dump-core` on a parameterized handler shows the state binder
  threaded through each arm body.
- Existing (un-parameterized) effect fixtures remain green; the 9 fixtures
  in `tests/parity/effect_*.flx` must all report the same pass/fail
  status as before this proposal.

## Drawbacks
[drawbacks]: #drawbacks

- One new syntactic form (`handle Eff(init) { … }`) and one new arity
  convention (arms gain a trailing state parameter iff the handle has an
  initializer). Parser surface grows slightly.
- The `resume(v)` vs `resume(v, s)` arity distinction is decided at the
  call site, so a typo that drops the `s` argument silently preserves
  the previous state instead of erroring. Mitigation: a lint that warns
  when a parameterized handler arm calls `resume` with the wrong arity
  given its signature.
- Extending `CoreHandler` and `HandlerFrame` with `Option<_>` fields
  touches every pass that inspects handlers. The changes are mechanical
  but wide — estimated at ~15 one-line edits across Core passes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why not imperative `var`/`:=` cells inside arms?** Detailed in the
  motivation. Short version: breaks purity, breaks multi-shot, breaks
  Aether, and duplicates what effects already do.
- **Why a trailing state binder instead of a separate `state` block?**
  A separate `state { get(); set(v); }` block would require declaring the
  state's shape independently of the arms and would split the arm body
  from its update logic. Trailing binders keep the arm self-contained.
- **Why not multiple parameters?** A single parameter is sufficient — a
  tuple or record covers the multi-field case without new syntax, and
  the compiler already optimizes small tuples via `AdtFields` inline
  storage.
- **Why not defer this until 0162 lands?** Because 0162 Phase 2's
  specialization targets `State`/`Reader` shapes that today do not exist
  in Flux. Landing 0169 gives 0162 Phase 2 real input to match against.
  The orderings are independent but complementary.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Should the initial parameter expression be allowed to perform
  effects?** `handle State(read_config())` is expressive but means the
  init expression runs in whatever effect row the enclosing context has.
  Starting restriction: init must be in the pure-row only. Revisit when
  Proposal 0161 Phase 2 (sealing) lands.
- **Error message for arity mismatch in `resume`.** Today's `resume` is
  monomorphic in arity. With parameterized handlers, `resume(v)` and
  `resume(v, s)` are both valid in the same arm. Need a specific E4xx
  diagnostic explaining which was expected; probably shared with the
  lint from Drawbacks.
- **Interaction with named handlers (0162 follow-on).** If Flux later
  grows named handler references (first-class handler values that can be
  passed across functions), the named handler's type must carry the
  parameter type. Tracked separately — named handlers are not part of
  this proposal.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Polymorphic parameter types.** `effect State<s>` with the handler
  parameter carrying `s` would make `State` first-class generic.
  Requires a small extension to the handler-typing rule; the runtime
  representation is unaffected since all values are NaN-boxed.
- **Mutually recursive handler state.** A group of handlers sharing one
  state cell (e.g. read-and-write on the same log). The surface syntax
  would need `handle {Log(init), File(init)} { … }` with a shared init
  block; tracked as a later extension.
- **Specialized `State<Int>` / `State<Float>` unboxing.** Once
  Proposal 0162 Phase 2 is awake and parameterized handlers exist, the
  compiler can emit an i64/f64 slot instead of a NanBox for monomorphic
  `State<Int>` / `State<Float>` — zero-overhead state, same cost as a
  mutable local. Depends on 0162 Phase 2.
