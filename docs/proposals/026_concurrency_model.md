# Proposal 026: Concurrency Model — Async/Await + Actors

**Status:** Proposed  
**Date:** 2026-02-12  
**Depends on:** None

---


**Status:** Proposed
**Priority:** High
**Created:** 2026-02-12
**Related:** Proposal 025 (Pure FP Vision), Proposal 024 (Runtime Instrumentation), Proposal 017 (GC)

## Summary

Flux adopts a two-layer concurrency model with an **actor-first rollout** for current compiler/runtime maturity:

1. **Actors (MVP first)** — isolated VM instances communicating via message passing for safe parallelism.
2. **Async/await (after actor MVP)** — cooperative concurrency for IO-bound work, layered on top of runtime scheduling primitives.

Shared-memory concurrency is explicitly rejected — it contradicts pure FP and would require replacing `Rc` with `Arc` across the entire value layer.

Why actor-first now:
- it matches Flux purity/isolation constraints with minimal aliasing risk,
- it avoids shipping partial async semantics before scheduler/error contracts are stable,
- it aligns with current parity-first release discipline (VM/JIT behavior lock before feature breadth).

## Motivation

Flux currently has no concurrency story. The VM runs a single instruction stream to completion. Real programs need:

- **IO concurrency** — fetch two URLs in parallel, read files while waiting for user input
- **CPU parallelism** — process data on multiple cores
- **Isolation** — failures in one task don't crash everything

Pure FP makes concurrency *easier* than in imperative languages: immutable values can be freely shared (or copied) without data races, and referential transparency means task ordering doesn't affect results.

## Current Architecture Constraints

### `Rc` Is Not Thread-Safe

Every heap-allocated value uses `Rc` ([value.rs:1](src/runtime/value.rs#L1)):

```rust
pub enum Value {
    String(Rc<str>),
    Some(Rc<Value>),
    Array(Rc<Vec<Value>>),
    Closure(Rc<Closure>),
    Hash(Rc<HashMap<HashKey, Value>>),
    // ...
}
```

`Rc` is `!Send + !Sync` in Rust. Values **cannot** cross thread boundaries without either:
- Replacing `Rc` with `Arc` (10-30% atomic refcount overhead on every value operation)
- Deep-copying values when sending between threads

### Single-Threaded VM

The VM has one stack, one frame list, one globals array:

```rust
pub struct VM {
    constants: Vec<Value>,
    stack: Vec<Value>,       // single stack
    globals: Vec<Value>,     // shared mutable globals
    frames: Vec<Frame>,      // single call chain
}
```

### Mutable Globals

`OpSetGlobal` writes to `globals` — a data race in any concurrent model. In a pure language, globals become constants set once at initialization, eliminating this problem.

## Design

### Layer 1: Actors (MVP, Multi-Threaded Isolation)

Each actor is an **isolated VM instance** running on its own OS thread. Actors communicate exclusively through message passing. Values are **deep-copied** when sent between actors — each actor has its own `Rc` heap.

#### Syntax (proposed)

```flux
actor Counter(initial: Int) {
  state count = initial

  receive Increment {
    count = count + 1
  }

  receive Get {
    reply(count)
  }
}

fn main() with IO, Async {
  let c = spawn Counter(0)
  send(c, Increment)
  let n = await ask(c, Get)
  print("count=" + to_string(n))
}
```

#### Message Operations

| Operation | Syntax | Behavior |
|-----------|--------|----------|
| **Spawn** | `spawn ActorName(args)` | Create actor, return `ActorRef<T>` |
| **Send**  | `send(actor, Msg)` | Fire-and-forget enqueue |
| **Ask**   | `await ask(actor, Msg)` | Request-reply (suspends caller) |
| **Stop**  | `stop(actor)` | Graceful actor shutdown |

#### Actor Runtime Contract

1. Actor mailbox ordering is FIFO per sender.
2. Actor processes one message at a time (no internal shared-state races).
3. Cross-actor values use transfer representation (`TransferValue`) and are reconstructed into local heap values.
4. Actor crash does not corrupt other actors; caller receives deterministic runtime failure.

### Layer 2: Async/Await (Single-Threaded Cooperative)

Cooperative multitasking on a single thread. The VM suspends a task at `await` points and resumes another ready task. **No threading, no `Rc` changes.**

#### Syntax

```flux
// Mark a function as async
fn fetch_user(id) with IO, Async {
  let response = await http_get("/users/#{id}")
  parse_json(response.body)
}

// Concurrent execution
fn load_dashboard(user_id) with IO, Async {
  // spawn concurrent tasks
  let profile = async fetch_profile(user_id)
  let posts = async fetch_posts(user_id)
  let notifications = async fetch_notifications(user_id)

  // await results
  {
    profile: await profile,
    posts: await posts,
    notifications: await notifications,
  }
}

// Sequential async (just use await inline)
fn process_in_order(urls) with IO, Async {
  urls |> map(\url -> await http_get(url))
}
```

#### Keywords

| Keyword | Meaning |
|---------|---------|
| `async` | Spawn a concurrent task, returns a `Future<T>` |
| `await` | Suspend current task until a `Future` completes |

#### VM Changes

New opcodes:

| Opcode | Operand | Effect |
|--------|---------|--------|
| `OpAsync` | const_idx | Spawn a new task from the closure at const_idx. Push `Future` handle onto stack. |
| `OpAwait` | — | Pop `Future` from stack. If resolved, push result. If pending, suspend current task, yield to scheduler. |

New runtime structures:

```rust
/// A suspended task — its own stack + frame state
struct Task {
    stack: Vec<Value>,
    sp: usize,
    frames: Vec<Frame>,
    frame_index: usize,
    status: TaskStatus,
}

enum TaskStatus {
    Ready,
    Suspended(FutureId),
    Completed(Value),
    Failed(String),
}

/// The scheduler manages tasks on a single thread
struct Scheduler {
    tasks: Vec<Task>,
    current: usize,
    event_loop: EventLoop,
}
```

#### Execution Model

```
1. Main task runs normally
2. `async expr` creates a new Task, adds it to Scheduler, pushes Future handle
3. Main task continues until it hits `await future`
4. If future is resolved → push result, continue
5. If future is pending → suspend main task, switch to next Ready task
6. When an IO event completes → mark waiting task as Ready
7. Scheduler round-robins through Ready tasks
8. All tasks share the same constants and globals (read-only after init)
```

No threads involved. The event loop polls for IO readiness (using `mio` or `polling` crate).

#### Error Handling

```flux
// Async tasks can fail — await propagates errors
fn safe_fetch(url) with IO, Async, Fail<HttpError> {
  let result = try {
    await http_get(url)
  }
  match result {
    Ok(response) => response.body,
    Err(e) => "fallback",
  }
}

// Multiple concurrent tasks — collect results
fn fetch_all(urls) with IO, Async {
  let futures = urls |> map(\url -> async http_get(url))
  let results = futures |> map(\f -> await f)
  results
}
```

#### What `Async` Covers

| Use Case | Example |
|----------|---------|
| HTTP requests | `await http_get(url)` |
| File IO | `await read_file(path)` |
| Timers | `await sleep(1000)` |
| Concurrent tasks | `let f = async compute(data)` |
| Parallel IO | Spawn multiple, await all |

> Note: async/await remains part of Proposal 026 scope, but lands after actor MVP to keep parity and diagnostics deterministic.

#### Message Types

Messages are defined by the actor's `receive` blocks. Each message is a tagged value:

```flux
actor Logger {
  receive Info(msg: String) {
    print("[INFO] #{msg}")
  }

  receive Warn(msg: String) {
    print("[WARN] #{msg}")
  }

  receive Error(msg: String, code: Int) {
    print("[ERROR #{code}] #{msg}")
  }
}
```

#### Actor Communication

| Operation | Syntax | Behavior |
|-----------|--------|----------|
| **Send** (fire-and-forget) | `send(actor, Message)` | Enqueue message, don't wait |
| **Ask** (request-reply) | `await ask(actor, Message)` | Send message, suspend until reply |
| **Spawn** | `spawn ActorName(args)` | Create new actor, returns `ActorRef` |
| **Stop** | `stop(actor)` | Gracefully shut down actor |

#### Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Actor A    │     │   Actor B    │     │   Actor C    │
│              │     │              │     │              │
│  Own VM      │     │  Own VM      │     │  Own VM      │
│  Own Stack   │     │  Own Stack   │     │  Own Stack   │
│  Own Rc Heap │     │  Own Rc Heap │     │  Own Rc Heap │
│              │     │              │     │              │
│  Mailbox ◄───┼─msg─┼──────────────┼─msg─┼───Mailbox   │
└──────────────┘     └──────────────┘     └──────────────┘
      │                     │                     │
      └─────── OS Thread ───┴──── OS Thread ──────┘
```

Each actor:
- Has its own `VM` instance (stack, frames, locals)
- Shares only compiled bytecode and constants (read-only, `Arc`-wrapped at the module level)
- Has a **mailbox** (channel receiver) for incoming messages
- Processes messages one at a time (no internal concurrency)
- Can `spawn` child actors
- Can use `async`/`await` internally for IO

#### Value Passing Between Actors

When `send(actor, value)` crosses an actor boundary:

1. The value is **deep-cloned** into a thread-safe representation
2. Sent through a `crossbeam` or `std::sync::mpsc` channel
3. Reconstructed as `Rc`-based values in the receiving actor's heap

For primitives (`Integer`, `Float`, `Boolean`, `None`) this is zero-cost — they're `Copy`.
For containers, the cost is proportional to size. In practice, messages should be small.

```rust
/// Thread-safe value representation for cross-actor messages
enum TransferValue {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(String),             // owned, not Rc
    None,
    Some(Box<TransferValue>),
    Array(Vec<TransferValue>),
    Hash(HashMap<HashKey, TransferValue>),
    // Functions/Closures: serialize as bytecode reference + captured values
}

impl Value {
    /// Deep-clone into thread-safe representation
    fn to_transfer(&self) -> TransferValue { ... }

    /// Reconstruct from transfer representation
    fn from_transfer(tv: TransferValue) -> Value { ... }
}
```

## Under the Hood: VM and JIT Execution Model

This section specifies how concurrency features should execute in Flux's two backends so implementation remains parity-safe.

### A. VM Runtime Path (authoritative semantics)

Concurrency semantics are defined by VM runtime behavior first. JIT must match these semantics.

#### A1. Actor operation flow

1. `spawn ActorName(args)`:
   - Evaluate args on current VM stack.
   - Serialize args to `TransferValue`.
   - Create actor instance (thread + mailbox + isolated VM state).
   - Return `ActorRef` handle value.

2. `send(actor, Msg(...))`:
   - Evaluate message payload.
   - Serialize to `TransferValue`.
   - Enqueue to actor mailbox.
   - Return `Unit`/`None`.

3. `ask(actor, Msg(...))` + `await`:
   - Evaluate and serialize message payload.
   - Create reply promise/future handle.
   - Enqueue request with correlation id.
   - Suspend caller task at `await` until reply is ready.
   - On resume, deserialize reply payload into VM value.

4. `stop(actor)`:
   - Send stop signal to actor loop.
   - Join/cleanup according to policy (graceful timeout is configurable).

#### A2. VM internal structures (minimum)

```rust
struct ActorRef {
    actor_id: u64,
}

struct ActorRuntime {
    mailbox_tx: Sender<TransferMessage>,
    // thread/join metadata
}

enum TransferMessage {
    FireAndForget { tag: String, payload: Vec<TransferValue> },
    Request { request_id: u64, tag: String, payload: Vec<TransferValue>, reply_to: Sender<TransferReply> },
    Stop,
}

struct TransferReply {
    request_id: u64,
    result: Result<TransferValue, TransferError>,
}
```

#### A3. VM opcode/lowering strategy

MVP strategy should prefer **builtin lowering first** (lower risk):
- `spawn/send/ask/stop` compile to Base/Runtime function calls via existing call path.
- This avoids introducing new opcodes before semantics stabilize.

Optional optimization (post-MVP):
- add dedicated opcodes (`OpSpawnActor`, `OpSendActor`, `OpAskActor`, `OpStopActor`) once behavior is locked.

### B. JIT Runtime Path (must mirror VM)

JIT should reuse VM-authoritative runtime helpers and avoid duplicating concurrency semantics.

#### B1. JIT execution contract

1. Parser + compiler still produce same bytecode-level semantics.
2. JIT lowers actor/concurrency operations to runtime helper calls (`src/jit/runtime_helpers.rs`).
3. Helper implementations reuse shared transfer/message runtime logic.
4. Error handling and diagnostics formatting must match VM class/signature policy.

#### B2. JIT integration points

Expected touchpoints:
- `src/jit/compiler.rs`: lower concurrency builtins/calls to helper stubs.
- `src/jit/runtime_helpers.rs`: actor operations + transfer conversion bridge.
- `src/jit/context.rs`: store async wait state, pending replies, and runtime error.
- `src/jit/mod.rs`: orchestration only (no semantic divergence).

#### B3. JIT parity requirements

For curated parity cases:
- successful runs: same observable output/value.
- failing runs: same normalized error class/signature.
- compile diagnostics: same tuple lock (`code/title/primary label`).

### C. Scheduling/Blocking Rules

1. Actor mailbox processing is single-threaded per actor.
2. `ask` is logically async; callers should not block OS threads in busy loops.
3. `await` suspension points are explicit and deterministic.
4. Backpressure defaults:
   - unbounded mailbox in MVP, bounded optional in Phase 3.
   - bounded behavior must return deterministic runtime error when full.

### D. Failure Semantics

1. Actor panic/crash:
   - does not crash unrelated actors by default.
   - requester receives failure reply (or timeout failure).
2. Unknown message tag:
   - deterministic runtime error (or compile-time rejection where statically known).
3. Reply timeout (if configured):
   - deterministic timeout error signature.

### E. Determinism and Diagnostics

1. VM and JIT share error class taxonomy for actor/runtime failures.
2. Diagnostic wording can differ in full text, but parity tests compare normalized signatures.
3. Compile-time checks should keep existing families when semantics match:
   - type mismatch: `E300`
   - effect boundary: `E400` family
   - unresolved strict boundary: `E425`

#### Supervision (Future Phase)

Basic fault tolerance — an actor can monitor its children:

```flux
actor Supervisor {
  receive Start {
    let worker = spawn Worker()

    // Monitor — get notified if worker dies
    monitor(worker)
  }

  receive ActorDown(ref, reason) {
    print("Worker died: #{reason}")
    // Restart
    let new_worker = spawn Worker()
    monitor(new_worker)
  }
}
```

Full supervision trees (Erlang OTP-style) are a future extension.

## Implementation Roadmap

### Phase 1: Actor MVP (Release Candidate Track)

| Step | What | Effort |
|------|------|--------|
| 1.1 | `actor` syntax + parser AST nodes (`actor`, `receive`, `state`) | Medium |
| 1.2 | `TransferValue` conversion (`Value <-> TransferValue`) | Medium |
| 1.3 | Actor runtime: mailbox + thread-per-actor loop | Medium |
| 1.4 | Builtins: `spawn` / `send` / `ask` / `stop` | Medium |
| 1.5 | Effect integration: `Async`/`Actor`-style effect requirements | Medium |
| 1.6 | Deterministic diagnostics + VM/JIT parity fixture matrix | Medium |

**Dependencies:** existing type/effect hardening and parity governance.

**Milestone:** isolated actor concurrency available with deterministic compile/runtime diagnostics and parity tests.

#### Phase 1 Detailed Instructions (VM/JIT)

1. Parser + AST
   - Add actor declaration and receive-arm parsing.
   - Keep actor syntax isolated from existing function/module grammar.
   - Add parser pass/fail fixtures for actor declarations and message patterns.

2. Compiler lowering
   - Lower `spawn/send/ask/stop` to runtime builtin call path first.
   - Enforce effect requirements at typed callsites.
   - Add compile-time message arity/type checks where constructor/message metadata is known.

3. VM runtime
   - Implement actor registry and mailbox runtime.
   - Implement `TransferValue` conversion for cross-actor payloads.
   - Implement request-reply correlation for `ask`.
   - Return deterministic runtime errors on serialization/unknown actor/reply mismatch.

4. JIT runtime
   - Add helper wrappers in `runtime_helpers.rs` that call shared actor runtime operations.
   - Ensure JIT error path sets context error with VM-matching signature fragments.
   - Avoid backend-specific behavior branches for actor semantics.

5. Parity tests
   - Add compile parity fixtures in existing snapshot matrix style.
   - Add runtime parity tests (value + error signature) following `tests/runtime_vm_jit_parity_release.rs` pattern.
   - Gate merges on VM/JIT parity for curated concurrency fixtures.

#### Phase 1 Repo File Targets (Decision-Complete)

Parser and syntax:
- `src/syntax/token_type.rs`
- `src/syntax/lexer/mod.rs`
- `src/syntax/parser/statement.rs`
- `src/syntax/parser/expression.rs`
- `src/syntax/parser/parser_test.rs`

AST and compiler front-end:
- `src/ast/statement.rs`
- `src/ast/expression.rs`
- `src/bytecode/compiler/statement.rs`
- `src/bytecode/compiler/expression.rs`
- `src/bytecode/compiler/mod.rs`
- `src/diagnostics/compiler_errors.rs`

Runtime VM:
- `src/runtime/value.rs`
- `src/runtime/vm/mod.rs`
- `src/runtime/vm/dispatch.rs`
- `src/runtime/vm/function_call.rs`
- `src/runtime/base/mod.rs`
- new actor runtime module(s): `src/runtime/actor/*`

Runtime JIT:
- `src/jit/compiler.rs`
- `src/jit/runtime_helpers.rs`
- `src/jit/context.rs`
- `src/jit/mod.rs`

CLI and integration wiring:
- `src/main.rs` (if new flags or runtime setup needed)

#### Backend Adapter Boundary (VM/JIT)

Concurrency semantics should live in one shared runtime module, with VM/JIT as backend adapters.

Proposed module layout:
- `src/runtime/actor/mod.rs` — public actor runtime API and orchestration.
- `src/runtime/actor/types.rs` — `ActorId`, envelopes, reply correlation IDs.
- `src/runtime/actor/mailbox.rs` — mailbox queue/channel logic.
- `src/runtime/actor/registry.rs` — global actor registry.
- `src/runtime/actor/supervisor.rs` — MVP one-for-one restart logic.
- `src/runtime/actor/transfer.rs` — cross-actor transfer representation and conversion.
- `src/runtime/actor/backend.rs` — backend trait + VM/JIT adapter contracts.

Backend contract (initial sketch):

```rust
pub trait RuntimeBackend {
    type ExecCtx;

    fn spawn_actor(&self, entry_fn: Symbol, args: Vec<TransferValue>) -> RuntimeResult<ActorId>;
    fn send(&self, to: ActorId, msg: TransferValue) -> RuntimeResult<()>;
    fn ask(&self, to: ActorId, msg: TransferValue, timeout_ms: Option<u64>) -> RuntimeResult<TransferValue>;
    fn stop(&self, id: ActorId) -> RuntimeResult<()>;
}
```

Implementations:
- `VmBackend` maps actor execution to bytecode VM instances (`src/runtime/vm/*`).
- `JitBackend` maps actor execution to JIT contexts/runtime helpers (`src/jit/*`).

Rules:
1. Actor semantics are backend-independent and implemented once in `runtime/actor/*`.
2. VM and JIT may differ in execution mechanics, never in actor semantics/diagnostics class.
3. `main` is treated as the root actor endpoint to keep request/reply symmetric.

#### Phase 1 Test Inventory (to add)

Rust tests:
- `tests/actor_parser_tests.rs`
- `tests/actor_compiler_rules_tests.rs`
- `tests/actor_vm_jit_parity_release.rs`
- `tests/actor_supervision_tests.rs` (phase-1 minimal restart)

Fixture files:
- pass fixtures under `examples/type_system/`:
  - `96_actor_spawn_send_ask_ok.flx`
  - `97_actor_typed_reply_ok.flx`
  - `98_actor_supervisor_restart_ok.flx`
- failing fixtures under `examples/type_system/failing/`:
  - `92_actor_pure_context_forbidden.flx`
  - `93_actor_message_arity_mismatch.flx`
  - `94_actor_ask_typed_mismatch.flx`
  - `95_actor_unknown_message_arm.flx`

Docs updates required per phase:
- `examples/type_system/README.md`
- `examples/type_system/failing/README.md`
- `docs/internals/type_system_effects.md`
- `docs/proposals/043_pure_flux_checklist.md`

#### Phase 1 Required Commands (release gate style)

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --test actor_parser_tests
cargo test --test actor_compiler_rules_tests
cargo test --all --all-features --test actor_vm_jit_parity_release
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Targeted smoke commands:

```bash
cargo run -- --no-cache examples/type_system/96_actor_spawn_send_ask_ok.flx
cargo run --features jit -- --no-cache examples/type_system/96_actor_spawn_send_ask_ok.flx --jit
cargo run -- --no-cache examples/type_system/failing/92_actor_pure_context_forbidden.flx
cargo run --features jit -- --no-cache examples/type_system/failing/92_actor_pure_context_forbidden.flx --jit
```

#### Phase 1 Acceptance Criteria

1. Actor syntax parses deterministically with actionable parser diagnostics.
2. `spawn/send/ask/stop` compile with effect/type enforcement in typed contexts.
3. VM actor runtime supports request-reply and deterministic failure signatures.
4. JIT matches VM behavior on curated actor parity matrix.
5. Compile diagnostics parity tuple (`code/title/primary label`) remains locked.

### Phase 2: Async Runtime (Cooperative)

| Step | What | Effort |
|------|------|--------|
| 2.1 | `Task` struct — save/restore VM state (stack, frames, sp) | Medium |
| 2.2 | `Scheduler` — round-robin task switching | Medium |
| 2.3 | `OpAsync` / `OpAwait` opcodes | Small |
| 2.4 | `EventLoop` — poll-based IO readiness (`mio` or `polling`) | Medium |
| 2.5 | Built-in async operations: `sleep`, `http_get`, `read_file_async` | Medium |
| 2.6 | Structured async diagnostics and cancellation policy | Medium |

**Milestone:** cooperative IO concurrency that composes with actors.

### Phase 3: Supervision and Tooling

| Step | What | Effort |
|------|------|--------|
| 3.1 | Supervision trees — restart strategies (one-for-one, all-for-one) | Large |
| 3.2 | Actor debugging — message tracing, mailbox inspection | Medium |
| 3.3 | Backpressure — bounded mailboxes, flow control | Medium |
| 3.4 | Actor pools — worker pools for load balancing | Medium |

**Milestone:** Production-ready actor system.

## What We Explicitly Skip

| Feature | Why |
|---------|-----|
| **Shared-memory threading** | Contradicts pure FP. Would require `Rc` → `Arc` everywhere (10-30% perf hit on all value ops). |
| **Mutexes / locks** | No shared mutable state in a pure language. |
| **Go-style goroutines** | Requires a complex work-stealing scheduler. Actors are simpler and more aligned with FP. |
| **Callback-based async** | Callback hell. `async`/`await` is strictly better UX. |
| **Colored function problem** | `async` functions are a different "color" than sync functions. We accept this trade-off for clarity — effects already distinguish pure from impure. |

## Diagnostics Contract (Proposed)

New diagnostics must follow existing Flux convention (stable code/title/primary label + actionable hint):

1. Actor operation in pure/incompatible effect context (`E4xx` family extension).
2. Invalid actor message shape/arity at send site.
3. `ask` result type mismatch (compile-time when typed, runtime fallback when dynamic).
4. Unknown actor message arm in `receive`.
5. Invalid `await` usage context (when async layer lands).

VM/JIT parity contract:
- compile diagnostics parity on tuple: `code/title/primary label`,
- runtime parity on normalized error class/signature for curated cases.

## Example: Complete Concurrent Program (MVP-oriented)

```flux
module App {
  // A worker that processes jobs
  actor Worker(id: Int) {
    receive Process(data: String) {
      let result = heavy_computation(data)
      reply(result)
    }
  }

  // A coordinator that distributes work
  actor Coordinator(worker_count: Int) {
    state workers = []

    receive Init {
      workers = range(0, worker_count)
        |> map(\i -> spawn Worker(i))
    }

    receive Submit(data: String) {
      // Round-robin to workers
      let worker = workers[len(workers) % hash(data)]
      let result = await ask(worker, Process(data))
      reply(result)
    }

    receive Shutdown {
      workers |> each(\w -> stop(w))
    }
  }

  fn main() with IO, Async {
    let coord = spawn Coordinator(4)
    send(coord, Init)

    // Submit work concurrently
    let jobs = ["data1", "data2", "data3", "data4"]
    let futures = jobs |> map(\j -> async ask(coord, Submit(j)))
    let results = futures |> map(\f -> await f)

    results |> each(\r -> print("Result: #{r}"))

    send(coord, Shutdown)
  }
}
```

## Additional Example Matrix (Pass/Fail)

### Pass: actor request-reply with typed payload

```flux
actor MathBox {
  receive Add(x: Int, y: Int) {
    reply(x + y)
  }
}

fn main() with IO, Async {
  let m = spawn MathBox()
  let n: Int = await ask(m, Add(20, 22))
  print(to_string(n))
}
```

### Fail: actor operation in pure function

```flux
fn bad() -> Int {
  let a = spawn MathBox()
  1
}
```

Expected: compile-time effect error (`E400` family policy).

### Fail: typed ask mismatch

```flux
fn main() with IO, Async {
  let m = spawn MathBox()
  let s: String = await ask(m, Add(1, 2))
  print(s)
}
```

Expected: compile-time type mismatch (`E300`).

## References

- Erlang/OTP actor model — supervision, mailboxes, "let it crash"
- Elixir `Task` and `GenServer` — friendly actor API
- Kotlin coroutines — structured concurrency with `async`/`await`
- Rust `tokio` — poll-based async runtime architecture
- Gleam — pure FP on BEAM with actors (closest spiritual match)
