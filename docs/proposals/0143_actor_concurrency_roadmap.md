- Feature Name: Actor Concurrency Roadmap (Aether-Aware)
- Start Date: 2026-03-21
- Last Updated: 2026-04-18
- Status: Draft (realistic implementation plan, 2026-04-18)
- Proposal PR:
- Flux Issue:
- Supersedes: [0026](superseded/0026_concurrency_model.md), [0065](superseded/0065_actor_effect_stdlib.md), [0066](superseded/0066_thread_per_actor_handler.md), [0067](superseded/0067_gchandle_actor_boundary_error.md), [0071](superseded/0071_mn_scheduler_actor_handler.md), [0095](superseded/0095_actor_runtime_architecture.md)
- Depends on: [0161](0161_effect_system_decomposition_and_capabilities.md) (Effect System Decomposition), [0162](0162_unified_effect_handler_runtime.md) (Unified Effect Handler Runtime), [0152](0152_named_fields_for_data_types.md) (Named Fields)

# Proposal 0143: Actor Concurrency Roadmap (Aether-Aware)

## Summary
[summary]: #summary

Define the canonical path for bringing real concurrency to Flux on top of the
landed Aether memory model. Flux ships **actors with isolated heaps** first,
backed by **typed mailboxes** and **supervision** primitives, with a
**deterministic test scheduler** from day one so concurrent code benefits from
Flux's proof-oriented testing identity.

Shared-memory concurrency is explicitly deferred and may never land — the
goal is an Erlang/OCaml-5-quality actor story, not a C++-style thread/shared-
state story.

## Motivation
[motivation]: #motivation

Flux is a pure functional language with algebraic effects and a single-
threaded reference-counting memory model (Aether/Perceus). None of those
choices are obstacles to concurrency; together they point cleanly at the one
model that actually fits: isolated actors communicating by typed messages,
with the scheduler implemented as an effect handler.

The earlier concurrency proposal set (0026, 0065, 0066, 0067, 0071) was
drafted before Aether and before the effect-system closure. They are now
superseded by this roadmap.

## Hard constraints from the current codebase
[hard-constraints]: #hard-constraints

These constraints determine the space of realistic designs:

- **`Value` is `Rc`-everywhere** ([src/runtime/value.rs:272-294](../../src/runtime/value.rs#L272-L294)): not `Send`, not `Sync`. Any scheme that requires sharing `Value` across threads requires a runtime rewrite larger than the concurrency feature itself. Ruled out for the foreseeable future.
- **Aether is single-threaded by design** ([docs/proposals/implemented/0084_aether_memory_model.md](implemented/0084_aether_memory_model.md)). Making it multi-threaded means atomic refcounts on the hot path — which Koka's Perceus design explicitly avoids.
- **Effect handlers exist** and are being unified ([Proposal 0162](0162_unified_effect_handler_runtime.md)). OCaml 5 demonstrates that effect handlers are the cleanest building block for cooperative concurrency (fibers-as-effects).

These constraints force one conclusion: **isolated actors with message-
passing, not shared memory**.

## Reference-language synthesis
[reference-languages]: #reference-languages

| Language | Model | What Flux takes | What Flux skips |
|---|---|---|---|
| **Erlang/OTP** | Isolated actors, untyped mailboxes, supervision trees, copy-on-send | Supervision, monitoring, death propagation, fail-fast philosophy | Untyped mailboxes (Flux has HM) |
| **Pony** | Actors + reference capabilities (`iso`, `val`, `ref`) proven at compile time | Capability-typed send (`iso` = unique-move, `val` = deep-immutable share) | Full capability system (too heavy for MVP) |
| **GHC Haskell** | Green threads on M:N, MVar/STM | M:N scheduler design (work-stealing, fuel-based preemption) | Shared mutable state via STM |
| **OCaml 5** | Domains (OS threads) + fibers scheduled via effect handlers | Fibers-via-effects: `spawn`/`await` are handler operations, scheduler is a user-swappable handler | Multicore shared memory |
| **Koka** | Async as effect; task-based parallelism via threads + promise | Effect-handler foundation for async — same architecture Flux is already on | Their actor story (there isn't one) |

**Synthesis:** actors with isolated heaps (Erlang-style), typed mailboxes
(Pony-style), scheduler as an effect handler (OCaml-5-style), supervision
built in from day one (Erlang-style). No shared memory.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### User-facing shape (Phase A+B after typing)

```flux
import Flow.Actor

fn worker(req: Request) with Actor, IO {
    match req {
        Ping -> send(self_id(), Pong)
        Stop -> ()
    }
}

fn main() with Actor, IO {
    let w: ActorId<Request> = spawn(\() with Actor -> worker_loop())
    send(w, Ping)
    let reply = recv::<Response>()
    print(reply)
}
```

### What ships in the first concurrency release

- actor-based, effect-typed
- copy-on-send at the boundary
- VM and LLVM parity tested
- supervision + cancellation
- deterministic test scheduler for property testing

### What does not ship, explicitly

- no shared-memory concurrency
- no `Arc`-backed general `Value`
- no cross-actor sending of closures, continuations, handlers, or runtime-owned
  control state
- no `async/await` syntax in the first milestone (use actors for now)
- no M:N scheduler in the first milestone
- no shared-object reuse

## Canonical design decisions
[canonical-design-decisions]: #canonical-design-decisions

### 1. Concurrency model
- **actors only**
- **message passing only**
- **isolated actor-local heaps/runtime state**

### 2. Actor boundary semantics

Cross-actor transfer is defined by sendability rules:
- primitives are sendable
- immutable structural values are sendable through boundary conversion
- `ActorId<m>` values are sendable
- closures are not sendable as ordinary messages
- continuations are not sendable
- handler descriptors / perform descriptors are not sendable
- any future runtime-private value remains actor-local unless explicitly
  specified otherwise

### 3. Aether interaction

Aether remains intentionally single-threaded inside an actor:
- `Rc` remains the ownership mechanism for actor-local values
- Perceus dup/drop/borrow/reuse optimizations continue to reason within one
  actor
- actor sendability is an additional boundary layer, not a replacement for
  Aether

### 4. `Actor` as a Flow.Effects label

The `Actor` effect is declared in [Flow.Effects](../../lib/Flow/) (per
[0161](0161_effect_system_decomposition_and_capabilities.md)) as a phantom
label. Actor operations (`spawn`, `send`, `recv`, `self_id`) are primops
declared in `Flow.Primops` with `with Actor` signatures.

### 5. Scheduler as an effect handler

Following OCaml 5: the scheduler itself is a handler installed at program
start. Default: thread-per-actor or worker pool (Phase A / Phase D). User-
swappable: deterministic scheduler for tests (Phase D).

## Phased rollout with realistic time estimates
[phased-rollout]: #phased-rollout

The estimates below assume one engineer working primarily on concurrency.
Each phase produces a shippable release slice.

### Phase A — Actor MVP (~6 weeks)

**Goal:** `spawn → send → recv` end-to-end, VM and LLVM parity.

**Scope:**
- New module `src/runtime/actor/` with thread-per-actor runtime. Each actor
  owns its own VM instance with its own `Rc` heap.
- New stdlib `lib/Flow/Actor.flx` declaring:
  ```flux
  public extern fn spawn<a>(f: () -> a with Actor) -> ActorId with Actor
  public extern fn send<m>(to: ActorId, msg: m) -> () with Actor
  public extern fn recv<m>() -> m with Actor
  public extern fn self_id() -> ActorId with Actor
  ```
- Send semantics: deep-copy the message into a serialized representation,
  reconstruct on recv. Slow but trivially safe.
- Runtime-level sendability check: at send time, panic if the value contains
  a closure, continuation, or handler descriptor. Compile-time checking lands
  in Phase B.
- New diagnostics: `E470` (runtime: send of non-sendable value), `E471`
  (runtime: recv received wrong message type).
- Dead-actor policy: `send` to a dead actor raises a runtime error.

**Ships:**
- `examples/actors/counter.flx`, `examples/actors/ping_pong.flx`.
- `tests/actor_mvp_tests.rs` — spawn/send/recv smoke, dead-actor behaviour,
  cross-backend parity.
- `docs/guide/16_actors.md` — user-facing guide chapter.

**Does not ship:** compile-time sendability, typed mailboxes beyond bare
monomorphic, supervision, cancellation, scheduler cleverness.

**Prerequisites:** 0161 Phase 1 (Flow.Effects exists). 0162 Phase 1 is
recommended but not blocking.

### Phase B — Typed mailboxes + compile-time sendability (~4 weeks)

**Goal:** Replace runtime panics with compile errors where possible.

**Scope:**
- `ActorId<m>` parameterized by message type. `send : ActorId<m> -> m -> ()`.
- Compile-time `Sendable<T>` predicate derived structurally:
  - primitives → sendable
  - ADTs of sendable types → sendable
  - immutable collections of sendable → sendable
  - closures / continuations / `Mutable<T>` → NOT sendable
- New diagnostics: `E472` (spawn captures non-sendable value), `E473` (send
  value does not satisfy mailbox type).
- Users with polymorphic mailboxes wrap messages in a sum ADT.

**Prerequisites:** [0152](0152_named_fields_for_data_types.md) (named fields)
makes structured messages ergonomic.

### Phase C — Supervision + cancellation (~5 weeks)

**Goal:** Systems that don't fall over on one actor panic.

**Scope:**
- Erlang-style supervision primitives in `Flow.Actor`:
  - `spawn_linked(parent, f)` — parent receives exit signal on child death.
  - `link(a, b)` — bidirectional death propagation.
  - `trap_exit` — convert death signals into regular messages.
- Cancellation tokens threaded through the `Actor` effect. A cancelled
  actor's next `perform` on `Actor` or `IO` fires the cancellation handler.
- Timeouts: `recv_timeout(Duration) -> Option<msg>`.
- Supervision tree example in `examples/actors/supervisor.flx`.

**Prerequisites:** [0162](0162_unified_effect_handler_runtime.md) Phase 3
(unified yield algorithm) — cancellation composes cleanly on top of the
unified handler runtime, avoids special-casing per backend.

### Phase D — Scheduler upgrade (~6 weeks)

**Goal:** Work-stealing M:N scheduler without changing actor semantics.

**Scope:**
- N OS worker threads, M actors → work-stealing queue per worker.
- Actors park on `recv` and wake when a message arrives.
- Fuel-based preemption: count bytecode instructions or piggyback on the
  0162 yield checkpoints at function returns.
- **Deterministic test scheduler** installable as a handler for property
  testing — single thread, explicit interleaving control, reproducible
  traces. This is where Flux's testing story beats Erlang's.
- Semantics unchanged: same `Actor` surface behavior as Phase A–C.

**Prerequisites:** 0162 Phase 3 fully landed (yield checkpoints are the
preemption mechanism).

### Phase E — Unique-move transfer optimization (~4 weeks)

**Goal:** Elide the deep-copy when Aether proves the value is unique at the
send site.

**Scope:**
- At a `send` call site, if Aether's borrow inference proves the value is
  unique (single-owner, not aliased) at that point, transfer the `Rc`
  directly (zero-copy move). Otherwise fall back to deep-copy.
- Requires a small extension to the Aether verifier at actor boundaries.
- Zero user-visible change.
- Benchmarked to verify the optimization fires on common patterns (forwarded
  messages, freshly-constructed payloads, `recv → transform → send` chains).

**Prerequisites:** Aether borrow inference stable (already landed).

### Phase F — Shared-RC substrate (deferred — may never land)

**Honest recommendation: defer indefinitely.** Actors + unique-move
(Phases A–E) cover the overwhelming majority of use cases and keep Aether
clean. Shared RC is a large, risky runtime rewrite.

Do this only if a specific workload (e.g. shared read-only caches that
dwarf the working set, read-mostly configuration fan-out to many actors)
proves it necessary. Most Erlang-scale systems never need it.

If pursued, the 0143 earlier draft's Phase E/F/G content applies: explicit
promotion, shared-safe payload representation, verifier scope restrictions.

## Sequencing against other roadmap work
[sequencing]: #sequencing

| Release | Concurrency work | Prerequisites |
|---|---|---|
| v0.0.7 | Nothing. 0161 Phase 1 provides the `Actor` label infra for later. | 0161 Phase 1 |
| v0.0.8 | **Phase A** — Actor MVP. | 0161 done; 0162 Phase 1 optional |
| v0.0.9 | **Phase B** — Typed mailboxes + compile-time sendability. | 0152 (named fields) |
| v0.2.0 | **Phase C** — Supervision + cancellation. | 0162 Phase 3 strongly preferred |
| v0.3.0 | **Phase D** — Scheduler upgrade + deterministic test scheduler. | 0162 Phase 3 |
| v0.4.0 | **Phase E** — Unique-move transfer optimization. | Aether borrow inference stable |
| ≥ v1.0.0 | Phase F (shared RC) | user demand |

That's a two-year horizon to a robust actor story. Not ambitious —
realistic given per-phase scope and the prerequisite chain.

## Open design decisions
[open-decisions]: #open-decisions

### D1. One thread per actor vs. worker pool in Phase A?

- **Thread-per-actor:** simpler to reason about, no scheduler to debug, OS handles fairness. Doesn't scale past a few thousand actors.
- **Worker pool:** harder to implement, matches Erlang/OCaml 5. Needed for 10K+ actor workloads.

**Recommendation (locked in):** thread-per-actor for Phase A, worker pool in Phase D.

### D2. Typed mailbox: single-typed vs. open union?

- **Single-typed (`ActorId<Request>` receives only `Request`):** clean types, caller wraps in sum ADT for polymorphic mailboxes.
- **Open union (`ActorId<Request | Control | Query>`):** more flexible, complicates inference.

**Recommendation (locked in):** single-typed. Callers explicitly declare message variants via an ADT.

### D3. Default root handler for the `Actor` effect?

Yes — `main() with IO, Actor` needs someone to install the runtime handler.
The compiler injects a root handler at program entry that routes to the
real scheduler (thread-per-actor in Phase A, work-stealing pool in Phase D).

**Recommendation (locked in):** compiler-provided root handler, user-
swappable via `with_scheduler { … }` in tests.

### D4. Cancellation: cooperative or asynchronous?

- **Cooperative:** cancellation is checked at effect-handler boundaries. Simple, predictable, no UB.
- **Asynchronous:** thread injection. Fast, but notoriously hard to reason about (clean-up semantics, resource leaks, interrupt-safety).

**Recommendation (locked in):** cooperative only. The effect system makes
cancellation points natural — every `perform` is a potential yield point.

### D5. Determinism harness in scope for Phase D?

**Recommendation (locked in):** yes. Makes Flux's "proof-oriented" identity
meaningful for concurrent code. A deterministic scheduler + property test
harness + recorded traces is genuinely rare across FP languages and a
credible differentiator.

### Still open

- **D6. Mailbox bound:** unbounded queues (Erlang default) vs. bounded with backpressure (Pony default). Backpressure is safer for production; unbounded is simpler for MVP. Decision deferred to Phase B.
- **D7. Spawn-closure capture rules under module-scoped classes:** if a closure captures a class-method dispatch that itself captures handlers, is that sendable? Depends on 0151's semantics. Decision deferred until 0151 Phase 1b lands.

## Proposal merge guidance
[merge-guidance]: #merge-guidance

This proposal is the canonical concurrency roadmap.

- **0026** superseded — treated as historical vision, not active delivery plan.
- **0065** superseded — its `Actor` effect surface is absorbed into Phase A's `Flow.Actor` design.
- **0066** superseded — folded into Phase A implementation guidance.
- **0067** superseded — targeted the pre-Aether `Value::Gc` model which no longer exists.
- **0071** superseded — folded into Phase D scheduler implementation guidance.
- **0095** superseded — pre-Aether draft of this roadmap.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why actors first (not async/await)

Async/await is a language-surface convenience for a cooperative scheduler.
You can get it via effect handlers once the actor runtime exists — OCaml 5
demonstrates this. Shipping async/await before the runtime would be syntax
in search of semantics.

Actors solve the immediate need for concurrency while fitting Flux's
existing purity/effect story more naturally.

### Why not switch all values to `Arc`

Atomic refcounts on every Rc operation for a feature that 90% of code will
never touch. Koka explicitly rejected this with Perceus and Flux should too.

### Why explicit promotion instead of ambient sharing

Explicit promotion keeps local Aether assumptions valid and prevents hidden
backend/runtime broadening of ownership semantics.

### Why shared reuse is intentionally excluded

Shared reuse would require a separate legality and proof story. The initial
shared-RC extension (if it ever lands) keeps shared objects conservative and
preserves the current local fast path.

### Why supervision matters from day one

Erlang's defining insight: isolated actors make *systems* robust only if
death can be observed and acted on. Shipping actors without supervision
gives users a tool that can't survive its own first production incident.
Phase C is not optional.

### Why a deterministic test scheduler is non-negotiable

Flux's static-typing closure (0160) already makes "proof of correctness"
the brand. Shipping concurrency without a deterministic scheduler means
concurrent tests are probabilistic — incompatible with the brand.

## Prior art
[prior-art]: #prior-art

- **Erlang/OTP** — the gold standard for actor-based concurrency and supervision.
- **Elixir `GenServer` / `Task`** — ergonomic actor patterns on BEAM.
- **Pony** — capability-typed actors; proof-driven sendability.
- **OCaml 5** — effect-handler-based concurrency; scheduler as handler.
- **Koka / Perceus** — ownership optimization context, though no actor story.
- **Gleam** — actor-oriented FP direction on BEAM; close to where Flux lands syntactically.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

Superseded by the "Open design decisions" section above (D1–D7).

## Future possibilities
[future-possibilities]: #future-possibilities

- typed actor protocols (`ActorProtocol<State>` with state machines)
- `ask` / reply patterns as a library on top of Phase B
- `async/await` syntactic sugar over spawn+await once Phase D lands
- STM-style retries for actor state (nothing shared, but within-actor transactions could use effect handlers)
- distributed actors across OS processes (far future)
- shared-memory concurrency beyond actors in a separate future proposal (Phase F or its own umbrella)
