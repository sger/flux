- Feature Name: Async Effect & Concurrency Roadmap
- Start Date: 2026-04-27
- Status: Draft (revision 9 — supersedes the original five-phase plan; see "Revision history" at end)
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
- **Phase 2 — Concurrency closeout + runtime gaps.** Ten slices. The first seven pin down concurrency semantics that Phase 1b left under-specified — real fiber-suspending `Task.await`, N-way `race`/`first`, the `Flow.Channel` decision, cancellation observation in pure loops (`Async.check_cancelled`), `Http.serve` production-knobs design, fiber panic propagation, and centralised `RuntimeConfig`. The last three close the runtime prerequisites the original Phase 1a/1b plan listed but did not land: blocking-thread DNS resolver / `blocking_pool.rs`, transparent type aliases (the `alias Name = TypeExpr` extension), and `Sendable<T>` ADT auto-derivation. No user-facing API regressions; the surface that exists today keeps working. Inserted at revision 9 — see "Revision history" at end.
- **Phase 3 — HTTP/1.1 + JSON + Streams.** HTTP/1.1 parser scratch-built in Rust under `src/runtime/http/` over the existing `mio` TCP substrate (no `vendor/`, no third-party HTTP library). JSON ships parser + manual codec instances first; `deriving (Json.Encode, Json.Decode)` synthesis lands as a follow-on sub-slice. Streams unblocked by Phase 2's transparent aliases.
- **Phase 4 — TLS + database client.** Was Phase 4 in the original 0174 (post-revision-8 numbered Phase 3).
- **(Optional) Phase 5 — io_uring backend for Linux.** Backend swap behind the same `AsyncBackend` seam if perf measurements justify it.

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
| **Phase 1a** — Multi-threaded runtime substrate | ✅ complete for current scope | All seven slices landed, including the Flux `Flow.Task` source surface, VM task dispatch, native `flux_task_*` runtime, and `Sendable` enforcement for current consumers. Outstanding follow-up: optional Rust-staticlib task unification if a future feature needs one shared task table across backends. (`Sendable` ADT auto-derivation, originally listed as a 1a-v follow-up, was found in revision-9 audit to already be implemented via `synthesize_sendable_instances`; closed as Phase 2 slice 2-x.) |
| 1a-i — `mio` dependency + reactor skeleton | ✅ | [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs): dedicated reactor thread owning `mio::Poll`; `start`/`shutdown` lifecycle with `Waker`-driven wake + `JoinHandle` cleanup; `Drop` joins to guard against leaked threads on Windows. No I/O sources registered yet. |
| 1a-ii — Timer service | ✅ | [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs): runtime-owned `BinaryHeap` of `(deadline, RequestId)`; `Poll::poll` uses next deadline as its timeout; expired entries produce `CompletionPayload::Unit` into a shared completions queue. `cancel(req)` suppresses the fire and drops any already-queued completion. `timer_start` and `next_completion` extend the [`AsyncBackend`](../../src/runtime/async/backend.rs) trait; the in-memory test backend implements them with deterministic semantics. |
| 1a-iii — Worker pool + `RuntimeTarget` | ✅ | [`task_manager.rs`](../../src/runtime/async/task_manager.rs): N-thread worker pool with a shared per-priority FIFO (`MAX_PRIO = 2`), `Condvar`-parked workers, `start`/`submit`/`shutdown` lifecycle, `Drop` joins on teardown to keep libtest from wedging on Windows. [`runtime_target.rs`](../../src/runtime/async/runtime_target.rs): `TaskId` + `RuntimeTarget` enum (Task variant; Fiber variant lands in 1b). End-to-end completion routing waits on the actual `Task<a>` user surface (1a-vi). |
| 1a-iv — Hybrid atomic-on-share RC | ✅ (C side) | [`runtime/c/rc.c`](../../runtime/c/rc.c): `FluxHeader.refcount` is now `_Atomic(int32_t)` with sign-bit encoding (`rc > 0` ST mode, relaxed; `rc < 0` MT mode, atomic; last MT drop is acq_rel). New API in [`flux_rt.h`](../../runtime/c/flux_rt.h): `flux_rc_promote` (recursive ST → MT promotion with release ordering, walks evidence vectors and standard scan offsets), `flux_rc_is_shared`. LLVM-emitted inline `rc == 1` reuse/uniqueness checks (in [`prelude.rs`](../../src/llvm/codegen/prelude.rs)) naturally fail for negative refcounts and fall back to `flux_drop` — no LLVM changes required. Native `Task.spawn` is the first C-side cross-worker consumer and promotes the closure before handing it to a worker thread. **Rust `Value` mirror still deferred** because the VM task path remains sequential and does not send `Rc<Value>` across OS threads. Regression coverage: the full native-LLVM test suite (which exercises dup/drop heavily) passes unchanged, proving the ST hot path is encoding-equivalent. |
| 1a-v — `Sendable<T>` type class | ✅ (primitives + structural + ADTs) | Marker class registered in [`class_env.rs`](../../src/types/class_env.rs)'s `register_builtins`, no methods. Built-in primitive instances: `Int`, `Float`, `String`, `Bool`, `Unit`. Positive-only structural derivation in [`class_solver.rs`](../../src/types/class_solver.rs)'s `has_structural_builtin_instance`: tuples, `Option`, `List`, `Array`, `Map`, `Either` auto-derive `Sendable` when their element types satisfy it. User ADTs whose fields satisfy `Sendable` are synthesized by [`synthesize_sendable_instances`](../../src/types/class_env.rs); function-typed fields and closures remain non-sendable. Tests in [`sendable_tests.rs`](../../tests/type_inference/sendable_tests.rs) cover primitives, tuples, collections, ADTs, recursive ADTs, and negative function cases. The stale ADT follow-up was closed by the revision-9 audit as Phase 2 slice 2-x. |
| 1a-vi — `Task<a>` + `Flow.Task` | ✅ end-to-end VM + native surface | **Rust scheduler:** [`task_scheduler.rs`](../../src/runtime/async/task_scheduler.rs) — `TaskScheduler` wraps the 1a-iii [`TaskManager`](../../src/runtime/async/task_manager.rs) with per-task `Arc<TaskState>` (outcome `Mutex` + `Condvar` + cancel `AtomicBool`). `spawn(action: FnOnce() -> T + Send + 'static)` returns a `TaskHandle<T>`; `blocking_join` consumes it and surfaces `TaskJoinError::Cancelled`/`Panicked` for non-value outcomes (panics are caught by the worker so the pool isn't poisoned). **Flux source surface:** [`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) — `data Task<a> { Task(Int) }` plus `spawn<a: Sendable>` / `blocking_join<a: Sendable>` / `cancel<a>` / `await<a: Sendable>` signatures. **VM:** sequential task table executes the action and stores the result; `cancel` before join/await surfaces an error. **Native:** [`runtime/c/tasks.c`](../../runtime/c/tasks.c) implements `flux_task_spawn` / `flux_task_blocking_join` / `flux_task_cancel` / `flux_task_await` using POSIX pthreads or Win32 threads, promotes closures with `flux_rc_promote`, and matches the VM-observable cancel-before-join/await behavior. Native `Task.await` is a Phase 1b closeout shim over blocking join; true fiber-suspending task await is a post-1b follow-up. [`tests/integration/flow_task_tests.rs`](../../tests/integration/flow_task_tests.rs) verifies the VM source surface, cross-module `Sendable` enforcement, and native Int/String/cancel/await fixtures. |
| 1a-vii — TCP readiness state machines | ✅ | New `IoHandle` type and `CompletionPayload::TcpHandle` variant in [`backend.rs`](../../src/runtime/async/backend.rs); `AsyncBackend` extended with `tcp_connect` / `tcp_read` / `tcp_write` / `tcp_close` (default impls panic; in-memory backend left unimplemented since real-socket round-trips have no deterministic stub). [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs) gets a per-iteration TCP command queue (owning thread → reactor) and per-`IoHandle` `TcpConnState` holding pending connect/read/write requests. The reactor resolves a pending connect on writable + `take_error()`, services pending reads on readable (`WouldBlock` is "wait for next event", empty buffer = EOF), and loops writes through `WouldBlock` so partial writes resume from the recorded offset under the same `RequestId`. Loopback echo round-trip and refused-connect tests prove the cross-thread substrate end-to-end. **Cancellation of in-flight TCP ops not yet wired** — falls back to the registry-side cancel-set rewrite from 0e (sound but does some wasted I/O work); explicit reactor-side teardown lands when there's a consumer for it. |
| **Phase 1b** — Fiber layer + structured concurrency | ✅ complete for current scope | **VM path fully functional for current scope:** Slices 1b-i through 1b-v landed (effect seams, FiberScheduler, fiber CorePrimOps, Flow.Async source surface). 1b-vi-a/b₁/b₂.1/b₂.2 + `Async.timeout` landed: `Async.sleep` routes through mio reactor; every `Async.run_async` boundary owns a real `FiberScheduler`; `FiberSleep`, `FiberBoth`, `FiberRace`, `FiberTimeout` capture continuations and park/resume through dispatch loop, giving genuine wall-clock overlap. TCP parity (1b-vi-e) works on VM and LLVM/native: both fixtures pass with concurrent server/client through the shared mio-backed async request/completion flow. **LLVM/native core async works:** native `sleep`, `both`, `race`, `timeout`, `scope`, `fork`, `cancel`, direct async calls, indirect async closure calls, and TCP primitives suspend/resume through the Rust native async runtime; native child fibers are assigned to home-worker queues and non-root workers run generated code concurrently on OS threads. `Task.await` is usable on VM and native for the closeout scope. Deferred post-1b follow-ups: VM OS-worker dispatch needs a VM `Value` promotion/thread-safety story, and true fiber-suspending native `Task.await` needs native tasks to publish scheduler completions instead of blocking join. |
| 1b-i to 1b-v | ✅ | Effect seams (`Suspend` / `Fork` / `GetContext` / `AsyncFail`) seeded in [`src/syntax/builtin_effects.rs`](../../src/syntax/builtin_effects.rs); 5 new fiber `CorePrimOp` variants (`fiber_suspend` / `fiber_fork` / `fiber_get_context` / `fiber_run_async` / `fiber_yield_now` / `fiber_sleep`) wired through Core / VM dispatch / LLVM lowering; [`src/runtime/async/fiber.rs`](../../src/runtime/async/fiber.rs) + [`src/runtime/async/scheduler.rs`](../../src/runtime/async/scheduler.rs) provide the `FiberScheduler` layer on top of 1a's `TaskManager`; [`lib/Flow/Async.flx`](../../lib/Flow/Async.flx) ships the structured-concurrency primitives (`scope` / `fork` / `both` / `race` / `timeout` / `timeout_result` / `bracket` / `finally` / `try_` / `fail` / `yield_now` / `sleep`); [`lib/Flow/Tcp.flx`](../../lib/Flow/Tcp.flx) ships the TCP wrapper surface (`connect` / `read` / `write` / `close` / `with_connection`) backed by [`runtime/c/tcp.c`](../../runtime/c/tcp.c) shims. 7 small parity fixtures under [`tests/parity/`](../../tests/parity/) validate the source-surface contract on VM and LLVM. |
| 1b-vi-a — `Async.sleep` through mio | ✅ | [`src/vm/core_dispatch.rs`](../../src/vm/core_dispatch.rs) `vm_async` thread-local lazily starts a process-global [`MioBackend`](../../src/runtime/async/backends/mio.rs); `FiberSleep` registers a one-shot timer, pumps `next_completion()` until it fires. With one fiber the OS-thread call stack is the continuation; multi-fiber suspend lands in b₂. Acid test: [`tests/integration/vm_fiber_sleep.rs`](../../tests/integration/vm_fiber_sleep.rs). |
| 1b-vi-b₁ — Fiber registry plumbing | ✅ | Every `Async.run_async` boundary lazy-inits a `FiberScheduler` (depth-counted for nesting); `FiberFork` allocates real `FiberId`s via `scheduler.spawn`; `vm_fibers::with_current` tracks the active fiber id. Execution still inline-sequential (no behaviour change) — the registry shape unblocks b₂. Tests: [`vm_fiber_registry.rs`](../../tests/integration/vm_fiber_registry.rs). |
| 1b-vi-b₂.1 — Single-fiber park/resume cycle | ✅ | `FiberSleep` now captures its continuation back to the `FiberRunAsync` boundary (via `capture_to_boundary`, extracted from `OpPerform`'s unwind loop), signals park, and is resumed by a dispatch loop in `vm_fibers::dispatch_loop` when the mio completion arrives. `RuntimeContext` gains `current_frame_index` / `current_sp` / `capture_to_fiber_boundary` / `resume_from_dispatch`. Single-fiber timing unchanged (same OS-thread block) — proves the cycle without behaviour change. Tests: [`vm_fiber_sleep.rs`](../../tests/integration/vm_fiber_sleep.rs) (now exercises the new path). |
| 1b-vi-b₂.2 — Concurrent `both` / `race` | ✅ | New `CorePrimOp::FiberBoth = 172` and `FiberRace = 173`. Both spawn two child fibers, park the parent on a synthetic await request id, and the dispatch loop's `on_fiber_done` walks an awaiter index to assemble resume values (tuple for `both`, winner for `race`) before calling `scheduler.complete` on the parent's request. Per-fiber `last_completion_req` lets the dispatch loop look up the right resume value before resuming each parked continuation. Race loser cancellation now lands through the 1b-vi-c scheduler/backend cancel path. Acid test: [`vm_fiber_overlap.rs`](../../tests/integration/vm_fiber_overlap.rs) — `both(sleep(500), sleep(500))` ≈ 500ms, `race(sleep(1000), sleep(50))` returns the fast result in ~50ms. |
| 1b-vi-timeout — `Async.timeout` real overlap | ✅ | New `CorePrimOp::FiberTimeout = 174`. Body fiber and a backend timer share a single request id; whichever fires first wins. `AwaitKind::Timeout` + `try_route_timer_for_timeout` set the parent's resume value to `None` if the timer routes through the dispatch loop's pump first; the body branch's `on_fiber_done` wraps its result in `Value::Some` if it gets there first. Acid test: [`vm_fiber_timeout.rs`](../../tests/integration/vm_fiber_timeout.rs) — `timeout(50, sleep_1000)` → `None` in ~50ms; `timeout(1000, sleep_50)` → `Some(42)` in ~50ms. |
| 1b-vi-c — Multi-worker + cancellation | ✅ complete for current scope | VM and native schedulers now cancel race losers, timeout losers, and scoped children at scheduler/backend-request boundaries. Native `scope`/`fork`/`cancel` are Rust scheduler-owned rather than pthread/no-op shims. `FiberScheduler` has logical per-worker ready queues, round-robin child-fiber assignment, and request-id based completion routing back to the parked fiber's home worker. Native `run_async` owns shared scheduler state, runs non-root logical workers on OS threads, promotes values that cross worker boundaries, and wakes home-worker queues through a condition variable. The native generated-code execution lock is gone: C effect/yield state remains TLS, allocation stats are atomic, and worker-thread allocation bypasses the process-global bump arena before touching bump pointers while the root thread keeps the fast path. Native cancellation is cooperative for already-running fibers: cancelled fiber ids and backend request ids are suppressed at execution, park, completion, and backend-completion boundaries, so race losers, timeout bodies, and scoped children cannot resume parents after cancellation. FIFO race ties keep the left/current-worker child first. VM dispatch intentionally remains logical-only on the `run_async` OS thread for Phase 1b because VM continuations carry `Rc<Value>`; real VM OS-worker dispatch is tracked as a post-1b follow-up after the VM value-promotion/thread-safety design. |
| 1b-vi-d — Native (LLVM) fiber suspend/resume | ✅ core async complete | Native `flux_fiber_run_async` enters a Rust-owned native async scheduler loop via C ABI shims. `flux_fiber_sleep` starts a Rust/mio timer and suspends instead of blocking. `flux_fiber_both`, `flux_fiber_race`, and `flux_fiber_timeout` enqueue child fibers under Rust scheduler state and resume parents with VM-observable semantics. Direct call-site yield propagation is implemented for transitive direct async calls; indirect async closure yield propagation is implemented conservatively for functions whose effect row proves `Async`. Native `scope`/`fork`/`cancel` allocate Rust-tracked scopes and cancel pending scoped child fibers. Tests: [`native_async_sleep_tests.rs`](../../tests/native_llvm/native_async_sleep_tests.rs) and [`native_async_indirect_tests.rs`](../../tests/native_llvm/native_async_indirect_tests.rs). **Not included:** indirect calls that lack `with Async` evidence and VM-side OS-thread fiber workers. |
| 1b-vi-e — Real TCP parity | ✅ VM + LLVM/native | [`tests/parity/tcp_listen_accept.flx`](../../tests/parity/tcp_listen_accept.flx) and [`tests/parity/tcp_connect_write_read.flx`](../../tests/parity/tcp_connect_write_read.flx) now run as `// parity: vm, llvm`. VM continues to route TCP through the fiber scheduler and mio reactor. LLVM/native `runtime/c/tcp.c` is now a thin fiber-suspending wrapper layer: it decodes Flux `String`/`Int` values, copies host/data into Rust-owned buffers, submits `connect`, `listen`, `accept`, `read`, and `write_all` to the Rust `MioBackend`, and suspends on the returned request id. Native completion routing converts `TcpHandle` to tagged `Int`, `Bytes` to Flux `String`, and `Unit`/errors to `None`; `close` remains synchronous. Windows bypasses the old C TCP stub path through this Rust backend bridge. Richer TCP `AsyncError` values remain a later cleanup. |
| **Phase 2** — Concurrency closeout + runtime gaps | ⏳ | |
| 2-i — Real fiber-suspending `Task.await` | ⏳ | Native `flux_task_await` publishes a scheduler completion instead of parking the OS worker on a condvar; VM `Task.await` integrates with `FiberScheduler`. Fixes the Phase 1b closeout footnote. |
| 2-ii — N-way `race` / `first` / `first_of` | ✅ | [`CorePrimOp::FiberFirstOf`](../../src/core/mod.rs) plus VM and native scheduler await records spawn a list of async thunks under one parent, resume with `(winning_index, value)`, cancel all losers, and preserve source-order FIFO ties. [`lib/Flow/Async.flx`](../../lib/Flow/Async.flx) exposes `first_of` and `first`; coverage: [`vm_fiber_first_of.rs`](../../tests/integration/vm_fiber_first_of.rs), [`native_async_sleep_tests.rs`](../../tests/native_llvm/native_async_sleep_tests.rs), and [`async_first_of.flx`](../../tests/parity/async_first_of.flx). |
| 2-iii — `Flow.Channel` decision | ✅ | Decision: `Flow.Channel` is **deferred to a follow-on proposal**. Phase 2-4 cross-worker communication uses `Task.spawn` / `Task.await` only. Stale `Channel.send` references in the Sendable section reworded to mention `Flow.Channel` only as a deferred future; the illustrative `module Flow.Channel { ... }` block in the Sendable example is flagged as non-deliverable. |
| 2-iv — Cancellation observation in pure loops | ✅ | New `CorePrimOp::FiberCheckCancelled = 178` with VM dispatch in [`src/vm/core_dispatch.rs`](../../src/vm/core_dispatch.rs); `flux_fiber_check_cancelled` C shim ([`runtime/c/tasks.c`](../../runtime/c/tasks.c)) over `flux_async_check_cancelled` extern in [`src/runtime/async/native_abi.rs`](../../src/runtime/async/native_abi.rs); LLVM emit-name in [`src/lir/emit_llvm.rs`](../../src/lir/emit_llvm.rs). Per-thread `CANCELLED_IDS: HashSet<FiberId>` in `vm_fibers` mirrors the scheduler's cancel set so a *currently executing* fiber can observe its scope's cancellation, not just suspended fibers. Library: `Async.check_cancelled() -> Bool with Async` plus convenience `Async.bail_if_cancelled()` that calls `Async.fail(Canceled)` (ergonomic shim; becomes a real catchable raise once slice 2-vi lands). **Signature deviation from the proposal text**: ships as `-> Bool` not `-> Unit`-with-raise because real raise machinery is slice 2-vi territory; helper covers the raise idiom. Tests: [`tests/integration/vm_fiber_check_cancelled.rs`](../../tests/integration/vm_fiber_check_cancelled.rs) (no-cancel + timeout-cancellation cases); [`tests/parity/async_check_cancelled_false_when_not_cancelled.flx`](../../tests/parity/async_check_cancelled_false_when_not_cancelled.flx) (vm/llvm). `yield_now` cancellation-point retrofit deferred to 2-vi (today still a no-op). |
| 2-v — `Http.serve` production-knobs design | ✅ | API spec landed in the Phase 3 HTTP section: `ServerConfig` with `max_connections` / `max_header_bytes` / `max_body_bytes` / `request_timeout_ms` / `worker_count`, `ServerHandle`, `default_config()`, `serve_config(addr, port, config, handler) -> ServerHandle`, `shutdown(h)` (graceful drain) and `shutdown_now(h)` (cancellation). Knob enforcement contract spelled out. Phase 3 implements against this signature. |
| 2-vi — Fiber panic semantics | ⏳ | Worker catches fiber panics, converts to `AsyncError.Panicked`, propagates up the structured-concurrency tree, observable through `try_`. Workers do not poison. |
| 2-vii — Runtime config knobs | ✅ | New `CorePrimOp::FiberRunAsyncWith = 179` (arity 4: workers, fs, dns, action) wired through VM dispatch, native ABI (`flux_async_run_root_with`), C shim (`flux_fiber_run_async_with`), and LLVM emit. Library: `data RuntimeConfig { worker_count: Option<Int>, fs_pool_size: Int, dns_pool_size: Int }` + `default_runtime_config()` + `with_worker_count(n)` builder + `run_async_with(cfg, action)`. `FLUX_WORKERS` env-var fallback parsed once on the VM via `OnceLock`. **Native runtime currently ignores `worker_count`** (still uses compile-time `LOGICAL_WORKERS = 2`); the extern accepts the value for API parity, full native runtime-config support is a follow-up. `fs_pool_size` and `dns_pool_size` are accepted today but only consulted once Phase 2 slice 2-viii lands the blocking pool. Tests in [`tests/integration/vm_runtime_config.rs`](../../tests/integration/vm_runtime_config.rs). |
| 2-viii — Blocking pool + DNS resolver | ⏳ | New `src/runtime/async/blocking_pool.rs`, `AsyncBackend::dns_resolve`, `CompletionPayload::AddressList` (already declared but unimplemented), `flux_async_dns_resolve` native ABI, hostname path in `lib/Flow/Tcp.flx`. |
| 2-ix — Transparent type aliases | ⏳ | Extend `alias Name = ...` to accept ordinary type expressions, not only effect rows. Detailed spec in [Required language features](#required-language-features). Unblocks `alias Stream<a> = () -> Option<a> with Async`. |
| 2-x — `Sendable` ADT auto-derivation | ✅ | Closed under closer audit: `synthesize_sendable_instances` in [`src/types/class_env.rs`](../../src/types/class_env.rs) already walks every `data` declaration, skips ADTs with function-typed fields, generates `instance <a: Sendable, b: Sendable> => Sendable<Foo<a, b>>` for parameterized ADTs, and is invoked from `register_user_classes`. Verified by [`tests/type_inference/sendable_tests.rs`](../../tests/type_inference/sendable_tests.rs) — 10 tests cover monomorphic ADTs, parameterized ADTs (positive and negative), function-field ADTs (rejected), recursive ADTs, and bare function types (rejected). The slice's only output was adding the recursive-ADT regression test that was missing from the original suite. |
| **Phase 3** — HTTP/1.1 + JSON + Streams | ⏳ | |
| **Phase 4** — TLS + database client | ⏳ | |
| **Phase 5** — `io_uring` backend (optional) | ⏳ | |

Current green bar after Phase 1b closeout: `cargo check --features llvm`, focused native async tests, VM fiber/TCP/task integration tests, `MioBackend` TCP unit tests, both TCP parity fixtures under `--ways vm,llvm`, full `cargo run --features llvm -- parity-check tests/parity`, and `cargo test --all --all-features` pass. Phase 2 (runtime gaps) is now unblocked.

## Relationship to 0143

[Proposal 0143](0143_actor_concurrency_roadmap.md) specifies an
Erlang-style actor concurrency roadmap (isolated heaps, typed mailboxes,
supervision, deterministic test scheduler). 0143 and this proposal model
two complementary layers, not competing alternatives:

- **0174 owns the I/O layer.** `Async` effect, Rust reactor backend, structured
  concurrency, HTTP, JSON, streams, TLS, database client. The runtime
  story for "one program doing many concurrent I/O operations."
- **0143 owns the isolation/reliability layer, built on top of 0174.**
  Actors as a userspace pattern over Phase 1a's worker thread pool plus
  `Sendable<T>` (Phase 1a), with 0143's typed-mailbox and supervision
  designs preserved as the type-system and library shape.

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
streams** — points at the I/O-layer story. 0174 Phase 1a+1b+2+3 ships a
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
Phases 1a, 1b, 2, 3, and 4.

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
user-facing syntax. The prerequisite language work was originally
intended to land alongside Phase 1b but did not; revision 9 of this
proposal moves it into the new Phase 2 (runtime gaps) — see slice
2-ix. The ergonomic items are documented here for future re-evaluation.

#### Prerequisite: transparent aliases (Phase 2 slice 2-ix)

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

Estimated effort: 1-2 weeks. The work lands as Phase 2 slice 2-ix
(originally scheduled as Phase 1b preparation; rescheduled at
revision 9 once Phase 1b shipped without it). Phase 3's
`lib/Flow/Stream.flx` and any future TCP/UDP/DNS/timer/signal/fs
wrapper use aliases from day one.

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
(today: `Task.spawn` / `Task.await`; future: actor/process sends and
the deferred `Flow.Channel` primitive), the runtime chooses one transfer
strategy:

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
continue. Phase 1b exposes `Task.await` as the async-surface join. In the
current closeout it is sequential/blocking-equivalent on VM and native; a true
fiber-suspending wait remains a post-1b scheduler-completion integration.
This keeps CPU-bound task parallelism distinct from fiber-level async I/O
without changing the public source API later.

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
- **TLS:** deferred to Phase 4 and driven by Rust `rustls` state machines over
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
    TlsError(Int, String),                    // Phase 4
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
The compile-time check at `Task.spawn` / `Task.await` boundaries (and
any future cross-worker boundary, including the deferred `Flow.Channel`)
refuses cross-worker sharing.

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

// Illustrative only — Flow.Channel is not a Phase 2/3/4 deliverable.
// Deferred to a follow-on proposal per Phase 2 slice 2-iii. Shown here
// to motivate Sendable's purpose at cross-worker boundaries.
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
are Phase 3/4 features; the example shows how Phase 1b's primitives
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
2. **`deriving` for codec derivation.** Phase 3 attaches `deriving
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
- **No streaming yet at this layer.** `Stream<a>` arrives in Phase 3; Phase 1b is one-shot operations only.

### Phase 2: Concurrency closeout + runtime gaps

Phase 2 closes the concurrency-design questions that Phase 1b left
under-specified, then lands the three runtime prerequisites that the
original Phase 1a/1b plan listed but did not actually land. No
user-facing API regressions; the surface that exists today keeps
working. The phase is structured as ten independent slices —
2-i through 2-vii pin down concurrency semantics that Phase 3 (HTTP)
would otherwise have to invent on the fly, and 2-viii through 2-x land
the missing runtime infrastructure (DNS, transparent type aliases,
`Sendable` ADT derive). Each slice has its own acceptance test.

#### 2-i — Real fiber-suspending `Task.await`

`Task.await` ships with the right type but the wrong semantics today.
[`runtime/c/tasks.c`](../../runtime/c/tasks.c) `flux_task_await` is a
blocking join: it parks the calling **OS worker** on a condvar until
the task finishes, which means every other fiber on that worker is
stalled too. For Phase 3 HTTP this is a real foot-gun — a request
handler that does `Task.spawn` for a heavy compute step and then
`Task.await`s its result will silently halt the server's other
in-flight connections that happen to share that worker.

```flux
import Flow.Async exposing (..)
import Flow.Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

// `compute` is a CPU-bound task; `tick` is a fiber doing periodic work.
// With today's blocking-shim Task.await, while `compute` runs, `tick`
// will not advance because the worker is parked. With real fiber-
// suspending await, both observe wall-clock progress.
fn body() -> Int with Async {
    both(
        fn() { Task.await(Task.spawn(fn() { fib(40) })) },
        fn() {
            sleep(10)
            sleep(10)
            sleep(10)
            42
        },
    ).0
}

fn main() with IO { let _ = run_async(body) }
```

Slice 2-i wires `Task.await` to the fiber scheduler instead of to a
condvar:

- Native: extend `runtime/c/tasks.c` so the task-completion path
  publishes a `Completion` record into the scheduler's completion
  channel keyed by the task's request id, rather than only signalling
  the per-task condvar. Existing blocking-thread / Win32 task workers
  remain.
- Both backends: `Task.await(t)` desugars to a fiber suspend on the
  task's request id; resume happens on the awaiting fiber's home
  worker when the completion arrives.
- `Task.blocking_join` keeps its current behaviour and remains the
  escape hatch for non-`Async` callers.

Acceptance: a parity test that observes wall-clock overlap between a
`Task.spawn(cpu_loop)` and a sibling `Async.sleep` running under
`Async.both`, passing on `vm` and `llvm`. Today both run sequentially
because the worker is parked; after this slice, total wall-clock
should be `max(cpu_loop, sleep)` not `cpu_loop + sleep`.

#### 2-ii — N-way `race` / `first` / `first_of`

The proposal specifies binary `race(f, g)` (above, in the Phase 1b
structured-concurrency primitives). Production code regularly wants
N-way "first ready": "accept loop OR shutdown signal", "any of N
upstream replicas," "wait for one of M user inputs." Recursive `race`
chains work but are awkward and lose information about which fiber
won.

Slice 2-ii adds:

```flux
public fn first<a>(fs: List<() -> a with Async>) -> a with Async
public fn first_of<a>(fs: List<() -> a with Async>)
    -> (Int, a) with Async   // returns (winning index, result)
```

Implementation landed as a dedicated `FiberFirstOf` scheduler primitive,
not as recursive binary `race` wrappers. VM and native paths decode the
Flux list into indexed child closures, spawn all children under one await
record, resume the parent with `(winning index, value)` from the first
completed child, and cancel every non-winning child through the same
scheduler-owned cancellation path used by binary `race`.

Tie handling matches native binary race: source order wins for immediate
children. A later completed child is deferred only while an earlier sibling
is still queued/ready; already-running or suspended earlier siblings do not
block the later completion.

Acceptance: [`tests/parity/async_first_of.flx`](../../tests/parity/async_first_of.flx)
checks `first`, `first_of`, and immediate source-order ties on VM and
LLVM/native. [`tests/integration/vm_fiber_first_of.rs`](../../tests/integration/vm_fiber_first_of.rs)
and [`tests/native_llvm/native_async_sleep_tests.rs`](../../tests/native_llvm/native_async_sleep_tests.rs)
cover fastest-index return and loser cancellation.

#### 2-iii — `Flow.Channel` decision

Above (in the Phase 1a `Sendable<T>` cross-thread RC discussion) the
proposal says "At every explicit cross-worker boundary
(`Channel.send`, `Task.spawn`, ...)" — but `Flow.Channel` is not
defined anywhere in the proposal, has no surface, and is not in any
deliverable list. Either Channel was intended as a real primitive and
got dropped between revisions, or `Task` is the only cross-worker
mechanism and the `Channel.send` reference is stale.

Slice 2-iii makes the decision explicit. Recommended: **delete the
stale reference for now and defer `Flow.Channel` to a follow-on
proposal.** Phase 3 HTTP does not need cross-fiber message passing —
each request is a single fiber that owns its connection. Cross-worker
communication for actor-style designs is properly the domain of
proposal 0143.

If a Channel primitive turns out to be needed for Phase 3 streams or
Phase 4 database pools, it lands as a separate slice with its own
design (one-shot vs MPSC, bounded vs unbounded, `Sendable<T>`
constraint, cancellation semantics on send/recv).

Acceptance: the §597 stale `Channel.send` reference is reworded to
`Task.spawn` only; a one-paragraph note in the proposal documents the
deferral.

#### 2-iv — Cancellation observation in pure loops

Cancellation today is delivered through `await` points: when a fiber
resumes from a backend completion under a cancelled scope, it raises
`AsyncError.Canceled`. A fiber in a long pure compute loop between
`await`s has no way to *check* whether its scope was cancelled and
exit early; the only documented mitigation (above, in Drawbacks) is
"`Async.yield()` for long pure loops," but the proposal does not
state that `Async.yield_now` is itself a cancellation point.

```flux
import Flow.Async exposing (..)

// Long pure loop with cooperative cancellation. Without slice 2-iv
// this loop runs to completion even if its scope is cancelled.
fn search(target: Int) -> Option<Int> with Async {
    fn go(i: Int) -> Option<Int> with Async {
        check_cancelled()                       // <-- new in slice 2-iv
        if i > 1_000_000 { None }
        else if expensive_pure_pred(i, target) { Some(i) }
        else { go(i + 1) }
    }
    go(0)
}

fn body() -> Option<Int> with Async {
    // Outer scope: 100ms budget. After timeout, search must observe
    // cancellation at the next check_cancelled() and return.
    timeout(100, fn() { search(0) }).flatten()
}
```

Slice 2-iv adds (as landed in revision 9 + post-revision-9 implementation):

- `Async.check_cancelled() -> Bool with Async` — returns `true` iff the
  current fiber's enclosing scope has been cancelled. No backend
  round-trip, no suspend, just a scheduler flag check.
- `Async.bail_if_cancelled() -> Unit with Async` — convenience wrapper
  for `if check_cancelled() { fail(Canceled) }`. Until slice 2-vi
  makes `Async.fail` a real catchable raise, this aborts the fiber via
  the existing soft-cancel path.
- New `CorePrimOp::FiberCheckCancelled = 178` wired through VM dispatch
  in [`src/vm/core_dispatch.rs`](../../src/vm/core_dispatch.rs), the
  C shim `flux_fiber_check_cancelled` in
  [`runtime/c/tasks.c`](../../runtime/c/tasks.c), and the Rust extern
  `flux_async_check_cancelled` in
  [`src/runtime/async/native_abi.rs`](../../src/runtime/async/native_abi.rs).
- A per-thread `CANCELLED_IDS: HashSet<FiberId>` set in `vm_fibers`
  tracks fibers whose enclosing scope was cancelled, populated by
  `cancel_losers` and queryable from a *currently executing* fiber.
  This is necessary because the scheduler's `cancel_fibers` only
  marks suspended fibers in its `suspended` map; a fiber executing
  inline cannot consult its own state through that path.

**Signature deviation from the original slice spec.** The proposal
revision 9 drafted `check_cancelled() -> Unit with Async` that raises
`AsyncError.Canceled`. In the codebase, raising a catchable async
error requires unwind machinery that does not yet exist:
`CorePrimOp::FiberFail` is currently a stub that returns an `Err`
string from VM dispatch, not a recoverable raise. Real async-error
raise is slice 2-vi (fiber panic semantics) territory — both need the
same path. To keep slice 2-iv self-contained, the primitive ships as
`-> Bool` and the raising idiom is provided as `bail_if_cancelled`.
Once slice 2-vi lands, `bail_if_cancelled` becomes a real catchable
raise and the proposal's original "raise on cancel" semantics holds.

`Async.yield_now()` retrofit as a cancellation point is **deferred to
slice 2-vi** for the same reason: making it raise requires the unwind
machinery. Today `yield_now` is a no-op on the VM sequential path.

Acceptance:

- `tests/integration/vm_fiber_check_cancelled.rs` — `check_cancelled`
  returns `false` for a non-cancelled fiber, and `true` after
  `timeout(20, body)` cancels a `body` that was suspended on
  `sleep(50)` and resumed by the dispatch loop's cancel path.
- `tests/parity/async_check_cancelled_false_when_not_cancelled.flx`
  — VM and LLVM/native produce identical output (false) for the
  no-cancel case.

#### 2-v — `Http.serve` production-knobs design

Phase 3's `Http.serve` surface as written today (see §1604 onward)
takes only `addr`, `port`, and `handler`. A production HTTP server
needs at minimum: connection limit, graceful shutdown signal,
per-connection timeout, max-header-size, max-body-size, optional
worker-count override. The 10k-connection acceptance load test
(carried over from Phase 1b) cannot run safely without at least a
connection cap.

This slice is **API design, not runtime work** — it pins down the
Phase 3 server signature before implementation. Recommended shape:

```flux
module Flow.Http {
    public data ServerConfig {
        ServerConfig {
            max_connections:    Int,           // default 10_000
            max_header_bytes:   Int,           // default 64 KiB
            max_body_bytes:     Int,           // default 8 MiB
            request_timeout_ms: Int,           // default 30_000
            worker_count:       Option<Int>,   // None => available_parallelism()
        }
    }

    public data ServerHandle { ServerHandle(Int) }

    public fn default_config() -> ServerConfig

    public fn serve_config<e>(
        addr:    String,
        port:    Int,
        config:  ServerConfig,
        handler: (Request) -> Response with <Async | e>,
    ) -> ServerHandle with <Async | e>

    public fn serve<e>(
        addr:    String,
        port:    Int,
        handler: (Request) -> Response with <Async | e>,
    ) -> ServerHandle with <Async | e>     // calls serve_config with default_config

    public fn shutdown(h: ServerHandle) -> Unit with Async       // graceful drain
    public fn shutdown_now(h: ServerHandle) -> Unit with Async   // cancel in-flight
}
```

Acceptance: this slice produces a written design document only — the
spec above lands in the Phase 3 section of the proposal.
Implementation is Phase 3 work.

#### 2-vi — Fiber panic semantics

Phase 1a documents that **task** worker panics are caught and surfaced
as `TaskJoinError::Panicked` (progress table 1a-vi). Phase 1b does not
specify what happens when a *fiber* panics — does it propagate up the
structured-concurrency tree? Get caught by `try_`? Take down the
entire `run_async` boundary? For Phase 3 HTTP, a panic in a request
handler **must** close that one connection only, not kill the server.

```flux
import Flow.Async exposing (..)

fn body() -> (Int, Result<Int, AsyncError>) with Async {
    both(
        fn() { 1 + 1 },                      // sibling: should still complete
        fn() {
            try_(fn() {
                panic("user code blew up")   // panic propagates as AsyncError
            })
            // try_ catches and returns Result<Int, AsyncError>::Err
        },
    )
}
```

Slice 2-vi pins down (and implements) these rules:

1. A fiber panic is caught by its home worker and converted to
   `AsyncError.Panicked(message)`.
2. The cancellation propagation rules already in place (above) deliver
   `AsyncError.Panicked` to the panicking fiber's scope: any siblings
   under the same `Async.scope` are cancelled (loser-cancellation
   semantics, same as `race`); cleanup in `Async.bracket`/`Async.finally`
   runs exactly once.
3. The panic re-raises at the nearest enclosing `Async.try_` (caught)
   or at the `Async.run_async` boundary (re-raised to the caller).
4. Workers do not poison: the worker that hosted the panic continues
   serving other fibers. Same property `TaskScheduler` already
   guarantees for tasks (1a-vi).

Acceptance: parity tests for each rule —
(a) `panic` inside `try_` is observable as `Err(Panicked(...))`,
(b) `panic` inside a `both` cancels the sibling and re-raises to the
parent,
(c) the post-panic `run_async` instance can submit and run further
work without restart.

#### 2-vii — Runtime config knobs

Today the worker count defaults to `available_parallelism()` (above,
in the Phase 1a worker pool description); `FLUX_FS_THREADS` is the
only documented env var, and it's mentioned only for the (still-
absent) FS pool. There's no centralised place a user can tune
worker count, blocking-pool sizes, or DNS-pool size without reading
source.

Slice 2-vii adds a single `RuntimeConfig` struct passed at
`Async.run_async` setup time, plus matching env-var fallbacks:

```flux
module Flow.Async {
    public data RuntimeConfig {
        RuntimeConfig {
            worker_count:    Option<Int>,   // None => available_parallelism()
            fs_pool_size:    Int,           // 0 => min(4, available_parallelism)
            dns_pool_size:   Int,           // 0 => 4
        }
    }

    public fn default_runtime_config() -> RuntimeConfig
    public fn with_worker_count(n: Int) -> RuntimeConfig

    public fn run_async_with<a>(
        cfg: RuntimeConfig,
        action: () -> a with Async,
    ) -> a
}
```

`Async.run_async(action)` keeps its current zero-config signature.
Env-var fallbacks: `FLUX_WORKERS`, `FLUX_FS_THREADS`, `FLUX_DNS_THREADS`
— parsed once via `OnceLock` and consulted only when the corresponding
`RuntimeConfig` field is the default sentinel (`None` for worker_count,
`0` for the pool sizes). An explicit `RuntimeConfig` always wins.

Wire path: `run_async_with` desugars `cfg` to four primop arguments
(workers, fs, dns, action) for `CorePrimOp::FiberRunAsyncWith = 179`.
The VM dispatch handler stores the knobs in a thread-local
`PendingRunConfig` that `enter_run_async` consults before constructing
the `FiberScheduler`. Native side calls `flux_async_run_root_with`
which currently forwards to `flux_async_run_root` and ignores the
worker count (full native config support is a follow-up; the surface
exists today so user code targeting `run_async_with` continues to
link and run on native).

Implementation note on the constructor surface: directly writing
`Async.RuntimeConfig { worker_count: ..., ... }` at a call site
currently runs into Flux's "could not infer concrete type" diagnostic
unless the type is otherwise constrained, and the bare `RuntimeConfig`
form is not brought into scope by `import ... exposing (..)` at the
time of writing. The `with_worker_count` builder side-steps the
problem for the common case; richer construction stays on hand via
`default_runtime_config` and direct field destructuring inside
library code. This is a Flux-source ergonomic gap, not a
slice-2-vii design choice — once the language support catches up,
the call-site form becomes available without library changes.

Acceptance landed: [`tests/integration/vm_runtime_config.rs`](../../tests/integration/vm_runtime_config.rs)
covers (1) explicit `with_worker_count(1)` returning the body's
value; (2) `default_runtime_config()` returning the body's value;
(3) `FLUX_WORKERS=1` env var not breaking the existing zero-config
`run_async`. Direct introspection of "exactly one worker thread was
started" is exposed through the in-process helper
`vm_fibers::current_num_workers()`; reachable by Rust unit tests but
not from a CLI-driven integration fixture.

#### 2-viii — Blocking pool + DNS resolver

Phase 1a's deliverable list (above) included a `blocking_pool.rs` with a
DNS resolver pool, but the file was never created and `tcp_connect`
today only accepts numeric IPs. Concretely,
[`src/runtime/async/native_abi.rs`](../../src/runtime/async/native_abi.rs)'s
`socket_addr_from_raw` calls `format!("{host}:{port}").parse().ok()` —
that is `SocketAddr::from_str`, which rejects hostnames. Hostname
connects therefore silently no-op. Phase 3's `Flow.Http.get(url)` is
unusable without DNS.

Slice 2-viii adds:

- `src/runtime/async/blocking_pool.rs`: a small thread pool sized
  `min(4, std::thread::available_parallelism())` that runs blocking
  service work and posts its results into the same completion channel
  the `mio` reactor uses. Used initially for DNS; reusable for future
  filesystem operations and any other service `mio` does not provide.
- New `AsyncBackend` method `dns_resolve(req: RequestId, host: String, port: u16)`
  and a use of the existing `CompletionPayload::AddressList(Vec<SocketAddr>)`
  variant declared in [`backend.rs`](../../src/runtime/async/backend.rs)
  but not yet implemented.
- `flux_async_dns_resolve` extern shim in
  [`native_abi.rs`](../../src/runtime/async/native_abi.rs). The
  implementation uses `std::net::ToSocketAddrs` on a blocking-pool
  worker.
- `lib/Flow/Tcp.flx` `connect(host, port)` first parses the host as a
  numeric IP; on failure it submits a DNS resolution and suspends until
  the address-list completion arrives, then routes to `tcp_connect_prim`.

Acceptance: a parity test `tests/parity/tcp_connect_hostname.flx`
connects to `"localhost"` and round-trips bytes, passing on `vm` and
`llvm`.

#### 2-ix — Transparent type aliases

Detailed specification in [Required language features](#required-language-features)
above (originally written for Phase 1b prep). Implementation summary,
restated here for the Phase 2 work-plan:

- Parser: extend `parse_alias_statement` in
  [`src/syntax/parser/statement.rs`](../../src/syntax/parser/statement.rs) —
  when the RHS does not start with `<`, parse a `TypeExpr`.
- AST: new variant `Statement::TypeAlias { is_public, name, params, body, span }`
  in [`src/syntax/statement.rs`](../../src/syntax/statement.rs) (today
  only `Statement::EffectAlias` exists).
- Name resolution: per-module transparent-alias table populated
  alongside the existing ADT, effect, and effect-alias tables.
- Type expansion: extend the existing substitution path to detect alias
  references and expand them, with a recursion-depth cap of 64.
- Cycle detection: emit E308 on direct or indirect self-reference.
- Restrictions enforced at the initial slice: no recursive aliases, no
  phantom type parameters, no constraints on alias parameters, no HKT
  aliases, no `deriving` on alias declarations.

Acceptance: parser tests for `alias Stream<a> = () -> Option<a> with Async`
and `alias AsyncFn<a, b, e> = (a) -> b with <Async | e>`, plus a
unifier round-trip showing an alias and its expansion are
interchangeable in function signatures, instance positions, and
pattern positions.

#### 2-x — `Sendable` ADT auto-derivation

Today, [`src/types/class_solver.rs`](../../src/types/class_solver.rs)'s
`has_structural_builtin_instance` auto-derives `Sendable` (and `Eq`/`Ord`)
for `Tuple`, `Option`, `List`, `Array`, `Map`, and `Either` only;
user-defined ADTs require an explicit `instance Sendable<Foo> {}`. The
proposal's stated rule (above, in the Phase 1a `Sendable<T>` section)
is "ADTs whose every field is `Sendable`" — recursive structural
derivation. Slice 2-x closes this gap.

- Extend `has_structural_builtin_instance` (or a new sibling) to handle
  user ADT applications: look up the data declaration, walk every
  constructor's field types, recursively check each is `Sendable`.
- Reuse the existing `seen: &mut HashSet<String>` cycle guard to handle
  recursive ADTs.
- Opaque runtime handles such as `Tcp.Connection` and `Tcp.Listener`
  remain non-`Sendable`: their data declarations either expose no
  user-readable fields the solver can inspect or wrap a runtime-only
  primitive whose type lacks a `Sendable` instance.

Acceptance: `tests/type_inference/sendable_tests.rs` adds positive
cases (a user `data Pair { Pair(Int, String) }` satisfies
`Sendable<Pair>` without an explicit instance; a recursive
`data Tree { Leaf | Node(Int, Tree, Tree) }` satisfies
`Sendable<Tree>`) and negative cases
(`data Bad { Bad(Connection) }` fails with E440).

#### Phase 2 deliverables

- `src/runtime/async/blocking_pool.rs` — ~150 lines Rust.
**Concurrency closeout (slices 2-i through 2-vii):**

- `runtime/c/tasks.c` and the native task path — `flux_task_await`
  publishes a scheduler completion record instead of parking the OS
  worker on a condvar; matching VM-side `Task.await` integration with
  the fiber scheduler (slice 2-i).
- `lib/Flow/Async.flx` — new `first<a>` and `first_of<a>` over
  `List<() -> a with Async>` (slice 2-ii); new `check_cancelled() with Async`
  and clarified-as-cancellation-point `yield_now()` (slice 2-iv); new
  `RuntimeConfig` data type plus `default_runtime_config()` and
  `run_async_with(cfg, action)` (slice 2-vii).
- Proposal text only (slice 2-iii): `Flow.Channel` is **deferred** to
  a follow-on proposal; the §597 stale `Channel.send` reference is
  reworded.
- Proposal text and `lib/Flow/Http.flx` skeleton (slice 2-v): pinned
  `ServerConfig` / `ServerHandle` / `serve_config` / `shutdown` /
  `shutdown_now` signatures for Phase 3 to implement.
- `src/runtime/async/scheduler.rs` and `fiber.rs` — fiber panic catch
  on the home worker, conversion to `AsyncError.Panicked`, propagation
  through `Async.scope` and `Async.try_` (slice 2-vi).
- Native ABI: small `flux_async_check_cancelled` shim and new
  `flux_async_run_async_with` entry that accepts a config struct.

**Runtime gaps (slices 2-viii through 2-x):**

- `src/runtime/async/blocking_pool.rs` — ~150 lines Rust.
- `src/runtime/async/backend.rs` and `backends/mio.rs` — `dns_resolve`
  surface and routing.
- `src/runtime/async/native_abi.rs` — `flux_async_dns_resolve` shim.
- `lib/Flow/Tcp.flx` — hostname-aware `connect`.
- Parser/AST/name-res/type-expansion code paths for transparent type
  aliases (~1-2 weeks per the original estimate).
- `src/types/class_solver.rs` — recursive `Sendable` derivation for
  user ADTs.

**Test surface:**

- New parity tests for slices 2-i, 2-ii, 2-iv, 2-vi, 2-viii, 2-ix.
- New type-inference tests for slices 2-ix and 2-x.
- New unit tests for slices 2-i (task scheduler completion routing),
  2-vi (worker non-poisoning under fiber panic), 2-vii (`RuntimeConfig`
  honoured, env-var fallbacks).
- `cargo test --all --all-features` and `cargo clippy --all-targets --all-features -- -D warnings`
  remain green; the Phase 1b green-bar (per
  [`scripts/release/release_check.sh`](../../scripts/release/release_check.sh))
  remains green.

#### Phase 2 explicit non-goals

These are deferred past Phase 2 / past Phase 3 and tracked separately:

- **VM cross-worker fiber dispatch.** The VM `FiberScheduler` remains
  logical-only on the `run_async` OS thread because VM `Value` carries
  `Rc<Value>` and a thread-safety design has not been done. Phase 3's
  HTTP server is therefore documented as multi-worker on native and
  single-worker on VM. Lifting this restriction needs its own design
  slice and is not bundled here.
- **10k-connection scale demo.** Originally a Phase 1b acceptance
  bullet, deferred to Phase 3's HTTP server acceptance — the load test
  is HTTP-shaped, not runtime-shaped.

### Phase 3: HTTP/1.1 + JSON + Streams

#### HTTP

Scratch-built HTTP/1.1 parser in Rust under `src/runtime/http/`, peer
to `src/runtime/async/`, on top of the existing `mio` TCP substrate.
**No third-party HTTP library and no `vendor/` directory** — the
runtime is Rust-owned end-to-end, mirroring the design choice that
made the scheduler Rust rather than libuv. (Earlier revisions of this
proposal vendored llhttp; revision 9 retracts that decision — see the
revision history.)

The parser is server-first, client-second. Concrete scope:

- Request line / status line tokenizer.
- Header block parser. Case-insensitive name matching, OWS handling,
  no obsolete-fold support (RFC 7230 deprecated fold; reject on
  encounter).
- Body framing dispatch on `Content-Length` vs
  `Transfer-Encoding: chunked`. Reject on conflicting framing or
  multiple `Content-Length` values (request smuggling defense).
- Chunked transfer decoder.
- Response writer (server side) and request writer (client side).
- Keep-alive connection state machine on top of the existing
  `MioBackend` TCP `tcp_read` / `tcp_write` / `tcp_close` calls.

Conformance corpus ported from publicly-licensed sources (h2spec,
RFC 9112 §3-§7 examples). The HTTP parser is a service used by
`Flow.Http`, not the async backend; it owns no I/O — it is fed bytes
from `tcp_read` completions and emits parsed structures back to the
fiber that owns the connection.

The server surface is pinned by Phase 2 slice 2-v — `ServerConfig`,
`ServerHandle`, `serve_config`, and the two `shutdown` variants land
together with `serve` so production deployments do not have to wait
for a follow-up.

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

    /// Production knobs for `serve_config`. Defaults via `default_config()`.
    /// Per-knob defaults (locked by Phase 2 slice 2-v):
    ///   max_connections    = 10_000
    ///   max_header_bytes   = 65_536      // 64 KiB
    ///   max_body_bytes     = 8_388_608   // 8 MiB
    ///   request_timeout_ms = 30_000      // 30 s
    ///   worker_count       = None        // available_parallelism()
    public data ServerConfig {
        ServerConfig {
            max_connections:    Int,
            max_header_bytes:   Int,
            max_body_bytes:     Int,
            request_timeout_ms: Int,
            worker_count:       Option<Int>,
        }
    }

    /// Opaque handle for graceful or forced shutdown.
    public data ServerHandle { ServerHandle(Int) }

    public fn default_config() -> ServerConfig

    /// Start serving with explicit production knobs. Returns a handle the
    /// caller can use to drain or cancel. Errors from `bind` / `accept`
    /// surface as `AsyncError.IoError(...)`.
    public fn serve_config<e>(
        addr:    String,
        port:    Int,
        config:  ServerConfig,
        handler: (Request) -> Response with <Async | e>,
    ) -> ServerHandle with <Async | e>

    /// Start serving with `default_config()`. Equivalent to
    /// `serve_config(addr, port, default_config(), handler)`.
    public fn serve<e>(
        addr:    String,
        port:    Int,
        handler: (Request) -> Response with <Async | e>,
    ) -> ServerHandle with <Async | e>

    /// Stop accepting new connections; let in-flight requests finish or
    /// hit `request_timeout_ms`; close listening sockets. The fiber that
    /// owns the listener is parked until drain completes.
    public fn shutdown(h: ServerHandle) -> Unit with Async

    /// Stop accepting new connections; cancel every in-flight request
    /// fiber via the standard cancellation path; close all sockets. In-
    /// flight `Async.bracket`/`Async.finally` cleanups still run.
    public fn shutdown_now(h: ServerHandle) -> Unit with Async

    public fn get(url: String)              -> Response with Async
    public fn post(url: String, body: Bytes) -> Response with Async
    public fn request(
        method:  Method,
        url:     String,
        headers: Map<String, String>,
        body:    Bytes,
    ) -> Response with Async
}
```

**Knob enforcement (Phase 3 implementation contract):**

- `max_connections`: when the live-connection count reaches this limit,
  `accept` does not pull new sockets off the listener; the kernel's
  listen backlog provides the queue. Reaching the limit is not an
  error — it is back-pressure.
- `max_header_bytes` / `max_body_bytes`: parser rejects with
  `AsyncError.ProtocolError(413, ...)` (Payload Too Large) before any
  user handler is invoked.
- `request_timeout_ms`: each request fiber wraps the user handler in
  `Async.timeout(request_timeout_ms, ...)`. Expiry returns a 504
  Gateway Timeout to the client and cancels the handler via the
  standard scope-cancellation rules from Phase 2 slice 2-vi.
- `worker_count`: passed through to the underlying `Async.RuntimeConfig`
  (slice 2-vii). On VM, values > 1 are accepted but only worker 0
  runs fibers (slice 2-vii non-goal documents this).

`shutdown` vs `shutdown_now`: both are idempotent and safe to call
from any fiber. `shutdown` is the production default; `shutdown_now`
is for unit tests and emergency-stop scenarios. Calling either one
twice is a no-op; calling on an already-dropped handle is a no-op.

Keep-alive and chunked transfer supported. HTTP/2 deferred to a future
proposal (significant complexity for marginal Phase-3 gain).

#### JSON

Two parts, sequenced as separate sub-slices:

**Phase 3-Json-a (ship first):** `data Json { ... }`, parser, encoder,
and **manual** `Json.Encode` / `Json.Decode` instances written once
for primitives plus `Option`, `List`, and `Map`.

- `Flow.Json.parse: String -> Json` — tagged union value
  (`data Json { JsonNull, JsonBool(Bool), JsonNumber(Float), JsonString(String), JsonArray(Array<Json>), JsonObject(Map<String, Json>) }`).
- Pretty-printer `Flow.Json.encode: Json -> String`.
- `class Json.Encode<a>` and `class Json.Decode<a>` declarations plus
  hand-written instances for the primitives above.

This sub-slice is independent of any compiler-synthesis work and is
sufficient for the Phase 3 demos (hello-world microservice, JSON
echo, parallel HTTP fetch).

**Phase 3-Json-b (follow-on):** `deriving (Json.Encode, Json.Decode)`
codec method-body synthesis. Today the parser/elaboration registers
derived instances with no method bodies (see
[`src/types/class_env.rs`](../../src/types/class_env.rs)
`collect_deriving`); body synthesis lands in
[`src/core/passes/dict_elaborate.rs`](../../src/core/passes/dict_elaborate.rs)
or a new `derive_codec` pass. Per-record codecs aim for
zero-allocation when the type permits (ADTs with all flat fields).
This sub-slice carries its own design review because synthesised
method bodies are a new compiler capability, not a glue layer.

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

The transparent `alias Stream<a> = ...` form depends on Phase 2 slice
2-ix. Phase 3 cannot ship `Flow.Stream` until Phase 2-ix lands.

#### Phase 3 deliverables

- `lib/Flow/Http.flx`, `lib/Flow/Json.flx`, `lib/Flow/Stream.flx` — ~600 lines Flux total.
- `src/runtime/http/` — scratch-built HTTP/1.1 parser, response/request writer, keep-alive state machine. ~600-900 lines Rust plus tests. **No `vendor/` directory, no third-party HTTP parser dependency.**
- `src/core/passes/dict_elaborate.rs` (or a new `derive_codec` pass) — JSON codec body synthesis (Phase 3-Json-b).
- Examples: hello-world microservice, JSON echo, SSE broadcaster, parallel HTTP fetch.
- Documentation: HTTP server quickstart, JSON codec guide.
- Acceptance load test: 10k concurrent HTTP/1.1 keep-alive connections (carried over from the original Phase 1b acceptance, deferred there to a real HTTP workload).

Estimated effort: 6 weeks.

### Phase 4: TLS + database client

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

#### Phase 4 deliverables

- `lib/Flow/Tls.flx`, `lib/Flow/Postgres.flx` — ~1100 lines Flux.
- `src/runtime/async/tls.rs` — rustls state-machine integration.
- Examples: HTTPS server, database-backed CRUD microservice (the
  motivating example from Summary).
- Integration tests against a real Postgres instance.

Estimated effort: 4 weeks.

### Phase 5 (optional): io_uring backend for Linux

**Not committed.** Ships only if the `mio` Linux backend becomes a measured
throughput bottleneck. The point of mentioning Phase 5 in this proposal is
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
- **HTTP/2 and gRPC are deferred.** Phase 3-4 ship HTTP/1.1 only. Adequate for most microservices.
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
  backend. It remains the Phase 5 optimization backend behind `AsyncBackend`.
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
- **Eio-style three native backends from day one** (io_uring + epoll/kqueue + IOCP). 5x the backend LOC. Multi-year overhead. Rejected; `mio` gives one portable readiness backend now, and Phase 5 keeps the io_uring door open if measurements justify it.
- **Goroutine-style M:N work-stealing scheduler.** Took Go a decade to mature. Out of scope. Rejected; per-worker scheduling without migration is sufficient for the stated workload, with Eio as the precedent.
- **Adopt Tokio.** Rejected because Flux should own its effect-driven scheduler semantics rather than encode them as Tokio futures/tasks. `mio` keeps the low-level reactor without importing Tokio's async model.
- **Per-thread heaps with linear-type send (Erlang-style).** Beautiful but requires major type-system work (uniqueness types, send-primitives). Multi-year scope. Rejected; `Sendable<T>` (Rust-style trait) gives most of the safety with much less type-system work.

## Prior art

- **Lean 4** — the closest existing ownership and task-manager substrate to Flux: Perceus RC, native compilation via LLVM, task manager, and hybrid RC. Flux copies the task-manager and hybrid-RC lessons, not Lean's libuv backend. Where the proposal diverges from Lean: Phase 1b adds a fiber layer that Lean does not have (Lean tasks block their worker threads on `await`); cancellation propagates through `race` (Lean's `race` does not).
- **OCaml/Eio** — the closest existing API surface to what Phase 1b ships. The three-effect seam (Eio `lib_eio/core/eio__core.ml:15-21`: `Suspend`, `Fork`, `Get_context`) is copied directly. The structured concurrency primitives (`Switch`, `Fiber.both`, `Fiber.first`) inform `both`/`race`. Per-domain non-migrating fiber model is adopted. Eio's pluggable-backend architecture (`lib_eio_linux/`, `lib_eio_posix/`, `lib_eio_windows/`) is the model for Phase 5's optional io_uring escape hatch.
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
2. **`Bytes` zero-copy vs. copy on TCP read.** Phase 1b ships copy-on-delivery for simplicity: the backend returns `Vec<u8>`, and the home worker constructs Flux `Bytes`. Phase 3 may move to zero-copy with scheduler-owned buffers. Decision deferred to benchmarking.
3. **Pool internal mutation.** Phase 4's `Postgres.Pool` has internal mutable state (idle connections, in-flight count). Modeled as parameterized handler state. Concrete representation TBD.
4. **JSON codec error reporting.** `Json.decode` failure on malformed input — returns `Result<T, JsonError>` with field-path information. Schema TBD.
5. **HTTP/1.1 keep-alive eviction policy.** Connection pool sizing and timeout defaults TBD.
6. **TLS certificate management.** Loading, rotation, and revocation policies TBD; `rustls` provides primitives, Flux-side ergonomics deferred to Phase 4 design.
7. **`Sendable<T>` derivation rules for closures.** A closure capturing only `Sendable` values is `Sendable`; a closure capturing a non-`Sendable` value is not. Compile-time check needed in `dict_elaborate.rs`. Detailed inference rules TBD.
8. **Single reactor contention under high fiber count.** Phase 1b runs all TCP/timer operations through one `mio` reactor thread. At very high concurrency the reactor queue or completion fan-out may become the bottleneck. Mitigations (sharded reactors, per-worker reactors, io_uring backend) deferred until measured.
9. **Shared backend handle table across forked branches** — *resolved in current revision.* Each worker VM spawned by `Async.both` / `Async.race` / `Async.fork` shares the parent's `mio` reactor (one process-wide `tcp_streams` / `tcp_listeners` table). Per-VM completion routing is implemented via a `RequestId → BackendCompletionSink` map on `MioBackend`: when a child handle (`MioBackendHandle::with_completion_sink`) submits a command, it registers a route entry so the reactor delivers that request's completion to the originating worker rather than to the parent's primary sink. `MioDriverBackend::child()` returns a child driver that the parent passes into `run_send_closure_on_worker`. Verified by `examples/async/parallel_both.flx` running TCP read on both branches of `Async.both` over distinct loopback connections, identical output on the VM and LLVM backends, with no regressions across the 80 existing parity fixtures.

## Revision history

- **Revision 1 (original)** — five-phase plan: single-threaded Async + TCP, HTTP/JSON/Streams, process-per-core, TLS+Postgres, conditional shared-state multi-threading via atomic-everywhere RC. Cited Koka as the precedent for "scheduler in source language, libuv substrate, atomic RC." See git history for original text.
- **Revision 2** — restructured into Phase 1a (multi-threaded substrate, modelled on Lean 4) + Phase 1b (fiber layer + structured concurrency, modelled on Eio), with Phases 2-3 unchanged in shape and an optional Phase 4 (io_uring backend) replacing the original Phase 5. Multi-threading lands in Phase 1a (was Phase 5). Process-per-core (was Phase 3) is removed; Phase 1a's worker pool subsumes it. Hybrid atomic-on-share RC (Lean's and Koka's actual scheme) replaces the original "atomic everywhere" Phase 5 plan. Scheduler moves from Flux source to Rust. Three-effect seam (Suspend/Fork/GetContext) replaces the single `Async` effect for backend extensibility. `Sendable<T>` constraint added (modelled on Rust's `Send`).
- **Revision 3** — strict syntax pass against the actual Flux grammar (`src/syntax/token_type.rs` keyword set, `src/syntax/parser/`). All code samples rewritten to use only supported constructs: named-field records via `data Foo { Foo { ... } }` (proposal 0152), `deriving` clauses on `data` declarations, positional function arguments, recursion in place of `loop`/`while`, library functions in place of `try`/`finally`/`catch`, `match` for tuple destructuring, and `<a: Class>` constraints inline in type-parameter lists. Plain type aliases were folded into this proposal as a "Required language features" section because Phase 1b's setup-closure pattern is awkward without them; ADT-sugar `type` was extended to accept any type expression on the right-hand side, with restrictions described in detail.
- **Revision 4** — concurrency syntax tightened. Transparent aliases now extend the existing `alias` declaration instead of overloading `type`; ADT-sugar `type` remains unchanged. User-facing structured concurrency now centers on `Async.scope`, scoped `fork`, `both`, `race`, `timeout`, `timeout_result`, `finally`, and `bracket`. `AsyncFail` is operation-bearing (`raise: AsyncError -> a`) rather than a payload-carrying label. `Sendable` is positive-only; no negative instance syntax is required for non-sendable handles. CPU-bound `Task.blocking_join` is distinct from fiber-suspending `Task.await`.
- **Revision 5** — I/O backend changed from libuv-first to `mio`-first. A mandatory Phase 0 now makes the effect runtime concurrency-ready before user-facing async work. Phase 1a uses a Rust `AsyncBackend` trait, a dedicated `mio` reactor thread, runtime-owned timer heap, TCP readiness state machines, small blocking DNS/file pools, and one Rust scheduler reached directly by the VM and through narrow C ABI shims from LLVM/native code. Lean 4 remains inspiration for task manager and hybrid RC, Eio remains inspiration for the user-facing structured concurrency seam, and Flux owns the Aether/Perceus boundary by requiring backend completion records rather than C callbacks that manipulate Flux heap values.
- **Revision 6** — bookkeeping pass after Phase 1a → Phase 1b transition: D1 (cross-module class-bound enforcement) closed via constraint preservation + consumer-side dict elaboration + internal-linkage dict CoreDefs; Phase 1b 1b-i through 1b-v landed (effect seams, FiberScheduler, 5 fiber CorePrimOps, Flow.Async + Flow.Tcp source surface) with sequential-equivalent VM dispatch; native Task<a> ships on POSIX (pthreads) and Windows (Win32 + SEH for panic propagation across LLVM frames). 1b-vi (real M:N fiber multiplexing) and Phase 2/3/4 remain. D5-c reframed: native Task is implemented entirely in `runtime/c/tasks.c` rather than via a Rust `TaskScheduler<i64>` staticlib bridge — D5-b/c's original integration is deferred until a feature needs it.
- **Revision 7** — Phase 1b 1b-vi VM-side multiplexing landed in five sub-slices (a, b₁, b₂.1, b₂.2, timeout). `Async.sleep` routes through the mio reactor; every `Async.run_async` boundary owns a real `FiberScheduler`; `FiberSleep`/`FiberBoth`/`FiberRace`/`FiberTimeout` capture continuations and park/resume through a dispatch loop in [`vm_fibers::dispatch_loop`](../../src/vm/core_dispatch.rs). Three new native primops added (`FiberBoth = 172`, `FiberRace = 173`, `FiberTimeout = 174`). Acid tests prove genuine wall-clock overlap (`both(sleep(500), sleep(500))` ≈ 500ms) and that `Async.timeout` actually bounds its body. Native (LLVM) fiber suspend/resume (1b-vi-d), per-worker queues + multi-OS-thread workers (1b-vi-c), and cancellation propagation remain. The continuation re-entry mechanic from "Unresolved questions" #1 — capturing and resuming through `OpPerform`'s machinery with the run_async boundary as the delimiter — is now settled on the VM path; the same pattern will apply on the native backend in 1b-vi-d.
- **Revision 8** — Phase 1b native LLVM core async is implemented through the proposal's narrow C ABI into Rust scheduler state rather than through pthread fiber workarounds. Native `sleep`, `both`, `race`, `timeout`, `scope`, `fork`, and `cancel` suspend/resume through the Rust native async runtime; direct async helper calls and conservative `with Async` indirect closure calls propagate native yield sentinels correctly. Native generated-code re-entry now runs in parallel on OS workers, cooperative cancellation suppresses cancelled running fibers at scheduler/backend boundaries, and LLVM TCP parity is implemented through the same Rust `AsyncBackend`/`MioBackend` path. Native `Task.cancel` and the Phase 1b closeout `Task.await` shim match VM-observable cancel-before-join/await behavior. The remaining VM OS-worker dispatch and true nonblocking native `Task.await` work are documented as post-1b follow-ups rather than Phase 1b blockers.
- **Revision 9 (this version)** — Restructure after a readiness audit of Phase 0/1a/1b against the proposal's deliverable lists found three runtime prerequisites that the original Phase 1a/1b plan listed but did not actually land: blocking-thread DNS resolver / `blocking_pool.rs` (Phase 1a deliverable, missing — `tcp_connect` today rejects hostnames because `socket_addr_from_raw` calls `SocketAddr::from_str`), transparent type aliases (Phase 1b prep, missing — only `Statement::EffectAlias` exists today, no `Statement::TypeAlias`), and `Sendable<T>` ADT auto-derivation (Phase 1a deliverable, missing — `has_structural_builtin_instance` only handles built-in containers). A second audit of the user-facing concurrency surface found seven semantics questions Phase 1b had left under-specified that Phase 3 (HTTP) would otherwise have to invent on the fly: (1) `Task.await` ships with the right type but the wrong semantics — it parks the OS worker on a condvar instead of suspending the calling fiber; (2) `race` is binary-only with no N-way `first` / `first_of`; (3) `Channel.send` is referenced once in the proposal text without a corresponding `Flow.Channel` definition; (4) cancellation is observable only at `await` points, with no `Async.check_cancelled` for long pure loops; (5) `Http.serve` lacks production knobs (connection limit, graceful shutdown, timeouts, per-server worker count); (6) fiber panic semantics are unspecified for Phase 1b (`Task` panics are caught, fibers' aren't documented); (7) runtime config has no centralised `RuntimeConfig` knob, only ad-hoc env vars. These ten items together form the new **Phase 2 (Concurrency closeout + runtime gaps)**, a no-user-facing-API-regression infrastructure phase analogous in shape to Phase 0. The previously-numbered Phase 2 (HTTP/JSON/Streams) becomes **Phase 3**, Phase 3 (TLS + database client) becomes **Phase 4**, and the optional Phase 4 (io_uring backend) becomes **Phase 5**. Independently, the Phase 3 HTTP design retracts the earlier "vendor llhttp" decision: the HTTP/1.1 parser is now scratch-built in Rust under `src/runtime/http/` over the existing `mio` TCP substrate, with no `vendor/` directory and no third-party HTTP-parser dependency, matching the broader proposal direction of a Rust-owned runtime. The JSON design is split: ship parser + manual `Json.Encode`/`Json.Decode` instances first, then synthesised `deriving` codec bodies as a follow-on Phase 3 sub-slice with its own design review. The Phase 1b 10k-connection acceptance bullet moves to Phase 3's HTTP server acceptance — the load test is HTTP-shaped, not runtime-shaped.

## Future possibilities

- **HTTP/2 multiplexing** — once HTTP/1.1 is stable. Significant complexity; likely a separate proposal.
- **WebSocket and Server-Sent Events** — both fall out of HTTP/1.1 + streams in Phase 3 with small additional work.
- **gRPC** — HTTP/2 + protobuf. Future proposal.
- **io_uring backend for Linux** — Phase 5 (optional). Eio demonstrates the dual-backend pattern.
- **Sharded or per-worker reactors** — replace the single `mio` reactor thread with sharded reactors or one reactor per worker. Adds cross-reactor handoff complexity. Deferred until measured.
- **Process-per-core** — was the original Phase 3. Removed because Phase 1a's worker pool already provides multi-core scaling. Can be reintroduced as a userspace library on top of `Process.spawn` if specific deployments want process isolation.
- **Distributed actor model** — built on Phase 1b's `Async` effect + `Sendable<T>` channels. Userspace library; replaces what 0143 originally proposed as language-level actors.
- **Job queue / scheduled tasks** — userspace library on top of `sleep` + persistent storage.
- **File watchers** — `inotify`/`fsevents` through platform-specific watcher backends.
- **GraphQL server** — HTTP + JSON + DataLoader-style fan-out via `both`.

## Appendix: end-to-end POST request trace (Phase 1b + Phase 3)

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
Phase 5's optional io_uring backend slots in below the `AsyncBackend` layer
without changing anything above it.

## Deferred follow-up issues

Surfaced while landing slices of this proposal but not load-bearing for the
slice that surfaced them. Each one is its own future slice; collected here so
they don't get lost between rows of the Progress table.

### D1 — ~~Cross-module class-bound enforcement on function types~~ (resolved)

*Surfaced by: 1a-v / 1a-vi follow-up. Closed in the cohesive multi-step
slice that the original investigation outlined.*

The local `Sendable` solver correctly fails when a constrained generic is
**defined inline** and applied to a function type. The original gap: when
the same generic is **defined in a module and called via import**, the
constraint solver did not flag function-typed payloads at the call site —
`List.contains([id, id, id], id)` typed clean instead of producing a
`No instance for Eq<(Int) -> Int>` diagnostic, and
`Task.spawn(fn() { id })` typed clean instead of producing
`No instance for Sendable<(Int) -> Int>`.

**Resolution.** Landed as the full cascade the original investigation laid
out, in three coordinated pieces:

1. **Constraint preservation.** `resolve_module_member_schemes` in
   [`src/ast/type_infer/mod.rs`](../../src/ast/type_infer/mod.rs) now
   preserves `scheme.constraints` instead of clearing them, so imported
   call sites instantiate with the full obligation list and the solver
   surfaces the right diagnostic.
2. **Consumer-side dict elaboration.** `elaborate_dictionaries` in
   [`src/core/passes/dict_elaborate.rs`](../../src/core/passes/dict_elaborate.rs)
   triggers in modules that *call* constrained imports, not only modules
   that *define* them. `insert_dict_args_at_call_sites` and
   `resolve_dict_arg` thread imported-scheme constraints + per-call-site
   `hm_expr_types` info to instantiate concrete `__dict_{Class}_{Type}`
   references at the call site, alongside the existing polymorphic-dict
   forwarding for caller-bound contexts.
3. **Linker dedup via internal linkage.** Dict `CoreDef` functions and
   their `.closure_entry` wrappers in
   [`src/lir/emit_llvm.rs`](../../src/lir/emit_llvm.rs) emit with
   `Linkage::Internal`, so each translation unit gets a private copy
   without lld-link reporting duplicate symbols on
   `flux___dict_Sendable_String` / `flux___dict_Eq_Int` / etc.

**Aether-pipeline determinism** (originally listed as cascade item #2)
landed independently before the D1 close: `AetherEnv` uses `BTreeSet`
instead of `HashSet` ([`src/aether/analysis.rs`](../../src/aether/analysis.rs)),
and the one capture-iteration site in
[`insert.rs`](../../src/aether/insert.rs) has an explicit sort. Both
remain in place as defensive improvements.

**Verified at close**: both static-check repros fire with the proper
diagnostic; runtime polymorphism through stdlib helpers (e.g.
`Flow.List.sort<a: Ord>`) works on both VM and native LLVM with the
emitted dict CoreDefs and call-site dict args resolving correctly.
`cargo test --all --all-features` is green.

### D2 — ~~`data Task(Int)` constructor-name shadowing~~ (resolved: false alarm)

*Surfaced and dismissed during 1a-vi follow-up.*

Initially logged as a parser bug after a `Task.blocking_join(t)` call site
appeared to lose its return type. Investigation showed the failure was a
missing-semicolon issue in the test source (`let _ = Task.blocking_join(t)`
followed on the next line by `()` was parsed as
`Task.blocking_join(t)(())`, applying the result to a unit argument).
[`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) uses the proposal's
`data Task<a> { Task(Int) }` spelling and the full test suite passes;
no parser change needed. Kept in this list as a record so the original
report doesn't get re-discovered.

### D3 — ~~Reactor-side cancellation of in-flight TCP ops~~ (resolved)

*Surfaced by: 1a-vii. Closed in a small follow-up slice.*

`MioBackend::cancel` now enqueues a `TcpCommand::CancelRequest(req)` in
addition to flagging the cancel-set and dropping any already-queued
completion. The reactor processes the command in its per-iteration drain
step and walks every live `TcpConnState`, clearing any
`pending_read` / `pending_write` / `pending_connect` whose `RequestId`
matches. Result: the reactor stops doing I/O work on the cancelled
caller's behalf, no stray completion is queued for the cancelled request,
and the same handle stays usable for fresh reads/writes.

Implementation: [`backends/mio.rs`](../../src/runtime/async/backends/mio.rs) —
new `TcpCommand::CancelRequest` variant + reactor handler. Regression
test: `tcp_cancel_clears_pending_read_so_no_completion_fires` (loopback
listener delays the write, client cancels the read between submit and
fire, asserts no completion is delivered, then proves the handle is
still usable for a fresh read).

### D4 — ~~`Sendable` ADT auto-derivation~~ (resolved)

*Surfaced by: 1a-v. Closed in a small follow-up slice.*

Implemented as a synthesis pass in [`class_env.rs`](../../src/types/class_env.rs)'s
`collect_from_statements`: after the regular class/instance walk, every
user-declared `data Foo<a, b, ...> { ... }` whose variants contain no
function-typed field anywhere gets a synthesized
`instance <a: Sendable, b: Sendable, ...> => Sendable<Foo<a, b, ...>>`.
The contextual bound on every type parameter pushes the actual
field-type checking onto the existing solver — `Sendable<Box<Int>>` is
satisfiable via `Sendable<Int>`; `Sendable<Box<Int -> Int>>` fails
because no `Sendable<(Int) -> Int>` instance exists.

Positive-only is enforced by `type_expr_contains_function`: if any field
is or contains a `TypeExpr::Function`, no instance is synthesized. Tests
in [`sendable_tests.rs`](../../tests/type_inference/sendable_tests.rs)
cover the four cases — monomorphic ADT auto-derived, parameterized ADT
contextual bound holds for `Box<Int>`, contextual bound rejects
`Box<Int -> Int>`, and ADT-with-function-field is not derived. Explicit
user `instance Sendable<Foo>` declarations still win — synthesis is
skipped when an instance for the same head already exists.

### D5 — Native FFI bridge for `Flow.Task`

*Surfaced by: 1a-vi follow-up. Sub-divided into D5-a (VM end-to-end) and
D5-b/c (native FFI bridge). D5-a is **resolved**; D5-c is resolved for the
current native C runtime path; D5-b remains deferred unless a future feature
requires one Rust staticlib task table shared across backends.*

#### D5-a — VM-side end-to-end ✅

[`Flow.Task.spawn`](../../lib/Flow/Task.flx) / `blocking_join` / `cancel`
now run for real on the VM backend. Implementation:

- Three new `CorePrimOp` variants — `TaskSpawn = 155`,
  `TaskBlockingJoin = 156`, `TaskCancel = 157` — wired through
  [`core/mod.rs`](../../src/core/mod.rs) (enum + `from_id` + `from_name`
  TABLE + `intrinsic_helper_name` + `arity`),
  [`core/display.rs`](../../src/core/display.rs),
  [`core/to_ir/primop.rs`](../../src/core/to_ir/primop.rs),
  [`core/passes/{primop_promote, helpers, disciplined_inline, specialize}.rs`](../../src/core/passes/),
  and [`lir/emit_llvm.rs`](../../src/lir/emit_llvm.rs).
- [`Flow.Task`](../../lib/Flow/Task.flx) replaced its panic bodies with
  three private `intrinsic fn ... = primop ...` declarations and three
  thin public wrappers that wrap the `Int` task id in `Task<a>` and
  pattern-match it back out. Public `Sendable<a>` bound is preserved.
- VM dispatch in [`vm/core_dispatch.rs`](../../src/vm/core_dispatch.rs):
  thread-local `RefCell<HashMap<i64, Value>>` stores results keyed by a
  fresh task id; thread-local `HashSet` tracks cancellations.
  `TaskSpawn` invokes the closure via `RuntimeContext::invoke_value`,
  stashes the result, returns the id; `TaskBlockingJoin` consumes the
  entry; `TaskCancel` flips the cancel flag (a subsequent join surfaces
  an error). Sequentially equivalent to the eventual parallel semantics.
- Native (LLVM) dispatch lowers to `flux_task_spawn` /
  `flux_task_blocking_join` / `flux_task_cancel` calls; those symbols are
  implemented by the D5-c native C runtime path below.
- End-to-end fixture
  [`tests/flux/flow_task_surface.flx`](../../tests/flux/flow_task_surface.flx)
  upgraded from "type-check inside an unused closure" to **6 round-trip
  tests** that actually run on the VM: Int / String / List / tuple / cancel
  / two-independent-spawns. Driven through `flux --test` from
  [`tests/integration/flow_task_tests.rs`](../../tests/integration/flow_task_tests.rs).

**Why VM is sequential.** VM `Value` carries `Rc<...>` (`!Send`), so genuine
parallel execution on the VM path needs a separate value-promotion story
(reuse of the C-side hybrid atomic-on-share scheme from 1a-iv, mirrored
into the VM's `Value` representation). That's deliberately deferred: the
sequential VM dispatch is correct for every Phase-1a use case that doesn't
race against itself.

#### D5-b — Staticlib infrastructure ⏳

Add `[lib] crate-type = ["lib", "staticlib"]` to
[Cargo.toml](../../Cargo.toml) so `cargo build` produces both `flux.exe`
and a `libflux.a` (or `flux.lib` on Windows-MSVC) artifact. Update the
LLVM pipeline ([`src/llvm/pipeline.rs`](../../src/llvm/pipeline.rs)) so
native linking pulls the staticlib in alongside `libflux_rt.a`.

#### D5-c — Native Task<a> end-to-end ✅ (C runtime path)

The original D5-c plan was to wire the panicking
[`runtime/c/tasks.c`](../../runtime/c/tasks.c) stubs to a global
[`TaskScheduler<i64>`](../../src/runtime/async/task_scheduler.rs)
Rust singleton via `extern "C"` shims exposed by the staticlib from
D5-b. The as-shipped path is different: native Task runs entirely in
[`runtime/c/tasks.c`](../../runtime/c/tasks.c) using OS-native
primitives, parallel to (not through) the Rust `TaskScheduler`.

**What ships today:**

- **POSIX path** — pthreads + per-task mutex/condvar + atomic
  cancel flag, with a 1024-slot task table, MT-RC promotion via
  `flux_rc_promote`, and the same VM-observable cancel-before-join
  semantics as the Flow.Task fixture. Worker threads set
  `flux_worker_thread = 1` to bypass the bump arena.
- **Windows path** — Win32 `_beginthreadex` + `CRITICAL_SECTION` +
  `CONDITION_VARIABLE`, mirroring the POSIX path 1:1; same slot
  table, same cancel semantics, same MT-RC discipline. The runtime
  also uses SEH (`RaiseException` + `__try`/`__except`) instead of
  `setjmp`/`longjmp` for `flux_panic` / `flux_assert_throws`, since
  Windows `longjmp` walks the SEH chain via `RtlUnwindEx` and trips
  on `STATUS_BAD_FUNCTION_TABLE` when crossing LLVM-emitted frames
  whose `.pdata` is incomplete. Every emitted Flux function carries
  `uwtable` so the unwinder can walk through it.
- **End-to-end test** — [`tests/flux/flow_task_native.flx`](../../tests/flux/flow_task_native.flx)
  drives Int / String / cancel-throws / await / two-independent-spawns
  through the full `flux --test --native` CLI on every supported
  platform.

**What does *not* yet ship:** the Rust-`TaskScheduler<i64>`-via-staticlib
integration. Native Task is *not* sharing scheduler state with VM Tasks —
each backend has its own task table. The staticlib (D5-b) is also still
pending. If a future feature needs cross-backend task state (e.g. a Flux
program that mixes VM-spawned and native-spawned tasks against the same
join handle, or wants a single Rust scheduler observing both), the
original D5-b/c integration becomes load-bearing again. For Phase 1b's
HTTP / streams / TLS scope the C-side native implementation is
sufficient; D5-b/c can stay deferred until a concrete consumer shows up.
