- Feature Name: `handler_linearity_and_scoped_concurrency`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft
- Umbrella: [0195](0195_checked_guarantees.md) (track T4)
- Depends on: [0162](implemented/0162_unified_effect_handler_runtime.md) (handler runtime), [0198](0198_opaque_types_and_total_stdlib.md) (opaque types, for Part B step 2)
- Relates to: [0171](0171_effect_system_polish_and_hardening.md), [0174](0174_async_effect_concurrency.md), [0143](0143_actor_concurrency_roadmap.md)

# Proposal 0199: Handler linearity and scoped concurrency

## Summary
[summary]: #summary

Two guarantees about control flow that Flux describes but does not check:

- **Part A, handlers.** An effect operation can say it must be resumed at most
  once (`@once`). The compiler infers how often each handler arm resumes, and
  rejects a handler that may resume a `@once` operation twice or let its
  continuation escape. Today `resume` is an ordinary function and the only
  check is a run-time abort on one native path.
- **Part B, concurrency.** No fiber outlives the `scope` it was forked in.
  First at run time (`scope` joins or cancels its children, which it does not do
  today), then statically: a `Scope` cannot be forged and cannot leave the
  lambda it was given to.

## Motivation
[motivation]: #motivation

### Part A: `resume` has no use tracking

- `resume` is bound as a plain, non-generalised function
  (`src/ast/type_infer/expression/effects_nodes.rs:82-101`). The only check is its
  arity (`calls.rs:232-250`).
- The VM supports multi-shot: `execute_resume` (`src/vm/function_call.rs:358`)
  moves the continuation out when it is the only owner, and deep-clones it
  otherwise. The native default (yield/prompt) path supports it too. The legacy
  direct path (`FLUX_YIELD_CHECKS=0`) counts resumes and aborts with E1200 (none)
  or E1201 (two or more) (`runtime/c/effects.c:493-503,618-660`).
- The docs disagree with the code. 0171 (lines 490-598) says the VM enforces
  one-shot with E1009. The E1201 message (`effects.c:654`) says "the VM backend
  also does not support multi-shot (it enforces one-shot continuations)". The
  test `tests/vm_runtime/vm_tests.rs:204` resumes twice and expects `3`.
- Nothing detects a continuation that escapes its arm, stored in a closure, and
  is called after the `handle` returned.

Some effects are meaningful to resume many times (choice, backtracking,
generators). Others wrap a resource that cannot be duplicated: reading a line
from a socket, a transaction, a fiber suspension. A second resume of those
replays the rest of the computation against a resource already consumed. A
single resume is also the cheap case: the VM's `Rc::try_unwrap` path and the
native direct path both exist because of it.

### Part B: `scope` does not scope

`lib/Flow/Async.flx:237`:

```flux
public fn scope<a>(f: (Scope) -> a with Async | e) -> a with Async | e {
    let s = fiber_new_scope_prim()
    f(s)
}
```

It neither waits for nor cancels the fibers forked under `s`. Checked
2026-11-03 on the VM: a child forked with `fork(s, child)` prints after `scope`
has returned. Yet:

- `docs/guide/20_async_and_fibers.md:326`: "a fiber's children cannot outlive
  their parent's scope … There are no orphan fibers."
- `examples/guide_async/09_scope_fork_workers.flx:15`: "When the scope's body
  returns, every forked child is awaited." (The example cancels explicitly, so
  its expected output does not show the difference.)

The handles are forgeable and free to escape. `Scope`, `FiberId`,
`CancelHandle` (`Async.flx:60-66`), `Channel<a>` (`lib/Flow/Channel.flx:5`) and
`Task<a>` (`lib/Flow/Task.flx:25`) are all `public data … (Int)`. `Scope(42)`
type-checks, and a `Scope` can be returned from the `scope` lambda, captured in
a closure, or stored.

Also, `fn main() with IO, Async` that calls `fork` without `run_async` stops the
VM with a Rust panic, "spawn_child_with_body outside Async.run_async"
(`src/vm/core_dispatch.rs:1159`), not a Flux diagnostic.

`both`, `race`, `first_of` and `timeout` are primops that await or cancel their
own children, so they are already structured. The gap is `scope`/`fork` and the
handles.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### `@once` operations

```flux
effect Input {
    @once read_line : () -> String
}

fn twice() {
    perform Input.read_line() handle Input {
        read_line(resume) -> resume("a") + resume("b")
        //                   error: `read_line` is @once, but this arm may resume twice
    }
}
```

An operation without `@once` is multi-shot, as today. `Async`'s internal
suspension operations become `@once`.

Hover on a handler arm shows its inferred multiplicity: `never`, `once` or `many`.
An arm that hands `resume` to another function or captures it in a closure is
`many` and *escaping*. That is allowed for a multi-shot operation and rejected for
a `@once` one.

### Scopes

```flux
fn crawl(urls) with Async {
    scope(fn(s) {
        map(urls, \u -> fork(s, \() -> fetch(u)))
        "started"
    })
    // every fiber forked under `s` has finished here
}
```

When the lambda returns, `scope` waits for every child. If the lambda fails
(`Exn`, 0197) or is cancelled, `scope` cancels every child, then propagates the
failure. A `Scope` cannot be returned from the lambda or stored where it
outlives it:

```flux
fn leak() with Async {
    scope(fn(s) { s })   // error: a `Scope` cannot leave the `scope` that created it
}
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Part A: handlers

**A1. Multiplicity inference.** For each handler arm, compute the number of
times `resume` is called on each path: 0, 1 or many. A `resume` passed as an
argument, captured by a lambda or stored is `many, escaping`. Start from the
existing syntactic classifiers: `is_handler_tail_resumptive` and
`is_handler_discard` (`src/compiler/tail_resumptive.rs:13,76`),
`resumes_only_as_last_act` (`src/lir/lower.rs:335`). Run it on the AST, where
the arms are, and record the result on `CoreHandler` so that both backends read
one answer instead of re-deriving it.

**A2. `@once`.** Parse `@once` before an operation in an `effect` declaration
(the `@` token and annotation parsing from `@fip`,
`src/syntax/parser/statement.rs:131`). A handler arm for a `@once` operation
whose multiplicity is `many` is an error with a new registered code, its span on
the second `resume`. `never` is allowed (exceptions are `never`).

**A3. Backends use the answer.** An arm that is `once` and not escaping can take
the VM's move path without the `Rc::try_unwrap` test, and the native direct
path without its run-time counter. The E1200/E1201 checks stay for arms the
analysis could not classify.

**A4. Semantics of `many` under Aether.** A continuation resumed twice shares
every value it captured. Add parity fixtures checking that in-place reuse in the
first resumption is not visible in the second. `IsUnique` should see the extra
reference, but this is untested today.

**A5. Docs.** The 0171 one-shot text and the E1201 message describe behaviour
that no longer exists. The E1201 message is diagnostic text: the maintainer
rewrites it.

### Part B: concurrency

**B1. `scope` joins or cancels (run time, both backends).** On normal return,
`scope` awaits every fiber registered under its id (the VM's `SCOPE_REGISTRY`,
`src/vm/core_dispatch.rs:416`; the native scheduler in `runtime/c/tasks.c`). On
failure or cancellation, it cancels them, waits for their cleanup arms, and then
propagates. `new_scope()` (`Async.flx:242`), which has no lexical extent, is
deprecated. The Rust panic outside `run_async` becomes a run-time Flux error
with a registered code and a stack trace.

Fixtures: `async_scope_joins_children.flx` (today's leak program, expecting the
child's line *before* the line after `scope`), `async_scope_cancels_on_exn.flx`,
`async_fork_outside_run_async.flx`.

**B2. Opaque handles.** With 0198, `Scope`, `FiberId`, `CancelHandle`,
`Channel<a>` and `Task<a>` become `public opaque data`. Their constructors are only
used inside `Flow.Async`, `Flow.Channel` and `Flow.Task`. `Channel.make` gains
`with Async`, since it allocates a runtime slot.

**B3. Static escape check.** The `ST` encoding needs rank-2 types
(`scope : (forall s. Scope<s> -> a) -> a` with `s` not free in `a`), which HM does
not have. A narrower rule fits Flux:

- `Scope` gets a phantom region parameter: `Scope<r>`.
- A function parameter can be marked as a region binder. `scope`'s lambda is
  checked with `r` as a fresh skolem constant, and the result type, after
  substitution, must not mention it. This is the one rank-2 shape needed, as GHC
  special-cased `runST` before it had impredicativity.
- A `Task<a>` forked under a scope carries the same region, so it cannot escape
  either.

The skolem check goes where `scope`'s call is inferred, next to the existing
handling of effect rows for higher-order calls (`src/ast/type_infer/expression/calls.rs`).

## Drawbacks
[drawbacks]: #drawbacks

- B1 changes behaviour: a program that relied on a child outliving its scope
  now waits. That program contradicts the guide, but it exists.
- B3 adds a type parameter users see in errors (`Scope<r>`). The error message
  has to explain regions without the word "skolem".
- `@once` is a new annotation to learn. It is optional, and the default stays
  multi-shot.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **One-shot by default (OCaml 5).** Rejected: Flux already supports and tests
  multi-shot, and choice and backtracking examples use it.
- **Linear types for `resume`.** Rejected as too broad: only `resume` needs
  counting, and it is syntactically known.
- **Leave scoping to run time (B1 only).** Acceptable as a first step, and it
  closes the guide's overclaim. B3 is what makes "no orphan fibers" checkable
  rather than enforced.
- **Full rank-N types.** Out of scope; B3 needs only the `runST` shape.

## Prior art
[prior-art]: #prior-art

- OCaml 5: one-shot continuations, `Continuation_already_resumed` at run time.
- Koka: `fun` (tail-resumptive), `ctl` (general) and the `linear` effect
  modifier, which forbids multi-shot handlers for an effect.
- Haskell `ST` (Launchbury and Peyton Jones, "Lazy Functional State Threads",
  PLDI 1994) for region escape; `runST`'s historic special typing rule.
- Structured concurrency: Trio nurseries, Kotlin `coroutineScope`, Swift
  `withTaskGroup`, Java `StructuredTaskScope`. All join on exit, cancel on failure.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Is `@once` per operation or per effect (Koka's `linear effect`)? Per operation is
  finer; per effect is simpler to explain.
- On a failure in one child, does `scope` cancel its siblings (Trio) or let them
  finish (Go `errgroup` without context)? Trio's choice is assumed above.
- Does `Channel` need a region too, or is an escaping channel harmless once its
  scope has joined?

## Future possibilities
[future-possibilities]: #future-possibilities

- Supervision trees and typed mailboxes (0143 Phases B and C) on top of scoped
  handles.
- `@once` inferred for effects whose every handler in the program resumes once,
  for optimisation only.
