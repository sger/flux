- Feature Name: Actor Runtime Architecture and Backend Isolation
- Start Date: 2026-03-08
- Status: Superseded (2026-04-18) by [Proposal 0143 — Actor Concurrency Roadmap (Aether-Aware)](../0143_actor_concurrency_roadmap.md)
- Proposal PR:
- Flux Issue:

# Proposal 0095: Actor Runtime Architecture and Backend Isolation

## Summary
[summary]: #summary

Define the runtime architecture for Flux actors across the VM and Cranelift JIT backends.

This proposal answers four core design questions:

1. Does `spawn` create a new VM/JIT execution context?
2. How do actors communicate with the currently running program, including `main`?
3. What state is shared across actors, and what is isolated?
4. How should the actor subsystem be organized in Rust?

The core decision is:

> In the MVP actor model, each actor has its own isolated execution context. For the VM
> backend, this means a new VM instance per actor. For the JIT backend, this means a new
> JIT actor execution context per actor. Actors communicate only through a shared
> `runtime::actor` subsystem using actor ids, mailboxes, and sendable messages.

This proposal complements:

- [0065](0065_actor_effect_stdlib.md) for the Flux-level `Actor` effect
- [0066](0066_thread_per_actor_handler.md) for the thread-per-actor runtime MVP
- [0084](implemented/0084_aether_memory_model.md) for actor-boundary memory rules
- [0086](0086_backend_neutral_core_ir.md) for backend-neutral lowering

## Motivation
[motivation]: #motivation

Flux's current runtime architecture is naturally single-execution-state:

- one VM stack
- one frame chain
- one current call path
- local values represented with `Rc`

That architecture does not safely generalize to concurrent actors by sharing a single
execution context. Trying to make actors "just tasks inside one VM" too early creates
problems immediately:

1. shared mutable execution state
2. difficult suspension/resumption semantics
3. unclear VM/JIT parity story
4. unsafe cross-thread use of actor-local `Rc` graphs
5. harder alignment with Aether's isolation model

Flux needs the simplest correct answer first:

- isolated execution contexts
- shared actor runtime coordinator
- explicit message boundaries

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The main program is also an actor

The main Flux program should be treated as the first actor:

```text
Actor 0 = main
Actor 1 = first spawned actor
Actor 2 = second spawned actor
...
```

This is important because it keeps the model uniform:

- `main` can send to spawned actors
- spawned actors can send back to `main`
- there is no special "out of band" control channel

### What `spawn` does

When a running actor evaluates:

```flux
let pid = spawn(\() -> worker())
```

the runtime should:

1. allocate a new actor id
2. create a mailbox for the new actor
3. create a new isolated execution context
4. start the actor at the target function/closure entry
5. return the new `ActorId` to the caller

For the VM backend, step 3 means a new VM instance.

For the JIT backend, step 3 means a new JIT actor execution context that uses the same
shared compiled code but has its own actor-local runtime state.

### What is shared

Actors may share immutable program-level artifacts:

- compiled function metadata
- module metadata
- symbol/interner tables
- JIT code memory
- actor runtime registry and mailbox infrastructure

### What is isolated

Actors must not share:

- stacks
- call frames
- local environments
- raw `Rc<Value>` graphs
- actor-local continuations
- actor-local mutable runtime state

### How communication works

Actors communicate only through the shared actor runtime.

Conceptually:

```text
Actor A
  -> ActorRuntime.send(ActorId(B), msg)
  -> mailbox of actor B

Actor B
  -> ActorRuntime.recv(ActorId(B))
  -> next queued message
```

No actor directly reaches into another actor's VM or JIT context.

### Main program communication example

If the main program spawns three actors:

```flux
fn main() with Actor, IO {
    let a = spawn(\() -> worker("A"))
    let b = spawn(\() -> worker("B"))
    let c = spawn(\() -> worker("C"))

    send(a, "job1")
    send(b, "job2")
    send(c, "job3")

    let r1 = recv()
    let r2 = recv()
    let r3 = recv()

    print(r1)
    print(r2)
    print(r3)
}
```

the runtime topology is:

```text
Actor 0 = main
Actor 1 = A
Actor 2 = B
Actor 3 = C
```

and messages flow through the registry/mailboxes:

```text
Actor 0 -> mailbox 1
Actor 0 -> mailbox 2
Actor 0 -> mailbox 3

Actor 1 -> mailbox 0
Actor 2 -> mailbox 0
Actor 3 -> mailbox 0
```

This is the same communication model for every actor. `main` is not special beyond being
the first actor started by the runtime.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Dedicated Rust module

The actor subsystem should live in a dedicated runtime module:

```text
src/runtime/actor/
  mod.rs
  runtime.rs
  registry.rs
  mailbox.rs
  sendable.rs
  context.rs
  vm.rs
  jit.rs
  error.rs
```

Responsibilities:

- `runtime.rs`
  - `ActorRuntime`
  - actor lifecycle orchestration
  - public runtime API for spawn/send/recv

- `registry.rs`
  - actor id allocation
  - actor lookup
  - mailbox sender registry

- `mailbox.rs`
  - blocking/non-blocking receive
  - queue semantics
  - close/shutdown behavior

- `sendable.rs`
  - backend-neutral message boundary format
  - local-value -> sendable conversion
  - sendable -> local-value reconstruction

- `context.rs`
  - shared actor execution context definitions/traits

- `vm.rs`
  - VM actor bootstrapping
  - new VM instance creation

- `jit.rs`
  - JIT actor bootstrapping
  - JIT actor execution context creation

- `error.rs`
  - actor runtime errors

### Shared coordinator model

The shared actor runtime is the communication hub:

```text
ActorRuntime
  registry: ActorId -> Mailbox sender
  program_handle
  backend handle/config
```

All actors in a running program share the same `ActorRuntime`.

This is the only shared mutable concurrency coordinator in the MVP design.

### Execution context model

Recommended backend-neutral shape:

```rust
pub struct ActorRuntime { /* registry, program handle, backend support */ }

pub struct ActorId(pub u64);

pub struct ActorHandle {
    pub id: ActorId,
}

pub enum BackendActorContext {
    Vm(VmActorContext),
    Jit(JitActorContext),
}
```

The exact Rust types may differ, but the semantic model should remain:

- each actor has its own execution context
- each execution context belongs to exactly one actor

### VM actor model

For the VM backend:

- spawning an actor creates a new VM instance
- that VM instance reuses shared immutable compiled program data where safe
- but has its own:
  - stack
  - frames
  - actor-local values
  - mailbox receive loop state

This avoids shared-VM concurrency semantics in Phase 1.

### JIT actor model

For the JIT backend:

- spawning an actor creates a new JIT actor execution context
- the actor may share compiled code/module handles with other actors
- but it has its own:
  - actor-local runtime context
  - call/continuation state
  - mailbox receive loop state

This keeps JIT semantics aligned with the VM model without requiring a literal VM object.

### Message boundary

Actor messages must cross a backend-neutral sendability boundary.

Required rule:

- no raw actor-local `Rc<Value>` graph crosses actor boundaries

Instead:

1. sender converts local value to `SendableValue`
2. runtime enqueues `SendableValue`
3. receiver reconstructs a local value from `SendableValue`

This rule applies equally to:

- VM -> VM
- JIT -> JIT
- VM -> JIT
- JIT -> VM

If mixed-backend actor communication is allowed in the same process, `SendableValue` is
what makes it coherent.

### Main actor bootstrapping

Program startup should create the main actor explicitly:

1. create `ActorRuntime`
2. allocate `ActorId(0)` for the main actor
3. create mailbox `0`
4. create the main execution context
5. run `main` inside that actor context

This ensures:

- `main` can call `recv()`
- workers can reply to `main`
- runtime accounting is uniform from the beginning

### Communication semantics

#### `spawn`

Logical flow:

```text
current actor
  -> ActorRuntime.spawn(entry)
  -> registry allocates ActorId
  -> mailbox created
  -> backend-specific actor context created
  -> actor thread/task started
  -> ActorId returned
```

#### `send`

Logical flow:

```text
current actor
  -> ActorRuntime.send(target, value)
  -> value converted to SendableValue
  -> target mailbox looked up in registry
  -> message enqueued
```

#### `recv`

Logical flow:

```text
current actor
  -> ActorRuntime.recv(self_actor_id)
  -> block on mailbox
  -> dequeue SendableValue
  -> reconstruct local value
  -> return to actor execution context
```

### Failure policy

The MVP actor runtime should keep failure rules simple and deterministic.

Recommended Phase 1 behavior:

- sending to a dead actor: no-op or explicit closed-mailbox failure, but consistent
- non-sendable value crossing boundary: deterministic runtime error
- actor crash: isolated to that actor unless a later supervision model is introduced
- closed mailbox receive: deterministic end-of-actor behavior or runtime error

Whatever choice is made must be consistent across VM and JIT.

### Relationship to Aether

This actor model aligns with Aether:

- within one actor, Aether may use `Rc` and reuse normally
- across actor boundaries, messages are copied/rebuilt through the sendable boundary
- future transfer semantics may optimize this, but are not required for the MVP

### Non-goals

This proposal does not require:

1. a shared multi-actor VM scheduler
2. general async/await semantics
3. zero-copy actor message transfer in Phase 1
4. supervision trees
5. distributed actors

## Drawbacks
[drawbacks]: #drawbacks

1. One execution context per actor is heavier than a future M:N scheduler.
2. Copy/rebuild message passing adds overhead compared with hypothetical zero-copy transfer.
3. The runtime must maintain shared actor infrastructure in addition to backend execution.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why create a new VM/JIT context per actor?

Because it is the simplest correct way to preserve actor isolation with Flux's current
runtime/value model.

### Why not put multiple actors inside one shared VM immediately?

Because that couples actor scheduling to stack management, continuation handling, and local
heap/value safety too early. It is a more advanced design and should be deferred.

### Why make `main` an actor?

Because it simplifies the semantics:

- uniform communication model
- no special reply channel
- `recv()` works naturally in `main`

### Why a dedicated Rust module?

Because actor semantics are a runtime subsystem, not just a VM feature or a JIT feature.
Keeping the subsystem centralized prevents semantic duplication.

## Prior art
[prior-art]: #prior-art

- actor runtimes with isolated process/task state and mailbox messaging
- BEAM-style "main process + child processes" mental model
- thread-per-actor MVP designs used before green-thread schedulers

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should `ActorId(0)` for `main` be user-visible or only available through helper APIs?
2. Should sending to a dead actor be a no-op or an explicit runtime error in Phase 1?
3. Should mixed VM/JIT actor execution be allowed in the same runtime, or should all
   actors in one program share one backend mode initially?
4. How should reply patterns (`ask`, `reply`) be layered over the mailbox model?
5. What minimal actor lifecycle metadata should be exposed to tooling/debugging?

## Future possibilities
[future-possibilities]: #future-possibilities

- zero-copy transfer of uniquely owned messages
- supervision trees
- M:N scheduler backend
- actor tracing and observability tooling
