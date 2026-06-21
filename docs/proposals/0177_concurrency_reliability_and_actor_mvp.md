- Feature Name: Concurrency Reliability & Actor MVP (v0.0.7)
- Start Date: 2026-06-05
- Status: Draft
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

Smaller than the prior roadmap implied — most slices are already implemented; this milestone
proves them and fills the one real gap.

- **T2.1 — Fiber panic propagation.** Ensure a panicking fiber propagates to its enclosing
  `scope` instead of poisoning a worker thread. (`try`/`fail` primops 161/181 already exist;
  the gap is *panic*, not performed-fail.)
  *Done when:* a panicking child surfaces as a catchable failure at the scope boundary, on
  VM + native, with a parity fixture.
- **T2.2 — Catchable-raise audit.** Verify `Async.fail` + `try` is genuinely catchable
  end-to-end; retire any remaining `bail_if_cancelled` shim semantics. Add parity fixtures.
- **T2.3 — `yield_now` / `check_cancelled` confirmation (test-only).** Both already perform
  real checkpoints; add deterministic-scheduler tests (T1.1) proving `yield_now` observes
  cancellation. *(Down-scoped from "implement" to "test".)*
- **T2.4 — N-way `race` tie-break decision.** `first_of` is already n-way; `race` is 2-way.
  Decide whether `race` stays 2-way (delegating n-way to `first_of`) and lock the deterministic
  source-order tie-break with tests.
- **T2.5 — Event `guard`/`nack` real semantics.** Make `guard` fire at sync-time (not eager)
  and `nack` actually fire — the placeholders at
  [lib/Flow/Event.flx:64,73](../../lib/Flow/Event.flx#L64). This is the real remaining
  channel/event gap.
  *Done when:* CML-style `guard`/`nack` parity fixtures pass on VM + native.

### M3 — Async parity gate

- **T3.1 — Wire async parity into `release_check.sh`** so VM↔native divergence is a release
  gate, not an ad-hoc check.
- **T3.2 — Backfill missing parity fixtures.** Today's blind spots, each tested on only one
  backend: `Task` spawn/join lifecycle, scope cancellation, and HTTP/TCP.
  *Done when:* `parity-check tests/parity --ways vm,llvm` covers every async op at 100%.

### M4 — Actor MVP (primary)

[0143](0143_actor_concurrency_roadmap.md) Phase A, re-scoped as a userspace layer over 0174.
Built on the now-solid fiber substrate + the M2 channel + `Sendable<T>` (already enforced by
the type system, [src/types/class_env.rs](../../src/types/class_env.rs)).

- **T4.1 — `Actor` effect label** via [0161](implemented/0161_effect_system_decomposition_and_capabilities.md)
  Phase-1 infra (phantom capability label, no new runtime).
- **T4.2 — `spawn` + mailbox.** `spawn` over the fiber substrate; a `Mailbox<T>` built on the
  shipping channel; `send` requires `Sendable<T>` (already enforced).
- **T4.3 — `receive`.** Suspends the actor fiber until a message arrives; cooperates with
  cancellation (uses the M1 checkpoints).
- **T4.4 — Examples + parity.** `examples/actors/*.flx` — ping/pong, counter, fan-out —
  running with identical output on VM + native; `tests/integration/actor_mvp.rs` (+ native
  twin) with deterministic-scheduler ordering tests.

**Explicitly not in scope (→ 0.0.8+):** typed per-message mailbox protocols (0143 Phase B),
supervision trees / restart strategies (Phase C), the M:N scheduler upgrade (Phase D), and any
0162 evidence-passing rework.

### M5 — Housekeeping (not optional)

- **T5.1 — Fix stale proposal statuses** in [0000_index.md](0000_index.md): 0175/0176 (REPL),
  0083 (typed holes), and 0152 (named fields) shipped in 0.0.6 but still read "Draft" — mark
  Implemented.
- **T5.2 — Refresh `roadmap_to_1_0_0.md`** — it still lists 0.0.7 as "effect system
  decomposition / 0161", which shipped in 0.0.5.
- **T5.3 — Changelog fragments** under `changes/` per task, per the release procedure.

### Dependency order

```
M1 ──┬──> M2 ──┐
     └──> M3 ──┴──> M4 ──> release
M5 runs throughout.
```

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

- **Actor surface syntax.** Is it a plain `spawn(fn(mailbox) with Actor { … })` function, or
  does it warrant sugar (an `actor { … }` block)? Pinned during M4; the MVP can ship as a
  function and add sugar later.
- **Does `race` stay 2-way?** (T2.4) — leaning yes, delegating n-way to `first_of`, but the
  tie-break contract needs to be written down and tested either way.
- **Mailbox capacity policy.** Bounded (back-pressure on `send`) vs unbounded (grow). Likely
  bounded on the shipping channel, but the default capacity and overflow behavior are open.
- **Deterministic-scheduler fidelity.** How faithfully must it model work-stealing/migration to
  be a trustworthy test oracle? Resolved during T1.1 by cross-checking against the stress harness.

## Future possibilities

- **0143 Phase B/C** — typed per-message mailbox protocols and supervision trees / restart
  strategies, once the MVP surface settles.
- **0174 Phase 4/5** — TLS + PostgreSQL, then an optional `io_uring` backend, slotting in once
  the substrate is proven and the HTTP-microservice target pulls them forward.
- **0162** unified handler runtime (Koka evidence passing) — a later, separate architectural
  bet (~0.0.9); explicitly *not* a prerequisite for this proposal.
- **CML-complete events** — with `guard`/`nack` real (T2.5), the door opens to fuller
  Concurrent-ML-style composable synchronization.
