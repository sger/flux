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
- proposal 0067 (`GcHandle` actor-boundary error`, now superseded)
- proposal 0071 (M:N actor scheduler)

This roadmap now covers both:

- actor isolation and sendability
- the later shared-RC / explicit-promotion extension

The sequencing is deliberate:

1. Flux ships **actor concurrency first**
2. actor boundaries use **isolation + sendability**, not shared-memory aliasing
3. Aether remains **single-threaded inside an actor** in the first rollout
4. shared RC arrives later through **explicit promotion/transfer only**
5. scheduling upgrades and transfer optimizations happen without silently
   broadening existing Aether semantics

This proposal does **not** add `async/await` in the first concurrency
milestone. Phases A-D do **not** require atomic/thread-shared reference
counting. Later phases define the shared-RC extension explicitly.

## Motivation
[motivation]: #motivation

The current proposal set reflects an earlier design era:

- 0026 mixes actors, async/await, syntax ideas, and runtime notes in one broad
  umbrella
- 0065, 0066, and 0071 capture a better actor-first direction, but predate
  Aether's current `Rc`-everywhere reality
- 0067 is now superseded because it targeted the pre-Aether `Value::Gc` /
  `GcHandle` runtime model, which no longer exists after GC elimination

Today the codebase is in a different place:

- Aether is landed as a single-threaded `Rc`-based memory model
- continuations and closures remain actor-local/runtime-local values
- there is no implemented actor runtime in `src/`
- there is no sendability checking in the type system

That means the next milestone is no longer "invent a concurrency vision." The
next milestone is "define one safe, implementable actor model that fits Aether
as it exists now, while preserving a later path to explicit shared RC."

Flux should therefore:

- ship actor isolation first
- keep local Aether optimizations valid inside one actor
- add shared RC later as a distinct runtime regime
- avoid silently generalizing `Dup` / `Drop` / `Reuse` to shared objects

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What Flux should aim to ship first

The first real concurrency release should be:

- actor-based
- effect-typed
- copy-on-send or equivalent sendable transfer at the boundary
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

- no shared-memory concurrency in the initial actor milestone
- no `Arc`-backed general `Value`
- no cross-actor sending of closures, continuations, handlers, or runtime-owned
  control state
- no `async/await` syntax in the first milestone
- no M:N scheduler in the first milestone
- no shared-object reuse

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

Shared-memory concurrency is explicitly out of scope for the first actor
milestone. Shared RC is a later extension to this roadmap, not a property of
all values from the start.

### 2. Actor boundary semantics

Cross-actor transfer is defined by sendability rules:

- primitives are sendable
- immutable structural values are sendable through boundary conversion
- `ActorId` values are sendable
- closures are not sendable as ordinary messages
- continuations are not sendable
- handler descriptors / perform descriptors are not sendable
- any future runtime-private value remains actor-local unless explicitly
  specified otherwise

Spawn policy:

- `spawn` may accept closures capturing only statically sendable/promotable
  values
- non-sendable captures must be rejected

Dead actor policy:

- sending to a dead actor is a runtime error

This is the crucial boundary that turns Aether from "good single-threaded memory
model" into "safe foundation for actor concurrency."

### 3. Aether interaction

Aether remains intentionally single-threaded inside an actor:

- `Rc` remains the ownership mechanism for actor-local values
- Perceus dup/drop/borrow/reuse optimizations continue to reason within one
  actor
- actor sendability is an additional boundary layer, not a replacement for
  Aether

Phases A-D therefore do not require:

- atomic `Arc` everywhere
- `Send + Sync` for the general `Value`
- cross-thread sharing of continuations

Flux does **not** silently generalize current `Dup` / `Drop` / `Reuse`
semantics to shared objects.

### 4. Shared-RC extension model

Later shared RC follows these rules:

- **Local RC**
  - actor-local
  - non-atomic
  - current Aether fast path
- **Shared RC**
  - concurrency-safe
  - entered only through explicit promotion/transfer
  - no initial reuse

Boundary model:

- values are local by default
- only explicit actor-boundary transfer/promotion may enter shared RC

Send path:

- actor send uses **unique move + shared fallback**

Shared reuse:

- prohibited in the initial shared-RC rollout

Verifier scope:

- local uniqueness/reuse/drop-spec assumptions stop at promotion/transfer
  boundaries

### 5. Runtime architecture

The first runtime implementation should be thread-per-actor or a small worker
pool with actor isolation, but the semantics must not depend on scheduler
cleverness.

The key runtime requirements are:

- each actor owns its own execution context
- messages are transferred through a sendable representation
- `recv()` blocks or parks according to the installed runtime handler
- later shared RC must remain explicit and must not penalize ordinary local
  execution paths

### 6. Effect-system integration

The `Actor` effect remains the right language-level abstraction.

The first milestones keep:

- `spawn`
- `send`
- `recv`

And defer:

- typed mailboxes/channels
- `ask`/reply protocols
- `async/await`

## Phased rollout

### Phase A: Actor semantic MVP

Scope:

- define `Flow.Actor`
- add runtime actor support in `src/runtime/actor/`
- implement `spawn`, `send`, `recv`
- use deep-copy or equivalent sendable conversion at the boundary
- reject obviously non-sendable runtime values
- allow thread-per-actor or a small worker pool, but keep semantics independent
  of scheduler strategy
- treat sending to a dead actor as a runtime error

Success criteria:

- actor programs run on VM, JIT, and LLVM
- no shared runtime state is unsafely aliased
- docs and parity tests define actor semantics

### Phase B: Static sendability and spawn-capture rules

Scope:

- add compile-time sendability checking where types are known
- define legal/illegal spawn captures
- reject closures, continuations, handler descriptors, perform descriptors, and
  runtime-private state across boundaries
- surface actor-boundary diagnostics in the type/effect checker

Success criteria:

- common invalid sends fail at compile time
- spawned closure legality is documented and tested
- runtime-only failures become narrow

### Phase C: Scheduler upgrade

Scope:

- fold in M:N scheduling guidance from 0071
- keep exact same actor semantics as Phases A/B
- add fairness, parking, wakeups, and fuel/preemption
- avoid semantic dependence on scheduler internals

Success criteria:

- same `Actor` surface behavior as earlier phases
- better scalability
- parity and liveness coverage

### Phase D: Local Aether transfer optimization

Scope:

- add unique-transfer fast paths on top of actor isolation
- avoid unnecessary deep copies when local uniqueness can be proved or checked
- optimize transfer without introducing shared RC as the default value model

Success criteria:

- actor-boundary transfer is measurably cheaper for unique values
- ordinary local workloads stay on the local Aether fast path
- semantics remain unchanged

### Phase E: Shared RC substrate

Scope:

- introduce explicit shared RC runtime support
- ensure shared RC is entered only through explicit promotion/transfer
- keep local execution non-atomic
- add shared-safe payload/runtime representation
- define receive-side conversion back into actor-local values unless the design
  intentionally retains a shared wrapper

Success criteria:

- shared RC exists without penalizing local-only execution
- actor boundary can carry shared-safe values using explicit runtime rules
- no backend invents different promotion behavior

### Phase F: Shared RC verification and boundary semantics

Scope:

- add verifier/runtime rules that forbid local-only Aether assumptions on shared
  values
- formalize that local `Reuse` cannot target shared objects
- formalize that `DropSpecialized` local fast paths do not apply to shared
  payloads
- make actor-boundary promotion rules explicit in docs and diagnostics

Success criteria:

- shared RC is explicitly scoped in semantics, verifier rules, and diagnostics
- local and shared ownership regimes are not conflated

### Phase G: Future shared-RC optimizations (explicitly deferred)

Scope:

- reserve this phase for any future shared-RC-specific optimizations
- state clearly that shared reuse is out of scope
- require any future revisit to arrive as a separate extension proposal

Success criteria:

- roadmap stays honest about what is and is not planned
- no accidental commitment to shared reuse

## Proposal merge guidance

This proposal should become the canonical concurrency roadmap.

### Merge / supersede

- **0026** should be treated as historical vision, not the active delivery plan
- **0065** should be retained only for detailed `Actor` effect surface syntax if
  still useful
- **0066** should be folded in as Phase A implementation guidance
- **0067** is already superseded and should be treated as historical context
  only
- **0071** should be folded in as Phase C implementation guidance
- the duplicate local/shared-runtime proposal content is now merged here and
  should no longer stand as a separate proposal identity

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
3. ship transfer optimization
4. add explicit shared RC substrate
5. consider any future shared-memory extensions

### Why explicit promotion instead of ambient sharing

Explicit promotion keeps local Aether assumptions valid and prevents hidden
backend/runtime broadening of ownership semantics.

### Why shared reuse is intentionally excluded at first

Shared reuse would require a separate legality and proof story. The initial
shared-RC extension should keep shared objects conservative and preserve the
current local fast path.

### Why not merge everything into one giant spec

This roadmap is canonical for sequencing and decisions, but focused proposals
and implementation notes can still exist for detailed syntax or runtime work.

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
2. How much compile-time sendability can Flux prove before typed mailboxes
   exist?
3. What exact shared payload/runtime representation is best for Phase E:
   dedicated shared value graph, sendable mirror representation, or a hybrid?

## Future possibilities
[future-possibilities]: #future-possibilities

- typed actor protocols
- `ask` / reply patterns
- supervision and monitors
- deterministic test/simulation handlers for actors
- effect sealing for spawn boundaries
- shared-memory concurrency beyond actors in a separate future proposal
