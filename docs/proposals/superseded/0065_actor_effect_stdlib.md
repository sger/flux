- Feature Name: Actor Effect Definition in Stdlib
- Start Date: 2026-03-01
- Status: Superseded (2026-04-18) by [Proposal 0143 — Actor Concurrency Roadmap (Aether-Aware)](../0143_actor_concurrency_roadmap.md)
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0065: Actor Effect Definition in Stdlib

## Summary
[summary]: #summary

Define the `Actor` algebraic effect in Flux's standard library as a first-class effect,
giving `spawn`, `send`, and `recv` typed effect operations rather than runtime primitives.
This establishes the semantic contract for Flux's concurrency model: actors are an effect,
the scheduler is a handler, and the type system enforces actor isolation at the boundary.

This proposal covers the Flux-level definition only. The runtime implementation
(threads, mailboxes, scheduler) is specified in proposal 0066.

## Motivation
[motivation]: #motivation

Adding `spawn`, `send`, and `recv` as PrimOps (like `print` or `len`) would be a design
mistake with permanent consequences:

1. **Purity holes**: PrimOps bypass the effect system. A pure function could call `recv()`
   without the compiler knowing. This contradicts Flux's pure-by-default guarantee.
2. **Non-composable**: PrimOps cannot be handled or intercepted. You could never write a
   test handler that mocks actor communication, or a simulation handler that runs actors
   deterministically.
3. **Wrong abstraction level**: The scheduler is a policy, not a language primitive.
   Different schedulers (thread-per-actor, M:N green threads, deterministic replay) should
   be swappable without changing user code.

The algebraic effect model solves all three:

```flux
-- Pure: cannot call recv() (type error E400)
fn compute(x: Int) -> Int {
    x * 2
}

-- Actor-capable: effect appears in signature
fn worker() with Actor, IO {
    let msg = recv()
    print(msg)
    worker()
}

-- The scheduler is a handler, swappable at program boundaries
fn main() with Actor, IO {
    let id = spawn(\() with Actor -> worker())
    send(id, "hello")
}
```

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The `Actor` effect

The `Actor` effect is defined in `lib/Flow/Actor.flx` and provides three operations:

```flux
-- lib/Flow/Actor.flx

-- ActorId is an opaque integer handle to a running actor.
-- It is safe to send across actor boundaries (it is just a u64).
type ActorId = Int

-- The Actor effect: concurrency operations
effect Actor {
    -- Spawn a new actor running the given function.
    -- The function must itself have the Actor effect (it will call recv).
    -- Returns the new actor's id.
    spawn : (fn() -> Unit with Actor) -> ActorId

    -- Send a message to the actor with the given id.
    -- Fire-and-forget: does not block.
    -- Returns Unit.
    send : (ActorId, Any) -> Unit

    -- Block until a message arrives in this actor's mailbox.
    -- Returns the message value.
    recv : () -> Any
}
```

### Using the Actor effect

```flux
import Flow.Actor

-- A worker actor that echoes messages to stdout
fn echo_worker() with Actor, IO {
    let msg = recv()
    print(msg)
    echo_worker()           -- tail-recursive: runs forever
}

-- A worker that processes exactly N messages then exits
fn count_worker(n: Int) with Actor, IO {
    if n == 0 {
        print("done")
    } else {
        let msg = recv()
        print(msg)
        count_worker(n - 1)
    }
}

-- Main spawns workers and sends messages
fn main() with Actor, IO {
    let worker1 = spawn(\() with Actor -> echo_worker())
    let worker2 = spawn(\() with Actor -> count_worker(3))

    send(worker1, "hello from main")
    send(worker2, "message 1")
    send(worker2, "message 2")
    send(worker2, "message 3")
}
```

### Effect propagation through higher-order functions

Because `send` and `recv` are effect operations, they propagate correctly through
higher-order combinators once proposal 0064 (row variables) is implemented:

```flux
-- Sending to all actors in a list: IO propagates naturally
fn broadcast(ids: List<ActorId>, msg: Any) with Actor {
    map(ids, \id -> send(id, msg))
}
```

### Typed messages (future, see §Future possibilities)

In Phase 1, messages are `Any`. This is intentional — the runtime performs deep copy on
send which is safe regardless of type. Typed channels are a future proposal.

### What `recv` returning `Any` means for the type system

`recv() -> Any` means the caller must pattern-match or cast the result. The type system
does not track message types in Phase 1. This is the same as Erlang/Elixir's approach.
Typed channels (proposal 0075, future) will refine this.

### Handlers: swapping the scheduler

The `Actor` effect can be handled by different handlers:

```flux
-- Thread-per-actor handler (default, from proposal 0066 runtime)
-- Installed automatically by the runtime for the main function.
-- User code does not write this handler; it is provided by the runtime.

-- Future: deterministic test handler
-- handle actor_program() with {
--     spawn(f) -> ... run f synchronously ...
--     send(id, msg) -> ... buffer the message ...
--     recv() -> ... return next buffered message ...
-- }
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### File: `lib/Flow/Actor.flx`

```flux
-- lib/Flow/Actor.flx
-- The Actor algebraic effect for Flux concurrency.

-- Opaque handle to a running actor.
-- Represented as a u64 at runtime (PrimOp ActorSpawn returns an Int).
type ActorId = Int

-- The Actor effect definition.
-- Operations: spawn, send, recv.
effect Actor {
    spawn : (fn() -> Unit with Actor) -> ActorId
    send  : (ActorId, Any) -> Unit
    recv  : () -> Any
}

-- Convenience: send to multiple actors
public fn broadcast(ids: List<ActorId>, msg: Any) with Actor {
    map(ids, \id -> send(id, msg))
}

-- Convenience: receive and apply a function
public fn recv_map(f: fn(Any) -> a) -> a with Actor {
    f(recv())
}

-- Convenience: receive exactly n messages, return as list
public fn recv_n(n: Int) -> List<Any> with Actor {
    if n == 0 {
        []
    } else {
        let msg = recv()
        [msg | recv_n(n - 1)]
    }
}
```

### Effect operation lowering

Each `Actor` effect operation lowers to a PrimOp through the `OpPerform` instruction:

```
Flux:       let id = spawn(fn)
Bytecode:   OpPush fn
            OpPerform ACTOR_SPAWN 1
            ; result (ActorId as Int) is on stack

Flux:       send(id, msg)
Bytecode:   OpPush id
            OpPush msg
            OpPerform ACTOR_SEND 2

Flux:       let msg = recv()
Bytecode:   OpPerform ACTOR_RECV 0
```

The `OpPerform` instruction looks up the current `HandlerDescriptor` for the `Actor`
effect. In the thread-per-actor runtime (proposal 0066), this descriptor dispatches to
the three PrimOps `ActorSpawn` (71), `ActorSend` (72), `ActorRecv` (73).

### Compiler: effect declaration parsing

The `effect` keyword introduces a new top-level statement type. The parser must support:

```
effect_decl := "effect" IDENT "{" effect_op* "}"
effect_op   := IDENT ":" type_sig
```

This requires a new `Statement::EffectDecl` variant:

```rust
// src/ast/mod.rs
pub enum Statement {
    // ... existing variants ...
    EffectDecl {
        name: Identifier,
        operations: Vec<EffectOperation>,
        span: Span,
    },
}

pub struct EffectOperation {
    pub name: Identifier,
    pub signature: FunctionType,   // param types + return type + effect row
    pub span: Span,
}
```

The compiler registers each effect operation in the symbol table during PASS 1, so that
calls to `spawn(...)`, `send(...)`, `recv()` within `with Actor` contexts resolve to
`OpPerform` rather than `OpCall`.

### Handler installation at program entry

The runtime installs the thread-per-actor handler before calling `main`:

```rust
// src/runtime/vm/mod.rs — in run_program() before dispatching main
fn install_actor_handler(vm: &mut Vm) {
    let descriptor = HandlerDescriptor {
        effect_name: interner.intern("Actor"),
        operations: vec![
            HandlerOp { name: "spawn", primop: PrimOp::ActorSpawn },
            HandlerOp { name: "send",  primop: PrimOp::ActorSend  },
            HandlerOp { name: "recv",  primop: PrimOp::ActorRecv  },
        ],
    };
    vm.push_handler(descriptor);
}
```

This means user code never writes `handle Actor with { ... }` to get the default
scheduler. It is ambient. Advanced users can shadow it with a custom handler for testing.

### Effect checking

The compiler enforces `Actor` effect membership in the same way as `IO`:

```
fn pure_fn() -> Int {
    recv()        -- E400: effect `Actor` not in scope (no `with Actor` annotation)
}

fn actor_fn() with Actor -> Int {
    recv()        -- OK
}
```

`spawn` requires the spawned function to itself have `with Actor` (transitively):

```
fn bad_spawn() with Actor {
    let id = spawn(\() -> 42)   -- E400: spawned fn must have `with Actor` effect
}

fn good_spawn() with Actor {
    let id = spawn(\() with Actor -> do {
        let _ = recv()
        unit
    })
}
```

## Drawbacks
[drawbacks]: #drawbacks

- The `effect` keyword requires a new statement kind in the parser and AST.
- `recv() -> Any` loses type information on messages. This is an intentional Phase 1
  simplification but may surprise users expecting type-safe channels.
- The stdlib file `lib/Flow/Actor.flx` requires `--root lib/` to be added to run commands
  that import it, adding friction to the getting-started experience.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not PrimOps?** Discussed in Motivation. PrimOps bypass the effect system, lose
composability, and cannot be mocked for testing.

**Why not a magic module?** A module `import Flow.Actor` that exports `spawn`, `send`,
`recv` as ordinary functions with `IO` effect would work superficially but cannot express
that `recv` *blocks the current actor* rather than performing generic I/O. The effect
operation distinction is semantic, not just organizational.

**Why `Any` for message type?** Typed channels require either parametric effect types
(`effect Channel<a>`) or a separate channel abstraction. Both are future proposals. `Any`
with runtime copy-on-send is the correct Phase 1 choice — safe, implementable, and
provides the semantics needed to validate the actor model end-to-end.

**Impact of not doing this:** Actors are added as PrimOps, creating a permanent purity
hole and non-composable API. The correct path requires redesigning from scratch later.

## Prior art
[prior-art]: #prior-art

- **Koka** — models `async`, `fork`, and channel effects identically. `effect channel<a>`
  with `send` and `recv` operations is the Koka reference design.
- **Eff language** — algebraic effects for concurrency, same model.
- **Frank** (Lindley, McBride, McLaughlin) — first-class effect handlers for concurrency.
- **Proposal 0026** — Flux's earlier concurrency model proposal (superseded by this).
- **Proposal 0032** — type system + effects foundation this builds on.
- **Proposal 0064** — row variables required for `Actor` to compose with higher-order fns.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should `ActorId` be an opaque `type` alias or a distinct newtype? Distinct newtype
   prevents `send(42, msg)` where `42` is a literal integer. Deferred to implementation.
2. Should `spawn` accept arguments to pass to the spawned function? For Phase 1, closure
   capture handles this. Explicit arguments are a future ergonomic improvement.
3. Should the `Actor` handler be installed globally (always available) or only when
   explicitly imported? Decision: install globally for `main`, require explicit import for
   library code.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Typed channels** (`effect Channel<a>` with typed `send`/`recv`) — eliminates `Any`
  and gives compile-time message type safety.
- **Selective receive** (`recv_match { pattern -> ... }`) — receive the first message
  matching a pattern, deferring non-matching messages.
- **Actor monitors** (`monitor(id) -> MonitorRef`) — receive a `Down` signal when a
  monitored actor dies.
- **Deterministic test handler** — a single-threaded handler for `Actor` that runs actors
  cooperatively in a fixed order, enabling deterministic actor unit tests.
- **Supervision trees** — a stdlib module built on `Actor` + monitors that provides
  one-for-one and one-for-all restart strategies.
