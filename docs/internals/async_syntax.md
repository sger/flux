# Async & Concurrency (Reference)

> **Proposal:** [0174 — Async Effect & Concurrency Roadmap](../proposals/0174_async_effect_concurrency.md)
> **Surface modules:** [`Flow.Async`](../../lib/Flow/Async.flx) · [`Flow.Task`](../../lib/Flow/Task.flx) · [`Flow.Channel`](../../lib/Flow/Channel.flx) · [`Flow.Event`](../../lib/Flow/Event.flx) · [`Flow.Stream`](../../lib/Flow/Stream.flx) · [`Flow.Http`](../../lib/Flow/Http.flx) · [`Flow.Tcp`](../../lib/Flow/Tcp.flx)
> **Examples:** [`examples/async/`](../../examples/async/) · **Tutorial:** [`examples/guide_async/`](../../examples/guide_async/)
> **Related internals:** [effect_row_system.md](effect_row_system.md) · [type_system_effects.md](type_system_effects.md)

This is the reference for Flux's async and concurrency surface. It starts from the **two core ideas a newcomer needs — fibers and tasks** — with runnable examples, then works outward to channels, events, streams, errors, HTTP, and finally the runtime internals and backend (VM vs native) details.

If you just want to *use* async, read §1–§4 and stop. The "Under the hood" (§18) and "Backend semantics" (§17) sections are for people working on the runtime.

---

## 1. Mental model

Flux async is an **effect-row + handler** system — not `async fn` / `await`. Three principles:

1. **No function coloring.** A function that may suspend simply carries the `Async` effect in its row (`with Async`). There is no `await` sigil and no contagious "async-ness" — just an effect that is type-checked like any other.
2. **Handlers at boundaries.** `run_async(action)` is the boundary that installs the async scheduler. Inside it, work is scheduled cooperatively. Outside it, you cannot call anything with `Async` in its row.
3. **Structured concurrency.** Every concurrent unit has a clear owner — a `scope`, a `both`/`race`, a `timeout`, or the root `run_async`. Cancellation is first-class and propagates down the ownership tree.

---

## 2. Fibers and Tasks — the two kinds of concurrency

Almost everything below builds on **two primitives**. Understanding the difference is the whole game.

### A fiber is a cheap, cooperative unit *inside* `run_async`

A **fiber** is a lightweight green-thread scheduled by the async runtime. You rarely create one directly — `both`, `race`, `timeout`, and `fork` all spawn fibers for you. Fibers are cheap (thousands per program), and they **cooperate**: a fiber runs until it *suspends* (e.g. `sleep`, waiting on I/O, `yield_now`), at which point another fiber gets to run.

```flux
import Flow.Async exposing (..)

fn slow()  -> Int with Async { sleep(50); 1 }   // suspends for 50ms
fn quick() -> Int with Async { sleep(10); 2 }   // suspends for 10ms

fn body() -> (Int, Int) with Async {
    both(slow, quick)        // both fibers overlap → ~50ms total, returns (1, 2)
}

fn main() with IO {
    print(run_async(body))   // → (1, 2)
}
```

`slow` and `quick` run *concurrently*: while `slow` is parked on its timer, `quick` runs. Total wall-clock is ~50ms, not 60ms.

### A task is an OS-thread unit for *parallel* work

A **task** (`Flow.Task`) runs on a real OS worker thread, so it gives you **CPU parallelism** for compute-bound work. Tasks cross a thread boundary, so their result type must be [`Sendable`](#14-the-sendable-type-class) (§14).

```flux
import Flow.Task as Task

fn sum_squares(n: Int, acc: Int) -> Int {
    if n <= 0 { acc } else { sum_squares(n - 1, acc + n * n) }
}

fn main() with IO {
    let a = Task.spawn(fn() { sum_squares(100000, 0) })   // runs on a worker thread
    let b = Task.spawn(fn() { sum_squares(200000, 0) })   // …in parallel with a
    print(Task.blocking_join(a) + Task.blocking_join(b))
}
```

`spawn` returns immediately with a handle; the two computations run on different cores; `blocking_join` waits for the result.

### Which one do I use?

| You want… | Use | Why |
|---|---|---|
| Overlap **I/O** (HTTP calls, timers, sockets) | **Fibers** — `both` / `race` / `fork` | Cheap; they suspend on I/O so one thread serves many |
| **CPU-bound** work across cores | **Tasks** — `Task.spawn` + `Task.await` | Real OS threads → true parallelism |
| Both at once | `Task.spawn` *from* a fiber, then `Task.await` | Offload compute, keep serving I/O on the fiber |
| Join from non-async `main` | `Task.blocking_join` | No fiber context needed |

> **Note on the two backends.** Both fibers *and* tasks run with real parallelism on **both** the VM and the native/LLVM backend — by default `run_async` uses `available_parallelism()` worker OS threads on each (see [§17](#17-backend-semantics-vm-vs-native)). The old "VM fibers are single-threaded" rule no longer holds; it applies only when you pin `run_async_with_workers(1, …)`.

The rest of this document is the detail behind these two ideas.

---

## 3. The `Async` effect row

`Async` is an effect alias declared in [`lib/Flow/Effects.flx`](../../lib/Flow/Effects.flx):

```flux
alias Async = <Suspend | Fork | GetContext | AsyncFail>
```

| Atom | Purpose |
|---|---|
| `Suspend` | Pause the current fiber (sleep, await, blocking I/O) |
| `Fork` | Spawn a child fiber inside the enclosing scope |
| `GetContext` | Read scheduler-owned state (cancel flag, fiber id, scope) |
| `AsyncFail` | Raise an `AsyncError`, the failure channel for structured concurrency |

You almost always write `with Async`, never the four atoms separately. The common exception is composing with another ambient effect:

```flux
fn fetch_then_log() -> Unit with Async, Console {
    let body = http_get("https://example.com")
    print(body)
}
```

`Async` composes with any other atom in a closed row. Effect-row rules (additivity, subtraction, row variables) are the general ones — see [effect_row_system.md](effect_row_system.md).

**Row variables** let higher-order combinators stay effect-generic:

```flux
public fn timeout<a>(ms: Int, f: () -> a with Async | e) -> Option<a> with Async | e
```

The `| e` lets the caller's body carry its own ambient effects without polluting the combinator's signature.

---

## 4. Entering async: `run_async` and friends

```flux
public fn run_async<a>(action: () -> a with Async | e) -> a
public fn run_async_with<a>(cfg: RuntimeConfig, action: () -> a with Async | e) -> a
public fn run_async_with_workers<a>(n: Int, action: () -> a with Async | e) -> a
```

`run_async` installs the scheduler, runs `action` to completion, and returns its value. It is the *only* way to call `Async` code from a non-async caller, and it **blocks the calling OS thread** until the root finishes (analogous to `tokio::Runtime::block_on`).

```flux
fn body() -> Int with Async { 42 }

fn main() with IO {
    print(run_async(body))                       // → 42
    print(run_async_with_workers(4, body))       // pin 4 workers
}
```

### `current_worker_count` (introspection)

```flux
public fn current_worker_count() -> Int with Async
```

Reports the worker count of the active scheduler — useful to confirm a `RuntimeConfig` was honoured. With no config, it resolves to `FLUX_WORKERS` → `available_parallelism()` → `2`.

```flux
fn main() with IO {
    print(run_async_with_workers(8, current_worker_count))   // → 8
}
```

See [`examples/async/16_current_worker_count.flx`](../../examples/async/16_current_worker_count.flx).

---

## 5. Suspending and interleaving

### `sleep`

```flux
public fn sleep(ms: Int) -> Unit with Async
```

Parks the current fiber for at least `ms` milliseconds, freeing its worker to run other ready fibers. Backed by the `mio` timer reactor.

### `yield_now`

```flux
public fn yield_now() -> Unit with Async
```

Cooperative reschedule hint: return the fiber to the back of its worker's ready queue so a sibling can run. See [`02_sleep_yield.flx`](../../examples/async/02_sleep_yield.flx).

---

## 6. Running things concurrently

### `both`

```flux
public fn both<a, b>(f: (() -> a with Async | e1), g: (() -> b with Async | e2)) -> (a, b) with Async
```

Runs `f` and `g` as siblings under a hidden scope; returns `(f_result, g_result)` once **both** finish. Tuple position is **source order**, not finish order. If either branch raises, the other is cancelled and the error propagates. See [`03_both.flx`](../../examples/async/03_both.flx).

### `race`

```flux
public fn race<a>(f: (() -> a with Async | e1), g: (() -> a with Async | e2)) -> a with Async
```

Runs both, returns the **first** to finish; the loser is cancelled (its pending I/O aborts, its `bracket`/`finally` cleanup arms run). Both branches must produce the same type. See [`04_race.flx`](../../examples/async/04_race.flx).

### `first_of` / `first`

```flux
public fn first_of<a>(fs: List<() -> a with Async>) -> (Int, a) with Async
public fn first<a>(fs: List<() -> a with Async>)    -> a with Async
```

N-way race. `first_of` returns `(winning_index, value)`; `first` drops the index. Source order breaks immediate ties (lower index wins). Empty list panics. See [`05_first_of.flx`](../../examples/async/05_first_of.flx).

---

## 7. Timeouts

```flux
public fn timeout<a>(ms: Int, f: () -> a with Async | e) -> Option<a> with Async | e
public fn timeout_result<a>(ms: Int, f: () -> a with Async | e) -> Result<a, AsyncError> with Async | e
```

`timeout` returns `Some(v)` if `f` finishes in time, `None` if the timer wins (and `f` is cancelled). `timeout_result` distinguishes the three outcomes: `Ok(v)`, `Err(TimedOut)`, or `Err(other)` (the body raised independently). Inspect with the `result_*` helpers (§8.3). See [`06_timeout.flx`](../../examples/async/06_timeout.flx).

---

## 8. Errors

### 8.1 `AsyncError`

```flux
public data AsyncError {
    Canceled,
    TimedOut,
    Panicked(String),
    IoError(Int, String, String),       // (errno, message, syscall)
    DnsError(Int, String, String),      // (code, message, host)
    ProtocolError(Int, String),         // (status, message)
    ConnectionClosed,
    InvalidAddress(String),
}
```

Only two variants have constructor helpers — `canceled_error()` and `protocol_error(status, msg)`. The rest are produced by the runtime when I/O fails; match on them in handlers, don't construct them.

### 8.2 `fail` / `try`

```flux
public fn fail<a>(err: AsyncError) -> a with Async
public fn try<a>(body: () -> a with Async | e) -> Result<a, AsyncError> with Async | e
```

`fail` raises in the current fiber and propagates outward — siblings under the same scope are cancelled, unwinding to the nearest `try` (or to `run_async`, where it surfaces as a panic). `try` is the recovery primitive and catches **both** explicit `fail` and panics.

```flux
fn body() -> Int with Async { fail(canceled_error()) }

fn caught() -> Bool with Async {
    result_is_ok(try(body))             // → false
}
```

See [`07_try_fail.flx`](../../examples/async/07_try_fail.flx).

### 8.3 Result helpers

`import Flow.Async exposing (..)` brings `Result<a, e>` and its `Ok`/`Err` constructors into scope, so direct pattern matching works. The helpers are for when you don't want to match:

| Helper | Purpose |
|---|---|
| `result_is_ok(r)` | `Bool` — true if `Ok` |
| `result_is_timed_out(r)` | `Bool` — true if `Err(TimedOut)` |
| `result_or(r, fallback)` | `a` — value or fallback |
| `result_or_else_async(r, fallback, ok_fn)` | continuation form |
| `result_or_timeout_with_async(r, t_val, e_val, ok_fn)` | three-way fork on `Ok` / `Err(TimedOut)` / other |

### 8.4 Resource safety: `finally` / `bracket`

```flux
public fn finally<a>(body: () -> a, cleanup: () -> Unit with Async) -> a with Async
public fn bracket<r, c, a>(acquire: () -> r, release: (r) -> c, body: (r) -> a with Async) -> a with Async
```

`finally` runs `cleanup` on success, failure, **and** cancellation. `bracket` is the acquire/use/release pattern; `release` always runs at the end (its return value is discarded). See [`08_finally_bracket.flx`](../../examples/async/08_finally_bracket.flx).

---

## 9. Structured concurrency

```flux
public fn scope<a>(f: (Scope) -> a with Async | e) -> a with Async | e
public fn fork<a>(s: Scope, f: () -> a with Async | e) -> Unit with Async | e
public fn cancel(s: Scope) -> Unit with Async
```

`scope` allocates a fresh cancellation boundary and passes it to `f`. `fork` schedules a child fiber under that scope (returns immediately). `cancel` cancels every fiber forked under `s` — each child's pending I/O aborts and its continuation resumes with `Canceled` so cleanup arms run. Idempotent.

With `import Flow.Async exposing (..)`, the `Scope` type is unqualified, so you can annotate helpers: `fn child(s: Scope) -> Unit with Async { … }`.

### Cooperative cancellation in CPU loops

```flux
public fn check_cancelled()    -> Bool with Async
public fn bail_if_cancelled() -> Unit with Async
```

A pure CPU loop has no suspension point at which the scheduler can deliver cancellation. Sprinkle `bail_if_cancelled()` (raises `Canceled` if the flag is set) or `check_cancelled()` (returns `Bool` so you can clean up and return a partial result) inside hot loops:

```flux
fn cpu_work(n: Int, acc: Int) -> Int with Async {
    if n <= 0 { acc }
    else {
        bail_if_cancelled()
        cpu_work(n - 1, acc + n)
    }
}
```

See [`09_scope_fork_cancel.flx`](../../examples/async/09_scope_fork_cancel.flx) and [`10_check_cancelled.flx`](../../examples/async/10_check_cancelled.flx).

---

## 10. Channels

[`Flow.Channel`](../../lib/Flow/Channel.flx) is a typed, fiber-aware queue for passing `Sendable` values between fibers (and tasks).

```flux
public fn make<a: Sendable>(capacity: Int) -> Channel<a>   // capacity 0 = rendezvous
public fn send<a: Sendable>(ch: Channel<a>, v: a) -> Unit with Async   // suspends if full
public fn recv<a: Sendable>(ch: Channel<a>) -> Option<a> with Async    // suspends if empty; None when closed
public fn try_send<a: Sendable>(ch: Channel<a>, v: a) -> Bool          // non-blocking
public fn try_recv<a: Sendable>(ch: Channel<a>) -> Option<a>           // non-blocking
public fn close<a>(ch: Channel<a>) -> Unit
public fn len<a>(ch: Channel<a>) -> Int
public fn cap<a>(ch: Channel<a>) -> Int
public fn is_closed<a>(ch: Channel<a>) -> Bool
```

`send`/`recv` suspend the *fiber* (not the OS thread) when the channel is full/empty. `recv` returns `None` once the channel is closed and drained. There are `send_move`/`try_send_move` ownership-transfer variants.

```flux
import Flow.Async as Async
import Flow.Channel as Channel

fn producer_consumer() -> String with Async {
    let ch = Channel.make(5)
    Channel.send(ch, 4)
    Channel.send(ch, 5)
    match (Channel.recv(ch), Channel.recv(ch)) {
        (Some(a), Some(b)) -> "total: " + to_string(a + b),
        _ -> "closed"
    }
}

fn main() with IO { print(Async.run_async(producer_consumer)) }   // → "total: 9"
```

See [`21_channel_capture.flx`](../../examples/async/21_channel_capture.flx).

---

## 11. `select` and Events

For waiting on **the first of several** channel/timer operations, Flux has a built-in `select` expression and a composable [`Flow.Event`](../../lib/Flow/Event.flx) (CML-style) layer.

### The `select` expression

`select` blocks until one arm is ready, commits exactly that arm, and runs its body. Arms are `recv <chan> as <name>`, `send <chan> <value>`, and `after <ms>` (a timer):

```flux
import Flow.Async as Async
import Flow.Channel as Channel
import Flow.Event as Event

fn first_message_or_timeout(ch: Channel<Int>) -> String with Async {
    select {
        recv ch as value -> match value {
            Some(n) -> "received: " + to_string(n),
            None    -> "closed",
        },
        after 100 -> "timeout",
    }
}
```

Losing arms are left untouched (a not-taken `recv` does not consume a message). See [`22_select_channel_timer.flx`](../../examples/async/22_select_channel_timer.flx) and [`23_select_send_recv.flx`](../../examples/async/23_select_send_recv.flx).

### First-class events (`Flow.Event`)

When you need to *build up* a choice programmatically, events are first-class values you compose and then `sync`:

```flux
public fn recv<a: Sendable>(ch: Channel<a>) -> Event<Option<a>>
public fn send<a: Sendable>(ch: Channel<a>, v: a) -> Event<Unit>
public fn after(ms: Int) -> Event<Unit>          // timer event
public fn choose<a>(events: List<Event<a>>) -> Event<a>   // first ready wins
public fn wrap<a, b>(e: Event<a>, f: (a) -> b) -> Event<b> // transform the value
public fn sync<a>(e: Event<a>) -> a with Async   // commit on whichever fires
```

`select` is sugar over `choose` + `sync`. See [`24_event_composition.flx`](../../examples/async/24_event_composition.flx) and the `async_select_*` parity fixtures.

---

## 12. Streams

[`Flow.Stream`](../../lib/Flow/Stream.flx) is a pull-based async sequence — a `Stream<a>` is a state machine whose `next` may suspend with `Async`. It has the usual combinators (`map`, `filter`, `flat_map`, `take`, `drop`, `chunk`, `append`, `zip`, `merge`, `fold`, `to_list`, …):

```flux
import Flow.Async as Async
import Flow.Stream as Stream

fn sum_evens() -> Int with Async {
    Stream.from_list([1, 2, 3, 4, 5, 6])
    |> Stream.filter(fn(n) { n % 2 == 0 })
    |> Stream.fold(0, fn(acc, n) { acc + n })     // fold drives the stream → 12
}
```

`fold`/`to_list`/`to_array`/`count` are the terminal operations that actually pull (they carry `Async`); the rest are lazy transformers. Streams underpin chunked HTTP responses (SSE) — see §15.

---

## 13. Tasks (OS-thread parallelism)

`Flow.Task` is the OS-thread surface introduced in §2. Tasks are **not** fibers: they live on a worker pool and run in true parallel. Values crossing the worker boundary must be [`Sendable`](#14-the-sendable-type-class).

```flux
public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
public fn blocking_join<a: Sendable>(t: Task<a>) -> a            // blocks the OS thread
public fn await<a: Sendable>(t: Task<a>) -> a with Async         // suspends only the fiber
public fn cancel<a>(t: Task<a>) -> Unit                          // idempotent; unconstrained in a
public fn spawn_scoped<a: Sendable>(s: Scope, action: () -> a) -> Task<a> with Async
```

- **`blocking_join`** — use from non-async code (e.g. `main`) when you have no fiber.
- **`await`** — the fiber-friendly join: suspends the current fiber, lets siblings keep running, resumes when the task completes. Awaiting a cancelled task raises — wrap in `try` to recover.
- **`cancel`** — pre-pickup short-circuits; post-completion is a no-op; in-flight is cooperative (the body must reach a yield point). It does **not** require `with Async`.
- **`spawn_scoped`** — ties the task's lifetime to a `Scope` so it is cancelled if the scope is.

`spawn_move` variants transfer ownership of the captured value rather than sharing it. See [`12_task_spawn_join.flx`](../../examples/async/12_task_spawn_join.flx) – [`14_task_cancel.flx`](../../examples/async/14_task_cancel.flx) and [`19_task_spawn_scoped.flx`](../../examples/async/19_task_spawn_scoped.flx).

---

## 14. The `Sendable` type class

```flux
class Sendable<a>     // marker class, no methods — declared in src/types/class_env.rs
```

`Sendable` gates which values may cross a worker boundary via `Task.spawn`/`join`/`await` and `Channel.send`. It is a compile-time check: a non-Sendable spawn fails at the *spawn site*, not at runtime.

**Auto-derived for:** primitives (`Int`, `Float`, `String`, `Bool`, `Unit`); tuples and `Option`/`List`/`Array`/`Map`/`Either` when their parameters are Sendable; and user ADTs whose every field is Sendable (synthesized by `synthesize_sendable_instances` in [`class_env.rs`](../../src/types/class_env.rs), including recursive ADTs and contextual instances like `<a: Sendable> => Sendable<Foo<a>>`).

**Not Sendable:** function values / closures (they may capture non-Sendable state), opaque runtime handles, and any ADT containing a function-typed field. So you cannot `Task.spawn` a closure that captures user state today — by design, pending a future closure-promotion story.

> ⚠️ `Sendable` has no teeth against a *hand-written* instance: the synthesizer skips an ADT that already has an explicit `instance Sendable<…>`, and nothing checks that yours is correct. **Don't write `Sendable` instances by hand** — let the synthesizer derive them. (Tracked as the "seal the class" limitation, §20.)

---

## 15. HTTP and TCP

The driving use case for 0174 is HTTP microservices. [`Flow.Http`](../../lib/Flow/Http.flx) provides a scratch-built HTTP/1.1 server and client over the `mio` TCP substrate ([`Flow.Tcp`](../../lib/Flow/Tcp.flx)).

```flux
// client
public fn get(url: String) -> Response with Async, AsyncFail
public fn post(url: String, body: String) -> Response with Async, AsyncFail

// server
public fn serve(addr: String, port: Int, handler: (Request) -> Response with Async | e)
    -> ServerHandle with Async, AsyncFail
public fn serve_config(addr: String, port: Int, cfg: ServerConfig, handler: …) -> ServerHandle …
public fn serve_stream<a>(addr, port, handler: (Request) -> StreamResponse<a> …) -> ServerHandle …   // chunked / SSE
public fn shutdown(h: ServerHandle) -> Unit …       // graceful drain
public fn shutdown_now(h: ServerHandle) -> Unit …   // forced
```

`ServerConfig` carries `max_connections`, `max_header_bytes`, `max_body_bytes`, `request_timeout_ms`, and `worker_count`. Streaming responses are driven by `Flow.Stream` (§12) — see the SSE example under `examples/`. JSON lives in `Flow.Json` (`encode`/`decode`, `deriving (Encode, Decode)`).

---

## 16. Runtime configuration

```flux
public data RuntimeConfig {
    RuntimeConfig { worker_count: Option<Int>, fs_pool_size: Int, dns_pool_size: Int }
}

default_runtime_config() : RuntimeConfig
with_worker_count(n)     : RuntimeConfig
with_dns_pool_size(n)    : RuntimeConfig
```

| Field | Default resolution | Status |
|---|---|---|
| `worker_count` | `None` → `FLUX_WORKERS` → `available_parallelism()` → `2` | Honoured on both backends |
| `fs_pool_size` | `0` → `FLUX_FS_THREADS` | Plumbed, reserved (unused) |
| `dns_pool_size` | `0` → `FLUX_DNS_THREADS`, fallback 4 | Honoured |

Explicit `RuntimeConfig` always wins over the env vars. See [`11_runtime_config.flx`](../../examples/async/11_runtime_config.flx).

---

## 17. Backend semantics: VM vs native

Flux runs async on **both** the bytecode VM and the LLVM/native backend, and the **type-level surface is identical** — source written against one compiles unchanged on the other.

Both backends are **multi-OS-threaded by default.** `run_async` resolves `worker_count` to `available_parallelism()` and, when that is `> 1` (any multicore machine), spawns real worker OS threads; the single-thread path is only used at `worker_count == 1`.

| Aspect | VM | Native (LLVM) |
|---|---|---|
| Default workers | `available_parallelism()` (OS threads via `enter_run_async_multi`) | `available_parallelism()` (pooled OS workers) |
| Single-thread path | `worker_count == 1` only (`dispatch_loop` on the caller thread) | `worker_count == 1` only |
| Fiber values across workers | `Rc`-backed; shared constants/globals via an `Arc<WorkerSharedState>` mirror, cross-thread results via `VmSendValue` | Shared C runtime heap; values promoted at worker boundaries |
| Fiber migration | Stolen parked/yielded fibers migrate via `ArcFiber` (`FLUX_FIBER_MIGRATION`, on by default) | Work-stealing across worker queues (`FLUX_WORK_STEALING`) |
| Tasks | Pooled isolated worker VMs behind the `Sendable` transfer boundary | Native worker threads |

> A single CPU-bound fiber that never suspends still occupies one worker; cooperative concurrency means *multiple* fibers overlap, not that one fiber is auto-parallelised. For dividing one heavy computation across cores, use `Task.spawn`/`await` (§13).

`FLUX_WORK_STEALING=0` (native) and `FLUX_FIBER_MIGRATION=0` (VM) restore owner-only FIFO scheduling for debugging.

---

## 18. Under the hood

The user surface above is built on a fiber scheduler worth understanding when reasoning about scheduling and cancellation. Source: [`src/runtime/async/`](../../src/runtime/async/), [`src/vm/core_dispatch.rs`](../../src/vm/core_dispatch.rs), [`runtime/c/tasks.c`](../../runtime/c/tasks.c).

> **Reliability status (2026-06).** The scheduler/cancellation/migration paths are the historically fragile part of the runtime — several reactive fixes have landed at the *cancel × completion × steal* boundary, and `Fiber`'s `Send` impl currently rests on a hand-maintained invariant rather than the type system (see [`fiber.rs`](../../src/runtime/async/fiber.rs)). Treat this layer as "works, hardening in progress" (proposal 0174 Phase 2). A deterministic test scheduler is the planned mitigation.

### 18.1 What a fiber is

A `Fiber` ([`src/runtime/async/fiber.rs`](../../src/runtime/async/fiber.rs)) owns:

- A monotonic `FiberId` (from `NEXT_FIBER_ID`), unique per scheduler lifetime.
- A `home_worker` assignment. Fibers are queued there and backend completions return there, but an idle worker may **steal** a ready fiber; stolen fibers cross threads only via the honestly-`Send` `ArcFiber` (`Fiber::promote` / `ArcFiber::demote`).
- A `state`: `Ready`, `Suspended { request_id }`, `Done`, or `Cancelled`.
- A `parked: Option<Rc<RefCell<Continuation>>>` — the captured delimited continuation when suspended.
- An owned `EffectContext` carrying yield/evidence state, the cancel flag, and scope id.

### 18.2 Park / resume cycle

When user code calls `sleep(20)`, the VM executes `CorePrimOp::FiberSleep` ([`core_dispatch.rs`](../../src/vm/core_dispatch.rs)):

1. Reserve a `RequestId` and submit a `timer_start(20ms)` to the `mio` backend.
2. `capture_to_fiber_boundary` — walk to the `FiberRunAsync` frame, snapshot the operand stack and frame index into a `Continuation`, store it in `Fiber.parked`.
3. Move the fiber from the worker's ready queue to its `suspended: HashMap<RequestId, Fiber>`.
4. Return control to the dispatch loop, which pumps `backend.next_completion()`.

When the timer fires, the scheduler looks the fiber up by request id, moves it back to `Ready`, restores the continuation, and resumes the suspending primop with the completion payload as its return value. The same machinery serves `both`/`race`/`timeout`/`first_of`/`Task.await` via an `AwaitKind` ([`await_coordinator.rs`](../../src/runtime/async/await_coordinator.rs)) that assembles the resume value from one or more child completions.

### 18.3 Cancellation propagation

`cancel(scope)` (and the implicit cancels from `race` losers, `timeout` losses, and error unwinds):

1. Sets each fiber's cancel bit in its `EffectContext`.
2. For suspended fibers, calls `backend.cancel(request_id)` — the reactor stops the I/O and synthesises a `Cancelled` completion.
3. Re-queues the fiber `Ready`; on resume the primop sees the cancelled completion and either short-circuits or fires cleanup before unwinding.

For *currently executing* fibers, a per-thread `CANCELLED_IDS` set lets `check_cancelled()` observe cancellation between suspension points.

### 18.4 Task internals

`Flow.Task` is backed by a worker pool ([`task_manager.rs`](../../src/runtime/async/task_manager.rs), [`task_scheduler.rs`](../../src/runtime/async/task_scheduler.rs)); native tasks use [`runtime/c/tasks.c`](../../runtime/c/tasks.c). A task's `outcome` (`Completed`/`Cancelled`/`Panicked`) is stored behind a `Mutex` + `Condvar`; panics inside the body are caught (`catch_unwind`) so one bad task does not poison the pool — it surfaces to the joiner as a fiber failure. A fiber `await` and a `blocking_join` on the same handle are mutually exclusive (the C side rejects a double-await).

---

## 19. Primop reference

User-facing async functions are thin wrappers over `CorePrimOp` variants, dispatched in [`core_dispatch.rs`](../../src/vm/core_dispatch.rs) (VM) and emitted via [`emit_llvm.rs`](../../src/lir/emit_llvm.rs) → [`tasks.c`](../../runtime/c/tasks.c) → [`native_abi.rs`](../../src/runtime/async/native_abi.rs) (native). The canonical numbered list is in [`src/core/mod.rs`](../../src/core/mod.rs).

| Surface | Primop | Surface | Primop |
|---|---|---|---|
| `run_async` | `FiberRunAsync` | `Task.spawn` / `_move` | `TaskSpawn` / `TaskSpawnMove` |
| `run_async_with` | `FiberRunAsyncWith` | `Task.spawn_scoped` / `_move` | `TaskSpawnScoped` / `…Move` |
| `sleep` | `FiberSleep` | `Task.blocking_join` | `TaskBlockingJoin` |
| `yield_now` | `FiberYieldNow` | `Task.await` | `TaskAwait` |
| `both` / `race` | `FiberBoth` / `FiberRace` | `Task.cancel` | `TaskCancel` |
| `first_of` | `FiberFirstOf` | `Channel.make`/`send`/`recv`/… | `ChanMake` / `ChanSend` / `ChanRecv` / … |
| `timeout` | `FiberTimeout` | `Event.recv`/`send`/`after`/`choose`/… | `EventRecv` / `EventSend` / `EventAfter` / … |
| `try` / `fail` | `FiberTry` / `FiberFail` | `Tcp.connect`/`read`/`write`/… | `TcpConnect` / `TcpRead` / … |
| `new_scope` / `fork` / `cancel` | `FiberNewScope` / `FiberForkScoped` / `FiberCancelScope` | `Http.*` / `Json.*` | `HttpServeConfig` / … / `JsonParse` / `JsonStringify` |
| `check_cancelled` | `FiberCheckCancelled` | `current_worker_count` | `FiberCurrentWorkerCount` |

> The numeric primop IDs are assigned in `src/core/mod.rs`; the `Fiber*` channel/event/cancellation primops in the 178–201 range are from the v0.0.6 async work. Do not hardcode the numbers — reference the enum.

---

## 20. Known limitations

These are real today; each has a tracked roadmap entry in [0174](../proposals/0174_async_effect_concurrency.md).

1. **Parens scope effect rows on callback parameters.** A callback that carries `with <effect>` must wrap its function type in parens unless it is the final parameter. `Flow.Async.both`/`race`/`bracket`/`finally` all use the parenthesised form:
   ```flux
   fn both<a, b>(f: (() -> a with Async | e1), g: (() -> b with Async | e2)) -> (a, b) with Async   // ✅
   fn finally<a>(body: () -> a, cleanup: () -> Unit with Async) -> a with Async                       // ✅ final bare form
   ```
2. **`AsyncError` runtime variants are opaque.** Only `canceled_error()` and `protocol_error(status, msg)` are exposed as constructors; the rest come back from the runtime.
3. **`Sendable` has no teeth against bad hand-written instances** (§14). Mitigation: let the synthesizer derive them.
4. **Scheduler hardening in progress.** The cancel × completion × steal boundary is historically fragile and `Fiber`'s `Send` rests on a prose invariant (§18). A deterministic test scheduler + stress harness are the planned fix (0174 Phase 2).

> *Historical note:* a v0.0.5-era "LLVM compile hang on ≥9 sequential `run_async_with*` sites" was traced to an LLVM optimizer pass (not a Flux runtime bug) and is fixed on current LLVM. A defensive outliner remains in [`src/lir/run_async_outline.rs`](../../src/lir/run_async_outline.rs); user source never needs to change. Reproducer preserved at [`examples/async/repro_native_seq.flx`](../../examples/async/repro_native_seq.flx).

---

## 21. Idiom cookbook

**Parallel fetch + combine** (fibers):
```flux
fn combined() -> String with Async {
    let (a, b) = both(fn() { http_get("https://a") }, fn() { http_get("https://b") })
    a + b
}
```

**Scatter + gather** (tasks for CPU work, awaited from a fiber):
```flux
fn scatter_gather() -> Int with Async {
    let n = current_worker_count()
    let handles = map(range(0, n), fn(i) { Task.spawn(fn() { job(i * 100) }) })
    sum_list(map(handles, Task.await))
}
```

**Race with cleanup** — `bracket` arms fire even on the loser:
```flux
fn body() -> String with Async {
    race(fn() { sleep(20); "fast" },
         fn() { bracket(acquire, release, fn(h) { sleep(2000); read(h) }) })   // release still runs
}
```

**Cancellable streaming reduction:**
```flux
fn reduce_until<a, b>(items: Stream<a>, seed: b, step: (b, a) -> b) -> b with Async {
    bail_if_cancelled()
    match Stream.next(items) {
        None            -> seed,
        Some((x, rest)) -> reduce_until(rest, step(seed, x), step)
    }
}
```

**First successful mirror, cancel the rest:**
```flux
fn fastest_mirror() -> String with Async {
    first([fn() { fetch("https://a") }, fn() { fetch("https://b") }, fn() { fetch("https://c") }])
}
```

**Timeout cascade** (sub-operation with its own deadline):
```flux
fn outer() -> Option<String> with Async {
    timeout(5000, fn() {
        match timeout(1000, fast_path) {
            Some(v) -> v,
            None    -> slow_path(),
        }
    })
}
```

More runnable, parity-tested examples for every surface live in [`examples/async/`](../../examples/async/); the teaching progression is in [`examples/guide_async/`](../../examples/guide_async/).

---

## 22. See also

- [`examples/async/`](../../examples/async/) — runnable, parity-tested examples per surface
- [`tests/parity/async_*.flx`](../../tests/parity/) · `channel_*.flx` · `task_*.flx` — VM/LLVM parity fixtures
- [proposal 0174](../proposals/0174_async_effect_concurrency.md) — full roadmap and runtime design
- [effect_row_system.md](effect_row_system.md) — effect rows, row variables, subtraction
- [type_system_effects.md](type_system_effects.md) — how inference treats `with` clauses
