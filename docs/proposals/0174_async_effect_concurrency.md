- Feature Name: Async Effect & Concurrency Roadmap (Phases 1-5)
- Start Date: 2026-04-27
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: existing effect handlers ([runtime/c/effects.c](../../runtime/c/effects.c), [src/runtime/continuation.rs](../../src/runtime/continuation.rs)), existing FFI primop machinery
- Relates to: [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) — see "Relationship to 0143" below

# Proposal 0174: Async Effect & Concurrency Roadmap

## Summary

Introduce concurrency to Flux as a multi-phase initiative built on the
existing algebraic-effect-handler runtime, with libuv as the foreign I/O
substrate. The driving use case is **HTTP microservices and data streams**;
the technical foundation is a Koka-style `Async` effect whose scheduler is
implemented in Flux itself (`lib/Flow/Async.flx`), reusing the continuation
capture machinery already shipping in the VM and LLVM backends.

Five phases are specified, each independently shippable:

1. **Async + TCP** — `Async` effect, `interleaved`/`firstof`/`timeout`/`cancelable`, libuv glue, TCP primitives. Single-threaded.
2. **HTTP/1.1 + JSON + Streams** — request/response over Async, derived JSON codecs, `Stream<a>` abstraction.
3. **Process-per-core** — OS-process workers, pipe IPC, multi-core scaling without changing the language runtime.
4. **TLS + Database client** — rustls FFI, Postgres or Redis driver, real microservice viability.
5. **(Conditional) Shared-state multi-threading** — atomic refcounts à la Koka. Deferred until workload data demands it.

The proposal commits to Phase 1 in full design detail, sketches Phase 2-4
at API level, and intentionally leaves Phase 5 open. Three small
forward-compatibility decisions in Phase 1 (header layout, thread-local-ready
globals, no user-visible `spawn`) protect future phases without overdesign.

## Relationship to 0143

[Proposal 0143](0143_actor_concurrency_roadmap.md) specifies an
Erlang-style actor concurrency roadmap (isolated heaps, typed mailboxes,
supervision, deterministic test scheduler). 0143 and this proposal model
two complementary layers, not competing alternatives:

- **0174 owns the I/O layer.** `Async` effect, libuv glue, structured
  concurrency, HTTP, JSON, streams, TLS, database client. The runtime
  story for "one program doing many concurrent I/O operations."
- **0143 owns the isolation/reliability layer, built on top of 0174.**
  Actors as a userspace pattern over Phase 3's process-per-core (and/or
  Phase 5's threads if/when they land), with 0143's typed-mailbox and
  supervision designs preserved as the type-system and library shape.

Concretely, the original 0143 phases re-scope as follows once 0174 lands:

- **0143 Phase A (thread-per-actor)** is subsumed by **0174 Phase 3
  (process-per-core)**. Actors become OS-process workers; the isolation
  guarantee is stronger (process boundary, not just heap), and libuv
  already handles process lifecycle.
- **0143 Phase B (typed mailboxes + compile-time `Sendable<T>`)** remains
  valuable as a type-system feature for messages crossing process or
  channel boundaries. Re-targets as a future proposal layered on 0174.
- **0143 Phase C (supervision + cancellation)** becomes a Flux library
  built from 0174's `firstof`, `cancelable`, and `Process.wait`.
  Erlang-style supervision trees are buildable from these primitives.
- **0143 Phase D (work-stealing M:N scheduler + deterministic test
  scheduler)** — the deterministic test scheduler is more naturally
  expressed against 0174's scheduler-as-handler design (which is already
  in Flux code, hence already swappable for tests). Work-stealing M:N
  is reconsidered only if process-per-core proves insufficient.

The driving goal stated by the project — **HTTP microservices and data
streams** — points clearly at the I/O-layer story first. 0174's Phase 1-2
ship a working microservice in ~2 months; 0143's Phase A-B alone is ~10
weeks before any network socket is touched. Sequencing 0174 first does
not abandon 0143's design work; it provides the runtime substrate on
which 0143's isolation and supervision story becomes more economical to
build.

0143 is therefore marked as **deferred** rather than superseded, with its
phases re-targeted to follow 0174 Phase 3+. Its sendability rules,
supervision design, and deterministic-scheduler advocacy remain
authoritative for the actor layer when that work becomes timely.

## Motivation

### What Flux can do today

Audit of the I/O surface ([lib/Flow/Effects.flx](../../lib/Flow/Effects.flx),
[runtime/c/flux_rt.c](../../runtime/c/flux_rt.c)):

- Console: `print`, `println`, `read_stdin` — blocking.
- File: `read_file`, `write_file`, `read_lines` — blocking, eager (entire-file).
- Clock: `clock_now`, `now_ms` — readout only, no `sleep`, no timers.
- Network: **none.**
- Subprocess: **none.**
- Streaming I/O: **none.**

The most ambitious Flux program in the corpus today reads an Advent of Code
input file and computes over it. No real workload exists that would benefit
from concurrency. **The motivation is not "users are blocked on async";
it is "Flux cannot host the use cases its design points toward."**

### The intended target: HTTP microservices

A working Flux microservice in roughly the shape we want:

```flux
import Flow.Http
import Flow.Json

type CreateUser = { name: String, email: String }
type UserId = { id: Int }
derive (Encode, Decode) for CreateUser
derive (Encode, Decode) for UserId

fn handler(req: Request) -> Response with Async {
  match (req.method, req.path) {
    (Post, "/users") -> {
      let body: CreateUser = Json.decode(req.body)?
      let id = db.insert("users", body)?
      Response.json(UserId { id: id })
    }
    _ -> Response.not_found()
  }
}

fn main() with Async {
  Http.serve(addr: "0.0.0.0:8080", handler: handler)
}
```

Reaching that shape requires Async, structured concurrency, HTTP, JSON,
streams, TLS, and a database driver. That's the five-phase scope.

### Why this is well-aligned with Flux's existing runtime

Algebraic effect handlers with continuation capture are already implemented:

- **C runtime evidence vector** at [runtime/c/effects.c:90-94](../../runtime/c/effects.c) — handler stack, marker IDs, parameterized state.
- **VM `OpPerform`** at [src/bytecode/op_code.rs:97-102](../../src/bytecode/op_code.rs) — full unwinding + continuation capture.
- **VM continuation compose/resume** at [src/runtime/continuation.rs:13-93](../../src/runtime/continuation.rs).
- **LLVM `flux_yield_to`** at [src/lir/emit_llvm.rs:3403-3511](../../src/lir/emit_llvm.rs) — yield protocol shared with C runtime.
- **`cont_split` pass** at [src/lir/lower.rs:3594-3685](../../src/lir/lower.rs) — synthesizes continuations across blocks.

These are the precise primitives `await` needs. Adding async I/O is mostly
**plumbing libuv into the existing yield/resume protocol**, not building
new compiler infrastructure. Koka, OCaml/Eio, and Eff have all demonstrated
that effect-handler languages can build their schedulers in the source
language with a thin C glue layer; this proposal follows that precedent.

### Why libuv

The I/O backend question was investigated against alternatives (libev,
libevent, io_uring, mio, Tokio, hand-rolled epoll/kqueue). The decisive
constraint is that the LLVM-backend native binary links a C runtime, so the
I/O library must expose a plain C ABI. That filter rules out mio, Tokio, and
all Rust async crates. Among C options, libuv has:

- 14 years of production stress-testing in Node.js, Julia, Luvit, neovim, libgit2.
- Cross-platform from day one (Linux, macOS, Windows, BSD).
- ~30k LOC C, ~200 KB statically linked.
- The exact feature set Phase 1-4 needs: timers, TCP, file I/O, signals, child processes, DNS.
- **Koka uses it** — direct precedent for a Perceus-RC effect-handler language.

Languages that skip libuv (Go, Rust, Erlang) each have specific runtime
ambitions (M:N preemption, `Future`/`Pin`, per-process heaps with reduction
counts) that Flux is not pursuing. libuv is the right choice for embedders;
Flux is in that category.

## Detailed design

### Phase 1: Async effect + TCP primitives

#### The `Async` effect

```flux
effect Async {
  await: AwaitSetup<a> -> a
  yield: () -> ()
}

type AwaitSetup<a> = (Result<a, AsyncError> -> Unit) -> CancelHandle

type CancelHandle = {
  cancel: () -> Unit
}

type AsyncError =
  | Canceled
  | TimedOut
  | IoError(String)
  | DnsError(String)
```

`AwaitSetup` is a Flux closure. The handler invokes it with a continuation
callback; the closure registers with libuv and returns a handle the
scheduler can use to cancel an in-flight operation.

#### Structured concurrency primitives

Implemented as Flux-side handlers in `lib/Flow/Async.flx`:

```flux
fn run_async<a>(action: () -> a with Async) -> a
fn interleaved<a, b>(f: () -> a with Async, g: () -> b with Async) -> (a, b) with Async
fn firstof<a>(f: () -> a with Async, g: () -> a with Async) -> a with Async
fn timeout<a>(ms: Int, f: () -> a with Async) -> Option<a> with Async
fn cancelable<a>(f: () -> a with Async) -> a with Async
fn sleep(ms: Int) with Async
```

No `spawn`, no `Fiber<a>`, no per-fiber handles. Concurrency is always
expressed as nested scopes. This is the Koka/Eio-flavoured API, intentionally
narrower than Promise/Tokio-style.

#### TCP primitives

```flux
module Flow.Tcp

fn connect(host: String, port: Int) -> Connection with Async
fn listen(addr: String, port: Int) -> Listener with Async
fn accept(listener: Listener) -> Connection with Async
fn read(conn: Connection, max: Int) -> Bytes with Async
fn write(conn: Connection, data: Bytes) -> Int with Async
fn close(conn: Connection) -> Unit with Async
```

Each function is a thin Flux wrapper that constructs an `AwaitSetup` and
performs `Async.await`.

#### Runtime: scheduler in Flux

`run_async` is the top-level handler:

```flux
fn run_async<a>(action: () -> a with Async) -> a {
  handle action() {
    await(resume, setup) -> {
      let cancel = setup(fn(result) {
        async_enqueue(resume, result)        // FFI primop
      })
      register_cancelable(cancel)
      drive_loop()                            // FFI primop wrapping uv_run
    }
    yield(resume) -> {
      async_enqueue(resume, ())
      drive_loop()
    }
    return v -> v
  }
}
```

`async_enqueue` and `drive_loop` are CorePrimOps mapped to C functions in
`runtime/c/async_io.c`. The scheduler logic itself — handler arms, queue
discipline, cancel-handle bookkeeping — is Flux code.

#### libuv glue: `runtime/c/async_io.c`

New file, ~400 lines. Surface:

```c
// Loop lifecycle
uv_loop_t* flux_uv_loop_init(void);
void       flux_uv_loop_close(void);

// Driver — called from Flux scheduler
int64_t    flux_uv_run_once(void);   // returns popped (resume, value) or sentinel

// Timer
void       flux_uv_timer_start(int64_t resume, int64_t ms);
void       flux_uv_timer_cancel(uv_timer_t* handle);

// TCP
void       flux_uv_tcp_connect(int64_t resume, int64_t host, int64_t port);
void       flux_uv_tcp_listen(int64_t resume, int64_t addr, int64_t port);
void       flux_uv_tcp_accept(int64_t resume, int64_t listener);
void       flux_uv_tcp_read(int64_t resume, int64_t conn, int64_t max);
void       flux_uv_tcp_write(int64_t resume, int64_t conn, int64_t data);
void       flux_uv_tcp_close(int64_t resume, int64_t conn);

// File I/O (libuv worker pool)
void       flux_uv_fs_read(int64_t resume, int64_t path);
void       flux_uv_fs_write(int64_t resume, int64_t path, int64_t data);

// Cancellation
void       flux_uv_cancel(uv_req_t* req);
```

Each function:

1. Allocates a `flux_uv_req_t` carrying duped Flux values escaping to libuv.
2. `flux_dup`s those values.
3. Calls the libuv API.
4. The libuv completion callback constructs the Flux result, drops the
   duped extras, and enqueues `(resume, value)`.

#### Perceus / Aether interaction

The single load-bearing rule: **every Flux value escaping to libuv is duped
on entry and dropped in the completion callback.** This pattern is identical
to how the existing synchronous C runtime handles arguments
([runtime/c/string.c](../../runtime/c/string.c),
[runtime/c/array.c](../../runtime/c/array.c)). Async is "synchronous FFI
with a delay" from the RC perspective.

Three Aether considerations:

1. **`perform Async.await` must not have its argument dropped before libuv completes.** The explicit dup at the FFI boundary balances Aether's drop after `perform` returns. The Aether algorithm itself does not change; the FFI shim takes responsibility for the extra dup.
2. **Continuation capture is RC-correct by construction.** Each captured frame slot is duped during composition ([src/runtime/continuation.rs:49-93](../../src/runtime/continuation.rs)). Resume drops on consumption. Continuations that never resume (cancellation) drop their captures normally.
3. **`@fip`/`@fbip` functions should not be called on values currently held by libuv.** While the value is live in a libuv request, refcount is ≥ 2; the in-place reuse path will not fire. Documented limitation; not enforced statically in Phase 1.

#### Cancellation contract

`uv_cancel(req)` guarantees the original libuv callback fires exactly once,
with `UV_ECANCELED` on cancellation. The C glue's completion path runs
identically; it just delivers a `Canceled` error to `resume`. The
suspended continuation receives the error, raises an `AsyncError` exception,
and unwinds. `cancelable { ... }` catches that exception at scope boundary.

`timeout(ms, f)` is a `cancelable` over `firstof { f(); sleep(ms); fail() }`.

#### Forward-compatibility rules baked into Phase 1

Three deliberate decisions to protect Phase 4-5:

1. **`FluxHeader.refcount` stays the last field, naturally aligned.** When/if the field becomes `_Atomic(int32_t)` in Phase 5, no layout change. Mirrors Koka ([kklib/include/kklib.h:101-135](/Users/s.gerokostas/Downloads/Github/koka/kklib/include/kklib.h)).
2. **Globals that would need to become thread-local are marked.** `current_evv` ([runtime/c/effects.c:96](../../runtime/c/effects.c)), the libuv loop pointer, the ready queue. A `// FUTURE: thread-local in Phase 5` comment on each. Phase 5 swap is mechanical.
3. **No `spawn` primitive in the user API.** `interleaved`/`firstof`/`timeout` are the front door. Phase 3 will add `Process.spawn` (OS process); Phase 5 may or may not add `Thread.spawn` (OS thread). Not exposing a generic `spawn` keeps that decision open.

#### Phase 1 deliverables

- `lib/Flow/Async.flx` — effect declaration + scheduler handler + structured-concurrency primitives. ~300 lines Flux.
- `lib/Flow/Tcp.flx` — TCP wrappers. ~150 lines Flux.
- `runtime/c/async_io.c` — libuv glue. ~400 lines C.
- ~15 new `CorePrimOp` enum entries in [src/core/mod.rs](../../src/core/mod.rs).
- VM dispatch entries in [src/vm/core_dispatch.rs](../../src/vm/core_dispatch.rs).
- LLVM builtin mappings in [src/llvm/codegen/builtins.rs](../../src/llvm/codegen/builtins.rs).
- Build-system: vendor libuv as git submodule under `vendor/libuv/`, link statically.
- Examples: TCP echo server, TCP echo client, parallel TCP fetch via `interleaved`, `timeout`-bounded connect.
- Parity tests in `tests/parity/async/` — VM and LLVM produce identical output for all examples.

Estimated effort: 3 weeks one engineer.

### Phase 2: HTTP/1.1 + JSON + Streams

#### HTTP

Wrap [llhttp](https://github.com/nodejs/llhttp) (Node.js's parser, ~3k lines
C, MIT-licensed). Vendor as `vendor/llhttp/`. Surface in
`runtime/c/http.c`, ~200 lines C glue.

```flux
module Flow.Http

type Method = | Get | Post | Put | Delete | Patch | Head | Options
type Request = { method: Method, path: String, headers: Map<String, String>, body: Bytes }
type Response = { status: Int, headers: Map<String, String>, body: Bytes }

fn serve(addr: String, port: Int, handler: (Request) -> Response with Async) with Async
fn get(url: String) -> Response with Async
fn post(url: String, body: Bytes) -> Response with Async
fn request(method: Method, url: String, headers: Map<String, String>, body: Bytes) -> Response with Async
```

Keep-alive and chunked transfer supported. HTTP/2 deferred to a future
proposal (significant complexity for marginal Phase-2 gain).

#### JSON

Two parts:

- `Flow.Json.parse: String -> Json` — tagged union value (`JsonNull | JsonBool | JsonNumber | JsonString | JsonArray | JsonObject`).
- `derive (Encode, Decode) for T` — type-class instances generated at compile time. Uses the existing dictionary-passing infrastructure from [proposal 0145](0145_type_classes.md).

Codec generation is added to the dict-elaboration pass
([src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs)).
Per-record codecs are zero-allocation when the type permits (ADTs with all
flat fields).

#### Streams

```flux
type Stream<a> = () -> Option<a> with Async

fn map<a, b>(s: Stream<a>, f: (a) -> b) -> Stream<b>
fn filter<a>(s: Stream<a>, p: (a) -> Bool) -> Stream<a>
fn fold<a, b>(s: Stream<a>, init: b, f: (b, a) -> b) -> b with Async
fn take<a>(s: Stream<a>, n: Int) -> Stream<a>
fn chunk<a>(s: Stream<a>, size: Int) -> Stream<List<a>>
fn merge<a>(s1: Stream<a>, s2: Stream<a>) -> Stream<a>
```

Pull-based by default — the consumer drives. HTTP request and response
bodies become streams; SSE and chunked transfer fall out naturally. A
`buffered(n)` adapter inserts a small queue between producer and consumer
when concurrency is desired.

#### Phase 2 deliverables

- `lib/Flow/Http.flx`, `lib/Flow/Json.flx`, `lib/Flow/Stream.flx` — ~600 lines Flux total.
- `runtime/c/http.c` — llhttp glue, ~200 lines C.
- Codec derivation added to `dict_elaborate.rs`.
- Examples: hello-world microservice, JSON echo, SSE broadcaster, parallel HTTP fetch.
- Documentation: HTTP server quickstart, JSON codec guide.

Estimated effort: 6 weeks.

### Phase 3: Process-per-core

Multi-core scaling without language-runtime changes. Each worker is an
independent Flux process with its own libuv loop and its own (non-atomic)
heap. Communication via libuv pipes or domain sockets, serialized as bytes
(typically JSON).

```flux
module Flow.Process

type Worker
fn spawn(program: String) -> Worker with Async       // OS process
fn send(w: Worker, data: Bytes) -> Unit with Async
fn recv(w: Worker) -> Bytes with Async
fn close(w: Worker) -> Unit with Async
fn wait(w: Worker) -> Int with Async                  // exit code
```

Standard pattern — listen on one port in the master, dispatch incoming
connections round-robin to workers via `SO_REUSEPORT` or a master-side
acceptor:

```flux
fn main() with Async {
  let workers = List.map(range(0, num_cores()), fn(i) {
    Process.spawn("flux run worker.flx")
  })
  Http.serve(addr: "0.0.0.0:8080", handler: dispatch_to(workers))
}
```

This is the same model PHP-FPM, Ruby Puma (in cluster mode), Node cluster,
and gunicorn use. It handles the "saturate all cores for stateless HTTP"
case for ~90% of microservice workloads.

#### Phase 3 deliverables

- `lib/Flow/Process.flx` — ~150 lines Flux.
- `runtime/c/async_io.c` extended with `uv_spawn` wrappers, ~100 lines C.
- Examples: load-balanced HTTP service, master-worker job queue.
- No changes to Aether, Perceus, or the type system.

Estimated effort: 2 weeks.

### Phase 4: TLS + database client

#### TLS

Link rustls via its C ABI (`rustls-ffi`). Vendor as
`vendor/rustls-ffi/`. Glue in `runtime/c/tls.c`, ~200 lines.

```flux
module Flow.Tls

fn handshake_client(conn: Connection, hostname: String) -> TlsConnection with Async
fn handshake_server(conn: Connection, cert: Cert, key: Key) -> TlsConnection with Async
fn read(c: TlsConnection, max: Int) -> Bytes with Async
fn write(c: TlsConnection, data: Bytes) -> Int with Async
fn close(c: TlsConnection) -> Unit with Async
```

`Http.get` and `Http.serve` transparently use TLS when the URL scheme is
`https://` or the listener is configured with a cert.

#### Database client

Choose **one** to start. Recommendation: **Postgres**.

Reasons: (a) wire protocol is well-documented, (b) microservice workloads
overwhelmingly target Postgres, (c) async-friendly (request/response with
prepared statements maps cleanly to `Async`), (d) no proprietary client
library required — wire protocol implementation in pure Flux is feasible.

```flux
module Flow.Postgres

type Pool
type Connection
type Row

fn pool(config: PoolConfig) -> Pool with Async
fn acquire(pool: Pool) -> Connection with Async
fn release(pool: Pool, conn: Connection) -> Unit with Async
fn query(conn: Connection, sql: String, params: List<Param>) -> List<Row> with Async
fn execute(conn: Connection, sql: String, params: List<Param>) -> Int with Async

fn with_connection<a>(pool: Pool, action: (Connection) -> a with Async) -> a with Async
fn transaction<a>(conn: Connection, action: () -> a with Async) -> a with Async
```

The `Pool` is internally mutable — but only behind the `Async` effect (it's
parameterized handler state). User code remains pure.

Wire-protocol parser in pure Flux, ~800 lines. Connection pool and
transaction logic ~300 lines.

#### Phase 4 deliverables

- `lib/Flow/Tls.flx`, `lib/Flow/Postgres.flx` — ~1100 lines Flux.
- `runtime/c/tls.c` — rustls-ffi glue, ~200 lines C.
- Examples: HTTPS server, database-backed CRUD microservice (the
  motivating example from Summary).
- Integration tests against a real Postgres instance.

Estimated effort: 4 weeks.

### Phase 5 (conditional): Shared-state multi-threading

**Not committed.** Ships only if Phase 3's process-per-core model proves
inadequate for real workloads — i.e., a use case appears that genuinely
requires shared in-memory state across cores (large in-process cache,
coordinated rate limiter, shared connection pool).

If/when triggered, the chosen approach is **atomic refcounts everywhere**,
mirroring Koka:

- `FluxHeader.refcount` becomes `_Atomic(int32_t)`.
- `flux_dup` → `atomic_fetch_add(&rc, 1, memory_order_relaxed)`.
- `flux_drop` → `atomic_fetch_sub` + acquire fence on the zero case.
- `current_evv` and other globals become `_Thread_local`.
- `Thread.spawn(action)` primitive added.
- `interleaved`/`firstof` may dispatch across threads (transparent to user code).
- Channels added as a shared-state primitive (still a Flux module, with
  internal mutation behind a `Channel` effect).

Cost: 10-30% perf on RC-heavy code paths. Mitigated by Aether's existing
borrow inference (most dups happen on stack-local values where compilers can
prove non-escape; the atomic cost only materializes for truly shared
values).

Hybrid approaches (Lean 4-style "atomic only when shared", or per-thread
heaps with linear-type send) are explicitly rejected for this proposal.
They are research-grade design with multi-year delivery; atomic-everywhere
is the boring, proven answer Koka has run in production for years.

Phase 5 specification deferred to a separate proposal authored if and when
real workload pressure demands it.

## Drawbacks

- **libuv adds a build dependency.** Vendored statically, ~200 KB binary cost. Acceptable.
- **The five-phase scope is roughly 3 months of focused work.** Anything less ships a tech preview, not a usable runtime. The proposal is honest about that.
- **No `Fiber<a>` is a departure from Promise/Tokio idioms.** Some users may expect `spawn`/`await` style. The Koka/Eio precedent argues structured scopes are the right primary API; spawn may be added later if real demand emerges, but as a Phase 5+ decision.
- **HTTP/2 and gRPC are deferred.** Phase 2-4 ship HTTP/1.1 only. Adequate for most microservices.
- **TLS via rustls-ffi adds a Rust toolchain dependency at runtime build time.** Vendored static lib avoids a runtime dep, but compiling Flux from source requires Cargo. Acceptable parallel to existing Rust compiler dependency.
- **Phase 5 commits to atomic RC if shared-state threading is needed.** This is a 10-30% perf hit on RC-heavy code. The alternative (hybrid or per-thread heaps) is research-grade and not viable for this team. Proposal accepts the tradeoff explicitly.
- **Process-per-core (Phase 3) does not allow in-process shared state.** Use cases needing a shared cache must wait for Phase 5 or use external state (Redis, Memcached). Documented limitation, not a defect.

## Rationale and alternatives

### Why Async-via-effects vs. Promise/Future types

Flux already has algebraic effect handlers with continuation capture. An
async effect reuses 100% of that machinery. A `Promise<a>` ADT layer would
duplicate it, require new compiler support for `await`-as-syntax, and lose
the composability of effect handlers (`run_async` as a userspace handler is
not possible with built-in promises).

The Koka and OCaml/Eio precedents both chose effect-based async for
the same reason. Haskell's `IO`/`async`/`STM` is the closest counter-example,
but Haskell doesn't have algebraic effect handlers — it has monads. Different
substrate, different optimal answer.

### Why scheduler-as-handler vs. scheduler-in-Rust

The scheduler is a state machine over continuations. Flux's effect system
makes that state machine a one-page handler. Writing it in Rust would
duplicate work for VM and LLVM (two backends, two implementations) and hide
the demonstration of Flux's own effect system.

Performance concern is real but bounded — Koka and Eio prove the approach
ships in production. If profiling reveals scheduler hot paths, individual
operations can be moved to FFI primops without changing the user-facing API.

### Why Koka's API shape vs. JavaScript-promise shape

Spawn-and-join with a `Fiber<a>` handle is the JavaScript/Tokio idiom. It
encourages unstructured concurrency (spawned fibers leaking past their
intended scope, no cancellation propagation, "fire-and-forget" mistakes).

Structured concurrency (`interleaved`, `firstof`, `timeout`,
`cancelable`) makes the lifetime relationship between concurrent
operations syntactically obvious. Cancellation is automatic — leaving a
scope cancels in-flight work. This matches Eio and Trio (Python) and is the
direction modern async design has converged on.

### Why libuv vs. alternatives

Investigated: libev, libevent, io_uring, mio, Tokio, Boost.Asio,
hand-rolled epoll/kqueue. The Rust crates (mio, Tokio) are eliminated by
the C-runtime constraint. libev is Unix-only. libevent is superseded.
io_uring is Linux-only and premature for an unbenchmarked language.
Hand-rolling is multi-month false economy. libuv is the only candidate that
combines maturity, cross-platform, and C ABI; Koka, Julia, and Node validate
the choice for Flux's category of language.

### Why process-per-core (Phase 3) before threads (Phase 5)

Process-per-core requires zero changes to the language runtime — no atomic
RC, no thread-local globals, no shared-state primitives. It handles the
"scale stateless HTTP across cores" case completely, which is the
overwhelming majority of microservice deployments. Shipping it as Phase 3
buys time and real-world data before committing to the much-harder atomic-RC
work in Phase 5.

### Alternatives considered and rejected

- **Phase 1 = subprocess concurrency only.** Cheaper, but doesn't move toward HTTP. Rejected because the stated user goal is HTTP microservices.
- **Phase 1 = HTTP server upfront.** Skips the foundation. The TCP+Async layer must work cleanly before wrapping HTTP; rushing it produces an unfixable mess. Rejected.
- **Skip libuv, use `epoll`/`kqueue` directly.** Months of work to reach feature parity, no Windows. Rejected.
- **Adopt Tokio, make the runtime Rust-only.** Requires rewriting the entire C runtime ([runtime/c/](../../runtime/c/)) in Rust. Months of work for no functional gain. Rejected.
- **Per-thread heaps with linear-type send (Erlang-style).** Beautiful but requires major type-system work (uniqueness types, send-primitives). Multi-year scope. Rejected for this proposal; revisitable as a research direction post-Phase 5.

## Prior art

- **Koka** — `lib/v1/std/async.kk:535-540` is the direct architectural model: `await(setup)` + `cancelable` + libuv + atomic RC. ([/Users/s.gerokostas/Downloads/Github/koka](/Users/s.gerokostas/Downloads/Github/koka))
- **OCaml/Eio** — structured-concurrency primitives (`Switch`, `Fiber.both`, `Fiber.first`) inform the API surface. Pluggable backend (libuv ↔ io_uring) is a Phase 5+ aspiration.
- **Node.js** — defined libuv. Single-threaded async-via-callbacks. Demonstrates scale of one event loop on real microservice workloads.
- **Lean 4** — Perceus + hybrid atomic-on-share RC. Considered for Phase 5; rejected as too ambitious for team size.
- **Erlang/BEAM** — per-process heaps + reduction-counted preemption. Considered as Phase 5 alternative; rejected (requires linear types).
- **Haskell `async` library** — `Async a` handles + `wait`/`cancel`. The unstructured-concurrency precedent we are intentionally diverging from.
- **Trio (Python)** — popularized "structured concurrency" terminology. API shape (nurseries, scoped cancel) directly influences `interleaved`/`cancelable`.
- **Flux proposal [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md)** — earlier exploration of actor-style concurrency for Flux. Superseded by this proposal; actor patterns can be built as a userspace library on top of Phase 1's `Async` effect.

## Unresolved questions

1. **Setup-closure re-entry from C.** When libuv fires a callback, the C glue must re-enter Flux to call `async_enqueue`. Mechanism: a small C-to-Flux re-entry primitive analogous to existing primop dispatch. Detailed design deferred to Phase 1 implementation.
2. **`Bytes` zero-copy vs. copy on TCP read.** Phase 1 ships copy-on-delivery for simplicity. Phase 2 may move to zero-copy (libuv alloc callback returns a Flux-allocated buffer). Decision deferred to benchmarking.
3. **Pool internal mutation.** Phase 4's `Postgres.Pool` has internal mutable state (idle connections, in-flight count). Modeled as parameterized handler state. Concrete representation TBD.
4. **JSON codec error reporting.** `Json.decode` failure on malformed input — returns `Result<T, JsonError>` with field-path information. Schema TBD.
5. **HTTP/1.1 keep-alive eviction policy.** Connection pool sizing and timeout defaults TBD.
6. **TLS certificate management.** Loading, rotation, and revocation policies TBD; rustls-ffi provides primitives, Flux-side ergonomics deferred to Phase 4 design.
7. **Phase 5 trigger criteria.** What workload data justifies committing to atomic RC? Not specified; intentionally requires future judgment based on real users.

## Future possibilities

- **HTTP/2 multiplexing** — once HTTP/1.1 is stable. Significant complexity; likely a separate proposal.
- **WebSocket and Server-Sent Events** — both fall out of HTTP/1.1 + streams in Phase 2 with small additional work.
- **gRPC** — HTTP/2 + protobuf. Future proposal.
- **io_uring backend for Linux** — once libuv becomes a perf bottleneck. Eio demonstrates the dual-backend pattern.
- **Distributed actor model** — built on Phase 3 process-per-core + Phase 4 networking. Userspace library.
- **Job queue / scheduled tasks** — userspace library on top of `sleep` + persistent storage.
- **File watchers** — `inotify`/`fsevents` via libuv's `uv_fs_event_t`.
- **GraphQL server** — HTTP + JSON + DataLoader-style fan-out via `interleaved`.

## Appendix: end-to-end POST request trace (Phase 1 + Phase 2)

A `POST` request from user code to the wire and back, illustrating how
Async, libuv, and the existing effect-handler runtime compose.

User code:

```flux
let resp = Http.post("https://api.example.com/users", body)
```

`Http.post` is Flux code: format request bytes, `Tcp.connect`,
`Tls.handshake`, `Tcp.write`, repeated `Tcp.read` until response complete,
parse. Each I/O call performs `Async.await(setup)`.

For one `Tcp.write`:

1. **VM:** `OpPerform` (opcode 69) executes at [src/vm/dispatch.rs:1287-1451](../../src/vm/dispatch.rs). Walks evidence vector, finds `Async` handler. Not direct (must capture continuation). Sets `yield_state.yielding = Pending`. Captures post-perform continuation via `Continuation::compose()` ([src/runtime/continuation.rs:49-93](../../src/runtime/continuation.rs)). Calls handler arm with `(composed_cont, write_setup, state)`.

   **LLVM:** equivalent — emits `flux_yield_to(htag, optag, arg, arity)`. The `cont_split` pass synthesized the continuation as a block parameter at compile time. Both backends call into the same C runtime functions for the yield protocol.

2. **Handler arm runs (Flux code).** Calls `write_setup`, which FFIs to `flux_uv_tcp_write(resume, conn, data)`. C glue dups `resume` and `data`, calls `uv_write`, returns. Handler arm calls `drive_loop()`, which FFIs to a C primop wrapping `uv_run(loop, UV_RUN_ONCE)`.

3. **`uv_run` blocks until libuv detects the socket is writable.** OS reports readiness. libuv calls our `uv_write_cb`. C glue: drops the duped `data`, calls `flux_async_enqueue(resume, n_bytes_written)`, returns. `uv_run` returns. `drive_loop` pops the queue, returns `(resume, value)` to the handler arm. Handler arm calls `resume(value)`.

4. **Resume.** VM: `execute_resume` restores frames, restores stack slice, pushes `n_bytes_written` where `perform` would have returned. LLVM: jumps to the post-perform block with the value as block parameter. `Tcp.write` returns. `Http.post` continues to the next `Tcp.read`, repeating the cycle.

5. **Many awaits later, response is fully read and parsed.** `Http.post` returns to user code. `let resp = ...` gets the response.

Throughout: refcounts are non-atomic (single-threaded Phase 1). The C glue's
dup-on-entry / drop-on-completion rule keeps Aether's static analysis
sound across the FFI boundary. Cancellation (e.g., from a surrounding
`timeout`) calls `uv_cancel`, which guarantees the original callback fires
exactly once with `UV_ECANCELED`, drops the duped Flux values normally, and
delivers a `Canceled` error to `resume`. The continuation raises and unwinds
into the `cancelable` scope.

This is the entire concurrency model for Phases 1-4. Phase 5 generalizes
the threading assumption (atomic RC, thread-local globals) without
changing the user-facing API.
