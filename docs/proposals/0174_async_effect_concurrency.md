- Feature Name: Async Effect & Concurrency Roadmap
- Start Date: 2026-04-27
- Status: Draft (revision 2 — supersedes the original five-phase plan; see "Revision history" at end)
- Proposal PR:
- Flux Issue:
- Depends on: existing effect handlers ([runtime/c/effects.c](../../runtime/c/effects.c), [src/runtime/continuation.rs](../../src/runtime/continuation.rs)), existing FFI primop machinery
- Includes: language feature work on plain type aliases (see "Required language features" below)
- Relates to: [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) — see "Relationship to 0143" below

# Proposal 0174: Async Effect & Concurrency Roadmap

## Summary

Introduce concurrency to Flux as a layered runtime whose substrate is
modelled directly on Lean 4 (`src/runtime/uv/`, ~3,800 lines C++), and
whose user-facing API is modelled on OCaml/Eio (`lib_eio/core/`,
three-effect seam).
The driving use case is **HTTP microservices and data streams**; the
technical foundation is a multi-threaded libuv runtime carrying a fiber
layer that uses Flux's existing continuation-capture machinery to provide
M:N cooperative concurrency with structured-concurrency primitives.

The roadmap is one phase split into two milestones, plus follow-on
phases:

- **Phase 1a — Multi-threaded runtime substrate.** Worker thread pool, mutex-protected libuv loop, hybrid atomic-on-share RC, `Task<a>` primitive. Multi-core from day one. Substrate identical in shape to Lean 4's.
- **Phase 1b — Fiber layer + structured concurrency.** Three-effect seam (`Suspend`/`Fork`/`GetContext`) on the Phase 1a substrate. Lightweight fibers via existing continuation capture. `interleaved`/`firstof`/`timeout`/`cancelable` as Flux source. M:N concurrency density: thousands of fibers per worker thread.
- **Phase 2 — HTTP/1.1 + JSON + Streams.** Unchanged from the original proposal.
- **Phase 3 — TLS + database client.** Was Phase 4 in the original.
- **(Optional) Phase 4 — io_uring backend for Linux.** Backend swap behind the same seam if perf measurements justify it.

The original Phase 3 (process-per-core) is removed: multi-threading
lands in Phase 1a, so process-per-core is no longer a stepping stone.
The original Phase 5 (shared-state multi-threading via atomic RC) is
removed: hybrid RC ships in Phase 1a, following Lean's and Koka's
actual production scheme rather than the misread "atomic everywhere"
target the original proposal aimed at.

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

- **0143 Phase A (thread-per-actor)** is subsumed by **0174 Phase 1a
  (worker thread pool)**. Actors become userspace patterns over the
  thread pool; the isolation guarantee is by convention plus
  `Sendable<T>` (Phase 1a) rather than by separate heap.
- **0143 Phase B (typed mailboxes + compile-time `Sendable<T>`)** is
  partially absorbed: `Sendable<T>` ships as part of Phase 1a's
  cross-thread RC discipline. Typed mailboxes remain a 0143 deliverable.
- **0143 Phase C (supervision + cancellation)** becomes a Flux library
  built from 0174's `firstof`, `cancelable`, and `Process.wait`.
  Erlang-style supervision trees are buildable from these primitives.
- **0143 Phase D (work-stealing M:N scheduler + deterministic test
  scheduler)** — the deterministic test scheduler is naturally
  expressed against 0174's three-effect seam, which is swappable at
  the handler level. Work-stealing across worker threads is explicitly
  out of scope for 0174 (Eio's no-fiber-migration model); revisited
  only if load imbalances become a measured problem.

The driving goal stated by the project — **HTTP microservices and data
streams** — points at the I/O-layer story. 0174 Phase 1a+1b+2 ships a
working multi-threaded microservice with c10k-class concurrency;
0143's Phase A-B alone is ~10 weeks before any network socket is
touched. Sequencing 0174 first does not abandon 0143's design work;
it provides the runtime substrate on which 0143's isolation and
supervision story becomes more economical to build.

0143 is therefore marked as **deferred** rather than superseded, with its
phases re-targeted to follow 0174. Its sendability rules,
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

A working Flux microservice in roughly the shape we want (using
named-field records via `data` per proposal 0152, and `deriving`
for codec generation):

```flux
import Flow.Http
import Flow.Json

data CreateUser { CreateUser { name: String, email: String } }
    deriving (Json.Encode, Json.Decode)

data UserId { UserId { id: Int } }
    deriving (Json.Encode, Json.Decode)

fn handler(req: Request) -> Response with Async {
    match req.method {
        Post -> match req.path {
            "/users" -> {
                let body: CreateUser = Json.decode(req.body)
                let new_id = Db.insert("users", body)
                Http.json_response(200, Json.encode(UserId { id: new_id }))
            },
            _ -> Http.not_found(),
        },
        _ -> Http.not_found(),
    }
}

fn main() with Async {
    Http.serve("0.0.0.0", 8080, handler)
}
```

Reaching that shape requires Async, structured concurrency, HTTP,
JSON, streams, TLS, and a database driver. That's the scope of
Phases 1a, 1b, 2, and 3.

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

### Required language features

Phase 1b's library API depends on one language feature that Flux
does not currently support, plus a handful of ergonomic gaps that
are not strict prerequisites but would meaningfully improve the
user-facing syntax. This proposal includes the prerequisite
language work; the ergonomic items are documented here for future
re-evaluation.

#### Prerequisite: plain type aliases (included in this proposal)

Phase 1b's setup-closure pattern (the contract for libuv-backed
async operations) is awkward without function-type aliases. Today,
`type Name<...> = ...` is restricted to ADT-sugar form
(constructor variants separated by `|`); any non-constructor type
expression on the right-hand side is a parse error. Without
aliases, every TCP/UDP/DNS/timer/signal/fs wrapper must inline the
full closure shape at every call site:

```flux
public fn await_one_shot<a>(
    setup: (FiberId, (Result<a, AsyncError>) -> Unit) -> CancelHandle
) -> a with Async
```

With aliases:

```flux
type ResumeFn<a> = (Result<a, AsyncError>) -> Unit
type SetupFn<a>  = (FiberId, ResumeFn<a>) -> CancelHandle

public fn await_one_shot<a>(setup: SetupFn<a>) -> a with Async
```

This proposal extends `type` to accept any type expression on the
right-hand side. ADT sugar continues to work unchanged; new shapes
(function types, applied generic types, tuples, effect-row-bearing
closure types) become legal.

##### Grammar change

Today's parser ([src/syntax/parser/statement.rs:1483](../../src/syntax/parser/statement.rs))
expects:

```
TypeDecl ::= 'type' Ident TypeParams? '=' AdtVariant ('|' AdtVariant)*
```

The proposed grammar:

```
TypeDecl ::= 'type' Ident TypeParams? '=' TypeRhs
TypeRhs  ::= AdtVariant ('|' AdtVariant)*       (existing — ADT sugar)
           | TypeExpr                            (new — alias)
```

Disambiguation between ADT sugar and alias is driven by what
follows `=`: an uppercase identifier in introducing position
(followed by `|`, `(`, or end-of-statement) parses as ADT sugar;
anything else (lowercase identifier, applied generic, function
type, tuple) parses as alias by calling the existing
`parse_type_expr`. Same rule already used to distinguish
constructor calls from function calls in expressions.

##### Semantics: transparent, not nominal

Type aliases are **fully transparent** — expanded by the type
checker before any structural comparison. Two values whose declared
types are different aliases of the same underlying type are
unifiable without coercion:

```flux
type Predicate<a> = (a) -> Bool
type Filter<a>    = (a) -> Bool

fn even(n: Int) -> Bool { n % 2 == 0 }

let p: Predicate<Int> = even
let f: Filter<Int>    = p   // OK — both expand to (Int) -> Bool
```

This matches Haskell's `type` synonym semantics and OCaml's `type`
abbreviation semantics. Aliases are abbreviations, not new types.
For *nominal* distinct types (a `UserId` distinct from a plain
`Int`), `data UserId { UserId(Int) }` remains the right answer.

##### Restrictions

To keep the feature small, the initial implementation rejects:

- **Recursive aliases.** `type Cycle = Cycle` and any cycle through
  alias expansion are errors (E308). ADT-sugar declarations remain
  recursive as today (term-level constructors give a base case).
- **Phantom type parameters.** Every type parameter must appear on
  the right-hand side, matching the existing rule for ADT sugar.
- **Constraints on alias parameters.** `type SortedArray<a: Ord> = Array<a>`
  is rejected — write a class instance instead.
- **Higher-kinded aliases.** `type Mapped<f, a> = f<a>` is out of
  scope; HKT exists in class declarations but not in aliases for
  this slice.
- **`deriving` on alias declarations.** Only ADT-sugar `type` may
  carry `deriving` — there are no constructors for the alias path.
- **Alias-expansion depth above 64.** The expander caps recursion
  to defend against pathological input.

These restrictions match what shipped first in Haskell and OCaml;
they can be lifted incrementally.

##### Effect rows in aliases

Aliases may contain effect-row syntax, including row variables:

```flux
type AsyncFn<a, b, e>   = (a) -> b with <Async | e>
type Handler<req, resp> = (req) -> resp with <Async | Console>
```

When such an alias expands into a function signature, the row check
happens against the expanded form. Row variables inside aliases are
bound at the alias declaration.

##### Visibility

`public type Name = ...` exports the alias; without `public` the
alias is module-local. Same convention as existing `data`/`fn`
declarations.

##### Implementation sketch

1. **Parser** ([src/syntax/parser/statement.rs:1483-1620](../../src/syntax/parser/statement.rs)): after `=`, peek the next token. If the next-token shape matches an ADT-variant introducer that is not in scope as a type, take the existing path. Otherwise call `parse_type_expr` and produce a new `Statement::TypeAlias` AST node.
2. **AST**: new variant `TypeAlias { name, params, body, span }`.
3. **Name resolution**: per-module alias table populated alongside the existing ADT and effect tables.
4. **Type expansion**: extend the existing substitution code to detect alias references and expand them; recursion-depth counter capped at 64.
5. **Cycle detection**: when registering an alias, traverse the expanded body for self-references; emit E308 on cycle.
6. **Diagnostics**: extend the type-mismatch reporter to show the alias name when the user wrote one, with the expansion available via verbose mode.
7. **Tests**: parity tests that an alias and its expansion are interchangeable in function signatures, type-class instances, and pattern positions.

Estimated effort: 1-2 weeks. The work lands as part of Phase 1b
preparation; library code in `lib/Flow/Async.flx` and
`lib/Flow/Tcp.flx` uses aliases from day one.

#### Ergonomic gaps to re-evaluate (not prerequisites)

The remaining items below are **not** prerequisites; Phase 1b
ships correctly with the syntax Flux has today. They are listed
here so readers see where the user-facing API would benefit from
future ergonomic work, in priority order:

- **String interpolation in plain string literals** — the lexer
  already has `InterpolationStart` (`#{...}`) tokens; threading
  them through the parser/typer eliminates the `String.concat`
  chains in log calls and URL construction. Likely a small ticket
  alongside Phase 1b.
- **Negative type-class instances or an opt-out marker** — Phase
  1a needs to express "`Connection` is not `Sendable`." The choice
  between explicit `instance !Sendable for T` syntax, default-out
  (only positive instances exist; absence means not-Sendable), or
  capture-driven inference is a Phase 1a design decision.
  Default-out needs no new syntax and is the recommended route;
  flagged here for visibility.
- **Tuple destructuring in `let`** — `let (a, b) = pair` instead
  of `match pair { (a, b) -> ... }`. Pure ergonomics.
- **`try` / `finally` / `catch` syntax sugar** — Phase 1b ships
  `Async.protect(body, cleanup)` and `Async.try_(body)` as plain
  functions. Sugar would compile to the same calls. Low priority.
- **`loop` / `while` keywords or stdlib `Async.forever`** — the
  recursive `accept_loop` pattern works (TCO ensures constant
  stack); a library helper `Async.forever(body)` covers the common
  case without new syntax.
- **Named function arguments** — `Http.serve(addr: ..., port: ...)`
  reads better than positional. The current workaround is a small
  `Config` record passed as one argument; this works but is heavier
  for 3+ argument call sites.

All items above are re-evaluable post-Phase 1b; none of them block
the runtime architecture.

### Architecture overview

The runtime is organised as four layers, bottom-up:

```
┌─────────────────────────────────────────────────┐
│  Flux source — Phase 1b                         │
│    interleaved, firstof, timeout, cancelable    │
│    Effect handler arms for Suspend/Fork/        │
│      GetContext                                  │
└────────────────────┬────────────────────────────┘
                     │  three-effect seam
┌────────────────────▼────────────────────────────┐
│  Rust scheduler — Phase 1b adds fibers          │
│                   Phase 1a has Tasks            │
│    Per-worker fiber ready queues                │
│    Continuation registry, wait registry         │
└────────────────────┬────────────────────────────┘
                     │  enqueue(target, result)
┌────────────────────▼────────────────────────────┐
│  C libuv glue — Phase 1a                        │
│    Mutex-protected uv_loop_t                    │
│    Timer, TCP, fs, signals                      │
│    dup-on-entry / drop-on-completion            │
│    Worker thread pool                           │
└────────────────────┬────────────────────────────┘
                     │  C ABI
┌────────────────────▼────────────────────────────┐
│  libuv (vendored) — epoll / kqueue / IOCP       │
└─────────────────────────────────────────────────┘
```

The bottom three layers are stable from Phase 1a onward. Phase 1b
adds the top layer plus per-worker fiber state inside the scheduler.

### Phase 1a: Multi-threaded runtime substrate

The minimum runtime that compiles, links, and runs Flux programs across
multiple OS threads with libuv-backed I/O. Modelled on Lean 4's
`task_manager` (Lean 4 `src/runtime/object.cpp:706-916`) and
`event_loop_t` (Lean 4 `src/runtime/uv/event_loop.h:24-30`).

#### The libuv loop

A single global `uv_loop_t`, mutex-protected. Threads contend for the
mutex when they need to drive the loop or register an operation; one
thread runs the loop at a time, the others sleep on a condition variable.
Cross-thread wakeup uses `uv_async_t`. Identical structure to Lean 4's:

```c
// runtime/c/async_io.c
typedef struct {
  uv_loop_t  *loop;
  uv_mutex_t  mutex;
  uv_cond_t   cond_var;
  uv_async_t  wakeup;
  _Atomic(int) n_waiters;
} flux_event_loop_t;

extern flux_event_loop_t flux_global_loop;
```

#### Hybrid atomic-on-share refcount

`FluxHeader.refcount` becomes a sign-bit-encoded `_Atomic(int32_t)`:

- `rc > 0` — single-threaded reference, increment/decrement non-atomically.
- `rc < 0` — thread-shared reference, increment/decrement with `memory_order_relaxed` atomic.
- `rc == 0` — unique (fast path for in-place reuse).

This is **the actual scheme used by both Lean 4** (`src/include/lean/lean.h:131-136, 544-568`) **and Koka**
(`kklib/include/kklib.h:101-135`), not the "atomic everywhere" scheme
the original 0174 misattributed
to Koka. Single-threaded paths pay no atomic cost. The transition from
positive to negative happens lazily when a value first crosses a thread
boundary (e.g., during `Sendable<T>`-typed channel send or `Task` spawn).

Aether's existing `dup`/`drop` insertion is unchanged. The only change
is in the `flux_dup`/`flux_drop` primitives in `runtime/c/rc.c`:

```c
static inline void flux_dup_ref(flux_object_t *o) {
  if (LEAN_LIKELY(o->rc > 0)) {
    o->rc++;
  } else if (o->rc != 0) {
    atomic_fetch_sub_explicit(&o->rc_atomic, 1, memory_order_relaxed);
  }
}
```

#### Worker thread pool

N OS threads, where N defaults to `uv_available_parallelism()`. Each
thread runs a loop that pulls work from a shared priority queue. Phase
1a's "work" is `Task<a>`; Phase 1b extends this to fibers.

```c
typedef struct {
  std::atomic<bool> shutdown;
  std::mutex mutex;
  std::condition_variable cond;
  std::deque<flux_task_t*> queues[FLUX_MAX_PRIO + 1];
  std::vector<std::thread> workers;
} flux_task_manager_t;
```

#### `Sendable<T>` constraint

Cross-thread types require `Sendable<T>`, a type class auto-derived for:
- All primitive types (`Int`, `Float`, `Bool`, `String`, etc.).
- ADTs whose every field is `Sendable`.
- Persistent collections of `Sendable` elements.

Inspired by Rust's `Send` trait but checked at compile time via Flux's
existing dictionary-elaboration pass
([src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs)).
This is meaningfully stronger than OCaml/Eio's by-convention warning,
which the Eio domain manager docstring (`lib_eio/domain_manager.mli`)
explicitly admits is unenforced.

#### `Task<a>` primitive

Phase 1a's user-facing concurrency primitive (Phase 1b adds a higher-level
fiber API on top). Constraints are written inline in the type-parameter
list, the form Flux already uses elsewhere (`fn keep<a: Num + Eq>(...)`):

```flux
module Flow.Task {
    public data Task<a> { Task(Int) }   // wraps an opaque task id

    public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
    public fn await<a: Sendable>(t: Task<a>) -> a
    public fn cancel<a>(t: Task<a>) -> Unit
}
```

Tasks run on whichever worker thread picks them up. `await` blocks the
calling thread until the task completes; the caller's worker is parked
on the condition variable, so other workers continue. This is **Lean 4's
exact model** and is sufficient for compute-bound parallelism.

#### libuv glue: `runtime/c/async_io.c`

~600 lines C. Surface:

```c
// Loop lifecycle (one global loop)
void       flux_uv_loop_init(int n_workers);
void       flux_uv_loop_close(void);

// Loop driver — called by whichever worker holds the mutex
void       flux_uv_run_until_idle(void);
void       flux_uv_wakeup(void);              // uv_async_send

// Timer
void       flux_uv_timer_start(int64_t fid_or_tid, int64_t ms);

// TCP
void       flux_uv_tcp_connect(int64_t fid, int64_t host, int64_t port);
void       flux_uv_tcp_listen(int64_t fid, int64_t addr, int64_t port);
void       flux_uv_tcp_accept(int64_t fid, int64_t listener);
void       flux_uv_tcp_read(int64_t fid, int64_t conn, int64_t max);
void       flux_uv_tcp_write(int64_t fid, int64_t conn, int64_t data);
void       flux_uv_tcp_close(int64_t fid, int64_t conn);

// File I/O (libuv worker pool — distinct from Flux's worker threads)
void       flux_uv_fs_read(int64_t fid, int64_t path);
void       flux_uv_fs_write(int64_t fid, int64_t path, int64_t data);

// DNS
void       flux_uv_dns_resolve(int64_t fid, int64_t host);

// Cancellation
void       flux_uv_cancel(uv_req_t *req);
```

Each `int64_t fid_or_tid` parameter identifies what to wake when the
operation completes — a `Task` ID in Phase 1a, a fiber ID in Phase 1b.
The C glue does not need to know which: it calls a single
`flux_runtime_complete(id, result)` upcall and the Rust scheduler
dispatches.

The single load-bearing RC rule, identical to the original 0174 and to
Lean's existing libuv glue: **every Flux value escaping to libuv is duped
on entry and dropped in the completion callback.**

#### Phase 1a deliverables

- `runtime/c/async_io.c` — libuv glue, ~600 lines C.
- `runtime/c/rc.c` — hybrid atomic refcount, ~30 lines added.
- `src/runtime/scheduler.rs` — task manager, ~400 lines Rust.
- `lib/Flow/Task.flx` — `Task<a>` API, ~80 lines Flux.
- `Sendable<T>` type class — ~150 lines across types/ and core/.
- ~10 new `CorePrimOp` enum entries.
- VM and LLVM dispatch for the new primops.
- Build-system: vendor libuv under `vendor/libuv/`, link statically.
- Examples: parallel CPU-bound work, `Task.spawn` + `Task.await` smoke tests.

#### Phase 1a forward-compatibility rules

Two decisions that keep Phase 1b cheap:

1. **The `flux_runtime_complete(id, result)` upcall is the single re-entry point from C to the runtime.** Phase 1b extends the meaning of `id` from `Task` to fiber/continuation; the C glue is unchanged.
2. **Worker threads run a dispatch loop in Rust, not in Flux source.** Phase 1b changes the dispatch loop body to pick a fiber and resume its captured continuation; the worker-thread management is shared with Phase 1a.

### Phase 1b: Fiber layer + structured concurrency

The Phase 1a substrate gives N OS threads × 1 active task per thread.
Phase 1b adds a fiber layer on top: N threads × M fibers per thread,
cooperatively scheduled. This delivers c10k-class concurrency density
required for the proposal's stated workload (HTTP microservices).

#### The three-effect seam

Modelled directly on Eio's seam
(Eio `lib_eio/core/eio__core.ml:15-21`):

```flux
module Flow.Async.Internal {
    // FiberContext carries the per-fiber state the scheduler needs to
    // resume, cancel, or interrogate a suspended fiber. It is opaque
    // to user code and only handled by the runtime.
    public data FiberContext {
        FiberContext {
            cancel_scope: CancelScope,
            fiber_id:     FiberId,
            parent:       Option<FiberContext>,
        }
    }

    // The three seam labels. Their operations are seeded by the
    // compiler (analogous to `Console`, `FileSystem` in
    // `Flow.Effects`); the declarations here are documentation that
    // the drift-protection test verifies against the seed.
    effect Suspend
    effect Fork
    effect GetContext
}
```

The bodies of `Suspend`, `Fork`, and `GetContext` are seeded by the
compiler rather than declared with operation rows in source. This
matches the approach `Flow.Effects` uses for `Console`/`FileSystem`/
`Clock`: the effect labels are documented in source but their
operation set is the compiler's source of truth, with a CI drift test
keeping the two in sync.

User code never sees `Suspend`/`Fork`/`GetContext` directly. They are
bundled as `with Async` in user-facing signatures; the
structured-concurrency primitives below are written in terms of them.

#### Structured concurrency primitives (Flux source)

All in `lib/Flow/Async.flx`, modelled on Eio's
`lib_eio/core/fiber.ml`:

```flux
fn run_async<a>(action: () -> a with Async) -> a
fn interleaved<a, b>(f: () -> a with Async, g: () -> b with Async) -> (a, b) with Async
fn firstof<a>(f: () -> a with Async, g: () -> a with Async) -> a with Async
fn timeout<a>(ms: Int, f: () -> a with Async) -> Option<a> with Async
fn cancelable<a>(f: () -> a with Async) -> a with Async
fn yield_now() with Async
fn sleep(ms: Int) with Async
```

Differences from Lean 4's API (`Std/Async/Basic.lean:524-528`):
**Flux's `firstof` cancels the loser**; Lean's `race` does not.
Cooperative scheduling makes cancellation straightforward (set a flag,
do not resume the continuation when libuv fires the callback);
thread-pool tasks make it hard, which is why Lean punted.

#### Per-worker fiber state

Each worker thread maintains a local fiber ready queue. When a fiber
`await`s, the runtime captures its continuation (using the existing
[src/runtime/continuation.rs](../../src/runtime/continuation.rs)
machinery), registers the continuation in the wait registry keyed by
the libuv operation ID, and the worker immediately picks the next ready
fiber. When libuv fires a completion, the runtime moves the corresponding
fiber back to its worker's ready queue.

Fibers do not migrate between workers (Eio's model). A fiber spawned via
`interleaved` runs on the same worker as its parent. Cross-worker
parallelism comes from many top-level requests landing on different
workers (e.g., the HTTP listener round-robins on accept), not from
splitting one request across workers.

#### Cancellation propagation

A `cancelable<a> { f(); }` block establishes a `CancelScope`. When cancellation
is requested:

1. The scope's `canceled` flag is set.
2. Any libuv requests registered under fibers in the scope have `uv_cancel(req)` called.
3. libuv guarantees each cancelled callback fires exactly once with `UV_ECANCELED` (Linux/macOS) or with whatever result was in flight (Windows IOCP — see Drawbacks).
4. The completion handler delivers a `Canceled` error to the suspended fiber.
5. The fiber's resume raises `AsyncError.Canceled`, which unwinds to the `cancelable` boundary.

`timeout(ms, f)` is a `cancelable` over `firstof { f(); sleep(ms); fail() }`.

#### Aether / Perceus interaction

Three considerations, identical in shape to the original 0174:

1. **`perform Suspend` must not have its argument dropped before libuv completes.** The dup-on-entry / drop-on-completion rule at the FFI boundary balances Aether's drop after the perform returns.
2. **Continuation capture is RC-correct by construction.** Captured frame slots are duped during composition; resume drops on consumption. Continuations that never resume (cancellation) drop their captures via the cancellation path.
3. **`@fip`/`@fbip` functions called during a fiber's lifetime do not interact with cross-thread RC** because the fiber's heap stays on its worker thread. Only values that explicitly cross thread boundaries via `Sendable<T>` channels see atomic RC.

#### Phase 1b deliverables

- `lib/Flow/Async.flx` — effect declarations + structured concurrency primitives. ~250 lines Flux.
- `lib/Flow/Tcp.flx` — TCP wrappers expressed as `Async` operations. ~150 lines Flux.
- `src/runtime/scheduler.rs` — fiber layer added on top of the Phase 1a task manager. ~600 additional lines Rust.
- ~5 new `CorePrimOp` entries for fiber suspend/resume.
- Examples: TCP echo server (10k concurrent connections), parallel TCP fetch via `interleaved`, `timeout`-bounded connect, cancellation propagation tests.
- Parity tests in `tests/parity/async/` — VM and LLVM produce identical output for all examples.

### Phase 1b: Detailed networking syntax design

This section spells out the user-facing Flux syntax for networking
calls, including closure shapes, effect-row composition, error
handling, cancellation, resource lifecycles, and how the underlying
three-effect seam (`Suspend`/`Fork`/`GetContext`) is hidden behind
ergonomic library APIs.

#### The `Async` effect row alias

User code never names `Suspend`, `Fork`, or `GetContext` directly. They
appear only inside the runtime and inside library implementations. User
signatures use a row alias declared in `Flow.Effects` alongside `IO`
and `Time`:

```flux
// lib/Flow/Effects.flx — additions

effect Suspend
effect Fork
effect GetContext
effect AsyncFail        // recoverable async I/O failures

// Async is what shows up in user signatures
alias Async = <Suspend | Fork | GetContext | AsyncFail>
```

The seeding mechanism documented at the top of `Flow.Effects.flx`
applies: these are compiler-seeded labels and aliases. `Async` is the
only async-related row that appears in user signatures; the underlying
labels are implementation detail. Adding new I/O capabilities extends
library code that performs `Async`, not the effect declaration itself.

#### The `AsyncError` data type

All Async-aware library functions surface failures via the
`AsyncFail` effect carrying an `AsyncError` payload. The error
type is a plain Flux `data` declaration:

```flux
public data AsyncError {
    Canceled,                                 // cooperative cancel from cancelable/timeout/firstof
    TimedOut,                                 // distinct from Canceled — surfaced by `timeout`
    IoError(Int, String, String),             // (code, message, syscall)
    DnsError(Int, String, String),            // (code, message, host)
    TlsError(Int, String),                    // Phase 3
    ProtocolError(Int, String),               // HTTP, Postgres
    ConnectionClosed,
    InvalidAddress(String),                   // (input)
}
```

`AsyncError` is the standard error payload for all Phase 1b–3
libraries. Functions surface failure via `AsyncFail` (an effect label
in the `Async` row), which a top-level handler converts into a
`Result<a, AsyncError>` outcome. The function signature simply lists
`Async` as part of its effect row — there is no Haskell-style
parameterized `Exn<E>`, because Flux effect labels are unparameterized.

```flux
fn connect(host: String, port: Int) -> Connection with Async
```

The fact that `connect` may fail is encoded in the `AsyncFail` label
inside `Async`, not in the return type. Library helpers (`try_async`,
`catch_async`, etc.) translate `AsyncFail` raises into
`Result<Connection, AsyncError>` at the boundary where the user wants
to inspect the error.

#### `Bytes` primitive

Phase 1b adds a new built-in scalar-array type `Bytes` (a packed
`Array<UInt8>` with native-array runtime layout). It is created by
network read operations and consumed by network write operations; user
code can also construct one from a `String`:

```flux
public intrinsic fn String.to_bytes(s: String) -> Bytes = primop StringToBytes
public intrinsic fn Bytes.length(b: Bytes) -> Int = primop BytesLength
public intrinsic fn Bytes.slice(b: Bytes, start: Int, end: Int) -> Bytes = primop BytesSlice
public intrinsic fn Bytes.to_string(b: Bytes) -> String = primop BytesToString
```

`Bytes` is `Sendable` (its content is a primitive scalar array with no
shared mutable state).

#### Connection types: nominal opaque, RC-counted, with attached lifecycle

```flux
module Flow.Tcp {
    // Single-constructor data types whose constructors stay private to
    // this module give nominal opacity. Consumers see the type name but
    // cannot deconstruct or build instances.
    public data Connection { Connection(Int) }   // wraps a runtime handle id
    public data Listener   { Listener(Int) }
    public data Address    { Address(String, Int) }   // (host, port)

    // Construction
    public fn connect(host: String, port: Int) -> Connection with Async { ... }
    public fn listen(addr: String, port: Int)  -> Listener   with Async { ... }
    public fn accept(listener: Listener)        -> Connection with Async { ... }

    // Operations
    public fn read(conn: Connection, max: Int)         -> Bytes with Async { ... }
    public fn read_exact(conn: Connection, n: Int)     -> Bytes with Async { ... }
    public fn write(conn: Connection, data: Bytes)     -> Int   with Async { ... }
    public fn write_all(conn: Connection, data: Bytes) -> Unit  with Async { ... }
    public fn close(conn: Connection) -> Unit { ... }            // synchronous and infallible

    // Inspection
    public fn local_addr(conn: Connection)  -> Address { ... }
    public fn remote_addr(conn: Connection) -> Address { ... }
}
```

`Connection` is a single-constructor `data` type whose constructor is
not re-exported from the module — consumers see the type name but
cannot deconstruct or fabricate instances. The wrapped `Int` is an
opaque runtime handle id resolved by the scheduler. When the handle's
refcount drops to zero, the runtime calls `uv_close` —
**explicit `close` is optional but recommended for predictable lifecycle**.

`Connection` is **not** `Sendable` — a connection is bound to the
worker thread that opened it and cannot be sent to another worker.
This is a deliberate choice: socket FDs are not safely usable across
threads in all OS combinations Flux supports. Phase 1a's `Sendable`
class is a marker class; types are `Sendable` by default and library
authors opt out with an explicit `instance !Sendable for Connection`.
The compile-time check at `Channel.send` / `Task.spawn` boundaries
refuses cross-worker sharing.

#### Closure-style scoped resource lifecycles: `with_*` combinators

The recommended idiom for connection lifecycles is the `with_*`
pattern, which guarantees `close` is called whether the body
completes, fails, or is cancelled. Flux has no `try`/`finally`
syntax; cleanup is provided by a library function `Async.protect`
that takes a body closure and a cleanup closure:

```flux
module Flow.Tcp {
    public fn with_connection<a, e>(
        host: String,
        port: Int,
        body: (Connection) -> a with <Async | e>
    ) -> a with <Async | e> {
        let conn = Tcp.connect(host, port)
        Async.protect(
            fn() { body(conn) },
            fn() { Tcp.close(conn) }
        )
    }
}

fn fetch(host: String, port: Int) -> Bytes with Async {
    Tcp.with_connection(host, port, fn(conn) {
        let _ = Tcp.write_all(conn, String.to_bytes("GET /\r\n\r\n"))
        Tcp.read(conn, 4096)
    })
}
```

Three things to notice:

1. **The closure's effect row is `<Async | e>`** — `e` is a row
   variable inherited from the caller. `with_connection` does not
   constrain what other effects the body uses; it just guarantees
   `close` runs. This is standard Flux row polymorphism.
2. **The closure receives the connection by RC handle.** The
   closure uses it freely but does not own its lifetime;
   `with_connection` retains responsibility for `close`.
3. **`Async.protect` is the cleanup primitive.** It runs the cleanup
   closure whether the body returns, raises an `AsyncFail`, or is
   cancelled. Implemented internally via the `Cancel.protect`
   mechanism in `Flow.Async`.

#### Servers: handler closures and the listener loop

A TCP server in Phase 1b is a single function that recursively
accepts connections and forks a daemon fiber per connection. Flux
has no `loop`/`while` keyword; iteration is via tail-recursive
helpers (a familiar pattern in `Flow.IO`):

```flux
module Flow.Tcp {
    public fn serve<e>(
        addr: String,
        port: Int,
        handler: (Connection) -> Unit with <Async | e>
    ) -> Unit with <Async | e> {
        let listener = Tcp.listen(addr, port)
        Async.cancelable(fn() { accept_loop(listener, handler) })
    }

    fn accept_loop<e>(
        listener: Listener,
        handler: (Connection) -> Unit with <Async | e>
    ) -> Unit with <Async | e> {
        let conn = Tcp.accept(listener)
        // Fork a daemon fiber per connection; per-connection failures
        // are caught locally and do not bring down the server.
        Async.fork_daemon(fn() {
            Async.protect(
                fn() {
                    let _ = Async.try_(fn() { handler(conn) })
                    ()
                },
                fn() { Tcp.close(conn) }
            )
        })
        accept_loop(listener, handler)
    }
}

fn main() with Async {
    Tcp.serve("0.0.0.0", 8080, fn(conn) {
        let _req = Tcp.read(conn, 4096)
        let _ = Tcp.write_all(
            conn,
            String.to_bytes("HTTP/1.1 200 OK\r\n\r\nhello")
        )
        ()
    })
}
```

Three design decisions surface here:

1. **`fork_daemon` vs `fork`.** `fork` requires a parent scope and the
   parent awaits the child (Eio's `Switch`-style). `fork_daemon` is
   for long-lived background fibers whose completion the parent does
   not await. The accept loop spawns daemons because accept loops are
   inherently unbounded.
2. **Error handling is per-connection.** `Async.try_` catches
   `AsyncFail` raised by the handler and yields a `Result`; here it
   is discarded so that one bad request does not kill the server.
   Cancellation (e.g., the surrounding `cancelable` block exiting)
   propagates to all forked daemons and triggers their `Async.protect`
   cleanups.
3. **`cancelable` is the lifecycle scope.** Exiting `cancelable` —
   by failure, by external cancel, by timeout from a parent — cancels
   all in-flight handlers. Once accepted, a connection is fully owned
   by its fiber's `Async.protect` cleanup.

`accept_loop` is tail-recursive and the existing TCO detection pass
([ast/tail_position.rs](../../src/ast/tail_position.rs)) ensures it
runs in constant stack regardless of how many connections are
accepted.

#### Effect rows in practice: composition with other effects

User handlers commonly need additional effects beyond `Async`. The
effect row `<Async | Console | e>` below composes via Flux's existing
row-polymorphic syntax — `Console` is one of the I/O labels already
seeded by the compiler in `Flow.Effects`. `e` is a row variable that
lets a caller layer further effects on top:

```flux
fn http_handler<e>(req: Request) -> Response
    with <Async | Console | e>
{
    let _ = perform println(String.concat("request: ", Http.path(req)))
    match Http.path(req) {
        "/users" -> handle_users(req),
        "/posts" -> handle_posts(req),
        _        -> Http.not_found(),
    }
}
```

The `with_*` combinators and `serve` are row-polymorphic in `e` —
they require `Async` in the row but propagate any other effects to
the caller of `serve` unchanged. This is what lets handler code carry
logging, config, metrics, etc., without `serve` having to know about
them.

`Console` is illustrative — a richer structured-logging effect is a
userspace library; Phase 1b only adds the `Async`-related labels.

#### Structured concurrency: closure shapes and cancellation propagation

```flux
module Flow.Async {
    // Run two operations concurrently, return both results.
    public fn interleaved<a, b, e>(
        f: () -> a with <Async | e>,
        g: () -> b with <Async | e>,
    ) -> (a, b) with <Async | e>

    // Race two operations; first to complete wins; loser is cancelled.
    public fn firstof<a, e>(
        f: () -> a with <Async | e>,
        g: () -> a with <Async | e>,
    ) -> a with <Async | e>

    // Bound an operation by time. Returns Some(v) on completion,
    // None if the timeout expires.
    public fn timeout<a, e>(
        ms: Int,
        f: () -> a with <Async | e>,
    ) -> Option<a> with <Async | e>

    // Establish a cancellation boundary. In-flight async operations
    // within the closure are cancelled if `cancel` is called from any
    // nested fiber.
    public fn cancelable<a, e>(
        f: () -> a with <Async | e>,
    ) -> a with <Async | e>
}
```

Example — fetching from two services in parallel with a 5-second budget:

```flux
fn user_url(uid: Int) -> String {
    String.concat("https://api/users/", Int.to_string(uid))
}

fn posts_url(uid: Int) -> String {
    String.concat(user_url(uid), "/posts")
}

fn fetch_user_dashboard(uid: Int) -> Option<Dashboard> with Async {
    Async.timeout(5000, fn() {
        let pair = Async.interleaved(
            fn() { Http.get_json(user_url(uid)) },
            fn() { Http.get_json(posts_url(uid)) }
        )
        match pair {
            (user, posts) -> Dashboard.build(user, posts),
        }
    })
}
```

Tuple destructuring goes through `match` — Flux does not have
let-bind tuple patterns. The two URL helpers exist because Flux does
not have multi-argument `String.concat` or interpolation in plain
strings (interpolation tokens exist in the lexer but are out of scope
for this design); naming the small helpers is the idiomatic
workaround.

Cancellation semantics, made explicit:

- If `interleaved`'s `f` raises (via `AsyncFail`), `g` is cancelled (its in-flight libuv operations get `uv_cancel`); both fibers' `Async.protect` cleanups run.
- If `firstof`'s `f` completes first, `g` is cancelled. Note this differs from Lean 4's `race`, which does not cancel the loser (Lean 4 `Std/Async/Basic.lean:524-528`).
- If `timeout`'s budget expires, the wrapped closure is cancelled and `None` is returned. Cancellation in this case raises `AsyncError.Canceled` inside the closure; cleanup blocks run; control returns to `timeout`, which converts the cancel into `None`.
- A `cancelable` block can be cancelled by any descendant fiber calling `Async.cancel()`. The cancel is delivered as `AsyncError.Canceled` to all suspended fibers within the scope.

#### The setup-closure pattern (for library authors)

Library authors who add new I/O operations write a thin Flux wrapper
that constructs a setup closure and performs `Suspend`. End users
never write this code, but it is the contract that defines what
"a libuv-backed operation" looks like in Flux.

Two opaque handle types and two callback-shape aliases (using the
type-alias feature added in this proposal):

```flux
module Flow.Async {
    // Returned to the runtime when an async operation is registered.
    public data CancelHandle { CancelHandle(Int) }

    // Opaque to user code — the runtime uses it to identify the
    // suspended fiber for completion delivery.
    public data FiberId { FiberId(Int) }

    // Callback shapes for the setup-closure pattern.
    public type ResumeFn<a> = (Result<a, AsyncError>) -> Unit
    public type SetupFn<a>  = (FiberId, ResumeFn<a>) -> CancelHandle
}
```

A library wrapper looks like this:

```flux
module Flow.Async.Internal {
    // Internal library code, not user-facing.
    public fn await_one_shot<a>(setup: SetupFn<a>) -> a with Async {
        perform Suspend(fn(fid, resume) {
            let handle = setup(fid, resume)
            let ctx = perform GetContext
            CancelScope.register(ctx, handle)
        })
    }
}

module Flow.Tcp {
    // Concrete TCP read built using await_one_shot.
    public fn read(conn: Connection, max: Int) -> Bytes with Async {
        Flow.Async.Internal.await_one_shot(fn(fid, resume) {
            // FFI primop: register libuv read, return cancel handle.
            Tcp.Internal.uv_read_start(fid, conn, max, resume)
        })
    }
}
```

The setup closure receives the fiber's ID (so the C glue knows whom
to wake) and a resumption callback (so completion can deliver the
result), and synchronously returns a handle the runtime uses to
cancel the operation. This is exactly the Eio `Suspend` shape
(`lib_eio/core/suspend.ml`) adapted for Flux. `Suspend`, `Fork`, and
`GetContext` are compiler-seeded labels — user code does not declare
them.

#### `Sendable` in user code

`Sendable` is a Phase 1a marker class enforced by Flux's existing
type-class infrastructure. It shows up in two places in user code:

```flux
module Flow.Task {
    public data Task<a> { Task(Int) }

    // Spawn a CPU-bound task on a worker thread. The closure and its
    // captures must be Sendable, and the result must be Sendable.
    public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
    public fn await<a: Sendable>(t: Task<a>) -> a with Async
}

module Flow.Channel {
    public data Channel<a> { Channel(Int) }

    public fn send<a: Sendable>(ch: Channel<a>, msg: a) -> Unit with Async
    public fn recv<a: Sendable>(ch: Channel<a>) -> a with Async
}
```

`Sendable` is auto-derived for primitive types (`Int`, `Float`,
`Bool`, `String`, `Bytes`) and for `data` declarations whose every
field type is `Sendable`. Types backed by non-atomic interior
mutation, thread-local resources, or raw OS handles (`Connection`,
`Listener`) declare an explicit negative instance:

```flux
instance !Sendable for Tcp.Connection
instance !Sendable for Tcp.Listener
```

The compile-time check happens during dictionary elaboration
([src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs)).
Closures are `Sendable` iff every captured value is `Sendable`; the
free-variable list collected by `ast/free_vars.rs` drives the check.

#### Worked example: HTTP-style JSON microservice

Putting the pieces together — the motivating microservice from the
Motivation section, expressed in real Flux syntax. Some helpers
(JSON codecs, `Postgres.Pool`, `Http.method`/`Http.path`/`Http.body`)
are Phase 2/3 features; the example shows how Phase 1b's primitives
combine with them. Named-field records use Flux's `data Foo { Foo {
name: T, ... } }` form (proposal 0152), with field access via dot
and functional update via spread:

```flux
module App {
    import Flow.Http
    import Flow.Json
    import Flow.Postgres
    import Flow.Async
    import Flow.String

    public data CreateUser { CreateUser { name: String, email: String } }
        deriving (Json.Encode, Json.Decode)

    public data UserId { UserId { id: Int } }
        deriving (Json.Encode, Json.Decode)

    fn handle_create_user<e>(
        pool: Postgres.Pool,
        body_bytes: Bytes
    ) -> Http.Response with <Async | e> {
        let body: CreateUser = Json.decode(body_bytes)
        let new_id = Postgres.with_connection(pool, fn(conn) {
            Postgres.query_one_int(
                conn,
                "INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id",
                [Postgres.text(body.name), Postgres.text(body.email)]
            )
        })
        Http.json_response(200, Json.encode(UserId { id: new_id }))
    }

    fn handler<e>(pool: Postgres.Pool, req: Http.Request) -> Http.Response
        with <Async | Console | e>
    {
        let _ = perform println(String.concat("request: ", req.path))
        match req.method {
            Post -> match req.path {
                "/users" -> handle_create_user(pool, req.body),
                _        -> Http.not_found(),
            },
            Get -> match req.path {
                "/health" -> Http.text_response(200, "ok"),
                _         -> Http.not_found(),
            },
            _ -> Http.method_not_allowed(),
        }
    }

    public fn main() with <Async | Console> {
        let pool = Postgres.pool(Postgres.Config {
            host: "localhost",
            port: 5432,
            max_conns: 32,
        })
        Http.serve("0.0.0.0", 8080, fn(req) {
            handler(pool, req)
        })
    }
}
```

A few notes on what this example uses:

1. **Named-field records on `data` declarations.** The form
   `data UserId { UserId { id: Int } }` (proposal 0152) gives both
   the type name and a single-constructor record with named fields.
   Field access uses dot syntax (`req.path`, `body.name`) and record
   construction uses brace literals (`UserId { id: new_id }`).
2. **`deriving` for codec derivation.** Phase 2 attaches `deriving
   (Json.Encode, Json.Decode)` to the `data` declaration, matching
   Flux's existing `deriving` keyword.
3. **Effect rows.** `handler` has `<Async | Console | e>` — it
   suspends on I/O, may emit log records via `Console`, and is
   polymorphic in `e` so callers can layer further effects on top.
   `serve` and `with_connection` similarly carry row variables and
   never constrain the caller's effect set beyond requiring `Async`.

`Postgres.Pool` is a refcounted handle with internal mutable state
(idle connection list, in-flight count). The `Pool` declares an
explicit `Sendable` instance because its internal mutability uses
atomic operations on the hybrid-RC fast path. This makes it safe to
share the same `pool` value across worker threads — for example, when
an HTTP server's accept loop dispatches connections to different
workers.

#### Wishlist: ergonomic gaps

See [Required language features](#required-language-features) at the
top of the Detailed design section. Plain type aliases are the only
strict prerequisite (included in this proposal); the remaining items
(string interpolation, negative type-class instances, tuple
let-binds, `try`/`finally` sugar, `loop`/`while`, named arguments)
are documented there as ergonomic gaps to re-evaluate after Phase
1b lands and real user code is written against the API.

#### What this design does not do

To be clear about scope:

- **No `async fn` syntax sugar.** `with Async` in the effect row is the marker; no special `async`/`await` keywords. Calling an `Async` function from another `Async` function is just function call.
- **No `Future<a>`/`Promise<a>` type.** Concurrency is via fork/join scopes, not handles.
- **No user-visible `spawn`** (other than `Task.spawn` for CPU-bound work). Long-lived background work is `fork_daemon` inside a `cancelable` scope.
- **No automatic retry, backoff, or circuit-breaking.** These are userspace libraries built on `firstof`/`timeout`/`cancelable`, not language features.
- **No streaming yet at this layer.** `Stream<a>` arrives in Phase 2; Phase 1b is one-shot operations only.

### Phase 2: HTTP/1.1 + JSON + Streams

#### HTTP

Wrap [llhttp](https://github.com/nodejs/llhttp) (Node.js's parser, ~3k lines
C, MIT-licensed). Vendor as `vendor/llhttp/`. Surface in
`runtime/c/http.c`, ~200 lines C glue.

```flux
module Flow.Http {
    type Method = Get | Post | Put | Delete | Patch | Head | Options

    public data Request {
        Request {
            method:  Method,
            path:    String,
            headers: Map<String, String>,
            body:    Bytes,
        }
    }

    public data Response {
        Response {
            status:  Int,
            headers: Map<String, String>,
            body:    Bytes,
        }
    }

    public fn serve<e>(
        addr: String,
        port: Int,
        handler: (Request) -> Response with <Async | e>
    ) -> Unit with <Async | e>

    public fn get(url: String)              -> Response with Async
    public fn post(url: String, body: Bytes) -> Response with Async
    public fn request(
        method:  Method,
        url:     String,
        headers: Map<String, String>,
        body:    Bytes
    ) -> Response with Async
}
```

Keep-alive and chunked transfer supported. HTTP/2 deferred to a future
proposal (significant complexity for marginal Phase-2 gain).

#### JSON

Two parts:

- `Flow.Json.parse: String -> Json` — tagged union value (`type Json = JsonNull | JsonBool(Bool) | JsonNumber(Float) | JsonString(String) | JsonArray(Array<Json>) | JsonObject(Map<String, Json>)`).
- `deriving (Json.Encode, Json.Decode)` clause attached to `data` declarations — type-class instances generated at compile time. Uses the existing dictionary-passing infrastructure from [proposal 0145](0145_type_classes.md).

Codec generation is added to the dict-elaboration pass
([src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs)).
Per-record codecs are zero-allocation when the type permits (ADTs with all
flat fields).

#### Streams

```flux
module Flow.Stream {
    // A stream is a pull-based iterator that may suspend on Async I/O.
    // Defined as a transparent type alias (no runtime wrapper).
    public type Stream<a> = () -> Option<a> with Async

    public fn map<a, b>(s: Stream<a>, f: (a) -> b) -> Stream<b>
    public fn filter<a>(s: Stream<a>, p: (a) -> Bool) -> Stream<a>
    public fn fold<a, b>(s: Stream<a>, init: b, f: (b, a) -> b) -> b with Async
    public fn take<a>(s: Stream<a>, n: Int) -> Stream<a>
    public fn chunk<a>(s: Stream<a>, size: Int) -> Stream<List<a>>
    public fn merge<a>(s1: Stream<a>, s2: Stream<a>) -> Stream<a>
}
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

### Phase 3: TLS + database client

#### TLS

Link rustls via its C ABI (`rustls-ffi`). Vendor as
`vendor/rustls-ffi/`. Glue in `runtime/c/tls.c`, ~200 lines.

```flux
module Flow.Tls {
    public data TlsConnection { TlsConnection(Int) }
    public data Cert          { Cert(Bytes) }
    public data Key           { Key(Bytes) }

    public fn handshake_client(conn: Connection, hostname: String) -> TlsConnection with Async
    public fn handshake_server(conn: Connection, cert: Cert, key: Key) -> TlsConnection with Async
    public fn read(c: TlsConnection, max: Int)        -> Bytes with Async
    public fn write(c: TlsConnection, data: Bytes)    -> Int   with Async
    public fn close(c: TlsConnection)                 -> Unit  with Async
}
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
module Flow.Postgres {
    public data Pool       { Pool(Int) }            // opaque pool handle
    public data Connection { Connection(Int) }      // pooled wire connection
    public data Row        { Row(Array<Param>) }    // a result row
    public data Param      { ParamText(String) | ParamInt(Int) | ParamBytes(Bytes) | ParamNull }

    public data Config {
        Config { host: String, port: Int, max_conns: Int }
    }

    public fn pool(config: Config)                          -> Pool with Async
    public fn acquire(pool: Pool)                           -> Connection with Async
    public fn release(pool: Pool, conn: Connection)         -> Unit with Async
    public fn query(conn: Connection, sql: String, params: Array<Param>)   -> Array<Row> with Async
    public fn execute(conn: Connection, sql: String, params: Array<Param>) -> Int with Async

    public fn with_connection<a, e>(
        pool: Pool,
        action: (Connection) -> a with <Async | e>
    ) -> a with <Async | e>

    public fn transaction<a, e>(
        conn: Connection,
        action: () -> a with <Async | e>
    ) -> a with <Async | e>
}
```

The `Pool` is internally mutable — but only behind the `Async` effect (it's
parameterized handler state). User code remains pure.

Wire-protocol parser in pure Flux, ~800 lines. Connection pool and
transaction logic ~300 lines.

#### Phase 3 deliverables

- `lib/Flow/Tls.flx`, `lib/Flow/Postgres.flx` — ~1100 lines Flux.
- `runtime/c/tls.c` — rustls-ffi glue, ~200 lines C.
- Examples: HTTPS server, database-backed CRUD microservice (the
  motivating example from Summary).
- Integration tests against a real Postgres instance.

Estimated effort: 4 weeks.

### Phase 4 (optional): io_uring backend for Linux

**Not committed.** Ships only if libuv's epoll-based Linux backend
becomes a measured throughput bottleneck. The point of mentioning Phase 4
in this proposal is to document **what the seam protects**, not to commit
to building it.

Eio demonstrates the dual-backend pattern (`lib_eio_linux/` for
io_uring, `lib_eio_posix/` for epoll/kqueue).
The substitution sits below the three-effect seam, so user code and the
structured concurrency primitives remain unchanged. The Rust scheduler
gets a configuration knob (libuv vs io_uring); the C glue grows a second
implementation file.

Estimated effort: 4-6 weeks if/when triggered. Skipped for the foreseeable
future; libuv on epoll is more than adequate for the proposal's stated
workload.

## Drawbacks

- **libuv adds a build dependency.** Vendored statically, ~200 KB binary cost. Acceptable.
- **Phase 1a + 1b is roughly 2-3 months of work.** Less than the original 0174's five phases, but front-loads the multi-threading work (which the original 0174 deferred to Phase 5).
- **Continuation capture across libuv callbacks is the load-bearing technical risk.** Phase 1a sidesteps this; Phase 1b tackles it directly. Mitigation: prototype the simplest possible `Suspend` → libuv timer → resume cycle before committing the full Phase 1b scope. Eio proves the approach works on a comparable substrate.
- **No `Fiber<a>` handle is a departure from Promise/Tokio idioms.** Users coming from JavaScript or Rust may expect spawn-and-await. The Eio precedent argues structured scopes are the right primary API.
- **No work-stealing between worker threads.** A fiber spawned on worker N stays on worker N. This is Eio's model. Load imbalances (one worker busy, others idle) are possible but uncommon for HTTP workloads where fibers tend to be short-lived.
- **No preemption.** A fiber that does not `await` blocks every other fiber on its worker. Same limitation as Node and Eio. Mitigation: `Async.yield()` for long pure loops, or `Task.spawn` to hand work to a different worker.
- **Windows IOCP cancellation has weaker guarantees than epoll/kqueue.** A cancelled IOCP operation may still complete with the original result if the kernel has already processed it. Documented limitation; the cancellation contract is best-effort on Windows.
- **HTTP/2 and gRPC are deferred.** Phase 2-3 ship HTTP/1.1 only. Adequate for most microservices.
- **TLS via rustls-ffi adds a Rust toolchain dependency at runtime build time.** Vendored static lib avoids a runtime dep, but compiling Flux from source requires Cargo. Acceptable parallel to existing Rust compiler dependency.

## Rationale and alternatives

### Why Async-via-effects vs. Promise/Future types

Flux already has algebraic effect handlers with continuation capture. An
async effect reuses 100% of that machinery. A `Promise<a>` ADT layer would
duplicate it, require new compiler support for `await`-as-syntax, and lose
the composability of effect handlers (`run_async` as a userspace handler is
not possible with built-in promises).

OCaml/Eio is the closest direct precedent: a language with algebraic
effect handlers in the runtime that exposes structured concurrency via
three small effects. Lean 4 deliberately chose monads + typeclasses
instead, with a thread-pool runtime — a viable but less expressive
alternative that we explicitly chose against in Phase 1b. Haskell's
`IO`/`async`/`STM` is the closest counter-example, but Haskell doesn't
have algebraic effect handlers — it has monads + GHC RTS. Different
substrate, different optimal answer.

### Why scheduler-in-Rust vs. scheduler-in-Flux

The original 0174 proposed writing the scheduler as a Flux handler in
`lib/Flow/Async.flx`. Two pieces of evidence argued against this:

1. **Eio's actual scheduler is in OCaml + C stubs**, not in user code. Each backend has ~400-560 lines OCaml (`lib_eio_linux/sched.ml`, `lib_eio_posix/sched.ml`) plus C stubs.
2. **Lean 4's runtime is ~3,800 lines C++** for the libuv glue + task manager, with ~3,000 lines Lean source on top.

The original 0174's "~300 lines Flux" estimate for the scheduler was
unrealistic by ~5-8x. Moving the scheduler to Rust, where the rest of
the runtime ([src/runtime/](../../src/runtime/)) lives, gives the same
implementation for both backends (VM and LLVM call the same Rust
functions via primops), and concentrates RC + thread-pool + libuv
discipline in a single layer where Rust's ownership system catches
mistakes.

The Flux source layer keeps what genuinely benefits from being expressed
as effect handlers: the structured concurrency primitives. Those are
~250 lines of Flux that meaningfully exercise the effect system.

### Why Koka's API shape vs. JavaScript-promise shape

Spawn-and-join with a `Fiber<a>` handle is the JavaScript/Tokio idiom.
It encourages unstructured concurrency (spawned fibers leaking past
their intended scope, no cancellation propagation, "fire-and-forget"
mistakes).

Structured concurrency (`interleaved`, `firstof`, `timeout`,
`cancelable`) makes the lifetime relationship between concurrent
operations syntactically obvious. Cancellation is automatic — leaving
a scope cancels in-flight work. This matches Eio and Trio (Python) and
is the direction modern async design has converged on.

### Why libuv vs. alternatives

Investigated: libev, libevent, io_uring, mio, Tokio, Boost.Asio,
hand-rolled epoll/kqueue. The Rust crates (mio, Tokio) are eliminated
by the C-runtime constraint. libev is Unix-only. libevent is superseded.
io_uring is Linux-only and premature for an unbenchmarked language.
Hand-rolling is multi-month false economy. libuv is the only candidate
that combines maturity, cross-platform (Windows + Linux + macOS), and
C ABI; Lean 4, Julia, and Node validate the choice for Flux's category
of language.

Eio shows that going direct-to-OS (epoll/kqueue/IOCP/io_uring without
libuv) is feasible but expensive: three independent backend
implementations totaling ~5,200 lines OCaml + C stubs. For a project at
Flux's stage, libuv's ~600-line single-backend cost is a much better
trade. Phase 4 documents the io_uring escape hatch if/when measurements
justify the additional implementation.

### Why hybrid atomic-on-share RC vs. atomic-everywhere

The original 0174's Phase 5 plan was "atomic refcounts everywhere,
mirroring Koka." This was a misread of both Koka and Lean 4:

- **Koka** uses sign-bit-encoded hybrid RC: positive non-atomic, negative atomic. See `kklib/include/kklib.h:101-135` and `kklib/src/refcount.c:150-200` in the Koka source tree.
- **Lean 4** uses the same scheme: see `src/include/lean/lean.h:131-136, 544-568` in the Lean source tree.

Both production languages with Perceus RC and multi-threading use
hybrid. There is no production precedent for "atomic everywhere." Hybrid
costs ~30 lines of additional code in `flux_dup`/`flux_drop` and pays
for itself: single-threaded paths (the common case) remain unchanged
non-atomic operations.

### Why multi-threading in Phase 1a vs. Phase 5

The original 0174 deferred all threading work to Phase 5
(conditional). Two pieces of evidence argued against this:

1. **Node's deficiency under HTTP-microservice load** (the proposal's stated target) is widely documented. Process-per-core (the original Phase 3) papers over this for stateless services but breaks down for shared-state workloads (in-process cache, connection pool, rate limiter — exactly the cases where Node loses to Go in practice).
2. **Hybrid RC is cheap** (~30 lines), and **mutex-protected libuv loop is cheap** (~5 lines). Doing them in Phase 1a costs almost nothing and avoids a Phase 5 retrofit. Lean 4 ships this from day one for the same reason.

The Phase 1a + 1b structure is therefore strictly more capable than
the original Phase 1, with no meaningful additional work. The original
Phase 3 (process-per-core) is removed because Phase 1a already handles
multi-core; the original Phase 5 is removed because Phase 1a already
ships hybrid RC.

### Alternatives considered and rejected

- **Lean-style (thread pool, no fiber layer) only.** Simpler to ship but caps concurrency at ~thousands per process — insufficient for HTTP microservices (c10k pattern). Rejected; Phase 1b adds the fiber layer specifically to clear this ceiling.
- **Eio-style three native backends from day one** (io_uring + epoll/kqueue + IOCP, no libuv). 5x the backend LOC. Multi-year overhead. Rejected; Phase 4 keeps the door open if measurements justify it.
- **Goroutine-style M:N work-stealing scheduler.** Took Go a decade to mature. Out of scope. Rejected; per-worker scheduling without migration is sufficient for the stated workload, with Eio as the precedent.
- **Adopt Tokio, make the runtime Rust-only.** Requires rewriting the entire C runtime ([runtime/c/](../../runtime/c/)) in Rust. Tokio cannot link cleanly into the LLVM backend's C runtime. Rejected.
- **Per-thread heaps with linear-type send (Erlang-style).** Beautiful but requires major type-system work (uniqueness types, send-primitives). Multi-year scope. Rejected; `Sendable<T>` (Rust-style trait) gives most of the safety with much less type-system work.

## Prior art

- **Lean 4** — the closest existing substrate to Flux: Perceus RC, native compilation via LLVM, libuv-backed async I/O. The Phase 1a substrate is modelled directly on Lean 4's runtime. Relevant files in the Lean source tree: `src/runtime/uv/` (~3,800 lines C++ libuv glue), `src/runtime/object.cpp:706-916` (`task_manager`), `src/include/lean/lean.h:131-136` (hybrid RC header), `src/Std/Async/` (~3,000 lines Lean async stdlib). Where the proposal diverges from Lean: Phase 1b adds a fiber layer that Lean does not have (Lean tasks block their worker threads on `await`); cancellation propagates through `firstof` (Lean's `race` does not).
- **OCaml/Eio** — the closest existing API surface to what Phase 1b ships. The three-effect seam (Eio `lib_eio/core/eio__core.ml:15-21`: `Suspend`, `Fork`, `Get_context`) is copied directly. The structured concurrency primitives (`Switch`, `Fiber.both`, `Fiber.first`) inform `interleaved`/`firstof`. Per-domain non-migrating fiber model is adopted. Eio's pluggable-backend architecture (`lib_eio_linux/`, `lib_eio_posix/`, `lib_eio_windows/`) is the model for Phase 4's optional io_uring escape hatch.
- **Koka** — original source of the `await(setup)` API pattern (Koka `lib/v1/std/async.kk:521`). Note: Koka's libuv glue is via Node.js's existing event loop, not direct C; Lean 4 is the relevant precedent for direct libuv binding.
- **Rust** — Tokio and the Rust async ecosystem are not linkable from Flux's LLVM-backend C runtime, but Rust's `Send`/`Sync` trait discipline is the cleanest production formulation of compile-time thread-safety. Phase 1a's `Sendable<T>` is the analogous constraint, more expressive than OCaml/Eio's by-convention warning.
- **GHC** — the RTS-in-C / IO-manager-in-Haskell split (GHC `rts/Schedule.c`, 3,353 lines; `libraries/ghc-internal/src/GHC/Internal/Event/Manager.hs`, 544 lines) is the precedent for "scheduler in runtime, structured concurrency in source language" that the revised Phase 1b adopts. GHC has never used libuv (epoll/kqueue/IOCP via its own backends) — the scheduler split, not the I/O substrate, is the relevant lesson.
- **Trio (Python)** — popularised "structured concurrency" terminology. API shape (nurseries, scoped cancel) directly influences `interleaved`/`cancelable`.
- **Node.js** — defined libuv. Single-threaded async-via-callbacks. Demonstrates the scale of one event loop on real microservice workloads, and the limitations (cluster-instead-of-threads, slow-handler-stalls-loop) that Phase 1b's multi-worker fiber model is designed to avoid.
- **Erlang/BEAM** — per-process heaps + reduction-counted preemption. Considered as Phase 5 alternative; rejected (requires linear types and per-thread heaps).
- **Haskell `async` library** — `Async a` handles + `wait`/`cancel`. The unstructured-concurrency precedent we are intentionally diverging from.
- **Flux proposal [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md)** — earlier exploration of actor-style concurrency for Flux. Deferred; actor patterns can be built as a userspace library on top of Phase 1b's `Async` effect plus `Sendable<T>` channels.

## Unresolved questions

1. **Continuation re-entry from C.** When libuv fires a callback, the C glue calls a Rust upcall (`flux_runtime_complete(id, result)`) which must locate the suspended fiber and enqueue it. The mechanism is a Rust-side wait registry keyed by libuv operation ID. Detailed design deferred to Phase 1b implementation; prototype validates the cycle before committing.
2. **`Bytes` zero-copy vs. copy on TCP read.** Phase 1b ships copy-on-delivery for simplicity. Phase 2 may move to zero-copy (libuv alloc callback returns a Flux-allocated buffer). Decision deferred to benchmarking.
3. **Pool internal mutation.** Phase 3's `Postgres.Pool` has internal mutable state (idle connections, in-flight count). Modeled as parameterized handler state. Concrete representation TBD.
4. **JSON codec error reporting.** `Json.decode` failure on malformed input — returns `Result<T, JsonError>` with field-path information. Schema TBD.
5. **HTTP/1.1 keep-alive eviction policy.** Connection pool sizing and timeout defaults TBD.
6. **TLS certificate management.** Loading, rotation, and revocation policies TBD; rustls-ffi provides primitives, Flux-side ergonomics deferred to Phase 3 design.
7. **`Sendable<T>` derivation rules for closures.** A closure capturing only `Sendable` values is `Sendable`; a closure capturing a non-`Sendable` value is not. Compile-time check needed in `dict_elaborate.rs`. Detailed inference rules TBD.
8. **libuv mutex contention under high fiber count.** Phase 1b runs all fibers' libuv operations through a single mutex-protected loop. At very high concurrency the mutex may itself become the bottleneck. Mitigations (per-worker loops, lock-free queue for completions) deferred until measured.

## Revision history

- **Revision 1 (original)** — five-phase plan: single-threaded Async + TCP, HTTP/JSON/Streams, process-per-core, TLS+Postgres, conditional shared-state multi-threading via atomic-everywhere RC. Cited Koka as the precedent for "scheduler in source language, libuv substrate, atomic RC." See git history for original text.
- **Revision 2** — restructured into Phase 1a (multi-threaded substrate, modelled on Lean 4) + Phase 1b (fiber layer + structured concurrency, modelled on Eio), with Phases 2-3 unchanged in shape and an optional Phase 4 (io_uring backend) replacing the original Phase 5. Multi-threading lands in Phase 1a (was Phase 5). Process-per-core (was Phase 3) is removed; Phase 1a's worker pool subsumes it. Hybrid atomic-on-share RC (Lean's and Koka's actual scheme) replaces the original "atomic everywhere" Phase 5 plan. Scheduler moves from Flux source to Rust. Three-effect seam (Suspend/Fork/GetContext) replaces the single `Async` effect for backend extensibility. `Sendable<T>` constraint added (modelled on Rust's `Send`).
- **Revision 3 (this version)** — strict syntax pass against the actual Flux grammar (`src/syntax/token_type.rs` keyword set, `src/syntax/parser/`). All code samples rewritten to use only supported constructs: named-field records via `data Foo { Foo { ... } }` (proposal 0152), `deriving` clauses on `data` declarations, positional function arguments, recursion in place of `loop`/`while`, library functions in place of `try`/`finally`/`catch`, `match` for tuple destructuring, and `<a: Class>` constraints inline in type-parameter lists. Plain type aliases are folded into this proposal as a "Required language features" section because Phase 1b's setup-closure pattern is awkward without them; ADT-sugar `type` is extended to accept any type expression on the right-hand side, with restrictions described in detail.

## Future possibilities

- **HTTP/2 multiplexing** — once HTTP/1.1 is stable. Significant complexity; likely a separate proposal.
- **WebSocket and Server-Sent Events** — both fall out of HTTP/1.1 + streams in Phase 2 with small additional work.
- **gRPC** — HTTP/2 + protobuf. Future proposal.
- **io_uring backend for Linux** — Phase 4 (optional). Eio demonstrates the dual-backend pattern.
- **Per-worker libuv loops** — replace the single mutex-protected loop with one loop per worker thread, eliminating the loop mutex as a contention point at very high concurrency. Adds complexity to `uv_async_t`-based cross-loop wakeup. Deferred until measured.
- **Process-per-core** — was the original Phase 3. Removed because Phase 1a's worker pool already provides multi-core scaling. Can be reintroduced as a userspace library on top of `Process.spawn` if specific deployments want process isolation.
- **Distributed actor model** — built on Phase 1b's `Async` effect + `Sendable<T>` channels. Userspace library; replaces what 0143 originally proposed as language-level actors.
- **Job queue / scheduled tasks** — userspace library on top of `sleep` + persistent storage.
- **File watchers** — `inotify`/`fsevents` via libuv's `uv_fs_event_t`.
- **GraphQL server** — HTTP + JSON + DataLoader-style fan-out via `interleaved`.

## Appendix: end-to-end POST request trace (Phase 1b + Phase 2)

A `POST` request from user code to the wire and back, illustrating how
the three-effect seam, libuv, and the existing continuation-capture
runtime compose.

User code:

```flux
let resp = Http.post("https://api.example.com/users", body)
```

`Http.post` is Flux code: format request bytes, `Tcp.connect`,
`Tls.handshake`, `Tcp.write`, repeated `Tcp.read` until response complete,
parse. Each I/O call ultimately performs `perform Suspend(setup_closure)`.

For one `Tcp.write`:

1. **`Tcp.write` calls `perform Suspend(setup)` where `setup` registers the libuv operation.**

   **VM:** `OpPerform` ([src/bytecode/op_code.rs:97-102](../../src/bytecode/op_code.rs)) walks the evidence vector, finds the `Suspend` handler installed by `run_async`. Captures the post-perform continuation via `Continuation::compose()` ([src/runtime/continuation.rs:49-93](../../src/runtime/continuation.rs)). Hands `(continuation, setup_closure, fiber_context)` to the handler arm.

   **LLVM:** equivalent — emits `flux_yield_to(htag, optag, arg, arity)` ([src/lir/emit_llvm.rs:3403-3511](../../src/lir/emit_llvm.rs)). `cont_split` ([src/lir/lower.rs:3594-3685](../../src/lir/lower.rs)) synthesised the continuation at compile time. Both backends share the C-runtime yield protocol.

2. **The `Suspend` handler arm (~5 lines Flux) calls a Rust primop `flux_scheduler_suspend(fiber_id, setup, continuation)`.** The Rust scheduler:
   - Stores `(fiber_id, continuation)` in the wait registry.
   - Calls `setup(fiber_id)`, which calls a libuv primop like `flux_uv_tcp_write(fiber_id, conn, data)`.
   - The C glue `flux_dup`s `data`, calls `uv_write` with a callback that closes over `fiber_id`, returns.
   - The current worker thread now has a free slot — picks the next ready fiber from its local queue and resumes it.

3. **libuv detects socket-writable; the kernel reports readiness.** libuv invokes our `uv_write_cb` on whichever worker currently holds the loop mutex. The C glue:
   - `flux_drop`s the duped `data`.
   - Calls `flux_runtime_complete(fiber_id, n_bytes_written)`.

4. **`flux_runtime_complete` (Rust)** looks up `fiber_id` in the wait registry, retrieves the continuation, and enqueues `(continuation, n_bytes_written)` into the **fiber's home worker's** ready queue (not necessarily the worker that ran the libuv callback). Cross-worker enqueue uses an atomic on the target queue plus a `uv_async_t` wakeup if the target worker is parked.

5. **Eventually the home worker pulls the resumed fiber from its ready queue.** VM: `execute_resume` restores frames, pushes `n_bytes_written` where `perform` would have returned. LLVM: jumps to the post-perform block with the value as block parameter. `Tcp.write` returns. `Http.post` continues to the next operation.

6. **Many awaits later, the response is fully read and parsed.** `Http.post` returns to user code. `let resp = ...` gets the response.

Throughout: the fiber's heap stays on its home worker thread, so refcounts
on its working set remain non-atomic (positive `m_rc`). The `data` value
crossed the FFI boundary and was duped; the `fiber_id` is an opaque
handle that does not interact with RC. Cancellation (e.g., from a
surrounding `timeout`) sets the scope's `canceled` flag, calls
`uv_cancel(req)` on registered libuv operations, and prevents the
continuation from being resumed normally; instead the resume path
raises `AsyncError.Canceled` and unwinds into the `cancelable` scope.
On Windows IOCP, `uv_cancel` is best-effort (the operation may still
complete with the original result if the kernel has already processed
it); on Linux/macOS the cancellation guarantee is firm.

This is the entire concurrency model for Phases 1a, 1b, 2, and 3.
Phase 4's optional io_uring backend slots in below the C glue layer
without changing anything above it.
