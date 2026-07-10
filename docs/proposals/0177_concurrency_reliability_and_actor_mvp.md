- Feature Name: Concurrency Reliability & Actor MVP (v0.0.7)
- Start Date: 2026-06-05
- Status: Implemented (2026-07-09 — committed scope M1–M5 complete; M6 stretch
  deferred to 0.0.8)
- Progress (2026-07-09): M1 ✅ · M2 ✅ (follow-ups KI-1/KI-2 resolved) · M3 ✅ ·
  M4 ✅ (T4.2–T4.4 done VM+native; T4.1 re-scoped to a value-carried capability,
  first-class label deferred to 0161 follow-ups) · M5 ✅ ·
  M6 deferred to 0.0.8. KI-4/KI-5/KI-6 all fixed — no open backend issues.
  User guide: [docs/guide/21_actors.md](../guide/21_actors.md).
- Proposal PR:
- Flux Issue:
- Depends on: [0174_async_effect_concurrency.md](0174_async_effect_concurrency.md) (Phases 0–3, shipped in v0.0.6), `Sendable<T>` derivation, the scheduler-as-handler seam in [src/runtime/async/scheduler.rs](../../src/runtime/async/scheduler.rs)
- Relates to: [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) (Phase A is delivered here, re-scoped as a userspace layer over 0174)

# Proposal 0177: Concurrency Reliability & Actor MVP (v0.0.7)

## Summary

v0.0.6 shipped a broad, *additive* concurrency substrate (0174 Phases 0–3): fibers,
a multi-worker work-stealing scheduler, structured concurrency, cancellation,
channels, events, streams, HTTP/JSON — on both the VM and native backends. It works,
but it is **new** code, and every bug shaken out late in the 0.0.6 cycle clustered in
the same place: scheduler races, cross-worker continuation migration, and cancellation
timing — each found by luck, not by a harness.

v0.0.7 has **two co-equal primary objectives**:

1. **Reliability.** Make the runtime provably correct: a deterministic test scheduler so
   concurrency tests assert *semantics* instead of `sleep` margins, a stress/soak harness,
   a documented race audit of the cancel/steal/migration boundaries, and a VM↔native async
   parity gate wired into `release_check.sh`.
2. **Actor MVP.** Ship [0143](0143_actor_concurrency_roadmap.md) **Phase A** — `spawn`,
   a mailbox, `send`/`receive`, and an `Actor` effect label — as a Flux-source userspace
   pattern over the now-solid fiber substrate plus `Sendable<T>`.

No new IRs, no new backends, no deep effect-runtime rework ([0162](0162_unified_effect_handler_runtime.md)
evidence-passing stays out). This proposal consolidates the concurrency story so the actor
and I/O layers that follow stand on a stable base.

## Motivation

The concurrency substrate is the foundation every later feature inherits — the Actor MVP,
TLS, a database client, `io_uring`. If the scheduler has races, that cost compounds: each
new feature is built on, and re-triggers, the same unstable core. Three concrete reasons
to spend a release here:

- **The late-0.0.6 bugs were a pattern, not isolated incidents.** `cancel_fibers` had a
  TOCTOU race ([scheduler.rs:268](../../src/runtime/async/scheduler.rs#L268)); deep-baseline
  continuations were wrongly stolen across worker VMs; a work-stealing race-winner test
  flaked under load. All three live at the cancel × steal × completion boundary, and all
  three were point-patched reactively. There is no harness that would have caught them and
  no harness that would catch the next one.
- **Concurrency tests are timing-flaky by construction.** They assert on elapsed `sleep`
  thresholds (e.g. [tests/integration/vm_fiber_cancel_loser.rs](../../tests/integration/vm_fiber_cancel_loser.rs)
  asserts `elapsed < 1800ms`), so they pass or fail on machine load and timer granularity,
  and reproduce differently across Windows/macOS/Linux. Semantics can't be locked down with
  a stopwatch.
- **Users want actors.** The substrate is fibers + structured combinators; there is no
  `spawn`/`send`/`receive` model — the shape most programmers reach for first. 0143 Phase A
  is small *if* it sits on a solid fiber layer, which is exactly what objective (1) delivers.

A reliability-only release would be coherent but under-sells the substrate; an actors-only
release would build a user-facing surface on an unproven core. Doing both, in that order,
is the leverage.

## Guide-level explanation

Two stories ship in v0.0.7.

**Reliability is mostly invisible to users** — it shows up as "concurrency just works the
same every time." The one visible seam is a **deterministic scheduler** that tests (and
power users) can select, so a program's fiber interleaving is reproducible:

```flux
// A test can pin scheduling order instead of racing on sleep timers.
let cfg = default_runtime_config() |> with_deterministic_scheduler(seed: 42)
run_async_with(cfg, body)   // same interleaving every run, every OS
```

**The Actor MVP is the new user-facing surface.** An actor is a fiber that owns a mailbox
and processes `Sendable` messages one at a time:

```flux
// illustrative surface — exact syntax pinned during M4
type Msg = Inc | Get(Channel<Int>)

fn counter(mailbox: Mailbox<Msg>) with Actor {
    loop_with(0, fn(state) {
        match receive(mailbox) {
            Inc      -> state + 1,
            Get(rep) -> { send(rep, state); state },
        }
    })
}

fn main() with IO {
    run_async(fn() with Async {
        let c = spawn(counter)
        send(c, Inc)
        send(c, Inc)
        let reply = make(1)
        send(c, Get(reply))
        print("count = " + to_string(recv(reply)))   // "count = 2"
    })
}
```

How a Flux programmer should *think* about it: an actor is structured concurrency you don't
have to wire by hand. `spawn` gives you a typed handle; `send` enqueues (and the type system
already forces the payload to be `Sendable<T>`); `receive` suspends the actor fiber until a
message arrives and cooperates with cancellation. Mailboxes are built on the channels that
already ship — actors are a *pattern*, not a new runtime.

## Reference-level explanation

The work is five milestones. M1–M3 are the reliability objective; M4 is the Actor MVP;
M5 is non-optional housekeeping. Each task carries an acceptance check and a file anchor.

> **Note on scope correction.** A file-level audit of the v0.0.6 substrate shows several
> items the earlier roadmap listed as "to implement" are **already done**: `yield_now` and
> `check_cancelled` already do real cancellation checkpoints
> ([scheduler.rs](../../src/runtime/async/scheduler.rs)), `first_of` is already n-way
> ([core_dispatch.rs](../../src/vm/core_dispatch.rs) `FiberFirstOf`), and `Flow.Channel`
> already ships bounded + rendezvous ([lib/Flow/Channel.flx](../../lib/Flow/Channel.flx)).
> Those tasks are down-scoped to **verify/test**. The genuinely-open semantic gap is the
> `guard`/`nack` event placeholders ([lib/Flow/Event.flx:64,73](../../lib/Flow/Event.flx#L64)).

### M1 — Reliability foundation

**Status: 4/4 done.**

The headline. Nothing else is trustworthy without it; the Actor MVP and every later I/O
feature inherit the scheduler's correctness.

- **T1.1 — Deterministic test scheduler.** ✅ **Done.** A single-thread, seedable scheduler
  selected via `RuntimeConfig`. Implemented as a `SchedPolicy` enum + in-tree `SplitMix64` PRNG
  on `FiberScheduler` ([scheduler.rs](../../src/runtime/async/scheduler.rs),
  `new_deterministic`), selected through the new `with_deterministic_scheduler(seed)` builder and
  the arity-5 `FiberRunAsyncWith` primop; deterministic mode forces a single worker, so steal/
  migration are structurally unreachable and the only choice is the seeded ready-pick (`seed == 0`
  = strict FIFO). *(Note: the "swappable-scheduler seam" referenced here did not exist and was
  introduced as the `SchedPolicy` enum; the proposal's `FLUX_WORKERS=1` framing is superseded by
  the explicit `det_seed` config path.)* Unit tests pin the seeded permutation; the end-to-end
  test ([vm_deterministic_scheduler.rs](../../tests/integration/vm_deterministic_scheduler.rs))
  asserts a **fixed interleaving with zero `sleep()`, byte-identical across runs**, distinct per
  seed. Enabling this end-to-end test also required fixing a language limitation — effectful
  *closures* (un-annotated function literals couldn't perform effects); see the effectful-closures
  changelog fragment.
  *Done when:* a test asserts a fixed interleaving with zero `sleep()`, identical across runs
  and OSes. ✅
  *Scope note:* VM-only and cooperative/`yield_now`-only — timer/I/O completion order is **not**
  virtualized, and multi-worker steal/migration interleavings are **not** yet replayable. Those
  (virtual-time backend; multi-worker deterministic simulation) are follow-ups feeding T1.2/T1.4.
- **T1.2 — Stress/soak harness.** ✅ **Done.** New
  [tests/integration/async_stress.rs](../../tests/integration/async_stress.rs) (+ native twin
  [tests/native_llvm/native_async_stress_tests.rs](../../tests/native_llvm/native_async_stress_tests.rs)):
  thousands of fibers (up to a 4096-leaf `both`-tree) under forced migration + work-stealing,
  racing cancel/timeout. Three fixtures target the cancel × steal × completion boundary — a pure
  fan-out tree, 1024 concurrent `race` loser-cancellations, and 512 concurrent `timeout`
  body-cancellations — each folding N fibers into one integer with a known-correct total, so a
  lost completion undershoots and a double-resume overshoots (both caught by exact-equals). Each
  run carries a hard wall-clock kill deadline so a deadlock fails loudly instead of hanging CI.
  The harness immediately earned its keep: it shook out a VM lost-completion **deadlock** under
  migration + work-stealing (synthetic-await children were runnable before the parent registered
  its await), fixed by a reserve → register → park → activate sequence on `FiberScheduler`.
  *Done when:* runs clean N×100 in a loop on VM + native, in CI on all three OSes. ✅ (fan-out
  fixture: 12/15 fail before the fix → 25/25 after, on VM + native).
- **T1.3 — Race audit + invariants doc.** ✅ **Done.** New
  [docs/internals/concurrency_model.md](../../docs/internals/concurrency_model.md)
  systematically audits the cancel/steal/migration boundaries behind the three
  late-0.0.6 fixes — the `cancel_fibers` two-pass TOCTOU scan
  ([scheduler.rs:442](../../src/runtime/async/scheduler.rs#L442)), cross-worker steal in
  [`next_ready_or_steal`](../../src/runtime/async/scheduler.rs#L498), and the
  `unsafe impl Send for Fiber` invariant ([fiber.rs:127](../../src/runtime/async/fiber.rs#L127)) —
  plus the continuation-portability keystone (`is_migratable`, baseline-`(0,0)` only), the
  reserve→register→park→activate synthetic-await sequence (the T1.2 deadlock fix), and the
  await-coordinator ordering. Each boundary carries a file:line anchor, the race window opened
  if its invariant is violated, and the named regression test that pins it (§9 summary table).
  *Done when:* every `unsafe` / race-prone boundary has a written invariant and a test. ✅
- **T1.4 — De-flake existing tests.** ✅ **Done.** Retrofitted the
  `sleep`-margin assertions in [tests/integration/vm_fiber_*.rs](../../tests/integration/)
  ([native_work_stealing_tests.rs](../../tests/native_llvm/native_work_stealing_tests.rs)
  was already de-flaked in v0.0.6). Two load-insensitive replacements: (a)
  **cancellation/timeout/race** tests use a *wide-gap deadlock guard* — the
  branch that should be cancelled/skipped sleeps a large fixed amount (30s) and
  the test asserts completion under 8s, so a working run finishes in compile +
  ~50ms regardless of load while a regression blocks ~30s and trips the guard;
  (b) **overlap/parallelism** tests (`both`) prove concurrency *semantically* via
  a channel **rendezvous** (each child announces itself and waits for the other,
  so completion ⟺ overlap; sequential execution deadlocks) with zero sleeps. The
  converted binaries now run ~0.12s each (down from 0.5–3s) and looped 75×
  flake-free. Rationale recorded in
  [concurrency_model.md](../../docs/internals/concurrency_model.md) §1.
  *Done when:* no concurrency test depends on an elapsed-time threshold. ✅
  *Scope note:* a fully time-free form for the timer-cancellation cases (asserting
  *zero* wall-clock) awaits the virtual-time scheduler backend, a T1.1 follow-up;
  until then the wide-gap guards are robust deadlock checks rather than tight
  margins — they no longer flake under load, which is the de-flake objective.

### M2 — Phase 2 semantic closeout (verify-and-fill)

**Status: 5/5 done.** T2.1–T2.5 complete on VM + native. T2.5's `guard`/`nack` ship with
VM+native parity fixtures. The native `with_nack` crash was a missing yield check at
cross-module async call sites: a function performing `Async` written with the expanded effect
row was misclassified non-async (`effect_expr_contains_async`), and `with_nack` was absent from
the cross-module async allowlist (`is_direct_async_extern_symbol`). Both fixed — see
[changes/2026-06-23-native-async-resume-followups.md](../../changes/2026-06-23-native-async-resume-followups.md).

**M2 follow-ups (all since resolved).** Two pre-existing native-backend bugs were surfaced —
not caused — while landing T2.5; both are closed in
[docs/internals/known_issues.md](../internals/known_issues.md):

- **KI-1 — user-defined cross-module `async` functions miss native yield checks.**
  ✅ **Fixed (2026-07-03):** async-ness is data-driven from the callee's effect row
  (`ImportedNativeSymbol::is_async` → `LirProgram::async_extern_symbols`) instead of the
  hardcoded allowlist. A residual gap — the set was populated only on a lowering entry
  point the per-module native pipeline doesn't use, leaving bare `exposing`-imported
  library calls unclassified — was found while landing M4's `receive` and fixed
  2026-07-08 (KI-4/KI-5, see M4 T4.3).
- **KI-2 — effectful call duplicated in a generic cross-module `let r = f(); r` tail-return.**
  ✅ **Fixed (2026-07-03)** in the aether reuse pass (`rewrite_drop_body_with_env`); see the
  Resolved entry for the root cause and regression coverage.

Smaller than the prior roadmap implied — most slices are already implemented; this milestone
proves them and fills the one real gap.

- **T2.1 — Fiber panic propagation.** ✅ **Done.** The genuinely-open gap was a *Rust*
  `panic!` in a fiber body (a runtime fault: bad `unwrap`, overflow, ICE) — Flux `panic` was
  already routed through `signal_fiber_error` into a catchable `Err`. Such a Rust panic
  unwound the OS worker thread (poisoning a background worker; aborting `run_async` on the
  single-threaded loop). The VM dispatch now wraps both fiber-body invocation sites
  (`dispatch_loop` and the multi-worker `run_one_fiber`,
  [core_dispatch.rs](../../src/vm/core_dispatch.rs)) in `catch_unwind`, resets the reused
  worker VM to its pre-tick stack/frame boundary via the new
  `RuntimeContext::unwind_to_boundary` hook, and surfaces the fault as a catchable
  `AsyncError.Panicked(message)` at the enclosing scope. (`FiberFork`'s inline child runs
  within the parent tick and is covered by the parent's guard.)
  *Done when:* a panicking child surfaces as a catchable failure at the scope boundary, on
  VM + native, with a parity fixture. ✅ New parity fixture
  [tests/parity/async_both_child_panic.flx](../../tests/parity/async_both_child_panic.flx)
  (a `both`-forked child panics → `Err(Panicked("child boom"))`, byte-identical VM/native)
  plus a `core_dispatch` unit test asserting a Rust panic becomes a `WorkerFiberResult::Error`
  without poisoning the worker.
- **T2.2 — Catchable-raise audit.** ✅ **Done.** Confirmed `Async.fail` is a genuine
  catchable raise end-to-end on VM + native: `try` recovers it as `Err(err)` with the
  `AsyncError` payload intact, and an unwrapped raise propagates to the enclosing
  `scope`/await across `both`/`race` and forked children. `bail_if_cancelled` is already
  `if check_cancelled() { fail(Canceled) }` over that real raise — its pre-T2.1 shim
  semantics are retired and the stale "becomes catchable in slice 2-vi" framing was removed
  from the `FiberCheckCancelled` comment ([core_dispatch.rs](../../src/vm/core_dispatch.rs))
  and the `fail` / `bail_if_cancelled` library docs ([lib/Flow/Async.flx](../../lib/Flow/Async.flx)).
  No behavioral change — the machinery was already real (`FiberFail` → `signal_fiber_error` →
  catchable). A dedicated `bail_if_cancelled` runtime fixture is intentionally omitted: a
  flag-set fiber is normally a loser being torn down (whose errors are deliberately swallowed),
  so catchability follows compositionally from the separately-verified `check_cancelled`
  (flag read) and `fail` (catchable).
  *Done when:* parity fixtures pass on VM + native. ✅ New
  [tests/parity/async_fail_catchable.flx](../../tests/parity/async_fail_catchable.flx)
  (direct raise, multi-field payload, forked-child raise) alongside the pre-existing
  [tests/parity/async_try_panic.flx](../../tests/parity/async_try_panic.flx).
- **T2.3 — `yield_now` / `check_cancelled` confirmation (test-only).** ✅ **Done.** Both
  already perform real checkpoints. New
  [tests/integration/vm_yield_now_cancel.rs](../../tests/integration/vm_yield_now_cancel.rs)
  pins `yield_now`'s checkpoint under the seedable single-worker deterministic scheduler
  (T1.1, zero `sleep`): a `race` loser running a finite cooperative `yield_now` loop is
  cancelled when the winner resolves and is curtailed to ~1 tick instead of its 200-tick
  bound. The test is validated to isolate the checkpoint — temporarily removing the
  `is_current_cancelled()` guard in `FiberYieldNow` runs the same seeds (0, 7, 123) to the
  full 200 ticks, flipping the assertion (and the finite bound makes a regression fail loudly
  rather than hang). `check_cancelled`'s checkpoint remains covered by
  [vm_fiber_check_cancelled.rs](../../tests/integration/vm_fiber_check_cancelled.rs).
  *(Down-scoped from "implement" to "test".)*
- **T2.4 — N-way `race` tie-break decision.** ✅ **Done.** Decision: **`race` stays 2-way**
  and delegates the n-way case to `first_of` / `first` (`race(f, g)` ≡ `first([f, g])`). Both
  share one deterministic **source-order tie-break**: when several branches are simultaneously
  *runnable* (ready in the same cooperative round, including across `yield_now`), the earliest
  in source order wins; a later branch wins only once every earlier branch is *suspended on a
  real async wait* (`sleep` / I/O). This records the existing
  `AwaitCoordinator::{resolve_race, resolve_first_of}` `blocked_by_earlier_ready` semantics
  ([await_coordinator.rs](../../src/runtime/async/await_coordinator.rs)); documented on `race`
  and `first_of` in [lib/Flow/Async.flx](../../lib/Flow/Async.flx).
  *Done when:* the deterministic source-order tie-break is locked with tests. ✅ New
  [tests/integration/vm_race_tiebreak.rs](../../tests/integration/vm_race_tiebreak.rs) sweeps
  11 seeds under the deterministic scheduler (T1.1), asserting the tie-break holds for every
  seed — 2-way `race` ties, n-way `first_of` ties, and earlier-source priority across a
  `yield_now`.
- **T2.5 — Event `guard`/`nack` real semantics.** ✅ **Done (VM + native).** Replaced the
  placeholders at [lib/Flow/Event.flx](../../lib/Flow/Event.flx): `guard(f)` now defers building
  its event to **sync-time** (runs `f` once, memoized across re-polls) via a new `EventGuard`
  primop, and `with_nack(f)` now hands `f` a real nack event that **fires when its branch loses**
  the enclosing `choose` (silent on win, CML-style) via a new `EventWithNack` primop over a
  1-capacity channel.
  *Done when:* CML-style `guard`/`nack` parity fixtures pass on VM + native. ✅
  - `guard` defers/runs-once: [tests/parity/async_event_guard_defers.flx](../../tests/parity/async_event_guard_defers.flx). ✅
  - `nack` fire-on-loss: [tests/parity/async_event_nack_fires_on_loss.flx](../../tests/parity/async_event_nack_fires_on_loss.flx). ✅
  - `nack` silent-on-win: [tests/parity/async_event_nack_silent_on_win.flx](../../tests/parity/async_event_nack_silent_on_win.flx). ✅
  - The native `with_nack` crash was a **missing yield check at cross-module async call sites**:
    a function performing `Async` via the expanded effect row was misclassified non-async
    (`effect_expr_contains_async` now matches the expanded seam labels, not just the `Async`
    alias), and `with_nack` was absent from the cross-module async allowlist
    (`is_direct_async_extern_symbol`). Both fixed — details in
    [changes/2026-06-23-native-async-resume-followups.md](../../changes/2026-06-23-native-async-resume-followups.md).

### M3 — Async parity gate

**Status: T3.1 done; T3.2 done (HTTP unblocked — KI-3 fixed).**

- **T3.1 — Wire async parity into `release_check.sh`** so VM↔native divergence is a release
  gate, not an ad-hoc check. ✅ **Done.** Already wired: [release_check.sh](../../scripts/release/release_check.sh#L17)
  runs `parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict` under
  `set -e`, and `parity-check` exits non-zero on any mismatch — so a VM↔native divergence in
  `tests/parity/` already fails the release preflight. Confirmed against a forced-mismatch
  probe (exit code 1). No code change needed.
- **T3.2 — Backfill missing parity fixtures.** Today's blind spots, each tested on only one
  backend: `Task` spawn/join lifecycle, scope cancellation, and HTTP/TCP. ◐ **Mostly done.**
  - `Task` spawn/join lifecycle — ✅ [tests/parity/async_task_spawn_await.flx](../../tests/parity/async_task_spawn_await.flx)
    (async-surface `Task.await`, distinct from the existing `blocking_join` fixture).
  - Scope cancellation — ✅ [tests/parity/async_scope_cancel_stops_fork.flx](../../tests/parity/async_scope_cancel_stops_fork.flx)
    (`scope`/`fork`/`cancel`), moved from the ungated `examples/async` sweep into the gated dir.
  - TCP — ✅ already covered by the passing `tcp_*` fixtures (raw async socket I/O on both backends).
  - HTTP — ✅ [tests/parity/http_get_roundtrip.flx](../../tests/parity/http_get_roundtrip.flx)
    (`serve`/`get`/`shutdown` round-trip over `both`). Authoring this fixture surfaced a native
    heap-use-after-free (composed-continuation double-drop) that crashed all native HTTP inside
    `both`; **fixed** (was KI-3, now resolved in
    [known_issues.md](../../docs/internals/known_issues.md)) in `runtime/c/effects.c`, and the
    fixture promoted into the gated set.
  *Done when:* `parity-check tests/parity --ways vm,llvm` covers every async op at 100%. ✅

### M4 — Actor MVP (primary)

**Status: done (2026-07-09) — T4.2–T4.4 on VM + native; T4.1 re-scoped
(value-carried capability; first-class label deferred to 0161 follow-ups).**

[0143](0143_actor_concurrency_roadmap.md) Phase A, re-scoped as a userspace layer over 0174.
Built on the now-solid fiber substrate + the M2 channel + `Sendable<T>` (already enforced by
the type system, [src/types/class_env.rs](../../src/types/class_env.rs)).

- **T4.1 — `Actor` effect label** via [0161](implemented/0161_effect_system_decomposition_and_capabilities.md)
  Phase-1 infra (phantom capability label, no new runtime). ⤳ **Re-scoped: not expressible
  today.** Two effect-system limitations block it, confirmed empirically and logged as
  L1/L2 in [known_issues.md](../internals/known_issues.md): a `with` clause cannot carry two
  concrete labels (`with Actor | Async` fails E034), and an effect **alias** rides through a
  higher-order boundary as an opaque atom that cannot be discharged there — so a
  `with Actor`-typed body passed to `fork` forces the label all the way up to `main`. The MVP
  instead carries the receive capability **as a value**: `Mailbox<msg>`, only ever
  constructed by `spawn`. Revisit a first-class label under 0161 follow-ups (0.0.8+).
- **T4.2 — `spawn` + mailbox.** ✅ **Done** — [lib/Flow/Actor.flx](../../lib/Flow/Actor.flx):
  `spawn` / `spawn_sized` fork the body under a private scope via the `FiberForkScoped`
  primop (L3: a function *parameter* cannot be forwarded through a Flux-level
  effect-polymorphic wrapper, so `spawn` hands the body straight to the primop, like
  `both`/`first_of`); `Mailbox<msg>` wraps the shipping bounded channel; `tell` enqueues
  with back-pressure; `msg: Sendable` is enforced on every message-bearing op.
- **T4.3 — `receive`.** ✅ **Done** — suspends the actor fiber on the mailbox; a
  cancellation checkpoint (`stop` cancels the actor's scope and curtails a blocked
  `receive`; a closed-and-drained mailbox raises `Canceled`). Landing this on native
  surfaced — and fixed (2026-07-08) — **KI-5/KI-4**: a bare `exposing`-imported
  cross-module async call missed its native yield check because `async_extern_symbols`
  was never populated on the per-module (aether) lowering path; see the Resolved entry in
  [known_issues.md](../internals/known_issues.md). The actor repros now match the VM on
  native (single-`receive` → 141, adder → 42, looping counter → 3); regression tests
  `native_actor_receive_gets_yield_check` /
  `native_exposing_imported_channel_intrinsic_gets_yield_check` in
  [tests/native_llvm/native_async_cross_module_tests.rs](../../tests/native_llvm/native_async_cross_module_tests.rs).
  *Known edge — resolved:* **KI-6** (cancel-blocked-`recv` immediately before `run_async`
  teardown corrupted the boundary; `stop()`-then-exit patterns) was fixed 2026-07-09 —
  the dispatch loop no longer ticks fibers after the root completes. See the Resolved
  entry in [known_issues.md](../internals/known_issues.md).
- **T4.4 — Examples + parity.** ✅ **Done (2026-07-09)** —
  [examples/actors/](../../examples/actors/): `counter.flx` (stateful actor,
  request/reply via a channel carried in the message), `ping_pong.flx` (rally +
  graceful self-termination via a `Stop` message), `fan_out.flx` (one-shot worker
  pool), each verified byte-identical on VM + native.
  [tests/integration/actor_mvp.rs](../../tests/integration/actor_mvp.rs) runs the
  example files on the VM and adds the deterministic-scheduler ordering tests: actor
  fibers participate in the T1.1 seeded ready-pick, so a fixed seed replays a fixed
  spawn-wake interleaving (seeds 0/1/99 → `ABC`/`CAB`/`BAC`), while mailbox wake-ups
  resume in publish order by design (seed-independent). Native twin:
  [tests/native_llvm/native_actor_mvp_tests.rs](../../tests/native_llvm/native_actor_mvp_tests.rs)
  (deterministic tests stay VM-only until T6.3). The release parity gate gains
  [tests/parity/async_actor_receive_reply.flx](../../tests/parity/async_actor_receive_reply.flx).
  *Example-design note:* examples self-terminate (a `Stop` message or one-shot bodies)
  or let `run_async` teardown reap a parked actor; `stop()` immediately before exit
  (the former KI-6 edge) is now also safe and pinned by
  [tests/integration/vm_cancel_teardown.rs](../../tests/integration/vm_cancel_teardown.rs).

**Explicitly not in scope (→ 0.0.8+):** typed per-message mailbox protocols (0143 Phase B),
supervision trees / restart strategies (Phase C), the M:N scheduler upgrade (Phase D), and any
0162 evidence-passing rework.

### M5 — Housekeeping (not optional)

**Status: done (2026-07-09).** T5.1: 0083/0152/0175/0176 marked Implemented in
[0000_index.md](0000_index.md), files moved under `implemented/`, cross-references
fixed repo-wide. T5.2: `roadmap_to_1_0_0.md` 0.0.6/0.0.7 rows and detail sections
refreshed to the actual themes (displaced items preserved as explicit deferrals).
T5.3: fragments landed per task throughout.

- **T5.1 — Fix stale proposal statuses** in [0000_index.md](0000_index.md): 0175/0176 (REPL),
  0083 (typed holes), and 0152 (named fields) shipped in 0.0.6 but still read "Draft" — mark
  Implemented.
- **T5.2 — Refresh `roadmap_to_1_0_0.md`** — it still lists 0.0.7 as "effect system
  decomposition / 0161", which shipped in 0.0.5.
- **T5.3 — Changelog fragments** under `changes/` per task, per the release procedure.

### M6 — Deterministic scheduling follow-ups (stretch)

Deepens M1's deterministic scheduler along the two axes its T1.1 scope note explicitly
deferred (VM-only, cooperative/`yield_now`-only — timer/I/O order not virtualized,
multi-worker interleavings not replayable). **Status: deferred to 0.0.8 (decided
2026-07-09)** — M1–M5 landed and closed v0.0.7's committed scope; T6.1–T6.3 move to 0.0.8
planning (recorded in [roadmap_to_1_0_0.md](../roadmaps/roadmap_to_1_0_0.md)) together with
the T4.1 first-class `Actor` label follow-up. None of M2–M4 depended on it; it is the next
increment of test-oracle fidelity, not a prerequisite for the Actor MVP.

- **T6.1 — Virtual-time scheduler backend.** Virtualize timer (and, where feasible, I/O)
  completion ordering so `sleep`/`timeout`/timer-driven programs run on a logical clock the
  deterministic scheduler advances, rather than the wall clock. Lets timer-cancellation tests
  assert a fixed interleaving with **zero** real time elapsed — the "fully time-free form"
  the T1.4 scope note defers. Built behind the existing
  `SchedPolicy::Deterministic` seam in
  [scheduler.rs](../../src/runtime/async/scheduler.rs); the mio backend's timer source is the
  swap point.
  *Done when:* a `timeout`/`sleep` cancellation test asserts a fixed interleaving with zero
  wall-clock, reproducible across runs and OSes — converting the current wide-gap deadlock
  guards (T1.4) into exact assertions.
- **T6.2 — Multi-worker deterministic simulation.** Extend the deterministic scheduler past
  one logical worker: model >1 worker with seeded work-stealing / migration decisions so
  multi-worker interleavings (the cancel × steal × completion boundary the T1.2 harness
  hammers) become replayable, not just stress-fuzzed. Cross-checked against the T1.2 stress
  harness so the simulation tracks the production scheduler.
  *Done when:* a multi-worker program asserts a fixed, seed-selected interleaving identical
  across runs; a seed that reproduces a known steal/migration race is pinned as a regression.
- **T6.3 — Native deterministic scheduling + VM↔native deterministic parity.** Today native
  threads `det_seed` through its ABI but ignores it ([native_abi.rs](../../src/runtime/async/native_abi.rs));
  make the native scheduler honor it (single-worker, then the T6.2 simulation) so a given seed
  yields the same interleaving on both backends. Unblocks exact-count deterministic tests as
  **parity fixtures** (e.g. promoting `vm_yield_now_cancel.rs`'s exact assertion to vm + llvm),
  extending the M3 parity gate to deterministic schedules.
  *Done when:* `parity-check` shows vm and llvm produce byte-identical output under a fixed
  `with_deterministic_scheduler(seed)` for a cooperative and a timer-driven fixture.

### Dependency order

```
M1 ──┬──> M2 ──┐
     │         ├──> M4 ──> release
     ├──> M3 ──┘
     └┄┄> M6 (stretch; T6.3 also builds on M3)
M5 runs throughout.
```

M6 extends M1 and is independent of the M2→M4 critical path; the dotted edge marks it as
deferred. T6.3 additionally builds on the M3 parity gate (it adds deterministic-schedule
fixtures to it).

M4 depends on M1 (the actor mailbox/receive lean on the deterministic scheduler and the
hardened cancel path) and on M2's event/channel closeout. M2 and M3 can proceed in parallel
once M1's deterministic scheduler exists.

### Success criteria

- Concurrency tests pass deterministically — zero `sleep`-margin races; work-stealing and
  cancellation suites run 100/100 green in a loop on VM + native.
- The stress harness (thousands of fibers, forced steals, racing cancellation) runs clean on
  VM + native across CI OSes — no panics, no leaked continuations, no lost completions.
- `Async.fail` / a performed failure is catchable; a panicking fiber propagates to its scope
  instead of poisoning a worker; `yield_now` observes cancellation.
- `spawn`/`send`/`receive` runs a ping/pong + counter example with identical VM/native output.
- `parity-check tests/parity --ways vm,llvm` at 100%, wired into `release_check.sh`; all
  v0.0.6 async tests remain green.
- Proposal statuses corrected; `roadmap_to_1_0_0.md` refreshed.

## Drawbacks

- **Reliability work has no obvious "done."** Hardening can absorb arbitrary effort. Mitigated
  by gating on concrete artifacts (deterministic scheduler in place, stress harness green N×100,
  full async parity) rather than a subjective bar.
- **Pulling Actor Phase A earlier than the 0143 table** (which slated it for 0.0.8). Mitigated
  by sequencing: M1–M3 land the solid substrate first; if hardening overruns, M4 slips to 0.0.8
  and 0.0.7 ships as a pure solidity release — still a coherent theme.
- **A deterministic scheduler is a second scheduler to maintain.** It must track the production
  scheduler's semantics or tests drift from reality. Mitigated by building it on the existing
  scheduler-as-handler seam rather than as a parallel implementation.

## Rationale and alternatives

- **Why reliability before breadth (TLS/DB/io_uring)?** Every I/O feature inherits scheduler
  correctness. Hardening now is leverage; hardening after three more features is rework on a
  larger surface.
- **Why a deterministic scheduler instead of more `sleep` tuning?** Timer-threshold tests are
  flaky by construction and platform-dependent. A seedable single-thread scheduler makes races
  reproducible and assertions stable across OSes — the single highest-leverage reliability move.
- **Why actors as userspace, not a new runtime primitive?** The fiber substrate + channels +
  `Sendable<T>` already provide everything Phase A needs. A source-level pattern keeps the
  runtime small and lets the surface evolve (typed mailboxes, supervision) without IR churn.
- **Alternative — actors-only release.** Rejected: builds a user-facing surface on an unproven
  core, re-triggering the same race class with more code on top.
- **Alternative — reliability-only release.** Viable fallback (see Drawbacks), but under-sells
  the substrate; users are asking for `spawn`/`send`/`receive`.

## Prior art

- **Structured concurrency** (Trio, Kotlin coroutines, OCaml/Eio): scopes own their children;
  cancellation propagates down. 0174 follows this; this proposal hardens it.
- **Deterministic concurrency testing** (Loom for Rust, Coyote/`P#` for .NET, FoundationDB's
  simulation testing): a controlled scheduler turns Heisenbugs into reproducible failures. T1.1
  adopts the same idea at the fiber level.
- **The actor model** (Erlang/OTP, Akka): mailbox + `send`/`receive`; supervision and typed
  protocols are layered on top. v0.0.7 ships only the mailbox core; supervision/typed mailboxes
  are deferred to 0.0.8 exactly as Erlang layered OTP over raw processes.
- **Perceus / Koka & Lean hybrid RC**: the cross-thread ownership discipline the migration paths
  rely on; the race audit (T1.3) documents where `Sendable` promotion and `ArcFiber` migration
  meet.

## Unresolved questions

_All four resolved during execution:_

- **Actor surface syntax.** ✅ Resolved during M4: a plain function surface —
  `spawn(fn(mb) { body(mb) })` with the capability carried by `Mailbox<msg>` and bodies
  typed `with Async` (the `with Actor` label is deferred, see T4.1). No `actor { … }`
  sugar in the MVP; revisit only if 0143 Phase B's typed protocols create pull for it.
- **Does `race` stay 2-way?** ✅ Yes (T2.4): 2-way `race` delegating n-way to `first_of`;
  the tie-break contract is written down and tested
  ([changes/2026-06-23-race-tiebreak-contract.md](../../changes/2026-06-23-race-tiebreak-contract.md)).
- **Mailbox capacity policy.** ✅ Bounded on the shipping channel: `spawn` defaults to
  capacity 64, `spawn_sized` makes it explicit, and a full mailbox back-pressures the
  *sender* (`tell` suspends). No unbounded/grow mode; no overflow drop policy — the MVP
  prefers suspension.
- **Deterministic-scheduler fidelity.** ✅ Resolved during T1.1 by cross-checking against
  the stress harness; actor spawn-wake order participates in the seeded pick (pinned in
  `tests/integration/actor_mvp.rs`), while completion routing stays publish-ordered by
  design.

## Future possibilities

- **0143 Phase B/C** — typed per-message mailbox protocols and supervision trees / restart
  strategies, once the MVP surface settles.
- **0174 Phase 4/5** — TLS + PostgreSQL, then an optional `io_uring` backend, slotting in once
  the substrate is proven and the HTTP-microservice target pulls them forward.
- **0162** unified handler runtime (Koka evidence passing) — a later, separate architectural
  bet (~0.0.9); explicitly *not* a prerequisite for this proposal.
- **CML-complete events** — with `guard`/`nack` real (T2.5), the door opens to fuller
  Concurrent-ML-style composable synchronization.
