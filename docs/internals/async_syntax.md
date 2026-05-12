# Async Syntax (Internal Reference)

> **Proposal:** [0174 — Async Effect & Concurrency Roadmap](../proposals/0174_async_effect_concurrency.md)
> **Surface modules:** [`lib/Flow/Async.flx`](../../lib/Flow/Async.flx) · [`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) · [`lib/Flow/Tcp.flx`](../../lib/Flow/Tcp.flx)
> **Examples:** [`examples/async/`](../../examples/async/)
> **Related internals:** [effect_row_system.md](effect_row_system.md) · [type_system_effects.md](type_system_effects.md)

This document is the canonical reference for the user-facing async syntax of Flux as it stands after proposal 0174 phases 0, 1a, 1b, and the relevant Phase 2 slices. It describes the surface every user writes against — not the runtime implementation. For the runtime, see proposal 0174.

---

## 1. Mental model

Flux async is an **effect-row + handler** system, not `async fn` / `await`. Three principles:

1. **No function coloring.** A function that may suspend carries the `Async` effect in its row. Calls do not need a sigil. There is no contagious "async-ness" — only an effect row that is type-checked like any other.
2. **Handlers at boundaries.** `Async.run_async(action)` is the boundary that installs the async handler. Inside it, fibers are scheduled cooperatively. Outside it, you cannot call any function with `Async` in its row.
3. **Structured concurrency.** Every fiber has a clear owner: a `Scope`, a `both` / `race`, a `timeout`, or the root `run_async`. Cancellation is a first-class primitive that propagates down the ownership tree.

There are two cooperating layers:

- **Fibers** — lightweight cooperative tasks scheduled inside `run_async`. On the VM they are single-OS-threaded logical queues; on native they are scheduled over OS workers.
- **Tasks** — OS-thread-backed work spawned via `Flow.Task`. VM tasks run in isolated worker VMs; native tasks run on native worker threads. `Sendable<T>` gates which values can cross the worker boundary.

---

## 2. The `Async` effect row

`Async` is an effect alias declared in [`lib/Flow/Effects.flx`](../../lib/Flow/Effects.flx):

```flux
alias Async = <Suspend | Fork | GetContext | AsyncFail>
```

The four constituent effects map to:

| Atom | Purpose |
|---|---|
| `Suspend` | Pause the current fiber (sleep, await, blocking I/O) |
| `Fork` | Spawn a child fiber inside the enclosing scope |
| `GetContext` | Read scheduler-owned state (cancel flag, fiber id, scope) |
| `AsyncFail` | Raise an `AsyncError` that catches structured-concurrency cancellation |

User code virtually always writes `with Async`, never the four atoms separately. The only common exception is when you also need a non-async ambient effect:

```flux
fn handler() -> Unit with Async, Console { ... }
```

Effect-row rules (additivity, subtraction, row variables) are unchanged from the general system — see [effect_row_system.md](effect_row_system.md).

### 2.1 Mixing `Async` with other rows

```flux
fn fetch_then_log() -> Unit with Async, Console {
    let body = http_get("https://example.com")
    print(body)
}
```

`Async` and `Console` (or any other ambient effect) compose like ordinary atoms in a closed row.

### 2.2 Row variables

Higher-order async functions accept open rows:

```flux
public fn timeout<a>(ms: Int, f: () -> a with Async | e) -> Option<a> with Async | e
```

The `| e` lets the caller add their own ambient effects without polluting the combinator's signature.

---

## 3. Entry points

### 3.1 `run_async`

```flux
public fn run_async<a>(action: () -> a with Async | e) -> a
```

Installs the async handler, runs `action` to completion, returns its value. This is the only way to "enter" async code from a non-async caller.

```flux
fn body() -> Int with Async { 42 }

fn main() with IO {
    print(run_async(body))            // → 42
}
```

`run_async` blocks the calling OS thread until the root fiber finishes. This is intentional and analogous to `tokio::Runtime::block_on`.

### 3.2 `run_async_with`

```flux
public fn run_async_with<a>(cfg: RuntimeConfig, action: () -> a with Async | e) -> a
```

Same shape as `run_async` but with explicit knobs. See [§13 RuntimeConfig](#13-runtime-configuration).

### 3.3 `run_async_with_workers`

```flux
public fn run_async_with_workers<a>(n: Int, action: () -> a with Async | e) -> a
```

Sugar for the common case of pinning the worker count.

```flux
let result = run_async_with_workers(4, my_action)
```

### 3.4 `current_worker_count` (introspection)

```flux
public fn current_worker_count() -> Int with Async
```

Reports the worker count of the currently active `run_async` scheduler.
Backed by `CorePrimOp::FiberCurrentWorkerCount = 201`. Returns the number
of OS worker threads on native (one main thread + `flux-async-worker-{N}`
threads visible in `ps -T` / Process Explorer); on the VM, the
logical-worker count of the active `FiberScheduler`.

On the VM this is not a CPU parallelism signal. `run_async_with_workers(8, ...)`
creates eight logical ready queues, but the fiber dispatch loop still runs on
the caller OS thread. Use `Task.spawn` / `Task.await` for CPU-bound parallel
work on the VM today.

```flux
fn body() -> Int with Async {
    current_worker_count()
}

fn main() with IO {
    print(run_async_with_workers(8, body))   // → 8
    print(run_async(body))                    // → available_parallelism()
}
```

This is the recommended way to verify your `RuntimeConfig` is being
honoured. See also [`examples/async/16_current_worker_count.flx`](../../examples/async/16_current_worker_count.flx).

---

## 4. Suspension primitives

### 4.1 `sleep`

```flux
public fn sleep(ms: Int) -> Unit with Async
```

Suspends the current fiber for at least `ms` milliseconds.

- **Native:** parks the fiber, frees the OS thread for other fibers.
- **VM:** parks the fiber through the VM async backend; other ready VM fibers
  can run on the same OS thread while the timer is pending.

### 4.2 `yield_now`

```flux
public fn yield_now() -> Unit with Async
```

Cooperative reschedule hint. The fiber gives up its slice; another runnable fiber gets to run.

- **Native:** returns the fiber to the back of its worker queue.
- **VM:** returns the fiber to the back of its logical worker queue. This is a
  real cooperative reschedule, but still single-OS-threaded.

See [`02_sleep_yield.flx`](../../examples/async/02_sleep_yield.flx).

---

## 5. Concurrent combinators

### 5.1 `both`

```flux
public fn both<a, b>(f: (() -> a with Async | e1), g: (() -> b with Async | e2))
                  -> (a, b) with Async
```

Runs `f` and `g` as siblings under a hidden scope, returns `(f_result, g_result)` once both finish. Tuple position is **source order**, not finish order.

```flux
fn left()  -> Int with Async { sleep(50); 1 }
fn right() -> Int with Async { sleep(50); 2 }

fn body() -> (Int, Int) with Async {
    both(left, right)         // ~50ms wall clock, returns (1, 2)
}
```

If either branch raises, the other is cancelled and the error propagates.

See [`03_both.flx`](../../examples/async/03_both.flx).

### 5.2 `race`

```flux
public fn race<a>(f: (() -> a with Async | e1), g: (() -> a with Async | e2)) -> a with Async
```

Runs both fibers concurrently, returns the first to complete; the loser is cancelled (its pending I/O is aborted, its `bracket` / `finally` cleanup arms run).

Both branches must produce the same type.

See [`04_race.flx`](../../examples/async/04_race.flx).

### 5.3 `first_of` / `first`

```flux
public fn first_of<a>(fs: List<() -> a with Async>) -> (Int, a) with Async
public fn first<a>(fs: List<() -> a with Async>)    -> a with Async
```

N-way race. `first_of` returns `(winning_index, value)`; `first` discards the index. Source order breaks immediate ties (the lower index wins).

```flux
let win = first_of([s100, s50, s200])     // → (1, 50) — s50 wins
```

Calling `first` / `first_of` on an empty list panics.

See [`05_first_of.flx`](../../examples/async/05_first_of.flx).

---

## 6. Timeouts

### 6.1 `timeout`

```flux
public fn timeout<a>(ms: Int, f: () -> a with Async | e) -> Option<a> with Async | e
```

Bounds `f` by `ms` milliseconds. Returns `Some(value)` if `f` finishes in time, `None` if the timer fires first. On `None`, `f` is cancelled.

### 6.2 `timeout_result`

```flux
public fn timeout_result<a>(ms: Int, f: () -> a with Async | e)
                         -> Result<a, AsyncError> with Async | e
```

Same shape as `timeout` but flattens the timeout into a `Result<a, AsyncError>` that distinguishes:
- `Ok(v)` — body finished in time
- `Err(TimedOut)` — timer won
- `Err(other)` — body raised independently

Use the `result_*` helpers (§7.3) to inspect.

See [`06_timeout.flx`](../../examples/async/06_timeout.flx).

---

## 7. Error model

### 7.1 `AsyncError`

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

Variant constructors are not all reachable from user code — only those that come back through the constructor helper functions:

| Constructor | Helper |
|---|---|
| `Canceled` | `canceled_error()` |
| `ProtocolError(s, m)` | `protocol_error(s, m)` |

Other variants (`IoError`, `DnsError`, `ConnectionClosed`, ...) are produced by the runtime when I/O fails. Match on them in error handlers; do not construct them.

### 7.2 `fail` / `try`

```flux
public fn fail<a>(err: AsyncError) -> a with Async
public fn try<a>(body: () -> a with Async | e) -> Result<a, AsyncError> with Async | e
```

`fail` raises an `AsyncError` in the current fiber and propagates outward — siblings under the same scope are cancelled, ownership unwinds to the nearest `try` (or to the `run_async` boundary, where it surfaces as a panic).

`try` is the recovery primitive.

```flux
fn body() -> Int with Async { fail(canceled_error()) }

fn caught() -> Bool with Async {
    result_is_ok(try(body))             // → false
}
```

### 7.3 Result helpers

`import Flow.Async exposing (..)` brings the `Result<a, e>` type and its `Ok` / `Err` constructors into scope, so direct pattern matching on `Ok(v)` / `Err(e)` works at the call site. The helpers below remain available for cases where you don't want to pattern-match:

| Helper | Purpose |
|---|---|
| `result_is_ok(r)` | `Bool` — true if Ok |
| `result_is_timed_out(r)` | `Bool` — true if Err(TimedOut) |
| `result_or(r, fallback)` | `a` — value or fallback |
| `result_or_else_async(r, fallback, ok_fn)` | continuation form |
| `result_or_timeout_with_async(r, t_val, e_val, ok_fn)` | three-way fork on Ok / Err(TimedOut) / other |

See [`07_try_fail.flx`](../../examples/async/07_try_fail.flx).

---

## 8. Resource management

### 8.1 `finally`

```flux
public fn finally<a>(body: () -> a, cleanup: () -> Unit with Async) -> a with Async
```

Runs `body`, then unconditionally runs `cleanup`, returns `body`'s result. `cleanup` fires on success, failure, and cancellation.

### 8.2 `bracket`

```flux
public fn bracket<r, c, a>(acquire: () -> r,
                            release: (r) -> c,
                            body:    (r) -> a with Async)
                        -> a with Async
```

Resource-acquisition pattern: `acquire` produces a resource, `body` uses it, `release` always runs at the end.

Note that `release`'s return type `c` is polymorphic and **discarded**. In practice `release` typically returns `Unit`, but if the parser limitation (§14.1) prevents that, returning `Int` and ignoring the value is the standard workaround.

```flux
fn open_file() -> String { "fd-7" }
fn close_file(fd: String) -> Int { 0 }
fn use_file(fd: String) -> Int with Async { sleep(10); 42 }

fn body() -> Int with Async {
    bracket(open_file, close_file, use_file)
}
```

See [`08_finally_bracket.flx`](../../examples/async/08_finally_bracket.flx).

---

## 9. Structured concurrency

### 9.1 `scope`

```flux
public fn scope<a>(f: (Scope) -> a with Async | e) -> a with Async | e
```

Allocates a fresh cancellation boundary `Scope` and passes it to `f`. The scope ID is opaque; users pass it through `fork` / `cancel` rather than inspecting it. With `import Flow.Async exposing (..)` the `Scope` type is in scope unqualified, so user code can annotate helpers like `fn child_runner(s: Scope) -> Unit with Async { ... }`.

### 9.2 `fork`

```flux
public fn fork<a>(s: Scope, f: () -> a with Async | e) -> Unit with Async | e
```

Schedules `f` as a sibling fiber under scope `s`. Returns immediately. The child runs concurrently with the rest of the scope body.

### 9.3 `cancel`

```flux
public fn cancel(s: Scope) -> Unit with Async
```

Cancels every fiber forked under `s`. Each child's pending I/O is aborted; its continuation is resumed with `AsyncError.Canceled` so that `bracket` / `finally` cleanup arms run. Idempotent.

### 9.4 `check_cancelled` / `bail_if_cancelled`

```flux
public fn check_cancelled()    -> Bool with Async
public fn bail_if_cancelled() -> Unit with Async
```

Pure CPU loops have no I/O suspension point at which the scheduler can deliver a cancellation. To stay cooperative, sprinkle `bail_if_cancelled()` inside hot loops:

```flux
fn cpu_work(n: Int, acc: Int) -> Int with Async {
    if n <= 0 { acc }
    else {
        bail_if_cancelled()
        cpu_work(n - 1, acc + n)
    }
}
```

`check_cancelled()` is the inspection variant — it returns `Bool` so you can clean up gracefully and `return` a partial result. `bail_if_cancelled()` is the convenience that calls `fail(Canceled)` if the flag is set.

See [`09_scope_fork_cancel.flx`](../../examples/async/09_scope_fork_cancel.flx) and [`10_check_cancelled.flx`](../../examples/async/10_check_cancelled.flx).

---

## 9.5 Fibers in depth

The user-facing surface above is built on a fiber model that's worth understanding when reasoning about scheduling, cancellation, and the VM/native parity gap.

### 9.5.1 What a fiber is

A `Fiber` is the unit of cooperative concurrency inside `run_async`. A VM fiber
does not imply an OS thread; VM fibers time-share the caller thread until they
suspend or call `Async.yield_now`. Data-structurally a fiber owns
([src/runtime/async/fiber.rs](../../src/runtime/async/fiber.rs)):

- A monotonic `FiberId` allocated from `NEXT_FIBER_ID: AtomicU64` — unique per scheduler lifetime.
- A `home_worker` assignment. On the VM this remains a logical no-migration invariant. On native, the fiber is initially queued there and backend completions return it there, but the scheduler may let an idle worker steal ready work. Native fibers carry a C effect-context snapshot so handler/evidence state is restored before execution on whichever OS thread runs them.
- A `state: FiberState` — one of `Ready`, `Suspended { request_id }`, `Done`, `Cancelled`.
- A `parked: Option<Rc<RefCell<Continuation>>>` — the captured delimited continuation when suspended.
- A `last_completion_req: Option<RequestId>` — set by the dispatch loop just before resuming, so the fiber knows which completion woke it (used to assemble e.g. the `(left, right)` tuple for `both`).
- An owned `EffectContext` carrying the yield/evidence state, cancel flag, and scope id.

### 9.5.2 Park / resume cycle

When user code calls `Async.sleep(20)`, the VM executes `CorePrimOp::FiberSleep` ([src/vm/core_dispatch.rs](../../src/vm/core_dispatch.rs)):

1. Reserve a `RequestId` and submit a `timer_start(20ms)` to the mio backend.
2. Call `capture_to_fiber_boundary` — walks back to the `FiberRunAsync` frame, snapshots the operand stack and frame index into a `Continuation`, and stores it in `Fiber.parked`.
3. Move the fiber from the worker's ready FIFO to its `suspended: HashMap<RequestId, Fiber>`.
4. Return control to `vm_fibers::dispatch_loop`.

The dispatch loop pumps `backend.next_completion()`. When the timer fires:

1. The completion arrives as `(RequestId, payload)`.
2. Scheduler looks up the fiber by request id, sets `last_completion_req`, moves the fiber back to `Ready`.
3. Dispatch loop pops the fiber, restores its operand stack and frame from `Continuation`, and calls `resume_from_dispatch` with the payload as the return value of the suspending primop.

The same machinery is reused for `both` / `race` / `timeout` / `first_of` / `Task.await` — each has an `AwaitKind` ([src/runtime/async/await_coordinator.rs](../../src/runtime/async/await_coordinator.rs)) that tells the dispatch loop how to assemble the resume value from one or more child completions:

| `AwaitKind` | Resume value |
|---|---|
| `Both` | `(left_outcome, right_outcome)` once both arrive |
| `Race` | the first outcome; cancel the loser |
| `FirstOf` | `(winning_index, value)`; cancel the rest |
| `Timeout` | `Some(body_value)` if body wins, `None` if timer wins |
| `Task` | the task's stashed value (or `None` if cancelled) |

### 9.5.3 Worker assignment

Root fibers (the body of `run_async`) live on worker 0. On the VM, workers are
logical ready queues and fibers keep their home-worker affinity for their whole
lifetime; the dispatch loop drains those queues on the caller OS thread. On
native, child fibers spawned by `fork` / `both` / `timeout` are placed on the
least-loaded ready queue by default; `race` / `first_of` still enqueue immediate
candidates on the caller's worker to preserve source-order tie behavior. With
native work stealing enabled, idle OS workers may steal ready fibers from other
workers. `FLUX_WORK_STEALING=0` restores the original owner-only FIFO plus
round-robin placement fallback for debugging.

### 9.5.4 Cancellation propagation

`scope.cancel()` (and the implicit cancels from `race` losers, `timeout` body losses, error unwinds) walks the scope's fiber list and:

1. Sets each fiber's cancel bit in its `EffectContext`.
2. For suspended fibers, calls `backend.cancel(request_id)` — the reactor stops the I/O and produces a synthetic `Cancelled` completion.
3. Re-queues the fiber in `Ready`; when the dispatch loop resumes it, the resumed primop sees the cancelled completion and either short-circuits (`bail_if_cancelled`) or fires `bracket` / `finally` cleanup arms before unwinding.

For *currently executing* fibers, `vm_fibers` mirrors the cancel set in a per-thread `HashSet<FiberId>` so `Async.check_cancelled()` returns true even between suspension points.

### 9.5.5 VM vs native execution

| Aspect | VM | Native |
|---|---|---|
| Continuation type | `Rc<RefCell<Continuation>>` (`!Send`) | LLVM-generated stack frames + `flux_rt` C ABI |
| Worker count | Logical ready queues drained by the caller OS thread | Real OS-thread workers, default 2 |
| Fiber state | `Vm` instance state | C effect-context TLS + scheduler state in Rust |
| Cross-worker fiber dispatch | **Disabled**: VM stays single-OS-thread because `Rc<Value>` is non-Send | Enabled |

The VM logical-only constraint applies to fibers, not tasks. VM `Task.spawn`
crosses a sendable transfer boundary into an isolated worker VM, so task bodies
can run in parallel without making the normal `Rc<Value>` graph thread-safe.
CPU-bound code inside a VM fiber can starve sibling fibers until it suspends or
calls `Async.yield_now`; CPU-bound parallelism should use `Task.spawn` /
`Task.await` today.

---

## 10. Tasks

`Flow.Task` is the OS-thread surface. Tasks are **not** fibers; they live on a
worker pool and can run in true parallel. On the VM, each task runs inside an
isolated worker VM after crossing the `Sendable` transfer boundary. On native,
the task body runs through the C runtime task path.

### 10.1 `spawn`

```flux
public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
```

Schedules `action` on a worker thread. Returns immediately with a handle. The result type must satisfy `Sendable` (§11) so the value can cross a worker boundary.

### 10.2 `blocking_join`

```flux
public fn blocking_join<a: Sendable>(t: Task<a>) -> a
```

Blocks the calling **OS thread** until the task finishes and returns the result. Use this when you have no fiber context — typically from `main` before `run_async`.

### 10.3 `await`

```flux
public fn await<a: Sendable>(t: Task<a>) -> a with Async
```

The fiber-friendly join. On VM and native, suspends only the current fiber; other fibers on the same worker keep running while the task completes.

Awaiting a cancelled task panics — wrap in `try` to recover:

```flux
let r = try(fn() -> Int with Async { Task.await(t) })
```

### 10.4 `cancel`

```flux
public fn cancel<a>(t: Task<a>) -> Unit
```

Marks the task cancelled. Idempotent.

- Pre-pickup → worker observes the flag and short-circuits.
- Post-completion → no-op.
- In-flight → cooperative; the body must reach a fiber yield point for cancellation to be observed.

Note `cancel` does **not** require `with Async` and `<a>` is unconstrained.

See [`12_task_spawn_join.flx`](../../examples/async/12_task_spawn_join.flx), [`13_task_await.flx`](../../examples/async/13_task_await.flx), [`14_task_cancel.flx`](../../examples/async/14_task_cancel.flx).

### 10.5 Task internals

This section documents the runtime contract beneath the four-function surface above.

#### TaskHandle

`Task<T>` on the user surface wraps a `TaskHandle<T>` ([src/runtime/async/task_scheduler.rs](../../src/runtime/async/task_scheduler.rs)). A `TaskHandle` is cheap-to-clone and holds an `Arc<TaskState<T>>`:

```rust
struct TaskState<T> {
    outcome:    Mutex<Option<TaskOutcome<T>>>,
    finished:   Condvar,
    cancelled:  AtomicBool,
}

enum TaskOutcome<T> {
    Completed(T),
    Cancelled,
    Panicked(String),
}
```

The worker thread stores its result (or panic message, or `Cancelled`) into `outcome` and signals `finished`. Joiners wait on the condvar.

Native panics inside the task body are caught by `catch_unwind` so a single panicking task does not poison the worker pool — it surfaces back to the joiner as `TaskOutcome::Panicked(message)`, which `Task.blocking_join` / `Task.await` translate into a fiber failure.

#### Worker pool layout

`TaskManager` ([src/runtime/async/task_manager.rs](../../src/runtime/async/task_manager.rs)) owns N worker threads sharing a per-priority FIFO. `MAX_PRIO = 2` gives three priority levels (0–2). Workers park on a `Condvar`; submission grabs the queue lock, pushes, and signals `not_empty`. Shutdown sets a broadcast flag *while holding the queue lock* to avoid a TOCTOU window where a worker could observe an empty queue right before the shutdown bit and re-park forever. `Drop` joins every worker thread to keep libtest from wedging on Windows.

#### Native backend (`runtime/c/tasks.c`)

The native task registry is heap-backed and grows with live task handles. Each registered `FluxTaskEntry` has:

- `task_id` (real ids start at 1)
- platform thread handle (`pthread_t` on POSIX, `HANDLE` on Win32)
- per-task `mutex` + `finished` condvar (POSIX) or `CRITICAL_SECTION` + `CONDITION_VARIABLE` (Win32)
- atomic `cancelled_flag`
- result slot (`FluxValue` + tag byte)
- `await_request: int64_t` — non-zero when a fiber registered an async await on this task

Spawn flow:

1. Allocate a task id (atomic increment).
2. Allocate/register a task entry; abort with a diagnostic only if entry allocation or OS thread creation fails.
3. `flux_rc_promote(closure)` — recursively promote the closure's reference count from single-threaded to atomic mode (sign-bit encoding in [runtime/c/rc.c](../../runtime/c/rc.c)). Required because the worker thread will dup/drop concurrently.
4. `flux_dup(closure)` — bump the count once for the worker.
5. `pthread_create` (or `_beginthreadex` on Win32; we prefer `_beginthreadex` over `CreateThread` for proper CRT init).
6. The worker sets `flux_worker_thread = 1` (TLS) so allocations bypass the per-process bump arena and go through `malloc`.

Worker body:

1. Load `cancelled_flag` with `memory_order_acquire`. If set, store `TaskOutcome::Cancelled` and exit.
2. `catch_unwind` the Flux closure invocation.
3. On success, store `Completed(value)` into the slot's result field.
4. If `await_request` is non-zero, call `flux_async_task_complete(await_request, payload)` — that routes the result through the fiber scheduler back to the awaiting fiber's home worker. Otherwise, signal `finished` for the condvar joiner.

Two join paths are mutually exclusive — once a fiber registers an `await_request`, `blocking_join` on the same handle is rejected.

#### When to use which

| Pattern | Use |
|---|---|
| Concurrent I/O on a single core | Fibers (`both`, `race`, `Async.fork`) |
| CPU-bound parallelism on multiple cores | Tasks (`Task.spawn` + `Task.await`) |
| Mixing the two | `Task.spawn` from inside a fiber, `Task.await` to gather — see [§16.3](#163-worker-pool-with-task-fan-out) |
| Joining from non-async code | `Task.blocking_join` from `main` |

---

## 11. `Sendable` type class

```flux
class Sendable<a>     // declared in src/types/class_env.rs, no methods
```

`Sendable` is a marker class with no methods. It gates which values may cross a worker boundary via `Task.spawn` / `Task.blocking_join` / `Task.await`.

### 11.1 Built-in instances

Auto-derived for:

- Primitives: `Int`, `Float`, `String`, `Bool`, `Unit`
- Tuples: `(a, b)`, `(a, b, c)`, ... when all components satisfy `Sendable`
- Standard collections: `Option<a>`, `List<a>`, `Array<a>`, `Map<k, v>`, `Either<a, b>` when their parameters do

### 11.2 User ADTs

The compiler runs `synthesize_sendable_instances` on every `data` declaration ([`src/types/class_env.rs`](../../src/types/class_env.rs)) and emits an `instance Sendable<MyAdt>` if every field is `Sendable`. Parameterized ADTs get contextual instances:

```flux
data Foo<a, b> { Foo(a, b) }
// Auto-synthesised:
//   instance <a: Sendable, b: Sendable> => Sendable<Foo<a, b>>
```

### 11.3 What is NOT Sendable

- Function values / closures (intentional — closures may capture non-Sendable state)
- Opaque runtime handles (e.g. raw TCP file descriptors) — flagged via `is_opaque_non_sendable_adt` in `class_env.rs`
- ADTs containing function-typed fields, detected by `type_expr_contains_function`

This means you cannot `Task.spawn` a closure that captures user state today. That is by design pending a future "promote-to-MT-RC" story for closures.

### 11.4 Synthesis algorithm

`synthesize_sendable_instances` ([src/types/class_env.rs](../../src/types/class_env.rs)) runs after parsing every program and walks each `Statement::Data`. For an ADT `data Foo<a, b> { Foo(F1, F2, ...) | Bar(...) }`:

1. **Skip if the user wrote an explicit `instance Sendable<Foo>`** — user wins, even if their instance is wrong (this is the open hole that motivates "seal the class" — see [§14.7](#147-sendable-has-no-teeth-against-bad-user-instances)).
2. **Skip if any variant has a function-typed field** — even one closure field disqualifies the whole ADT.
3. **Skip if the ADT is on the runtime opaque list** (e.g. `IoHandle`, `TaskHandle` — cannot cross worker boundary safely).
4. **Otherwise emit `instance <a: Sendable, b: Sendable> => Sendable<Foo<a, b>>`** — every type parameter gets a contextual `Sendable` constraint. At use sites, the solver checks each parameter satisfies `Sendable` recursively.

Recursive ADTs are handled by treating the ADT name itself as in-scope during the field walk (so `data List<a> { Cons(a, List<a>) | Nil }` correctly becomes `<a: Sendable> => Sendable<List<a>>`).

### 11.5 Where it's enforced

The `Sendable<a>` bound appears in three places on the user-facing surface ([`lib/Flow/Task.flx`](../../lib/Flow/Task.flx)):

```flux
public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
public fn blocking_join<a: Sendable>(t: Task<a>) -> a
public fn await<a: Sendable>(t: Task<a>) -> a with Async
```

Note `Task.cancel<a>(t)` is *unconstrained* in `a` — cancelling a handle does not actually transfer the inner value, so the bound isn't required.

The solver ([src/types/class_solver.rs](../../src/types/class_solver.rs) `has_structural_builtin_instance`) resolves `Sendable<T>` queries at type-checking time. If a user writes `Task.spawn(fn() { ... captures non-sendable ... })`, the type error fires at the spawn site, not at runtime.

---

## 12. Backend semantics: VM vs LLVM

| Surface | VM | LLVM/native |
|---|---|---|
| `run_async` | Single-OS-thread logical scheduler | Multi-OS-thread M:N scheduler |
| `sleep(ms)` | Suspends the current fiber through the VM async backend; other ready VM fibers may run on the caller OS thread | Suspends fiber; an OS worker can run other work |
| `yield_now()` | Real cooperative reschedule within the single-threaded VM scheduler | Real cooperative reschedule across native ready queues |
| `both` / `race` | Cooperative fiber overlap on a single OS thread | Fiber overlap across OS workers |
| `Task.spawn` | Body runs in an isolated worker VM on a real OS thread | Body runs on a real OS worker thread |
| `Task.blocking_join` | Waits for worker completion | Condvar wait |
| `Task.await` | Suspends current fiber, resumes when task completes | Suspends current fiber, resumes when task completes |
| `Async.scope` / `cancel` | Real cancellation through scheduler/backend | Real cancellation through scheduler/backend |

The **type-level surface is identical** on both backends. Source written today against the VM compiles unchanged on native and gains parallelism for free where the runtime supports it.

---

## 13. Runtime configuration

```flux
public data RuntimeConfig {
    RuntimeConfig {
        worker_count:  Option<Int>,
        fs_pool_size:  Int,
        dns_pool_size: Int,
    }
}
```

| Field | Default | Native today |
|---|---|---|
| `worker_count` | `None` → `FLUX_WORKERS` env → `available_parallelism()` → `2` | Honoured per call. VM uses logical queues on one OS thread; native sizes ready queues and the worker thread pool to the requested count. |
| `fs_pool_size` | `0` → reserved | Plumbed but unused |
| `dns_pool_size` | `0` → `FLUX_DNS_THREADS` env, fallback 4 | Honoured |

### 13.1 Builders

```flux
default_runtime_config()    : RuntimeConfig
with_worker_count(n)        : RuntimeConfig
with_dns_pool_size(n)       : RuntimeConfig
```

### 13.2 Environment fallbacks

When a field is the default sentinel, the runtime reads:

- `FLUX_WORKERS` for `worker_count`
- `FLUX_FS_THREADS` for `fs_pool_size`
- `FLUX_DNS_THREADS` for `dns_pool_size`

Explicit `RuntimeConfig` always wins over env.

See [`11_runtime_config.flx`](../../examples/async/11_runtime_config.flx).

---

## 14. Known surface limitations

These are real today; users will hit them. Each has a tracked roadmap entry.

### 14.1 Parens scope effect rows on callback parameters

A callback parameter that carries `with <effect>` must wrap its function type in parens when it is not the final parameter. The bare form `f: () -> a with Async` works only on the final parameter (where the `with` is unambiguously the enclosing function's effect clause).

```flux
// ✅ accepted — both callbacks carry Async via parens
fn both<a, b>(f: (() -> a with Async | e1), g: (() -> b with Async | e2)) -> (a, b) with Async

// ✅ accepted — final-callback bare form is also fine
fn finally<a>(body: () -> a, cleanup: () -> Unit with Async) -> a with Async
```

`Flow.Async.both` / `race` / `bracket` / `finally` all use this convention.

### 14.4 `AsyncError` runtime variants are opaque

Only `canceled_error()` and `protocol_error(status, msg)` are exposed as constructor helpers. Other variants come back via the runtime; users construct them only by calling failing primitives.

### 14.7 `Sendable` has no teeth against bad user instances

`Sendable` is a marker class with no methods, and the synthesis pass skips ADTs that already have an explicit user-written instance. There is no current check that user-written instances are *correct* — a user could write `instance Sendable<MyClosureType>` and the type checker would accept it. The runtime would then promote a closure across a worker boundary; behaviour is undefined if the closure captured non-Sendable state.

Mitigation: don't write `Sendable` instances by hand. Let the synthesizer derive them.

### 14.8 LLVM compile hang on many `run_async_with*` call sites + a suspending body (historical; fixed upstream)

Investigated 2026-05-08. **Not a runtime bug** — the symptom was the LLVM
compile/link of the user binary hanging at
`[12 of 13] Linking Flow.Either` (just before the user module would link).
Earlier writeups described this as a "native sequential deadlock"; that
characterisation was wrong, the program never reached runtime.

**Original reproducer:** in a single function, place **9 sequential
`run_async_with_workers(N, fn)` call sites** where at least one of the
bodies contains a suspending call (e.g. `sleep`). Eight call sites
compiled fine; nine hung the LLVM optimizer/linker indefinitely.

```flux
// Used to hang the build at the user-module link step:
fn main() with IO {
    print(run_async_with_workers(1, report))
    // ... 7 more identical calls ...
    print(run_async_with_workers(9, slept))      // body contains sleep(10)
}
```

**Suspected cause:** an LLVM optimization pass scaling super-linearly with the
number of `flux_fiber_run_async_with` call sites that share a suspending
closure body. The specific pass was never localized — see the re-verification
below; it could not be reproduced again, so the bisect was not redone.

**Re-verification (2026-05-12, LLVM 22 / 23):** the original reproducer —
*and* a 40-site stress version — compile and run cleanly **even with the
outline pass disabled** (`RUN_ASYNC_WITH_INLINE_SITE_LIMIT` set to
`usize::MAX`). The underlying LLVM regression appears fixed upstream. The
source-level "≤ 8 sequential `run_async_with*` sites per function" guidance is
**no longer necessary** on current LLVM.

**Compiler safety net:** the native LIR→LLVM path still transparently outlines
the 9th and later `run_async_with*` sites in a function into noinline helper
functions before LLVM optimization ([`src/lir/run_async_outline.rs`](../../src/lir/run_async_outline.rs),
limit `8`). It is cheap (a quick scan, a no-op below the limit) and is kept as
a defence for older toolchains; it can be removed once the project's CI LLVM
floor is known to include the fix. User source never needs to change either way.

**LLVM-side diagnostics (if it ever recurs):** run any pass-localization work
under an external timeout. Extract IR with `--emit-llvm -o file.ll` (or
`--dump-lir-llvm`), then `timeout 60 opt -passes='default<O2>' -opt-bisect-limit=N file.ll -o /dev/null`
(binary-search `N`) and/or `-debug-pass-manager` to find the offending pass.
Note: the *combined* `--emit-llvm` module did not reproduce the hang even on
the affected toolchain — the trigger lived in the per-module native compile of
the user module, so reproduce via `--native` (with a timeout), not `--emit-llvm`.

**Reproducer file:** [`examples/async/repro_native_seq.flx`](../../examples/async/repro_native_seq.flx)
— preserved as the smallest historical trigger.

**Related design implication:** the architecture *was* hypothesised to
have a runtime sequential-teardown bug. Phase B testing confirmed
sequential `run_async_with*` boundaries on native are fine — the
runtime correctly tears down each `NativeRun`, joins workers, and starts
the next run. The earlier proposal-0174 §14.8 wording about runtime
deadlock has been corrected.

---

## 15. Primop reference

User-facing async functions are thin wrappers over `CorePrimOp` variants. This table is for cross-referencing source ↔ runtime.

| Surface | Primop | Where dispatched |
|---|---|---|
| `fail` | `FiberFail = 161` | raise |
| `run_async` | `FiberRunAsync = 163` | [vm/core_dispatch.rs](../../src/vm/core_dispatch.rs) · [emit_llvm.rs](../../src/lir/emit_llvm.rs) |
| `yield_now` | `FiberYieldNow = 164` | same |
| `sleep` | `FiberSleep = 165` | timer via mio backend |
| `both` | `FiberBoth = 172` | dispatch loop awaiter |
| `race` | `FiberRace = 173` | dispatch loop awaiter |
| `timeout` | `FiberTimeout = 174` | dispatch loop + timer |
| `new_scope` | `FiberNewScope = 175` | scope alloc |
| `fork` | `FiberForkScoped = 176` | scheduler.spawn |
| `cancel` (scope) | `FiberCancelScope = 177` | scheduler.cancel_scope |
| `check_cancelled` | `FiberCheckCancelled = 178` | scheduler-flag read |
| `run_async_with` | `FiberRunAsyncWith = 179` | same as `run_async` + cfg |
| `first_of` | `FiberFirstOf = 180` | N-way awaiter |
| `try` | `FiberTry = 181` | error boundary |
| `current_worker_count` | `FiberCurrentWorkerCount = 201` | active scheduler introspection |
| `Task.spawn` | `TaskSpawn` | `flux_task_spawn` (native) |
| `Task.blocking_join` | `TaskBlockingJoin` | `flux_task_blocking_join` |
| `Task.await` | `TaskAwait` | `flux_task_await` |
| `Task.cancel` | `TaskCancel` | `flux_task_cancel` |

Primop numbers are stable as of v0.0.4; see [`src/core/mod.rs`](../../src/core/mod.rs) for the canonical list.

---

## 16. Idiom reference

### 16.1 Parallel fetch + combine

```flux
fn fetch_a() -> String with Async { http_get("https://a") }
fn fetch_b() -> String with Async { http_get("https://b") }

fn combined() -> String with Async {
    let pair = both(fetch_a, fetch_b)
    pair.0 + pair.1
}
```

### 16.2 Bounded backoff retry

```flux
fn try_once() -> Int with Async { ... }

fn with_deadline() -> Option<Int> with Async {
    timeout(2000, try_once)
}
```

### 16.3 Worker pool with task fan-out

```flux
fn job(n: Int) -> Int { sum_squares(n, 0) }

fn main() with IO {
    let t1 = Task.spawn(fn() { job(100) })   // closure must capture only Sendable values
    let t2 = Task.spawn(fn() { job(200) })
    print(Task.blocking_join(t1) + Task.blocking_join(t2))
}
```

### 16.4 Cancellable long loop

```flux
fn process(items: List<Int>) -> List<Int> with Async {
    bail_if_cancelled()
    match items {
        []        -> [],
        [x | rest] -> [transform(x)] + process(rest)
    }
}
```

### 16.5 Resource + timeout composition

```flux
fn with_socket(addr: String) -> String with Async {
    bracket(
        fn() { open_socket(addr) },
        fn(s) { close_socket(s) },
        fn(s) { read_with_timeout(s) }
    )
}

fn read_with_timeout(s: Socket) -> String with Async {
    match timeout(500, fn() { read_line(s) }) {
        Some(line) -> line,
        None       -> ""
    }
}
```

### 16.6 Pinning the worker count for a benchmark

```flux
fn benchmark() -> Int with Async {
    // Confirm the runtime actually allocated the workers we asked for.
    let n = current_worker_count()
    print("benchmark on " + to_string(n) + " workers")
    workload()
}

fn main() with IO {
    print(run_async_with_workers(8, benchmark))
}
```

### 16.7 Tuning at startup from an env var

```flux
// Caller already set FLUX_WORKERS=N; let the resolver pick it up.
fn main() with IO {
    print("running with " + to_string(run_async(fn() -> Int with Async {
        current_worker_count()
    })) + " workers")
}
```

### 16.8 Scatter + gather

Spawn N parallel tasks for CPU work, await them all from a fiber while
the OS thread keeps servicing other fibers.

```flux
fn job(seed: Int) -> Int { sum_squares(seed, 0) }

fn worker_count_or_default() -> Int with Async {
    let n = current_worker_count()
    if n > 0 { n } else { 4 }
}

fn scatter_gather() -> Int with Async {
    let n = worker_count_or_default()
    let handles = map(range(0, n), fn(i) { Task.spawn(fn() { job(i * 100) }) })
    sum_list(map(handles, Task.await))
}
```

### 16.9 Race with cleanup

`bracket` cleanup arms fire on every termination path including
`race`-loser cancellation:

```flux
fn slow() -> String with Async {
    bracket(
        fn()  { acquire_handle() },
        fn(h) { release_handle(h) },        // runs even when cancelled
        fn(h) { sleep(2000); read(h) }
    )
}

fn fast() -> String with Async { sleep(20); "fast" }

fn body() -> String with Async {
    race(fast, slow)                         // returns "fast" in ~20ms
}                                            // slow's release_handle still runs
```

### 16.10 Cooperative checkpoint inside a streaming reduction

```flux
fn reduce_until<a, b>(items: Stream<a>, seed: b, step: (b, a) -> b) -> b with Async {
    bail_if_cancelled()                      // honour scope cancellation
    match Stream.next(items) {
        None         -> seed,
        Some((x, rest)) -> reduce_until(rest, step(seed, x), step)
    }
}
```

### 16.11 Bounded concurrency with first_of

Run N candidates, return the first one that succeeds, cancel the rest:

```flux
fn fetch(url: String) -> String with Async { http_get(url) }

fn fastest_mirror() -> String with Async {
    let mirrors = [
        fn() { fetch("https://a.example.com") },
        fn() { fetch("https://b.example.com") },
        fn() { fetch("https://c.example.com") }
    ]
    first(mirrors)                           // returns the fastest, cancels the others
}
```

### 16.12 Catching panics in workers

`Async.try` catches both explicit `fail` and panics, returning a
`Result<a, AsyncError>` you can pattern-match or inspect with helper functions:

```flux
fn risky() -> Int with Async {
    if random() < 0.5 { panic("nope") }
    else              { 42 }
}

fn safe() -> Int with Async {
    result_or(try(risky), -1)                // -1 on panic or fail
}
```

### 16.13 Timeout cascades

Stack timeouts when a sub-operation has its own deadline:

```flux
fn outer() -> Option<String> with Async {
    timeout(5000, fn() {
        let early = timeout(1000, fast_path)
        match early {
            Some(v) -> v,                    // fast path won
            None    -> slow_path()           // 4s budget remains
        }
    })
}
```

---

## 17. See also

- [`examples/async/`](../../examples/async/) — runnable, parity-tested examples for each surface in this doc
- [`tests/parity/async_*.flx`](../../tests/parity/) — minimal parity fixtures pinning VM/LLVM behaviour
- [proposal 0174](../proposals/0174_async_effect_concurrency.md) — full roadmap and runtime design
- [`docs/internals/effect_row_system.md`](effect_row_system.md) — effect rows, row variables, subtraction
- [`docs/internals/type_system_effects.md`](type_system_effects.md) — how the inference layer treats `with` clauses
