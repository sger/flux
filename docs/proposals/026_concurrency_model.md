# Proposal 026: Concurrency Model — Async/Await + Actors

**Status:** Proposed
**Priority:** High
**Created:** 2026-02-12
**Related:** Proposal 025 (Pure FP Vision), Proposal 024 (Runtime Instrumentation), Proposal 017 (GC)

## Summary

Flux adopts a two-layer concurrency model:

1. **Async/await** — cooperative, single-threaded concurrency for IO-bound work. No threading, no `Rc` changes. Ships first.
2. **Actors** — isolated VM instances communicating via message passing for CPU-bound parallelism. Each actor owns its own `Rc` heap. Ships second.

Shared-memory concurrency is explicitly rejected — it contradicts pure FP and would require replacing `Rc` with `Arc` across the entire value layer.

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

### Layer 1: Async/Await (Single-Threaded)

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

### Layer 2: Actors (Multi-Threaded)

Each actor is an **isolated VM instance** running on its own OS thread. Actors communicate exclusively through message passing. Values are **deep-copied** when sent between actors — each actor has its own `Rc` heap.

#### Syntax

```flux
// Define an actor
actor Counter(initial: Int) {
  // State is local to this actor — mutable within the actor
  state count = initial

  // Handle incoming messages
  receive Increment {
    count = count + 1
  }

  receive Decrement {
    count = count - 1
  }

  receive Get {
    reply(count)
  }

  receive Add(n) {
    count = count + n
    reply(count)
  }
}

// Usage
fn main() with IO, Async {
  // Spawn actor — returns an ActorRef
  let counter = spawn Counter(0)

  // Fire-and-forget message
  send(counter, Increment)
  send(counter, Increment)
  send(counter, Add(10))

  // Request-reply (async — suspends until reply)
  let value = await ask(counter, Get)  // => 12
  print("Count: #{value}")

  // Stop the actor
  stop(counter)
}
```

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

### Phase 1: Async Runtime (No Threading)

| Step | What | Effort |
|------|------|--------|
| 1.1 | `Task` struct — save/restore VM state (stack, frames, sp) | Medium |
| 1.2 | `Scheduler` — round-robin task switching | Medium |
| 1.3 | `OpAsync` / `OpAwait` opcodes | Small |
| 1.4 | `EventLoop` — poll-based IO readiness (using `mio` or `polling`) | Medium |
| 1.5 | `Async` effect annotation in type system | Small (after type system exists) |
| 1.6 | Built-in async operations: `sleep`, `http_get`, `read_file_async` | Medium |

**Dependencies:** Compiler changes to parse `async`/`await` keywords. No type system required for initial version (add effect annotation later).

**Milestone:** `async`/`await` works for IO concurrency on a single thread.

### Phase 2: Actor System

| Step | What | Effort |
|------|------|--------|
| 2.1 | `actor` keyword in parser — parse actor declarations | Medium |
| 2.2 | `TransferValue` — serialize/deserialize values across thread boundaries | Medium |
| 2.3 | Actor runtime — spawn OS thread per actor, mailbox via `crossbeam` channel | Medium |
| 2.4 | `spawn` / `send` / `ask` / `stop` builtins | Medium |
| 2.5 | Actor-internal `async`/`await` — each actor runs its own scheduler | Small (reuse Phase 1) |
| 2.6 | `monitor` / `ActorDown` — basic fault notification | Medium |

**Dependencies:** Phase 1 (async runtime). Shared bytecode wrapped in `Arc` for thread safety.

**Milestone:** Actors run on multiple threads with message passing.

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

## Example: Complete Concurrent Program

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

## References

- Erlang/OTP actor model — supervision, mailboxes, "let it crash"
- Elixir `Task` and `GenServer` — friendly actor API
- Kotlin coroutines — structured concurrency with `async`/`await`
- Rust `tokio` — poll-based async runtime architecture
- Gleam — pure FP on BEAM with actors (closest spiritual match)
