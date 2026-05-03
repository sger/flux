- Feature Name: Async Effect & Concurrency Roadmap
- Start Date: 2026-04-27
- Status: Draft (revision 5 — supersedes the original five-phase plan; see "Revision history" at end)
- Proposal PR:
- Flux Issue:
- Depends on: existing effect handlers ([runtime/c/effects.c](../../runtime/c/effects.c), [src/runtime/continuation.rs](../../src/runtime/continuation.rs)), existing FFI primop machinery
- Includes: language feature work on transparent `alias` declarations (see "Required language features" below)
- Relates to: [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) — see "Relationship to 0143" below

# Proposal 0174: Async Effect & Concurrency Roadmap

## Summary

Introduce concurrency to Flux as a layered runtime whose task manager and
cross-thread ownership discipline are inspired by Lean 4, whose I/O substrate
is a Rust `mio` reactor owned by Flux, and whose user-facing API is modelled
on OCaml/Eio (`lib_eio/core/`, three-effect seam).
The driving use case is **HTTP microservices and data streams**; the
technical foundation is a multi-threaded Rust runtime carrying a fiber
layer that uses Flux's existing continuation-capture machinery to provide
M:N cooperative concurrency with structured-concurrency primitives.

The roadmap has a mandatory runtime-preparation phase, then one feature phase
split into two milestones, plus follow-on phases:

- **Phase 0 — Concurrency-ready effect runtime.** Move native yield/evidence
  state out of process-global storage, define scheduler-owned effect contexts,
  and prove that VM and LLVM/native can host multiple suspended effects without
  state collision. No user-facing async API yet.
- **Phase 1a — Multi-threaded runtime substrate.** Worker thread pool, Rust `mio` reactor thread, timer heap, blocking DNS/file pools, hybrid atomic-on-share RC, `Task<a>` primitive. Multi-core from day one; task manager and RC discipline follow Lean 4's shape while the I/O backend is Flux-owned Rust.
- **Phase 1b — Fiber layer + structured concurrency.** Three-effect seam (`Suspend`/`Fork`/`GetContext`) on the Phase 1a substrate. Lightweight fibers via existing continuation capture. `both`/`race`/`timeout`/`scope` as Flux source. M:N concurrency density: thousands of fibers per worker thread.
- **Phase 2 — HTTP/1.1 + JSON + Streams.** Unchanged from the original proposal.
- **Phase 3 — TLS + database client.** Was Phase 4 in the original.
- **(Optional) Phase 4 — io_uring backend for Linux.** Backend swap behind the same `AsyncBackend` seam if perf measurements justify it.

The original Phase 3 (process-per-core) is removed: multi-threading
lands in Phase 1a, so process-per-core is no longer a stepping stone.
The original Phase 5 (shared-state multi-threading via atomic RC) is
removed: hybrid RC ships in Phase 1a, following Lean's and Koka's
actual production scheme rather than the misread "atomic everywhere"
target the original proposal aimed at.

## Progress

| Phase / slice | Status | What landed |
|---|---|---|
| **Phase 0** — Concurrency-ready effect runtime | ✅ done | All four mandated invariants pass: VM and native runtime each host multiple suspended effects with independent state; `Suspend → completion → resume` round-trips deterministically; cancellation before completion delivers a synthesised cancelled error; abandoned continuations clean up without leaks. |
| 0a — Audit | ✅ | Catalogued the 13 process-globals in [`runtime/c/effects.c`](../../runtime/c/effects.c) and confirmed VM yield/evidence state is already per-instance. |
| 0b — `EffectContext` | ✅ | [`src/runtime/async/context.rs`](../../src/runtime/async/context.rs) — scheduler-owned effect/fiber context (yield state, evidence vector, continuation token, cancel scope, home worker). |
| 0c — VM migration | ✅ | [`Vm`](../../src/vm/mod.rs) routes yield/evidence state through `EffectContext` instead of separate fields. |
| 0d — Native C runtime migration | ✅ | [`runtime/c/effects.c`](../../runtime/c/effects.c): all 13 globals moved into a per-thread `FluxEffectContext` (`_Thread_local` / `__declspec(thread)`); vestigial extern declarations removed from [`flux_rt.h`](../../runtime/c/flux_rt.h). |
| 0e — `AsyncBackend` + registry + integration | ✅ | [`backend.rs`](../../src/runtime/async/backend.rs), [`request_registry.rs`](../../src/runtime/async/request_registry.rs), [`backends/in_memory.rs`](../../src/runtime/async/backends/in_memory.rs); the three proposal-mandated invariant tests in [`phase0_integration_tests.rs`](../../src/runtime/async/phase0_integration_tests.rs). |
| **Phase 1a** — Multi-threaded runtime substrate | 🚧 in progress | |
| 1a-i — `mio` dependency + reactor skeleton | ✅ | [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs): dedicated reactor thread owning `mio::Poll`; `start`/`shutdown` lifecycle with `Waker`-driven wake + `JoinHandle` cleanup; `Drop` joins to guard against leaked threads on Windows. No I/O sources registered yet. |
| 1a-ii — Timer service | ✅ | [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs): runtime-owned `BinaryHeap` of `(deadline, RequestId)`; `Poll::poll` uses next deadline as its timeout; expired entries produce `CompletionPayload::Unit` into a shared completions queue. `cancel(req)` suppresses the fire and drops any already-queued completion. `timer_start` and `next_completion` extend the [`AsyncBackend`](../../src/runtime/async/backend.rs) trait; the in-memory test backend implements them with deterministic semantics. |
| 1a-iii — Worker pool + `RuntimeTarget` | ✅ | [`task_manager.rs`](../../src/runtime/async/task_manager.rs): N-thread worker pool with a shared per-priority FIFO (`MAX_PRIO = 2`), `Condvar`-parked workers, `start`/`submit`/`shutdown` lifecycle, `Drop` joins on teardown to keep libtest from wedging on Windows. [`runtime_target.rs`](../../src/runtime/async/runtime_target.rs): `TaskId` + `RuntimeTarget` enum (Task variant; Fiber variant lands in 1b). End-to-end completion routing waits on the actual `Task<a>` user surface (1a-vi). |
| 1a-iv — Hybrid atomic-on-share RC | ✅ (C side) | [`runtime/c/rc.c`](../../runtime/c/rc.c): `FluxHeader.refcount` is now `_Atomic(int32_t)` with sign-bit encoding (`rc > 0` ST mode, relaxed; `rc < 0` MT mode, atomic; last MT drop is acq_rel). New API in [`flux_rt.h`](../../runtime/c/flux_rt.h): `flux_rc_promote` (recursive ST → MT promotion with release ordering, walks evidence vectors and standard scan offsets), `flux_rc_is_shared`. LLVM-emitted inline `rc == 1` reuse/uniqueness checks (in [`prelude.rs`](../../src/llvm/codegen/prelude.rs)) naturally fail for negative refcounts and fall back to `flux_drop` — no LLVM changes required. **Rust `Value` mirror deferred to 1a-vi**, where `Task.spawn` is the first cross-worker consumer; until then the VM stays single-threaded so `Rc<T>` semantics are still sound. Regression coverage: the full native-LLVM test suite (which exercises dup/drop heavily) passes unchanged, proving the ST hot path is encoding-equivalent. MT-path tests land alongside the first consumer in 1a-vi. |
| 1a-v — `Sendable<T>` type class | ✅ (primitives + structural) | Marker class registered in [`class_env.rs`](../../src/types/class_env.rs)'s `register_builtins`, no methods. Built-in primitive instances: `Int`, `Float`, `String`, `Bool`, `Unit`. Positive-only structural derivation in [`class_solver.rs`](../../src/types/class_solver.rs)'s `has_structural_builtin_instance`: tuples, `Option`, `List`, `Array`, `Map`, `Either` auto-derive `Sendable` when their element types satisfy it. Closures, opaque runtime handles, and ADTs without an explicit instance fail with the standard E440 "no instance" diagnostic — absence means "not sendable." Tests in [`sendable_tests.rs`](../../tests/type_inference/sendable_tests.rs) cover the positive primitive/tuple/collection cases plus a closure negative case. **ADT auto-derivation is not yet wired** — currently every user-defined ADT requires an explicit `instance Sendable<Foo> {}`. The recursive-on-fields synthesis is straightforward to add when the first ADT consumer arrives in 1a-vi/1a-vii. |
| 1a-vi — `Task<a>` + `Flow.Task` | ✅ (Rust scheduler) | [`task_scheduler.rs`](../../src/runtime/async/task_scheduler.rs): `TaskScheduler` wraps the 1a-iii [`TaskManager`](../../src/runtime/async/task_manager.rs) with per-task `Arc<TaskState>` (outcome `Mutex` + `Condvar` + cancel `AtomicBool`). `spawn(action: FnOnce() -> T + Send + 'static)` returns a `TaskHandle<T>`; `blocking_join` consumes it and surfaces `TaskJoinError::Cancelled`/`Panicked` for non-value outcomes (panics are caught by the worker so the pool isn't poisoned). Cancellation: pre-pickup short-circuits the body; post-completion is a no-op; mid-flight runs to completion (yield points come in 1b). 7 tests cover happy path, parallel execution across workers, panic isolation, cancel-before-pickup, cancel-after-completion, and Drop-joins-promptly. **`Flow.Task` Flux source + LLVM C-shim wiring + `flux_rc_promote` integration deferred to a follow-up slice** — that's where Flux closures actually cross worker boundaries and the MT-RC encoding from 1a-iv first runs end-to-end. |
| 1a-vii — TCP readiness state machines | ⏳ | `tcp_connect` / `tcp_read` / `tcp_write` over `mio` registration; backend-owned `Vec<u8>` buffers. |
| **Phase 1b** — Fiber layer + structured concurrency | ⏳ | Three-effect seam, fibers, `scope` / `both` / `race` / `timeout`. |
| **Phase 2** — HTTP/1.1 + JSON + Streams | ⏳ | |
| **Phase 3** — TLS + database client | ⏳ | |
| **Phase 4** — `io_uring` backend (optional) | ⏳ | |

Test count at end of slice 1a-vi (Rust scheduler): **2459 passed / 0 failed** under `cargo test --all --all-features`.

## Relationship to 0143

[Proposal 0143](0143_actor_concurrency_roadmap.md) specifies an
Erlang-style actor concurrency roadmap (isolated heaps, typed mailboxes,
supervision, deterministic test scheduler). 0143 and this proposal model
two complementary layers, not competing alternatives:

- **0174 owns the I/O layer.** `Async` effect, Rust reactor backend, structured
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
  built from 0174's `race`, scoped cancellation, and `Process.wait`.
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
**connecting a Flux-owned reactor and scheduler to the existing yield/resume
protocol**, not building new compiler infrastructure. OCaml/Eio and Eff
demonstrate the effect-handler shape; Lean 4 demonstrates that a compiled
RC language can pair a worker/task substrate with native async I/O. Flux
keeps the substrate in Rust so the scheduler can own request registries,
completion queues, cancellation state, and Aether/Perceus ownership
boundaries directly.

### Why mio

The I/O backend question was investigated against alternatives (libuv,
libevent, io_uring, Tokio, hand-rolled epoll/kqueue/IOCP). The decisive
constraint is not only native linking; it is **ownership control**. Flux's
hard problem is resuming the right continuation on the right worker without
letting an external callback runtime manipulate Aether-owned values. A
Rust `mio` reactor gives Flux a low-level readiness substrate while keeping
the scheduler and request lifecycle in Rust.

`mio` has the right shape for Flux:

- Cross-platform readiness over epoll/kqueue/IOCP without adopting Rust's
  `Future`/`Pin` model.
- Rust-owned request registries, completion queues, and cancellation state.
- A narrow exported C ABI can still serve the LLVM/native runtime path.
- Timers, DNS, file I/O, TLS, process handling, and signals remain Flux
  runtime services layered above the reactor rather than assumptions baked
  into an external callback library.
- A deterministic test backend can implement the same internal
  `AsyncBackend` interface without touching user code.

The tradeoff is explicit: `mio` is not batteries-included. Phase 1 therefore
ships timers and TCP on the reactor, plus small blocking service pools for
DNS and file I/O. TLS, processes, signals, and Linux-specific `io_uring`
remain later backend/service work.

## Detailed design

### Required language features

Phase 1b's library API depends on one language feature that Flux
does not currently support, plus a handful of ergonomic gaps that
are not strict prerequisites but would meaningfully improve the
user-facing syntax. This proposal includes the prerequisite
language work; the ergonomic items are documented here for future
re-evaluation.

#### Prerequisite: transparent aliases (included in this proposal)

Phase 1b's setup-closure pattern (the contract for backend-backed
async operations) is awkward without function-type aliases. Today,
Flux already has `alias Name = <Effect | Row>` for effect-row aliases,
but `alias` cannot abbreviate ordinary type expressions. This proposal
extends `alias` to cover transparent type aliases as well. Without that
extension, every TCP/UDP/DNS/timer/signal/fs wrapper must inline the full
closure shape at every call site:

```flux
public fn await_one_shot<a>(
    setup: (FiberId, (Result<a, AsyncError>) -> Unit) -> CancelHandle
) -> a with Async
```

With aliases:

```flux
alias ResumeFn<a> = (Result<a, AsyncError>) -> Unit
alias SetupFn<a>  = (FiberId, ResumeFn<a>) -> CancelHandle

public fn await_one_shot<a>(setup: SetupFn<a>) -> a with Async
```

This keeps the surface split crisp:

- `data` declares nominal data types.
- legacy `type Name = Ctor | Other` remains ADT sugar and is not extended.
- `alias` declares transparent abbreviations for effect rows and ordinary
  type expressions.

##### Grammar change

Today's parser has two relevant declaration paths:

```
TypeDecl ::= 'type' Ident TypeParams? '=' AdtVariant ('|' AdtVariant)*
AliasDecl ::= 'alias' Ident '=' '<' EffectRow '>'
```

The proposed grammar:

```
TypeDecl  ::= 'type' Ident TypeParams? '=' AdtVariant ('|' AdtVariant)*  (unchanged)
AliasDecl ::= 'alias' Ident TypeParams? '=' AliasRhs
AliasRhs  ::= '<' EffectRow '>'                  (existing — effect-row alias)
            | TypeExpr                           (new — transparent type alias)
```

This avoids the ambiguous `type Name = String` case entirely: `type`
continues to parse as ADT sugar, while `alias Name = String` is always a
transparent alias. The implementation can reuse the existing
`parse_type_expr` path after `alias Name<...> =` when the right-hand side
does not start with `<`.

##### Semantics: transparent, not nominal

Aliases are **fully transparent** — expanded by the type
checker before any structural comparison. Two values whose declared
types are different aliases of the same underlying type are
unifiable without coercion:

```flux
alias Predicate<a> = (a) -> Bool
alias Filter<a>    = (a) -> Bool

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

- **Recursive aliases.** `alias Cycle = Cycle` and any cycle through
  alias expansion are errors (E308). ADT-sugar declarations remain
  recursive as today (term-level constructors give a base case).
- **Phantom type parameters.** Every type parameter must appear on
  the right-hand side, matching the existing rule for ADT sugar.
- **Constraints on alias parameters.** `alias SortedArray<a: Ord> = Array<a>`
  is rejected — write a class instance instead.
- **Higher-kinded aliases.** `alias Mapped<f, a> = f<a>` is out of
  scope; HKT exists in class declarations but not in aliases for
  this slice.
- **`deriving` on alias declarations.** Aliases cannot carry
  `deriving` — there are no constructors for the alias path.
- **Alias-expansion depth above 64.** The expander caps recursion
  to defend against pathological input.

These restrictions match what shipped first in Haskell and OCaml;
they can be lifted incrementally.

##### Effect rows in aliases

Aliases may contain effect-row syntax, including row variables:

```flux
alias AsyncFn<a, b, e>   = (a) -> b with <Async | e>
alias Handler<req, resp> = (req) -> resp with <Async | Console>
```

When such an alias expands into a function signature, the row check
happens against the expanded form. Row variables inside aliases are
bound at the alias declaration.

##### Visibility

`public alias Name = ...` exports the alias; without `public` the
alias is module-local. Same convention as existing `data`/`fn`
declarations.

##### Implementation sketch

1. **Parser**: extend `parse_effect_alias_statement` into a general
   `parse_alias_statement`. If the RHS begins with `<`, keep producing
   `Statement::EffectAlias`; otherwise call `parse_type_expr` and produce
   a new `Statement::TypeAlias`.
2. **AST**: new variant `TypeAlias { is_public, name, params, body, span }`.
3. **Name resolution**: per-module transparent-alias table populated alongside
   the existing ADT, effect, and effect-alias tables.
4. **Type expansion**: extend the existing substitution code to detect
   alias references and expand them; recursion-depth counter capped at 64.
5. **Cycle detection**: when registering an alias, traverse the expanded
   body for self-references; emit E308 on cycle.
6. **Diagnostics**: extend the type-mismatch reporter to show the alias
   name when the user wrote one, with the expansion available via verbose
   mode.
7. **Tests**: parser tests for `alias Stream<a> = ...`, effect-alias
   regression tests for `alias IO = <...>`, and parity tests that an alias
   and its expansion are interchangeable in function signatures,
   type-class instances, and pattern positions.

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
- **Negative type-class instances or an opt-out marker** — explicitly
  deferred. Phase 1a uses a positive-only `Sendable` model: absence of a
  `Sendable<T>` instance means not sendable, so `Connection` and `Listener`
  need no negative syntax. If future library authors need to say "this
  otherwise-derivable structural type is intentionally not sendable", that
  should be a separate type-class proposal.
- **Tuple destructuring in `let`** — `let (a, b) = pair` instead
  of `match pair { (a, b) -> ... }`. Pure ergonomics.
- **`try` / `finally` / `catch` syntax sugar** — Phase 1b ships
  `Async.bracket(acquire, release, body)`, `Async.finally(body, cleanup)`,
  and `Async.try_(body)` as plain functions. Sugar would compile to the
  same calls. Low priority.
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
│    scope, fork, both, race, timeout, bracket      │
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
│  AsyncBackend — Phase 1a                        │
│    mio reactor thread, Waker, timer heap        │
│    TCP readiness state machines                 │
│    DNS/fs blocking service pools                │
│    completion records only                      │
└────────────────────┬────────────────────────────┘
                     │  readiness / completions
┌────────────────────▼────────────────────────────┐
│  mio — epoll / kqueue / IOCP                    │
└─────────────────────────────────────────────────┘
```

The bottom three layers are stable from Phase 1a onward. Phase 1b adds the
top layer plus per-worker fiber state inside the scheduler. The backend layer
is intentionally completion-oriented: `mio` internally deals in readiness,
but the scheduler receives completed request records.

The same runtime serves both execution paths:

```text
VM bytecode  ─────────────┐
                          ├─ Rust scheduler ─ AsyncBackend ─ mio reactor
LLVM/native ─ C ABI shim ─┘
```

### Phase 0: Concurrency-ready effect runtime

Before the `mio` reactor or worker pool can safely resume Flux computations,
the existing effect runtime must stop relying on process-global yield/evidence
state. Phase 0 is a hard prerequisite for Phase 1a/1b and has no user-facing
API.

Scope:

- Move native yield payloads (`flux_yield_*`), evidence-vector state
  (`current_evv`, marker allocation), and resume bookkeeping into an explicit
  runtime/effect context rather than process-global storage.
- Define the scheduler-owned context that binds together a running fiber/task,
  its evidence vector, yield payload, continuation registry entry, cancellation
  scope, and home worker.
- Make VM and LLVM/native share the same logical suspend/resume contract:
  perform captures a continuation, stores it in the scheduler, returns control
  to the worker loop, and resumes only when a completion is delivered.
- Add focused tests proving two suspended effects can coexist without
  overwriting each other's yield payload, evidence vector, or resume state.
- Keep backend I/O out of this phase. The smallest validation target is a
  deterministic in-memory backend or timer stub that performs
  `Suspend -> completion -> resume`.

Deliverables:

- `src/runtime/async/context.rs` — scheduler-owned effect/fiber context.
- Native C runtime shims updated so generated LLVM code passes or retrieves the
  active context instead of reading process-global yield slots.
- VM runtime updated to store suspended continuations through the same
  scheduler-facing abstractions used by native.
- VM/native parity tests for two concurrent suspended effects, cancellation
  before completion, and cleanup on abandoned continuation.

### VM and LLVM/native runtime bridge

The `mio` backend and scheduler live in Rust. Both execution backends reach the
same Rust runtime; they differ only in the call boundary:

- **VM path:** bytecode `OpPerform` and async primops call Rust scheduler
  functions directly. Values are already Rust `Value`s, so the VM can hand
  scheduler-owned request records to `src/runtime/async/` without a C ABI hop.
- **LLVM/native path:** generated native code still links the C runtime, so it
  calls stable `extern "C"` shims exported by the Rust runtime (or thin C
  wrappers that forward to Rust). Those shims accept opaque handles, tagged
  values, request IDs, and copied buffers; they do not expose `mio` directly to
  generated code.

The boundary looks like:

```text
VM bytecode OpPerform
  -> Rust scheduler / AsyncBackend
  -> mio reactor

LLVM generated code
  -> C ABI shim: flux_async_suspend / flux_async_tcp_write / ...
  -> Rust scheduler / AsyncBackend
  -> mio reactor
```

The C ABI is intentionally narrow. It is not a second implementation of async;
it is a native-code entry surface into the same Rust scheduler. This preserves
one concurrency model across VM and LLVM:

- one request registry,
- one completion-record shape,
- one cancellation state machine,
- one `AsyncBackend` trait,
- one set of Aether/Perceus ownership rules.

### Phase 1a: Multi-threaded runtime substrate

The minimum runtime that compiles, links, and runs Flux programs across
multiple OS threads with `mio`-backed TCP/timer I/O and small blocking pools
for services `mio` does not provide directly. The worker/task manager is
modelled on Lean 4's `task_manager` (Lean 4
`src/runtime/object.cpp:706-916`); the I/O reactor is Flux-owned Rust.

#### The mio reactor

Phase 1a uses one dedicated reactor thread. Worker threads submit I/O
requests to the reactor through a scheduler-owned request registry and wake
it with `mio::Waker`. The reactor owns `mio::Poll`, TCP readiness state
machines, and a timer heap. It never resumes Flux code directly; it emits
completion records back to the scheduler, which delivers them on each
fiber's home worker.

```rust
// src/runtime/async/backends/mio.rs
struct MioBackend {
    poll: mio::Poll,
    waker: mio::Waker,
    requests: RequestRegistry,
    timers: TimerHeap,
    completions: CompletionSender,
    fs_pool: BlockingPool,
    dns_pool: BlockingPool,
}

trait AsyncBackend {
    fn start(&self) -> Result<()>;
    fn shutdown(&self) -> Result<()>;
    fn timer_start(&self, req: RequestId, ms: u64);
    fn tcp_connect(&self, req: RequestId, host: String, port: u16);
    fn tcp_read(&self, req: RequestId, handle: IoHandle, max: usize);
    fn tcp_write(&self, req: RequestId, handle: IoHandle, bytes: BytesBuf);
    fn cancel(&self, req: RequestId);
}
```

#### Hybrid atomic-on-share refcount

`FluxHeader.refcount` becomes a sign-bit-encoded `_Atomic(int32_t)`:

- `rc > 0` — single-threaded reference, increment/decrement non-atomically.
- `rc < 0` — thread-shared reference, increment/decrement with `memory_order_relaxed` atomic.
- `rc == 0` — unique (fast path for in-place reuse).

This is **the actual scheme used by both Lean 4** (`src/include/lean/lean.h:131-136, 544-568`) **and Koka**
(`kklib/include/kklib.h:101-135`), not the "atomic everywhere" scheme
the original 0174 misattributed
to Koka. Single-threaded paths pay no atomic cost.

`Sendable<T>` authorizes crossing a worker boundary; it does not by itself
mean "shallow atomic RC is safe." At every explicit cross-worker boundary
(`Channel.send`, `Task.spawn`, future actor/process sends), the runtime chooses
one transfer strategy:

- **copy** the value into a backend/scheduler-owned representation,
- **deep shared-promotion** of the full reachable Flux object graph, or
- **opaque handle transfer** for runtime-owned resources whose lifetime is not
  represented by ordinary Flux object graphs.

Phase 1 prefers copy or opaque handles. Deep shared-promotion is reserved for
cases where copying is too expensive and the reachable graph can be proven safe
to promote.

Aether's existing `dup`/`drop` insertion is unchanged. The primitive RC support
needed for shared-promoted objects lives in both runtime implementations:
`runtime/c/rc.c` for native objects and the corresponding Rust runtime value
representation for VM-owned values.

#### Worker thread pool

N OS threads, where N defaults to `std::thread::available_parallelism()`. Each
thread runs a loop that pulls work from a shared priority queue. Phase
1a's "work" is `Task<a>`; Phase 1b extends this to fibers.

```rust
struct TaskManager {
    shutdown: AtomicBool,
    queues: Mutex<[VecDeque<TaskId>; MAX_PRIO + 1]>,
    parked: Condvar,
    workers: Vec<JoinHandle<()>>,
}
```

#### `Sendable<T>` constraint

Cross-thread types require `Sendable<T>`, a positive-only type class
auto-derived for:
- All primitive types (`Int`, `Float`, `Bool`, `String`, etc.).
- ADTs whose every field is `Sendable`.
- Persistent collections of `Sendable` elements.

Inspired by Rust's `Send` trait but checked at compile time via Flux's
existing dictionary-elaboration pass
([src/core/passes/dict_elaborate.rs](../../src/core/passes/dict_elaborate.rs)).
This is meaningfully stronger than OCaml/Eio's by-convention warning,
which the Eio domain manager docstring (`lib_eio/domain_manager.mli`)
explicitly admits is unenforced.

Absence of a `Sendable<T>` instance means "not sendable." Phase 1a does not
add negative type-class instances; opaque runtime handles such as
`Tcp.Connection` and `Tcp.Listener` simply do not receive `Sendable`
instances.

#### `Task<a>` primitive

Phase 1a's user-facing concurrency primitive (Phase 1b adds a higher-level
fiber API on top). Constraints are written inline in the type-parameter
list, the form Flux already uses elsewhere (`fn keep<a: Num + Eq>(...)`):

```flux
module Flow.Task {
    public data Task<a> { Task(Int) }   // wraps an opaque task id

    public fn spawn<a: Sendable>(action: () -> a) -> Task<a>
    public fn blocking_join<a: Sendable>(t: Task<a>) -> a
    public fn await<a: Sendable>(t: Task<a>) -> a with Async
    public fn cancel<a>(t: Task<a>) -> Unit
}
```

Tasks run on whichever worker thread picks them up. Phase 1a exposes
`blocking_join`, which blocks the calling OS thread until the task completes;
the caller's worker is parked on the condition variable, so other workers
continue. Phase 1b adds `Task.await`, which suspends the current fiber and is
therefore marked `with Async`. This keeps CPU-bound task parallelism distinct
from fiber-level async I/O.

#### Async backend: `src/runtime/async/backends/mio.rs`

The production Phase 1 backend is `mio`. It exposes a completion-oriented
surface to the scheduler; readiness, partial reads/writes, and reconnectable
state machines stay inside the backend:

```rust
enum CompletionPayload {
    Unit,
    Bytes(Vec<u8>),
    TcpHandle(IoHandle),
    AddressList(Vec<SocketAddr>),
    Error(AsyncError),
}

struct Completion {
    request_id: RequestId,
    target: RuntimeTarget, // Task in Phase 1a, Fiber in Phase 1b
    payload: CompletionPayload,
}
```

`mio` itself supplies TCP readiness and wakeups. Flux implements services that
`mio` intentionally does not provide:

- **Timers:** a runtime-owned min-heap/timer wheel; `Poll::poll` uses the next
  deadline as its timeout.
- **File I/O:** a small blocking pool (`FLUX_FS_THREADS`, default
  `min(4, available_parallelism)`) that returns copied bytes in completion
  records.
- **DNS:** a small resolver pool using the platform resolver first; a dedicated
  async resolver can replace it later.
- **TLS:** deferred to Phase 3 and driven by Rust `rustls` state machines over
  the same TCP readiness backend.

The load-bearing ownership rule is: **the backend never owns, inspects, drops,
or resumes ordinary Flux heap values.** Async requests copy data into
backend-owned buffers or store opaque handles; completions return raw/copied
payloads to the scheduler. The fiber's home worker constructs or drops Flux
values when the completion is delivered.

#### Phase 1a deliverables

- `src/runtime/async/backend.rs` — `AsyncBackend` trait and completion types.
- `src/runtime/async/backends/mio.rs` — `mio` reactor, TCP state machines,
  timer heap, and wakeups.
- `src/runtime/async/blocking_pool.rs` — DNS/file blocking service pools.
- `runtime/c/rc.c` and Rust VM runtime values — shared-promotion support for
  explicit cross-worker transfer boundaries.
- `src/runtime/scheduler.rs` — task manager, ~400 lines Rust.
- `lib/Flow/Task.flx` — `Task<a>` API, ~80 lines Flux.
- `Sendable<T>` type class — ~150 lines across types/ and core/.
- ~10 new `CorePrimOp` enum entries.
- VM and LLVM dispatch for the new primops.
- Build-system: add `mio` dependency behind the default `async-mio` Cargo feature.
- Examples: parallel CPU-bound work, `Task.spawn` + `Task.blocking_join`
  smoke tests; Phase 1b adds `Task.await` examples.

#### Phase 1a forward-compatibility rules

Two decisions that keep Phase 1b cheap:

1. **`RuntimeTarget` is the single completion target abstraction.** Phase 1b
   extends the target from `Task` to fiber/continuation; the backend still emits
   the same `Completion` shape.
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
data Scope { Scope(Int) }

fn run_async<a>(action: () -> a with Async) -> a
fn scope<a>(f: (Scope) -> a with Async) -> a with Async
fn fork(scope: Scope, f: () -> Unit with Async) -> Unit with Async
fn both<a, b>(f: () -> a with Async, g: () -> b with Async) -> (a, b) with Async
fn race<a>(f: () -> a with Async, g: () -> a with Async) -> a with Async
fn timeout<a>(ms: Int, f: () -> a with Async) -> Option<a> with Async
fn timeout_result<a>(ms: Int, f: () -> a with Async) -> Result<a, AsyncError> with Async
fn finally<a>(body: () -> a with Async, cleanup: () -> Unit with Async)
    -> a with Async
fn bracket<r, a>(
    acquire: () -> r with Async,
    release: (r) -> Unit with Async,
    body: (r) -> a with Async
) -> a with Async
fn try_<a>(body: () -> a with Async) -> Result<a, AsyncError> with Async
fn fail<a>(err: AsyncError) -> a with Async
fn yield_now() with Async
fn sleep(ms: Int) with Async
```

Differences from Lean 4's API (`Std/Async/Basic.lean:524-528`):
**Flux's `race` cancels the loser**; Lean's `race` does not.
Cooperative scheduling makes cancellation straightforward (set a flag,
do not resume the continuation when the backend reports completion);
thread-pool tasks make it hard, which is why Lean punted.

#### Per-worker fiber state

Each worker thread maintains a local fiber ready queue. When a fiber
`await`s, the runtime captures its continuation (using the existing
[src/runtime/continuation.rs](../../src/runtime/continuation.rs)
machinery), registers the continuation in the wait registry keyed by
the backend request ID, and the worker immediately picks the next ready
fiber. When the backend emits a completion, the runtime moves the
corresponding fiber back to its worker's ready queue.

Fibers do not migrate between workers (Eio's model). A fiber spawned via
`both` runs on the same worker as its parent. Cross-worker
parallelism comes from many top-level requests landing on different
workers (e.g., the HTTP listener round-robins on accept), not from
splitting one request across workers.

#### Cancellation propagation

`Async.scope(fn(scope) { ... })` establishes a cancel scope and makes that
scope explicit at every child-fiber spawn site. When cancellation is
requested:

1. The scope's `canceled` flag is set.
2. Any backend requests registered under fibers in the scope are marked
   cancel-requested and the backend is asked to cancel or deregister them.
3. Cancellation is semantic, not delegated to the OS: late readiness or
   blocking-pool results are ignored if the request has already been cancelled.
4. The completion path delivers a `Canceled` error to the suspended fiber.
5. The fiber's resume raises `AsyncError.Canceled`, which unwinds to the nearest
   scope boundary. `Async.finally` and `Async.bracket` cleanup functions run
   exactly once during that unwind.

`timeout(ms, f)` is a scoped `race` between `f` and `sleep(ms)`: the winner
cancels the loser. `timeout` maps timeout to `None`; `timeout_result` keeps
the error channel and returns `Err(TimedOut)`.

#### Aether / Perceus interaction

Three considerations, identical in shape to the original 0174:

1. **`perform Suspend` must not let backend-owned state borrow ordinary Flux heap values.** TCP write copies `Bytes` into a backend-owned `Vec<u8>`; TCP read returns a backend-owned `Vec<u8>` that the home worker converts into Flux `Bytes`.
2. **Continuation capture is RC-correct by construction.** Captured frame slots are duped during composition; resume drops on consumption. Continuations that never resume (cancellation) drop their captures via the cancellation path on the home worker.
3. **`@fip`/`@fbip` functions called during a fiber's lifetime do not interact with cross-thread RC** because the fiber's heap stays on its worker thread. Only values that explicitly cross thread boundaries via `Sendable<T>` channels see copy/shared-promotion boundary logic.

#### Phase 1b deliverables

- `lib/Flow/Async.flx` — effect declarations + structured concurrency primitives. ~250 lines Flux.
- `lib/Flow/Tcp.flx` — TCP wrappers expressed as `Async` operations. ~150 lines Flux.
- `src/runtime/scheduler.rs` — fiber layer added on top of the Phase 1a task manager. ~600 additional lines Rust.
- ~5 new `CorePrimOp` entries for fiber suspend/resume.
- Examples: TCP echo server (10k concurrent connections), parallel TCP fetch via `both`, `timeout`-bounded connect, cancellation propagation tests.
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
effect AsyncFail {
    raise: AsyncError -> a
}

// Async is what shows up in user signatures
alias Async = <Suspend | Fork | GetContext | AsyncFail>
```

The seeding mechanism documented at the top of `Flow.Effects.flx`
applies: these are compiler-seeded labels and aliases. `Async` is the
only async-related row that appears in user signatures; the underlying
labels are implementation detail. Adding new I/O capabilities extends
library code that performs `Async`, not the effect declaration itself.

#### The `AsyncError` data type

All Async-aware library functions surface recoverable failures by performing
`AsyncFail.raise(err)`. The error type is a plain Flux `data` declaration:

```flux
public data AsyncError {
    Canceled,                                 // cooperative scope cancellation
    TimedOut,                                 // surfaced by `timeout_result`
    IoError(Int, String, String),             // (code, message, syscall)
    DnsError(Int, String, String),            // (code, message, host)
    TlsError(Int, String),                    // Phase 3
    ProtocolError(Int, String),               // HTTP, Postgres
    ConnectionClosed,
    InvalidAddress(String),                   // (input)
}
```

`AsyncError` is the standard recoverable error type for all Phase 1b–3 libraries.
Functions surface failure via the `AsyncFail.raise` operation in the
`Async` row, which helpers such as `try_` and `timeout_result` convert into
`Result<a, AsyncError>` values. The function signature simply lists `Async`
as part of its effect row — there is no Haskell-style parameterized `Exn<E>`,
because Flux effect labels are unparameterized.

```flux
fn connect(host: String, port: Int) -> Connection with Async
```

The fact that `connect` may fail is encoded in the `AsyncFail.raise`
operation inside `Async`, not in the return type. Library helpers
(`try_`, `timeout_result`, etc.) translate raises into
`Result<Connection, AsyncError>` at the boundary where the user wants to
inspect the error.

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
refcount drops to zero, the runtime deregisters and closes the backend handle —
**explicit `close` is optional but recommended for predictable lifecycle**.

`Connection` is **not** `Sendable` — a connection is bound to the
worker thread that opened it and cannot be sent to another worker.
This is a deliberate choice: socket FDs are not safely usable across
threads in all OS combinations Flux supports. Phase 1a's `Sendable`
class is positive-only: primitives, safe standard-library values, and
structurally-sendable ADTs receive instances; runtime handles do not.
The compile-time check at `Channel.send` / `Task.spawn` boundaries refuses
cross-worker sharing.

#### Closure-style scoped resource lifecycles: `with_*` combinators

The recommended idiom for connection lifecycles is the `with_*`
pattern, which guarantees `close` is called whether the body
completes, fails, or is cancelled. Flux has no `try`/`finally`
syntax; cleanup is provided by `Async.bracket`, which takes separate
acquire, release, and body closures:

```flux
module Flow.Tcp {
    public fn with_connection<a, e>(
        host: String,
        port: Int,
        body: (Connection) -> a with <Async | e>
    ) -> a with <Async | e> {
        Async.bracket(
            fn() { Tcp.connect(host, port) },
            fn(conn) { Tcp.close(conn) },
            body
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
3. **`Async.bracket` is the resource primitive.** It runs `release`
   exactly once whether the body returns, performs `AsyncFail.raise`, or is
   cancelled. `Async.finally(body, cleanup)` is the lower-level cleanup
   helper for cases without an acquired resource.

#### Servers: handler closures and the listener loop

A TCP server in Phase 1b is a single function that recursively accepts
connections and forks a scoped child fiber per connection. Flux has no
`loop`/`while` keyword; iteration is via tail-recursive helpers (a familiar
pattern in `Flow.IO`):

```flux
module Flow.Tcp {
    public fn serve<e>(
        addr: String,
        port: Int,
        handler: (Connection) -> Unit with <Async | e>
    ) -> Unit with <Async | e> {
        let listener = Tcp.listen(addr, port)
        Async.scope(fn(scope) { accept_loop(scope, listener, handler) })
    }

    fn accept_loop<e>(
        scope: Async.Scope,
        listener: Listener,
        handler: (Connection) -> Unit with <Async | e>
    ) -> Unit with <Async | e> {
        let conn = Tcp.accept(listener)
        // Fork a scoped child fiber per connection; per-connection
        // failures are caught locally and do not bring down the server.
        Async.fork(scope, fn() {
            Async.finally(
                fn() {
                    let _ = Async.try_(fn() { handler(conn) })
                    ()
                },
                fn() { Tcp.close(conn) }
            )
        })
        accept_loop(scope, listener, handler)
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

1. **Scoped `fork`.** A child fiber is always attached to an explicit
   `Async.Scope`. The accept loop is unbounded, but the scope still owns all
   children and cancels them on server shutdown.
2. **Error handling is per-connection.** `Async.try_` catches
   `AsyncFail.raise` performed by the handler and yields a `Result`; here
   it is discarded so that one bad request does not kill the server.
   Cancellation propagates to all scoped children and triggers their
   `Async.finally` cleanups.
3. **`scope` is the lifecycle owner.** Exiting `scope` — by failure, by
   external cancel, or by timeout from a parent — cancels all in-flight
   handlers. Once accepted, a connection is fully owned by its fiber's
   `Async.finally` cleanup.

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
    public data Scope { Scope(Int) }

    // Establish a cancellation boundary. Child fibers are attached
    // explicitly through the Scope value.
    public fn scope<a, e>(
        f: (Scope) -> a with <Async | e>,
    ) -> a with <Async | e>

    public fn fork<e>(
        scope: Scope,
        f: () -> Unit with <Async | e>,
    ) -> Unit with <Async | e>

    // Run two operations concurrently, return both results.
    public fn both<a, b, e>(
        f: () -> a with <Async | e>,
        g: () -> b with <Async | e>,
    ) -> (a, b) with <Async | e>

    // Race two operations; first to complete wins; loser is cancelled.
    public fn race<a, e>(
        f: () -> a with <Async | e>,
        g: () -> a with <Async | e>,
    ) -> a with <Async | e>

    // Bound an operation by time. Returns Some(v) on completion,
    // None if the timeout expires.
    public fn timeout<a, e>(
        ms: Int,
        f: () -> a with <Async | e>,
    ) -> Option<a> with <Async | e>

    public fn timeout_result<a, e>(
        ms: Int,
        f: () -> a with <Async | e>,
    ) -> Result<a, AsyncError> with <Async | e>

    public fn finally<a, e>(
        body: () -> a with <Async | e>,
        cleanup: () -> Unit with <Async | e>,
    ) -> a with <Async | e>

    public fn bracket<r, a, e>(
        acquire: () -> r with <Async | e>,
        release: (r) -> Unit with <Async | e>,
        body: (r) -> a with <Async | e>,
    ) -> a with <Async | e>

    public fn try_<a, e>(
        body: () -> a with <Async | e>,
    ) -> Result<a, AsyncError> with <Async | e>

    public fn fail<a>(err: AsyncError) -> a with Async
    public fn yield_now() with Async
    public fn sleep(ms: Int) with Async
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
        let pair = Async.both(
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

- If `both`'s `f` performs `AsyncFail.raise`, `g` is cancelled (its in-flight
  backend requests are cancel-requested); both fibers' `Async.finally` /
  `Async.bracket` cleanups run.
- If `race`'s `f` completes first, `g` is cancelled. Note this differs from
  Lean 4's `race`, which does not cancel the loser (Lean 4
  `Std/Async/Basic.lean:524-528`).
- If `timeout`'s budget expires, the wrapped closure is cancelled and `None`
  is returned. `timeout_result` preserves the reason as `Err(TimedOut)`.
- `Async.scope` owns all child fibers forked with its `Scope` value. Leaving
  the scope cancels in-flight children and runs their cleanup handlers exactly
  once.

#### The setup-closure pattern (for library authors)

Library authors who add new I/O operations write a thin Flux wrapper
that constructs a setup closure and performs `Suspend`. End users
never write this code, but it is the contract that defines what
"a backend-backed operation" looks like in Flux.

Two opaque handle types and two callback-shape aliases (using the transparent
alias feature added in this proposal):

```flux
module Flow.Async {
    // Returned to the runtime when an async operation is registered.
    public data CancelHandle { CancelHandle(Int) }

    // Opaque to user code — the runtime uses it to identify the
    // suspended fiber for completion delivery.
    public data FiberId { FiberId(Int) }

    // Callback shapes for the setup-closure pattern.
    public alias ResumeFn<a> = (Result<a, AsyncError>) -> Unit
    public alias SetupFn<a>  = (FiberId, ResumeFn<a>) -> CancelHandle
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
            // Runtime primop: register backend read, return cancel handle.
            Tcp.Internal.backend_read_start(fid, conn, max, resume)
        })
    }
}
```

The setup closure receives the fiber's ID (so the scheduler knows whom
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
    public fn blocking_join<a: Sendable>(t: Task<a>) -> a
    public fn await<a: Sendable>(t: Task<a>) -> a with Async
}

module Flow.Channel {
    public data Channel<a> { Channel(Int) }

    public fn bounded<a: Sendable>(capacity: Int) -> Channel<a> with Async
    public fn send<a: Sendable>(ch: Channel<a>, msg: a) -> Unit with Async
    public fn recv<a: Sendable>(ch: Channel<a>) -> Option<a> with Async
    public fn close<a>(ch: Channel<a>) -> Unit with Async
}
```

`Sendable` is auto-derived for primitive types (`Int`, `Float`,
`Bool`, `String`, `Bytes`) and for `data` declarations whose every
field type is `Sendable`. Types backed by non-atomic interior
mutation, thread-local resources, or raw OS handles (`Connection`,
`Listener`) simply have no `Sendable` instance.

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
top of the Detailed design section. Transparent aliases are the only
strict prerequisite (included in this proposal); the remaining items
(string interpolation, negative type-class instances, tuple
let-binds, `try`/`finally` sugar, `loop`/`while`, named arguments)
are documented there as ergonomic gaps to re-evaluate after Phase
1b lands and real user code is written against the API.

#### What this design does not do

To be clear about scope:

- **No `async fn` syntax sugar.** `with Async` in the effect row is the marker; no special `async`/`await` keywords. Calling an `Async` function from another `Async` function is just function call.
- **No `Future<a>`/`Promise<a>` type.** Concurrency is via fork/join scopes, not handles.
- **No user-visible unscoped `spawn`** (other than `Task.spawn` for CPU-bound work). Long-lived background work is attached to an explicit `Async.Scope`.
- **No automatic retry, backoff, or circuit-breaking.** These are userspace libraries built on `race`/`timeout`/`scope`, not language features.
- **No streaming yet at this layer.** `Stream<a>` arrives in Phase 2; Phase 1b is one-shot operations only.

### Phase 2: HTTP/1.1 + JSON + Streams

#### HTTP

Wrap [llhttp](https://github.com/nodejs/llhttp) (Node.js's parser, ~3k lines
C, MIT-licensed) behind Rust runtime bindings. Vendor as `vendor/llhttp/`;
the HTTP parser is a service used by `Flow.Http`, not the async backend.

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

- `Flow.Json.parse: String -> Json` — tagged union value (`data Json { JsonNull, JsonBool(Bool), JsonNumber(Float), JsonString(String), JsonArray(Array<Json>), JsonObject(Map<String, Json>) }`).
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
    public alias Stream<a> = () -> Option<a> with Async

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
- `src/runtime/http/llhttp.rs` plus vendored `llhttp` — parser binding and HTTP glue.
- Codec derivation added to `dict_elaborate.rs`.
- Examples: hello-world microservice, JSON echo, SSE broadcaster, parallel HTTP fetch.
- Documentation: HTTP server quickstart, JSON codec guide.

Estimated effort: 6 weeks.

### Phase 3: TLS + database client

#### TLS

Use Rust `rustls` directly in the runtime. TLS connections are state
machines driven by the same `mio` TCP readiness backend; no C-ABI TLS wrapper
is needed for the Rust scheduler path.

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
- `src/runtime/async/tls.rs` — rustls state-machine integration.
- Examples: HTTPS server, database-backed CRUD microservice (the
  motivating example from Summary).
- Integration tests against a real Postgres instance.

Estimated effort: 4 weeks.

### Phase 4 (optional): io_uring backend for Linux

**Not committed.** Ships only if the `mio` Linux backend becomes a measured
throughput bottleneck. The point of mentioning Phase 4 in this proposal is
to document **what the seam protects**, not to commit to building it.

Eio demonstrates the dual-backend pattern (`lib_eio_linux/` for
io_uring, `lib_eio_posix/` for epoll/kqueue).
The substitution sits below the three-effect seam, so user code and the
structured concurrency primitives remain unchanged. The Rust scheduler gets
a configuration knob (`mio` vs `io_uring`) and a second implementation of
the same `AsyncBackend` trait.

Estimated effort: 4-6 weeks if/when triggered. Skipped for the foreseeable
future; `mio` on epoll is more than adequate for the proposal's stated
workload until measurements prove otherwise.

## Drawbacks

- **`mio` is lower-level than libuv.** Flux must own timers, DNS/file blocking
  pools, process/signal support, and TCP state machines instead of receiving
  them from a batteries-included C runtime. This is accepted to keep scheduler
  ownership and Aether/Perceus boundaries in Rust.
- **Phase 1a + 1b is roughly 2-3 months of work.** Less than the original 0174's five phases, but front-loads the multi-threading work (which the original 0174 deferred to Phase 5).
- **Continuation capture across backend completions is the load-bearing technical risk.** Phase 1a sidesteps this; Phase 1b tackles it directly. Mitigation: prototype the simplest possible `Suspend` → runtime timer → resume cycle before committing the full Phase 1b scope. Eio proves the approach works on a comparable semantic substrate.
- **No `Fiber<a>` handle is a departure from Promise/Tokio idioms.** Users coming from JavaScript or Rust may expect spawn-and-await. The Eio precedent argues structured scopes are the right primary API.
- **No work-stealing between worker threads.** A fiber spawned on worker N stays on worker N. This is Eio's model. Load imbalances (one worker busy, others idle) are possible but uncommon for HTTP workloads where fibers tend to be short-lived.
- **No preemption.** A fiber that does not `await` blocks every other fiber on its worker. Same limitation as Node and Eio. Mitigation: `Async.yield()` for long pure loops, or `Task.spawn` to hand work to a different worker.
- **Backend cancellation is best-effort at the OS layer.** `mio` cannot make
  epoll/kqueue/IOCP cancellation uniform. Flux cancellation is therefore a
  scheduler state-machine guarantee: cancelled requests may complete late, but
  they are ignored or finalized without resuming the fiber twice.
- **HTTP/2 and gRPC are deferred.** Phase 2-3 ship HTTP/1.1 only. Adequate for most microservices.
- **TLS via rustls keeps more runtime code in Rust.** This is architecturally
  cleaner with `mio`, but it means Flux owns TLS state-machine integration
  rather than delegating to a C shim.

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
2. **Lean 4's runtime has a substantial task/async substrate**, with the task manager and ownership discipline living in runtime code and ~3,000 lines Lean source on top.

The original 0174's "~300 lines Flux" estimate for the scheduler was
unrealistic by ~5-8x. Moving the scheduler to Rust, where the rest of
the runtime ([src/runtime/](../../src/runtime/)) lives, gives the same
implementation for both backends (VM and LLVM call the same Rust
functions via primops), and concentrates RC, worker scheduling, backend
requests, completion delivery, and cancellation in a single layer where
Rust's ownership system catches mistakes.

The Flux source layer keeps what genuinely benefits from being expressed
as effect handlers: the structured concurrency primitives. Those are
~250 lines of Flux that meaningfully exercise the effect system.

### Why Koka's API shape vs. JavaScript-promise shape

Spawn-and-join with a `Fiber<a>` handle is the JavaScript/Tokio idiom.
It encourages unstructured concurrency (spawned fibers leaking past
their intended scope, no cancellation propagation, "fire-and-forget"
mistakes).

Structured concurrency (`scope`, `fork`, `both`, `race`, `timeout`,
`bracket`) makes the lifetime relationship between concurrent
operations syntactically obvious. Cancellation is automatic — leaving
a scope cancels in-flight work. This matches Eio and Trio (Python) and
is the direction modern async design has converged on.

### Why mio vs. alternatives

Investigated: libuv, libevent, io_uring, Tokio, Boost.Asio, and hand-rolled
epoll/kqueue/IOCP. `mio` is chosen because it gives Flux a portable
readiness substrate without forcing Flux into Rust's `Future`/`Pin` model
or libuv's callback ownership model. The runtime remains Flux-owned Rust:
request state machines, cancellation, home-worker delivery, and
Aether/Perceus ownership boundaries all live in one layer.

The alternatives each lose a key property:

- **libuv** is mature and batteries-included, but it makes the callback
  runtime the center of I/O. Flux would have to prevent C callbacks from
  touching ordinary Flux heap values and would still need Rust-side scheduler
  state. `mio` avoids that split.
- **Tokio** is production-grade, but its public model is Rust futures. Flux
  wants algebraic effects and structured scopes, not a wrapper around Tokio
  tasks.
- **io_uring** is attractive on Linux but not portable enough for the first
  backend. It remains the Phase 4 optimization backend behind `AsyncBackend`.
- **Hand-rolled OS backends** give maximum control but require separate
  epoll/kqueue/IOCP implementations from day one.

The cost of `mio` is that Flux must implement timers, DNS/file blocking
pools, TLS integration, and process/signal support as runtime services. That
cost is accepted because these services then obey the same scheduler and
ownership invariants as the rest of Flux.

### Why hybrid atomic-on-share RC vs. atomic-everywhere

The original 0174's Phase 5 plan was "atomic refcounts everywhere,
mirroring Koka." This was a misread of both Koka and Lean 4:

- **Koka** uses sign-bit-encoded hybrid RC: positive non-atomic, negative atomic. See `kklib/include/kklib.h:101-135` and `kklib/src/refcount.c:150-200` in the Koka source tree.
- **Lean 4** uses the same scheme: see `src/include/lean/lean.h:131-136, 544-568` in the Lean source tree.

Both production languages with Perceus RC and multi-threading use
hybrid. There is no production precedent for "atomic everywhere." Hybrid
keeps single-threaded paths (the common case) on non-atomic operations. The
local primitive change is small; the real implementation work is enforcing the
copy/shared-promotion/opaque-handle transfer discipline at every cross-worker
boundary.

### Why multi-threading in Phase 1a vs. Phase 5

The original 0174 deferred all threading work to Phase 5
(conditional). Two pieces of evidence argued against this:

1. **Node's deficiency under HTTP-microservice load** (the proposal's stated target) is widely documented. Process-per-core (the original Phase 3) papers over this for stateless services but breaks down for shared-state workloads (in-process cache, connection pool, rate limiter — exactly the cases where Node loses to Go in practice).
2. **Hybrid RC and the worker substrate belong in the first concurrency slice.**
   The local refcount primitive change is small, but the hard part is proving
   every cross-worker boundary copies or shared-promotes the full reachable
   graph correctly. Doing the boundary discipline in Phase 1a avoids a later
   semantic retrofit.

The Phase 1a + 1b structure is therefore strictly more capable than
the original Phase 1, with no meaningful additional work. The original
Phase 3 (process-per-core) is removed because Phase 1a already handles
multi-core; the original Phase 5 is removed because Phase 1a already
ships hybrid RC.

### Alternatives considered and rejected

- **Lean-style (thread pool, no fiber layer) only.** Simpler to ship but caps concurrency at ~thousands per process — insufficient for HTTP microservices (c10k pattern). Rejected; Phase 1b adds the fiber layer specifically to clear this ceiling.
- **libuv as the primary backend.** Mature and broad, but it moves the I/O
  lifecycle into C callbacks. Rejected for the first backend because Flux wants
  the scheduler, request registry, cancellation, and Aether boundary in Rust.
- **Eio-style three native backends from day one** (io_uring + epoll/kqueue + IOCP). 5x the backend LOC. Multi-year overhead. Rejected; `mio` gives one portable readiness backend now, and Phase 4 keeps the io_uring door open if measurements justify it.
- **Goroutine-style M:N work-stealing scheduler.** Took Go a decade to mature. Out of scope. Rejected; per-worker scheduling without migration is sufficient for the stated workload, with Eio as the precedent.
- **Adopt Tokio.** Rejected because Flux should own its effect-driven scheduler semantics rather than encode them as Tokio futures/tasks. `mio` keeps the low-level reactor without importing Tokio's async model.
- **Per-thread heaps with linear-type send (Erlang-style).** Beautiful but requires major type-system work (uniqueness types, send-primitives). Multi-year scope. Rejected; `Sendable<T>` (Rust-style trait) gives most of the safety with much less type-system work.

## Prior art

- **Lean 4** — the closest existing ownership and task-manager substrate to Flux: Perceus RC, native compilation via LLVM, task manager, and hybrid RC. Flux copies the task-manager and hybrid-RC lessons, not Lean's libuv backend. Where the proposal diverges from Lean: Phase 1b adds a fiber layer that Lean does not have (Lean tasks block their worker threads on `await`); cancellation propagates through `race` (Lean's `race` does not).
- **OCaml/Eio** — the closest existing API surface to what Phase 1b ships. The three-effect seam (Eio `lib_eio/core/eio__core.ml:15-21`: `Suspend`, `Fork`, `Get_context`) is copied directly. The structured concurrency primitives (`Switch`, `Fiber.both`, `Fiber.first`) inform `both`/`race`. Per-domain non-migrating fiber model is adopted. Eio's pluggable-backend architecture (`lib_eio_linux/`, `lib_eio_posix/`, `lib_eio_windows/`) is the model for Phase 4's optional io_uring escape hatch.
- **Koka** — original source of the `await(setup)` API pattern (Koka `lib/v1/std/async.kk:521`) and hybrid RC precedent.
- **Rust / mio** — `mio` supplies the low-level readiness abstraction without committing Flux to Rust futures. Rust's `Send`/`Sync` trait discipline remains the cleanest production formulation of compile-time thread-safety. Phase 1a's `Sendable<T>` is the analogous constraint, more expressive than OCaml/Eio's by-convention warning.
- **GHC** — the RTS-in-C / IO-manager-in-Haskell split (GHC `rts/Schedule.c`, 3,353 lines; `libraries/ghc-internal/src/GHC/Internal/Event/Manager.hs`, 544 lines) is the precedent for "scheduler in runtime, structured concurrency in source language" that the revised Phase 1b adopts. GHC has never used libuv (epoll/kqueue/IOCP via its own backends) — the scheduler split, not the I/O substrate, is the relevant lesson.
- **Trio (Python)** — popularised "structured concurrency" terminology. API shape (nurseries, scoped cancel) directly influences `scope`/`fork`/`both`.
- **Node.js** — defined libuv. Single-threaded async-via-callbacks. Demonstrates the scale of one event loop on real microservice workloads, and the limitations (cluster-instead-of-threads, slow-handler-stalls-loop) that Phase 1b's multi-worker fiber model is designed to avoid.
- **Erlang/BEAM** — per-process heaps + reduction-counted preemption. Considered as Phase 5 alternative; rejected (requires linear types and per-thread heaps).
- **Haskell `async` library** — `Async a` handles + `wait`/`cancel`. The unstructured-concurrency precedent we are intentionally diverging from.
- **Flux proposal [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md)** — earlier exploration of actor-style concurrency for Flux. Deferred; actor patterns can be built as a userspace library on top of Phase 1b's `Async` effect plus `Sendable<T>` channels.

## Unresolved questions

1. **Continuation re-entry from backend completions.** When the `mio` backend emits a completion, the scheduler must locate the suspended fiber and enqueue it on the home worker. The mechanism is a Rust-side wait registry keyed by backend request ID. Detailed design deferred to Phase 1b implementation; prototype validates the cycle before committing.
2. **`Bytes` zero-copy vs. copy on TCP read.** Phase 1b ships copy-on-delivery for simplicity: the backend returns `Vec<u8>`, and the home worker constructs Flux `Bytes`. Phase 2 may move to zero-copy with scheduler-owned buffers. Decision deferred to benchmarking.
3. **Pool internal mutation.** Phase 3's `Postgres.Pool` has internal mutable state (idle connections, in-flight count). Modeled as parameterized handler state. Concrete representation TBD.
4. **JSON codec error reporting.** `Json.decode` failure on malformed input — returns `Result<T, JsonError>` with field-path information. Schema TBD.
5. **HTTP/1.1 keep-alive eviction policy.** Connection pool sizing and timeout defaults TBD.
6. **TLS certificate management.** Loading, rotation, and revocation policies TBD; `rustls` provides primitives, Flux-side ergonomics deferred to Phase 3 design.
7. **`Sendable<T>` derivation rules for closures.** A closure capturing only `Sendable` values is `Sendable`; a closure capturing a non-`Sendable` value is not. Compile-time check needed in `dict_elaborate.rs`. Detailed inference rules TBD.
8. **Single reactor contention under high fiber count.** Phase 1b runs all TCP/timer operations through one `mio` reactor thread. At very high concurrency the reactor queue or completion fan-out may become the bottleneck. Mitigations (sharded reactors, per-worker reactors, io_uring backend) deferred until measured.
9. **Shared backend handle table across forked branches** — *resolved in current revision.* Each worker VM spawned by `Async.both` / `Async.race` / `Async.fork` shares the parent's `mio` reactor (one process-wide `tcp_streams` / `tcp_listeners` table). Per-VM completion routing is implemented via a `RequestId → BackendCompletionSink` map on `MioBackend`: when a child handle (`MioBackendHandle::with_completion_sink`) submits a command, it registers a route entry so the reactor delivers that request's completion to the originating worker rather than to the parent's primary sink. `MioDriverBackend::child()` returns a child driver that the parent passes into `run_send_closure_on_worker`. Verified by `examples/async/parallel_both.flx` running TCP read on both branches of `Async.both` over distinct loopback connections, identical output on the VM and LLVM backends, with no regressions across the 80 existing parity fixtures.

## Revision history

- **Revision 1 (original)** — five-phase plan: single-threaded Async + TCP, HTTP/JSON/Streams, process-per-core, TLS+Postgres, conditional shared-state multi-threading via atomic-everywhere RC. Cited Koka as the precedent for "scheduler in source language, libuv substrate, atomic RC." See git history for original text.
- **Revision 2** — restructured into Phase 1a (multi-threaded substrate, modelled on Lean 4) + Phase 1b (fiber layer + structured concurrency, modelled on Eio), with Phases 2-3 unchanged in shape and an optional Phase 4 (io_uring backend) replacing the original Phase 5. Multi-threading lands in Phase 1a (was Phase 5). Process-per-core (was Phase 3) is removed; Phase 1a's worker pool subsumes it. Hybrid atomic-on-share RC (Lean's and Koka's actual scheme) replaces the original "atomic everywhere" Phase 5 plan. Scheduler moves from Flux source to Rust. Three-effect seam (Suspend/Fork/GetContext) replaces the single `Async` effect for backend extensibility. `Sendable<T>` constraint added (modelled on Rust's `Send`).
- **Revision 3** — strict syntax pass against the actual Flux grammar (`src/syntax/token_type.rs` keyword set, `src/syntax/parser/`). All code samples rewritten to use only supported constructs: named-field records via `data Foo { Foo { ... } }` (proposal 0152), `deriving` clauses on `data` declarations, positional function arguments, recursion in place of `loop`/`while`, library functions in place of `try`/`finally`/`catch`, `match` for tuple destructuring, and `<a: Class>` constraints inline in type-parameter lists. Plain type aliases were folded into this proposal as a "Required language features" section because Phase 1b's setup-closure pattern is awkward without them; ADT-sugar `type` was extended to accept any type expression on the right-hand side, with restrictions described in detail.
- **Revision 4** — concurrency syntax tightened. Transparent aliases now extend the existing `alias` declaration instead of overloading `type`; ADT-sugar `type` remains unchanged. User-facing structured concurrency now centers on `Async.scope`, scoped `fork`, `both`, `race`, `timeout`, `timeout_result`, `finally`, and `bracket`. `AsyncFail` is operation-bearing (`raise: AsyncError -> a`) rather than a payload-carrying label. `Sendable` is positive-only; no negative instance syntax is required for non-sendable handles. CPU-bound `Task.blocking_join` is distinct from fiber-suspending `Task.await`.
- **Revision 5 (this version)** — I/O backend changed from libuv-first to `mio`-first. A mandatory Phase 0 now makes the effect runtime concurrency-ready before user-facing async work. Phase 1a uses a Rust `AsyncBackend` trait, a dedicated `mio` reactor thread, runtime-owned timer heap, TCP readiness state machines, small blocking DNS/file pools, and one Rust scheduler reached directly by the VM and through narrow C ABI shims from LLVM/native code. Lean 4 remains inspiration for task manager and hybrid RC, Eio remains inspiration for the user-facing structured concurrency seam, and Flux owns the Aether/Perceus boundary by requiring backend completion records rather than C callbacks that manipulate Flux heap values.

## Future possibilities

- **HTTP/2 multiplexing** — once HTTP/1.1 is stable. Significant complexity; likely a separate proposal.
- **WebSocket and Server-Sent Events** — both fall out of HTTP/1.1 + streams in Phase 2 with small additional work.
- **gRPC** — HTTP/2 + protobuf. Future proposal.
- **io_uring backend for Linux** — Phase 4 (optional). Eio demonstrates the dual-backend pattern.
- **Sharded or per-worker reactors** — replace the single `mio` reactor thread with sharded reactors or one reactor per worker. Adds cross-reactor handoff complexity. Deferred until measured.
- **Process-per-core** — was the original Phase 3. Removed because Phase 1a's worker pool already provides multi-core scaling. Can be reintroduced as a userspace library on top of `Process.spawn` if specific deployments want process isolation.
- **Distributed actor model** — built on Phase 1b's `Async` effect + `Sendable<T>` channels. Userspace library; replaces what 0143 originally proposed as language-level actors.
- **Job queue / scheduled tasks** — userspace library on top of `sleep` + persistent storage.
- **File watchers** — `inotify`/`fsevents` through platform-specific watcher backends.
- **GraphQL server** — HTTP + JSON + DataLoader-style fan-out via `both`.

## Appendix: end-to-end POST request trace (Phase 1b + Phase 2)

A `POST` request from user code to the wire and back, illustrating how
the three-effect seam, `mio` backend, and the existing continuation-capture
runtime compose.

User code:

```flux
let resp = Http.post("https://api.example.com/users", body)
```

`Http.post` is Flux code: format request bytes, `Tcp.connect`,
`Tls.handshake`, `Tcp.write`, repeated `Tcp.read` until response complete,
parse. Each I/O call ultimately performs `perform Suspend(setup_closure)`.

For one `Tcp.write`:

1. **`Tcp.write` calls `perform Suspend(setup)` where `setup` registers a backend write request.**

   **VM:** `OpPerform` ([src/bytecode/op_code.rs:97-102](../../src/bytecode/op_code.rs)) walks the evidence vector, finds the `Suspend` handler installed by `run_async`. Captures the post-perform continuation via `Continuation::compose()` ([src/runtime/continuation.rs:49-93](../../src/runtime/continuation.rs)). Hands `(continuation, setup_closure, fiber_context)` to the handler arm.

   **LLVM:** equivalent — emits `flux_yield_to(htag, optag, arg, arity)` ([src/lir/emit_llvm.rs:3403-3511](../../src/lir/emit_llvm.rs)). `cont_split` ([src/lir/lower.rs:3594-3685](../../src/lir/lower.rs)) synthesised the continuation at compile time. Both backends share the C-runtime yield protocol.

2. **The `Suspend` handler arm (~5 lines Flux) calls a Rust primop `flux_scheduler_suspend(fiber_id, setup, continuation)`.** The Rust scheduler:
   - Stores `(fiber_id, continuation)` in the wait registry.
   - Calls `setup(fiber_id)`, which calls a backend primop like `flux_backend_tcp_write(fiber_id, conn, data)`.
   - The backend copies `data` into a Rust-owned `Vec<u8>`, registers writable interest with the `mio` reactor, and returns a `CancelHandle`.
   - The current worker thread now has a free slot — picks the next ready fiber from its local queue and resumes it.

3. **`mio` reports socket-writable readiness.** The reactor thread advances the write state machine:
   - writes from the Rust-owned buffer until complete or blocked,
   - frees the buffer when the request is finalized,
   - emits a `Completion { request_id, target: Fiber(fiber_id), payload: BytesWritten(n) }`.

4. **The scheduler completion path** looks up `fiber_id` in the wait registry, retrieves the continuation, and enqueues `(continuation, n_bytes_written)` into the **fiber's home worker's** ready queue. Cross-worker enqueue uses the scheduler's worker wakeup mechanism if the target worker is parked.

5. **Eventually the home worker pulls the resumed fiber from its ready queue.** VM: `execute_resume` restores frames, pushes `n_bytes_written` where `perform` would have returned. LLVM: jumps to the post-perform block with the value as block parameter. `Tcp.write` returns. `Http.post` continues to the next operation.

6. **Many awaits later, the response is fully read and parsed.** `Http.post` returns to user code. `let resp = ...` gets the response.

Throughout: the fiber's heap stays on its home worker thread, so refcounts
on its working set remain non-atomic (positive `m_rc`). The `data` value
does not cross into the backend as a Flux heap object; the backend receives
a copied byte buffer. The `fiber_id` is an opaque handle that does not
interact with RC. Cancellation (e.g., from a surrounding `timeout`) sets
the scope's `canceled` flag, marks registered backend requests as
cancel-requested, and prevents the continuation from being resumed normally;
instead the resume path raises `AsyncError.Canceled` and unwinds into the
nearest `Async.scope` boundary. Late readiness or blocking-pool completions
are finalized without resuming the fiber twice.

This is the entire concurrency model for Phases 1a, 1b, 2, and 3.
Phase 4's optional io_uring backend slots in below the `AsyncBackend` layer
without changing anything above it.
