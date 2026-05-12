# Chapter 20 — Async and Fibers

> Beginner-friendly worked examples: [`examples/guide_async/`](../../examples/guide_async/) (12 progressively-built demos, each ~30 lines, parity-tested across VM and LLVM).
>
> More technical primop-by-primop demos: [`examples/async/`](../../examples/async/).

## Learning Goals

- Understand what async means and why a language needs it.
- See how Flux expresses async **without keywords** — it's just an effect (`with Async`) and a handful of library functions.
- Read the difference between a **fiber** and an **OS thread**, and know when to reach for each.
- Wire concurrent work with `both` / `race` / `first_of` / `timeout`.
- Handle failure with `try` / `fail` and cancel safely with `scope` / `fork` / `cancel`.
- Spawn parallel work with `Task.spawn` and pass values across thread boundaries with `Channel`.

This chapter assumes you've read [Chapter 10 (Effects and Purity)](10_effects_and_purity.md) and [Chapter 11 (HOF and Effect Polymorphism)](11_hof_effect_polymorphism.md). Async in Flux *is* an effect — if you're comfortable with `with IO`, you're already comfortable with `with Async`.

---

## 1. Why Async Exists

A program that downloads three URLs and adds up their byte counts could be written like this:

```flux
fn total_bytes() -> Int with IO {
    let a = fetch("https://a.example/")
    let b = fetch("https://b.example/")
    let c = fetch("https://c.example/")
    a + b + c
}
```

If each `fetch` waits 200ms for the network, the sequential version takes 600ms — even though the program itself is doing nothing for 595ms of that. **The slow part isn't your CPU; it's waiting.**

The classic fix is OS threads: spawn three threads, let them block in parallel, join them. That works, but threads are expensive (≈1 MB stack each), the OS scheduler is opaque, and shared mutable state across threads is a famous source of bugs.

The async model says: **don't block the OS thread when you're just waiting**. Instead, surrender control voluntarily, let other concurrent work run, and resume when the wait is over. The total wall-clock time becomes ~200ms instead of ~600ms.

Most languages express this with `async` and `await` keywords:

```javascript
async function totalBytes() {
    const [a, b, c] = await Promise.all([
        fetch("https://a.example/"),
        fetch("https://b.example/"),
        fetch("https://c.example/"),
    ]);
    return a + b + c;
}
```

Flux expresses the same thing with no new keywords — just the existing effect system:

```flux
import Flow.Async exposing (..)

fn total_bytes() -> Int with Async {
    let ((a, b), c) = both(fn() { fetch_one("a") }, fn() { both(fn() { fetch_one("b") }, fn() { fetch_one("c") }) })
    a + b.0 + b.1
}
```

That's it. There's no `async fn`, no `.await` punctuation, no `Promise<T>` wrapper type. There's an effect called `Async` that says "I might suspend," and library functions like `both` that schedule work concurrently.

The rest of this chapter explains why that's enough.

---

## 2. The Effect Replaces the Keywords

In `async`/`await` languages, two things happen at once:

1. **`async fn`** — a function-level mark that changes the function's return type from `T` to `Future<T>` / `Promise<T>` / `Task<T>`. The function body now runs lazily and produces a "pending value."
2. **`await expr`** — a per-call mark that says "drive this future to completion before continuing."

Flux fuses them into a single effect annotation:

```flux
fn fetch_one(url: String) -> Int with Async { ... }
```

`with Async` is the effect-system equivalent of `async fn`. It says: *this function may suspend the current fiber and resume later.* Calling `fetch_one(url)` from another `with Async` function works without any extra punctuation — the moral equivalent of `await` is just **calling the function**.

```flux
fn total() -> Int with Async {
    let a = fetch_one("a")    // suspends until "a" completes
    let b = fetch_one("b")    // then suspends until "b" completes
    a + b
}
```

If you forget `with Async` on the caller, the compiler tells you exactly what's missing:

```
error[E422]: Effect Requirement Mismatch
  |
  | fn total() -> Int {
  |     let a = fetch_one("a")
  |             ^^^^^^^^^^^^^^ this call needs effects that are not currently available
  |
note: missing required effects: Async
help: add `with Async` to the enclosing function or handle them at a boundary.
```

There is no implicit "the runtime will figure it out." The effect must propagate from the call site up to the `run_async` boundary.

### What about `with Async | e`?

When you read library signatures you'll see `with Async | e`:

```flux
public fn try<a>(body: () -> a with Async | e) -> Result<a, AsyncError> with Async | e
```

`e` is an **effect row variable**. It means: "whatever extra effects the caller's body has, those flow through." If you call `try(body)` where `body` is `with Async, IO`, then `e = IO` and the whole thing is `with Async, IO`. If `body` is just `with Async`, then `e = {}`.

This is the same row-polymorphism you saw with `map` in Chapter 11 — async isn't a special case, it's the same machinery.

---

## 3. Fibers — Cooperative, Not Preemptive

When you write `with Async` code, what's actually running it?

A **fiber** is a unit of cooperative concurrency. Think of it as a lightweight thread that the runtime — not the OS — schedules. Key properties:

- A fiber is much cheaper than an OS thread. You can have thousands of them.
- A fiber **only suspends at well-defined points**: `sleep`, `yield_now`, I/O, `both` / `race`, anywhere a `with Async` operation might wait. Between those points, your code runs uninterrupted.
- All fibers in a `run_async` block share the runtime's worker pool.
  On the **native backend** (compiled with `--native`) this is a pool of real
  OS threads (default: `available_parallelism()`, overridable via `FLUX_WORKERS`
  or `run_async_with_workers`). On the **VM** (the default interpreter) there
  is exactly **one OS thread** — the caller's — and `worker_count` creates
  logical FIFO queues drained on that thread. VM fibers overlap on I/O and
  sleeps but never run in parallel.

This is **cooperative** scheduling: a fiber has to *choose* to yield. Compare this to OS threads, which the kernel can preempt at any instruction boundary. Cooperation is simpler to reason about — you know exactly where another fiber might run — but it means a CPU-bound fiber that never yields will hog its worker.

> **VM vs native — concurrency model**
>
> |  | VM (default interpreter) | Native (`--native`) |
> |---|---|---|
> | Fiber OS threads | 1 (caller's thread) | `worker_count` real OS threads |
> | `worker_count` effect | Logical FIFO queues | Real worker thread pool |
> | CPU-bound fiber starvation | Yes — one fiber pegs the one core | Reduced — idle workers pick up ready fibers |
> | Fiber-level parallelism | No | Yes |
> | Parallelism path | `Task.spawn` / `Task.await` | `Task.spawn` or fibers across workers |
>
> Most async programs — I/O-bound servers, concurrent HTTP clients, select loops —
> work correctly on both backends. The difference only matters when you have
> CPU-bound work: on the VM, use `Task.spawn`; on native, fibers across workers
> are also an option.

If you're CPU-bound (no I/O, no sleeps), insert a `yield_now()` periodically:

```flux
fn long_compute() -> Int with Async {
    let mut total = 0
    for i in 0..1000000 {
        total = total + expensive(i)
        if i % 10000 == 0 { yield_now() }   // give other fibers a chance
    }
    total
}
```

For genuinely CPU-bound *parallel* work, use `Task.spawn` (section 9) — that gets a real OS thread.

### Mental model: "the awaitable function"

When you write:

```flux
fn handler(req: Request) -> Response with Async {
    let user = db_lookup(req.user_id)         // suspend here, scheduler runs other fibers
    let posts = db_query(user.id)             // suspend here too
    render(user, posts)
}
```

…the body runs straight through, top to bottom, *just like synchronous code*. Behind the scenes, the runtime might be handling 10,000 other concurrent requests on the same OS thread. Each `with Async` call site is a potential suspension point; the runtime resumes the fiber when its result is ready.

You don't write callbacks. You don't chain `.then()`. You don't `await` every line. You write straight-line code. That's the whole point.

---

## 4. The Boundary: `run_async`

Pure functions and `with IO` functions can't call `with Async` functions directly. The boundary that lets you "enter" the async world is `run_async`:

```flux
import Flow.Async exposing (..)

fn compute() -> Int with Async {
    sleep(50)    // legal — we're in `with Async`
    42
}

fn main() with IO {
    // sleep(50)                    // ❌ E422: `with IO` doesn't include Async
    let answer = run_async(compute) // ✅ runs the fiber to completion
    print("answer = " + to_string(answer))
}
```

`run_async` does three things:

1. Installs the async effect handler (the runtime).
2. Schedules `compute` as the root fiber.
3. Drives the runtime until the root fiber returns or panics, then returns the value.

Outside `run_async` you cannot call any `with Async` function. Inside, you can call all of them. It's the moral equivalent of `tokio::main` or `asyncio.run`.

For tuning the worker pool, use `run_async_with_workers`:

```flux
let answer = run_async_with_workers(8, compute)  // 8-thread fiber pool
```

A run can be nested — you can call `run_async` inside another `run_async`, but that's almost never what you want; it just creates a fresh sub-runtime. Stay at one level unless you have a specific reason.

---

## 5. Concurrent Combinators

This is the heart of the async surface. All of these are normal functions (no syntax), all live in `Flow.Async`, all carry `with Async`.

### 5.1 `both` — run two, get both results

```flux
fn body() -> (Int, Int) with Async {
    both(fn() { fetch_one("a") }, fn() { fetch_one("b") })
}
```

`both(f, g)` schedules `f` and `g` as sibling fibers and returns the pair `(f_result, g_result)` once both complete. Tuple position reflects source order, not completion order — even if `g` finishes first, you get `(f_result, g_result)`.

Wall-clock cost is `max(time(f), time(g))`, not `time(f) + time(g)`. That's the win.

### 5.2 `race` — run two, get the first

```flux
fn first_response() -> Int with Async {
    race(fn() { fetch_one("primary") }, fn() { fetch_one("backup") })
}
```

`race(f, g)` returns whichever finishes first. The loser is **cancelled** — its in-flight I/O is aborted, and any `bracket` / `finally` cleanup runs. Source order breaks immediate ties.

### 5.3 `first_of` — race over a list

```flux
let mirrors = [
    fn() { fetch_one("us-east") },
    fn() { fetch_one("us-west") },
    fn() { fetch_one("eu-central") }
]
let (idx, value) = first_of(mirrors)
```

Same semantics as `race`, but for any number of candidates. Returns the zero-based index plus the winning value.

### 5.4 `timeout` — bound wall-clock time

```flux
fn maybe_value() -> Option<Int> with Async {
    timeout(5000, fn() { slow_fetch() })   // 5-second deadline
}
```

`timeout(ms, f)` returns `Some(value)` if `f` completes within `ms` milliseconds, `None` if it doesn't. The body fiber is cancelled on timeout — same cleanup discipline as `race`.

For richer error info, use `timeout_result`:

```flux
let r: Result<Int, AsyncError> = timeout_result(5000, fn() { slow_fetch() })
match r {
    Ok(v)             -> print(to_string(v)),
    Err(TimedOut)     -> print("too slow"),
    Err(_)            -> print("other failure")
}
```

### 5.5 `sleep` and `yield_now`

```flux
sleep(100)      // suspend this fiber for 100ms; others keep running
yield_now()     // yield once and resume immediately if nothing else is ready
```

`sleep` is the obvious one. `yield_now` is the cooperative escape hatch — useful in CPU-bound loops to keep the scheduler responsive (see section 3).

---

## 6. Failure: `try` and `fail`

Async code can fail in two ways:

1. **Explicit failure** — `fail(error)` raises an `AsyncError` that propagates up.
2. **Panics** — `panic("...")` from inside a fiber.

`try` catches both:

```flux
fn risky() -> Int with Async {
    if random_bool() { fail(canceled_error()) }
    else             { 42 }
}

fn safe() -> Int with Async {
    match try(risky) {
        Ok(v)             -> v,
        Err(Canceled)     -> 0,
        Err(TimedOut)     -> -1,
        Err(_)            -> -2
    }
}
```

`try(body)` returns `Result<a, AsyncError>` — `Ok(value)` if the body completes, `Err(reason)` if it panicked or called `fail`. Pattern-match the `AsyncError` variants for fine-grained recovery, or use a helper like `result_or` for fallback values.

---

## 7. Structured Concurrency with `scope`

The combinators in section 5 (`both`, `race`, `timeout`) all enforce a property called **structured concurrency**: a fiber's children cannot outlive their parent's scope. If the parent returns, all children are cancelled. If the parent panics, all children are cancelled. There are no orphan fibers.

For ad-hoc concurrent work that doesn't fit `both` / `race`, Flow.Async exposes the primitives directly:

```flux
fn run_workers() -> Unit with Async {
    scope(fn(s) {
        fork(s, fn() { worker_a() })
        fork(s, fn() { worker_b() })
        fork(s, fn() { worker_c() })
        // when this lambda returns, all three forked fibers are awaited;
        // if it panics, they're cancelled.
    })
}
```

- `scope(body)` allocates a fresh cancellation boundary and passes a `Scope` handle to `body`.
- `fork(s, f)` schedules `f` as a sibling fiber under scope `s`.
- `cancel(s)` cancels every fiber forked under `s`. Idempotent.

Since [Chapter 17 surface polish](17_http_services.md), `Scope` is a nameable type — you can write `fn helper(s: Scope) -> Unit with Async { fork(s, ...) }` if your fork logic factors cleanly into a helper.

### Cancellation is cooperative

A cancelled fiber doesn't die instantly. It receives `AsyncError.Canceled` at its next suspension point — `sleep`, `yield_now`, I/O. If you have a long CPU loop, sprinkle `check_cancelled()` calls or `yield_now()` so cancellation can take effect:

```flux
fn long_loop() -> Int with Async {
    let mut total = 0
    for i in 0..huge_n {
        check_cancelled()           // raises AsyncError.Canceled if cancelled
        total = total + work(i)
    }
    total
}
```

`bracket` and `finally` run their cleanup arms when their body is cancelled, so resource handles get released correctly.

---

## 8. Channels — passing values between fibers

When two fibers need to send messages to each other, use a channel:

```flux
import Flow.Channel as Chan

fn producer(c: Chan.Sender<Int>) -> Unit with Async {
    Chan.send(c, 1)
    Chan.send(c, 2)
    Chan.send(c, 3)
    Chan.close(c)
}

fn consumer(c: Chan.Receiver<Int>) -> Int with Async {
    let mut total = 0
    loop {
        match Chan.recv(c) {
            Some(v) -> total = total + v,
            None    -> break  // sender closed
        }
    }
    total
}

fn body() -> Int with Async {
    let (tx, rx) = Chan.unbounded()
    let (_unit, total) = both(fn() { producer(tx) }, fn() { consumer(rx) })
    total
}
```

A channel decouples the producer and consumer fiber — they can run at different rates, and the channel buffers in between. Bounded channels apply backpressure (the sender suspends when the buffer is full), unbounded channels never block on send. Pick bounded for production; unbounded is convenient for tests.

### Selecting across events

`Flow.Event` and `select` let one fiber wait for the first ready channel or timer arm:

```flux
import Flow.Event as Event

fn wait_one(ch) -> String with Async {
    select {
        recv ch as value -> match value {
            Some(n) -> "received " + to_string(n),
            None -> "closed"
        },
        after 500 -> "timeout"
    }
}
```

`select` parks the fiber on readiness notifications and repolls when it wakes. The notification is only a hint: another fiber may win the race first, so the implementation can wait again. A committed `Event.sync` consumes the whole event tree; build a fresh event before syncing again, and do not share one sub-event across two choices after either choice has committed. `Event.guard` currently evaluates its function while constructing the event, not at sync-time. `Event.with_nack` exists as a placeholder for CML-style negative acknowledgement; loser notification is not implemented yet.

---

## 9. Tasks — Real OS Threads

Fibers share OS threads. If you have CPU-bound work that should run *in parallel* on multiple cores, use `Task.spawn`:

> **On the VM**, `Task.spawn` is the *only* way to achieve CPU parallelism.
> VM fibers run cooperatively on one OS thread; spawning more fibers does not
> use more cores. `Task.spawn` crosses a `Sendable` deep-copy boundary into
> an isolated worker VM on a real OS thread, so the two sides run in parallel.

```flux
import Flow.Task as Task

fn parallel_sum() -> Int with Async {
    let t1 = Task.spawn(fn() { heavy_compute(0, 1_000_000) })
    let t2 = Task.spawn(fn() { heavy_compute(1_000_000, 2_000_000) })
    Task.await(t1) + Task.await(t2)
}
```

`Task.spawn(f)` runs `f` on a dedicated OS thread and returns a handle. `Task.await(t)` suspends the current fiber until the task completes, then returns its value (panicking if the task panicked).

When to use which:
- **`both` / `fork`** — many concurrent operations, each mostly waiting (I/O). Hundreds or thousands of them. Cheap.
- **`Task.spawn`** — a small number of CPU-bound chunks you want to run on different cores. Each task is one OS thread. Expensive — don't spawn 10,000.

### Lifetimes are bounded by `run_async`

A `Task` that's never awaited *and* never cancelled would normally leak the OS thread when `run_async` returns. Flow.Task has a safety net: any unawaited task is cancelled at `run_async` teardown. So fire-and-forget is safe — but joining is still better practice when you care about the result.

For scoped variants (`Task.spawn_scoped`), the task is bound to a `Scope` and cancelled when the scope exits.

---

## 10. Putting it together

Here's a small concurrent program that fans out three HTTP requests, races them with a 2-second deadline, and recovers from individual failures:

```flux
import Flow.Async exposing (..)
import Flow.Http as Http

fn fetch_text(url: String) -> Result<String, AsyncError> with Async {
    try(fn() { Http.get(url).body })
}

fn body() -> String with Async {
    let urls = ["https://a/", "https://b/", "https://c/"]
    let race_thunks = map(fn(u) { fn() { fetch_text(u) } }, urls)

    match timeout(2000, fn() { first_of(race_thunks) }) {
        Some((idx, Ok(text)))  -> "winner #{idx}: #{text}",
        Some((idx, Err(_)))    -> "winner #{idx} failed",
        None                   -> "all timed out"
    }
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
```

Read it top to bottom. Each `with Async` call is a potential suspension point; the runtime weaves them through whatever workers are available. The shape of the code is the shape of the logic — no callback pyramids, no manual continuation passing.

---

## 11. Common Pitfalls

- **Forgetting `with Async`.** The compiler will tell you. Add it to the enclosing function.
- **Calling `sleep` outside `run_async`.** `with IO` is not `with Async`. Wrap the work: `run_async(fn() { sleep(100); ... })`.
- **CPU-bound fibers that never yield.** They hog a worker. Add `yield_now()` or use `Task.spawn`.
- **Spawning a task and never awaiting it.** Safe (the scope reaper cancels it), but you don't see the result. Prefer `Task.await(t)` or use a fork-style scope.
- **Pattern-matching `AsyncError` variants without `import Flow.Async exposing (..)`.** Without the import, `Canceled` / `TimedOut` aren't in scope.
- **Sharing mutable state between fibers.** Don't. Use channels or `Task.spawn` + return values.
- **Benchmarking fiber concurrency on the VM and expecting multi-core speedup.** VM fibers run on one OS thread. A benchmark of `both(cpu_task, cpu_task)` on the VM will show no parallel speedup — and shouldn't. Run with `--native` for multi-core fiber scheduling, or use `Task.spawn` on either backend.

---

## 12. What to Read Next

- [Chapter 17 — HTTP Services](17_http_services.md): puts async to work for real servers and clients.
- [Chapter 19 — Streams and SSE](19_streams_and_sse.md): pull-based streaming is built on top of fibers.
- The runnable examples in [`examples/async/`](../../examples/async/) — every primitive in this chapter has a tiny standalone demo.
- Internals deep-dive: [`docs/internals/async_syntax.md`](../internals/async_syntax.md) covers the runtime, scheduler, and primop layer for compiler hackers.

## Recap

- Async is a way to express "I'm waiting on something — don't block the OS thread, let other work run."
- Flux uses an **effect** (`with Async`) instead of `async`/`await` keywords. The effect is the keyword.
- A **fiber** is a cooperatively-scheduled lightweight thread. On native, many fibers share a pool of real OS workers. On the VM, all fibers share one OS thread — concurrency without parallelism.
- `run_async(action)` is the boundary that enters async-land. Outside it, you can't call `with Async` functions.
- `both` / `race` / `first_of` / `timeout` are the main combinators. They preserve structured concurrency: children can't outlive their parent.
- `try` / `fail` handle failure as values; `scope` / `fork` / `cancel` give you ad-hoc structured concurrency.
- `Task.spawn` reaches for real OS threads when you actually need parallel CPU work.

You're now ready to build concurrent services in Flux.
