- Feature Name: Actor Concurrency Roadmap (Aether-Aware)
- Start Date: 2026-03-21
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0115: Actor Concurrency Roadmap (Aether-Aware)

## Summary
[summary]: #summary

Define the canonical path for bringing real concurrency to Flux on top of the
landed Aether memory model.

This proposal supersedes the repo's older concurrency planning split across:

- proposal 0026 (`async/await + actors` umbrella)
- proposal 0065 (`Actor` effect definition)
- proposal 0066 (thread-per-actor MVP runtime)
- proposal 0067 (`GcHandle` actor-boundary error)
- proposal 0071 (M:N actor scheduler)

The new plan is deliberately narrower:

1. Flux ships **actor concurrency first**
2. actor boundaries use **isolation + sendability**, not shared-memory aliasing
3. Aether remains **single-threaded inside an actor** in the first rollout
4. scheduling upgrades happen **after** actor semantics are correct

This proposal does **not** add `async/await` in the first concurrency milestone.
It also does **not** require atomic/thread-shared reference counting.

## Motivation
[motivation]: #motivation

The current proposal set reflects an earlier design era:

- 0026 mixes actors, async/await, syntax ideas, and runtime notes in one broad
  umbrella
- 0065, 0066, and 0071 capture a better actor-first direction, but predate
  Aether's current `Rc`-everywhere reality
- 0067 is partially obsolete because `Value::Gc` no longer exists after Aether
  GC elimination

Today the codebase is in a different place:

- Aether is landed as a single-threaded `Rc`-based memory model
- continuations and closures remain actor-local/runtime-local values
- there is no implemented actor runtime in `src/`
- there is no sendability checking in the type system

That means the next milestone is no longer "invent a concurrency vision." The
next milestone is "define one safe, implementable actor model that fits Aether
as it exists now."

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What Flux should aim to ship first

The first real concurrency release should be:

- actor-based
- effect-typed
- copy-on-send by default
- backend-parity tested across VM, Cranelift JIT, and LLVM

User-facing shape:

```flux
import Flow.Actor

fn worker() with Actor, IO {
    let msg = recv()
    print(msg)
    worker()
}

fn main() with Actor, IO {
    let w = spawn(\() with Actor -> worker())
    send(w, "hello")
    send(w, "world")
}
```

### What this first milestone does not promise

- no shared-memory concurrency
- no `Arc`-backed general `Value`
- no cross-actor sending of closures, continuations, handlers, or runtime-owned
  control state
- no `async/await` syntax in the first milestone
- no M:N scheduler in the first milestone

### Why actors first

Actors fit Flux's current architecture best:

- they preserve purity at the language boundary
- they isolate each actor's runtime state
- they avoid forcing thread-safe RC into every value path immediately
- they let Aether keep optimizing within one actor before cross-actor transfer
  optimization is attempted

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

## Canonical design decisions

### 1. Concurrency model

Flux's first concurrency model is:

- **actors only**
- **message passing only**
- **isolated actor-local heaps/runtime state**

Shared-memory concurrency is explicitly out of scope for this proposal.

### 2. Actor boundary semantics

Cross-actor transfer is defined by sendability rules:

- primitives are sendable
- immutable structural values are sendable via deep-copy encoding
- `ActorId` values are sendable
- closures are not sendable
- continuations are not sendable
- handler descriptors / perform descriptors are not sendable
- any future runtime-private value remains actor-local unless explicitly
  specified otherwise

This is the crucial boundary that turns Aether from "good single-threaded memory
model" into "safe foundation for actor concurrency."

### 3. Aether interaction

Aether remains intentionally single-threaded inside an actor:

- `Rc` remains the ownership mechanism for actor-local values
- Perceus dup/drop/borrow/reuse optimizations continue to reason within one
  actor
- actor sendability is an additional boundary layer, not a replacement for
  Aether

This proposal therefore does not require:

- atomic `Arc` everywhere
- `Send + Sync` for the general `Value`
- cross-thread sharing of continuations

### 4. Runtime architecture

The first runtime implementation should be thread-per-actor or a small worker
pool with actor isolation, but the semantics must not depend on scheduler
cleverness.

The key runtime requirement is:

- each actor owns its own execution context
- messages are transferred through a sendable representation
- `recv()` blocks or parks according to the installed runtime handler

### 5. Effect-system integration

The `Actor` effect remains the right language-level abstraction.

However, this proposal narrows the first milestone:

- keep `spawn`, `send`, `recv`
- defer typed mailboxes/channels
- defer `ask`/reply protocols
- defer `async/await`

That keeps the first concurrency release aligned with current effect and runtime
maturity.

## Phased rollout

### Phase A: Actor semantic MVP

Scope:

- define `Flow.Actor`
- add runtime actor support in `src/runtime/actor/`
- implement `spawn`, `send`, `recv`
- deep-copy sendable values across actor boundaries
- reject non-sendable values at runtime with clear diagnostics where static
  proof is unavailable
- add actor fixtures and backend parity tests

Success criteria:

- actor programs run on VM, JIT, and LLVM
- no shared runtime state is unsafely aliased across actor boundaries
- concurrency semantics are documented and testable

### Phase B: Static sendability and diagnostics

Scope:

- add compile-time sendability checking where types are known
- surface actor-boundary diagnostics in the type/effect checker
- define what closure captures are legal for spawned computations
- ensure continuations/handlers cannot escape across actor boundaries

Success criteria:

- the common unsafe cases fail at compile time
- remaining dynamic failures are narrow and well-diagnosed

### Phase C: Scheduler upgrade

Scope:

- replace or augment the MVP runtime with an M:N scheduler
- add fairness, parking, wakeups, and fuel/preemption semantics
- preserve the exact same `Actor` surface semantics

Success criteria:

- no user-facing semantic change from Phase A
- improved scalability for large actor counts
- parity and liveness tests cover scheduler behavior

### Phase D: Transfer optimization on top of Aether

Scope:

- add unique-transfer fast paths
- avoid unnecessary deep copies when actor-boundary ownership can be proven
- explore specialized send encodings for persistent immutable values

Success criteria:

- transfer overhead drops measurably on maintained actor benchmarks
- optimizations preserve the same actor isolation semantics

## Proposal merge guidance

This proposal should become the canonical concurrency roadmap.

### Merge / supersede

- **0026** should be treated as historical vision, not the active delivery plan
- **0065** should be retained only for detailed `Actor` effect surface syntax if
  still useful
- **0066** should be folded in as Phase A implementation guidance
- **0067** should be retired or rewritten because `Value::Gc` is obsolete
- **0071** should be folded in as Phase C implementation guidance

### Keep separate

- **0075 effect sealing** should stay separate

Reason:

Effect sealing is a broader capability/sandboxing feature. It may later refine
actor spawning policy, but it is not required to ship the first actor
concurrency milestone.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not start with `async/await`

The older umbrella in 0026 mixes actor concurrency with async/await. That is too
much surface area for the first concurrency milestone.

Actors already solve the immediate need for concurrency while fitting Flux's
existing purity/effect story more naturally.

### Why not switch all values to `Arc`

That would impose cross-thread atomic overhead on all actor-local execution
before Flux has even shipped actor semantics. It is the wrong order.

The correct sequence is:

1. ship actor isolation
2. ship sendability
3. optimize transfer later if needed

### Why not merge everything into one giant spec

One giant document becomes hard to maintain once implementation starts.

This roadmap should be canonical for sequencing and decisions, but detailed
surface syntax and runtime internals may still live in focused sub-proposals or
implementation notes as work begins.

## Prior art
[prior-art]: #prior-art

- Erlang/OTP actor model
- Elixir `GenServer` / `Task`
- Gleam's actor-oriented FP direction
- Koka / Perceus for ownership optimization context
- Pony for actor isolation and sendability inspiration

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should Phase A use one OS thread per actor or a smaller fixed worker-pool
   runtime while preserving the same actor surface?
2. How much compile-time sendability can Flux prove before typed mailboxes exist?
3. What is the right failure model for dead actors in Phase A: silent drop,
   runtime error, or explicit result?
4. Should actor spawning initially permit only closed closures, or also closures
   capturing statically sendable values?

## Future possibilities
[future-possibilities]: #future-possibilities

- typed actor protocols
- `ask` / reply patterns
- supervision and monitors
- deterministic test/simulation handlers for actors
- effect sealing for spawn boundaries
- shared-memory concurrency in a separate future proposal
