# Flux v0.0.7 Implementation Plan

> **Status (2026-07-09): committed scope delivered.** Proposal
> [0177](../proposals/0177_concurrency_reliability_and_actor_mvp.md) M1–M5 are
> complete (KI-4/KI-5/KI-6 fixed along the way); M6 and the T4.1 `Actor` effect
> label carry to 0.0.8. Actor guide: [docs/guide/21_actors.md](../guide/21_actors.md).

## Overview

**Theme: Concurrency — make async/await rock-solid, then ship an Actor MVP on top.**

v0.0.6 delivered a large, *additive* concurrency substrate ([0174](../proposals/0174_async_effect_concurrency.md) Phases 0–1b and most of Phase 3 — fibers, work-stealing scheduler, structured concurrency, cancellation, HTTP, JSON, streams, on both the VM and native backends). It works, but it is **new** code, and the bugs we shook out late in the 0.0.6 cycle were all in the same place: scheduler races, cross-worker continuation migration, and cancellation timing.

v0.0.7 has two jobs:

1. **Solidify** — close the remaining [0174](../proposals/0174_async_effect_concurrency.md) Phase 2 concurrency-semantics gaps and make the runtime *reliable*: eliminate the scheduler/cancellation/migration races, add a deterministic test scheduler so concurrency tests stop being timing-flaky, and gate the full async surface on VM↔native parity.
2. **Actor MVP** — ship [0143](../proposals/0143_actor_concurrency_roadmap.md) **Phase A** as a userspace pattern over the now-solid fiber substrate plus `Sendable<T>` (already shipped): `spawn`, a mailbox, `send`/`receive`, and an `Actor` effect label.

No new IRs. No new backends. No deep effect-runtime rework (the [0162](../proposals/0162_unified_effect_handler_runtime.md) evidence-passing overhaul stays out — it is a later, separate bet). v0.0.7 consolidates the concurrency story so that the actor and I/O layers that follow stand on a stable base.

---

## Current State (v0.0.6 — Complete)

**Concurrency substrate delivered (0174):**

- **Phase 0** — concurrency-ready effect runtime (suspend/resume round-trips, cancellation, no continuation leaks) — ✅
- **Phase 1a** — multi-threaded substrate: worker pool, `mio` reactor, timer heap, blocking DNS/file pools, hybrid RC, `Flow.Task`, `Sendable<T>` (primitives + structural + ADT derivation) — ✅
- **Phase 1b** — fiber layer + structured concurrency: `sleep`/`both`/`race`/`timeout`/`scope`/`fork`/`cancel`, fiber-suspending `Task.await`, TCP, on VM **and** native — ✅
- **Phase 1b-vi-c** — multi-worker scheduling + cooperative cancellation (per-worker ready queues, request-id completion routing, cross-worker `ArcValue`/`ArcFiber` migration) — ✅
- **Phase 2 (partial)** — `Async.check_cancelled` (2-iv), `Http.serve` config design (2-v), `RuntimeConfig` (2-vii); blocking DNS pool (A-4), transparent aliases, `Sendable` ADT derivation — ✅
- **Phase 3** — HTTP/1.1 server + client, JSON (split int/float, structured errors, `deriving (Encode, Decode)`), Streams (`flat_map`/`merge`/`zip`) — ✅

**Observable gaps after v0.0.6:**

- **Reliability is not yet proven.** Three concurrency bugs were fixed reactively near release (`cancel_fibers` TOCTOU race; deep-baseline continuations stolen across worker VMs; a work-stealing race-winner test that flaked under load). These were found by luck, not by a harness. There is no deterministic test scheduler, so concurrency tests rely on `sleep` timing and are inherently flaky.
- **Phase 2 concurrency semantics are under-specified in places** — no real catchable raise / fiber panic propagation (2-vi: `bail_if_cancelled` is an ergonomic shim, `yield_now` is a no-op cancellation point), no N-way `race`/`first` (2-ii), and the `Flow.Channel` surface decision (2-iii) is unmade.
- **No actor surface.** Concurrency today is fibers + structured combinators; there is no `spawn`/`send`/`receive` model.

---

## Version Goals for v0.0.7

**Primary objectives — solidify (0174 Phase 2 closeout + reliability):**

1. **Deterministic test scheduler.** A swappable, single-thread, deterministic scheduler (the scheduler-as-handler design already anticipates this) so concurrency tests assert *semantics* without `sleep` races. Retrofit the existing flaky timing tests onto it.
2. **Scheduler/cancellation race hardening.** Audit and stress-test the worker-steal, migration, and cancellation paths; eliminate the class of bug behind the three late-0.0.6 fixes. Add a concurrency stress/soak harness (many fibers, forced steals, racing cancellation).
3. **Phase 2 semantic closeout** — 2-vi (real catchable raise + fiber panic propagation + `yield_now` as a true cancellation point), 2-ii (N-way `race`/`first`), 2-iii (`Flow.Channel` decision + minimal implementation).
4. **VM↔native async parity gate.** Every async/structured-concurrency fixture runs under `parity-check --ways vm,llvm` and is part of the release gate, not an ad-hoc check.

**Secondary objective — Actor MVP (0143 Phase A):**

5. **Actor MVP** — `Actor` effect label ([0161](../proposals/implemented/0161_effect_system_decomposition_and_capabilities.md) Phase 1 infra), `spawn`, a mailbox, `send`/`receive`, built as a Flux-source userspace pattern over the fiber substrate + `Sendable<T>`. Typed mailboxes (0143 Phase B) and supervision (Phase C) are explicitly deferred.

**Housekeeping (not optional):**

6. Update [`0000_index.md`](../proposals/0000_index.md): `0175`/`0176` (REPL), `0083`→`0469`-era typed holes, and `0152` (named fields) shipped in 0.0.6 but still read "Draft"; mark Implemented. Refresh the stale `roadmap_to_1_0_0.md` table (it still lists 0.0.7 as "effect system decomposition / 0161", which shipped in 0.0.5).

**Success criteria:**

- Concurrency tests pass deterministically with the test scheduler — zero `sleep`-margin race conditions; the work-stealing/cancellation suites run green 100/100 in a loop.
- A stress harness spawning thousands of fibers with forced steals and racing cancellation runs clean (no panics, no leaked continuations, no lost completions) on VM and native.
- `Async.fail` / a performed failure is catchable; a panicking fiber propagates to its scope instead of poisoning a worker; `yield_now` observes cancellation.
- `actor { ... }` (or the chosen surface) spawns, receives `Sendable` messages, and a ping/pong + counter example runs on VM and native with identical output.
- All v0.0.6 async tests remain green; `parity-check tests/parity --ways vm,llvm` at 100%.

---

## Timeline: ~6 weeks

```
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 1-2: Reliability foundation                               │
│   ✓ Deterministic test scheduler (swappable handler)            │
│   ✓ Retrofit flaky timing tests onto it                         │
│   ✓ Concurrency stress/soak harness (fibers × steals × cancel)  │
│   ✓ Race audit: cancel/steal/migration boundaries               │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 3: Phase 2 semantic closeout (0174)                        │
│   ✓ 2-vi catchable raise + fiber panic propagation              │
│   ✓ 2-vi yield_now becomes a real cancellation point            │
│   ✓ 2-ii N-way race / first                                     │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Week 4: Channels + parity gate                                  │
│   ✓ 2-iii Flow.Channel decision + minimal bounded channel       │
│   ✓ Async parity fixtures wired into the release gate           │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│ Weeks 5-6: Actor MVP (0143 Phase A) + release                   │
│   ✓ Actor effect label (0161 infra)                             │
│   ✓ spawn / mailbox / send / receive over fibers + Sendable     │
│   ✓ Examples + VM/native parity; docs; index + roadmap refresh  │
│   ✓ Release sign-off                                            │
└─────────────────────────────────────────────────────────────────┘
```

---

## Milestone Details

### M1: Reliability foundation — Weeks 1-2

**Why first:** the actor layer and every later I/O feature (TLS, DB) inherit the scheduler's correctness. Hardening now is leverage; hardening after actors is rework.

**Deliverables:**
1. **Deterministic test scheduler** — a single-thread, seedable scheduler selected via `RuntimeConfig` (the scheduler-as-handler seam already exists), so a test can drive `spawn`/`steal`/`cancel`/`resume` ordering explicitly instead of via `sleep`.
2. **Stress/soak harness** — `tests/integration/async_stress.rs` (+ native twin): thousands of fibers, forced migration, racing cancellation/timeouts; asserts no panics, no leaked continuations (Phase 0 invariant), no lost/duplicated completions.
3. **Race audit** — systematic review of the cancel/steal/migration boundaries that produced the three late-0.0.6 fixes; document the invariants in `docs/internals/` (a concurrency-model note).
4. **De-flake** — retire `sleep`-margin assertions in the existing work-stealing/cancellation tests in favour of the deterministic scheduler.

**Validation:** the work-stealing + cancellation suites run 100/100 green in a loop on VM and native; stress harness clean.

---

### M2: Phase 2 semantic closeout — Weeks 3-4

**Proposal:** [0174](../proposals/0174_async_effect_concurrency.md) Phase 2 (slices 2-ii, 2-iii, 2-vi)

- **2-vi — catchable raise + panic propagation.** Replace the `bail_if_cancelled` shim with a real performed failure that handlers can catch; a panicking fiber propagates to its enclosing `scope` instead of poisoning a worker thread; `yield_now` becomes a genuine cancellation checkpoint.
- **2-ii — N-way `race` / `first`.** Generalise the two-way combinators to N sources with deterministic tie-break (already the rule for immediate FIFO ties).
- **2-iii — `Flow.Channel`.** Make the surface decision (bounded vs unbounded, single vs multi consumer) and ship a minimal bounded channel; it is the substrate the Actor mailbox builds on.

**Validation:** parity fixtures for each on VM + native; deterministic-scheduler tests for cancellation/raise ordering.

---

### M3: Async parity gate — Week 4

**Deliverable:** every async / structured-concurrency / channel fixture runs under `parity-check --ways vm,llvm` and is part of `release_check.sh`, so VM↔native divergence is caught at the gate rather than by hand. Backfill parity fixtures for any async op currently tested on only one backend.

---

### M4: Actor MVP — Weeks 5-6

**Proposal:** [0143](../proposals/0143_actor_concurrency_roadmap.md) Phase A (re-scoped as a userspace layer over 0174)

**Scope (deliberately minimal):**

```flux
// illustrative surface — exact syntax decided in the milestone
let counter = spawn(fn(mailbox) with Actor {
    loop_with(0, fn(state) {
        match receive(mailbox) {
            Inc      -> state + 1,
            Get(rep) -> { send(rep, state); state },
        }
    })
})

send(counter, Inc)
```

**Deliverables:**
1. `Actor` effect label via [0161](../proposals/implemented/0161_effect_system_decomposition_and_capabilities.md) Phase 1 (phantom capability label, no new runtime).
2. `spawn` over the fiber substrate; a mailbox built on the M2 channel; `send` requires `Sendable<T>` (already enforced by the type system).
3. `receive` suspends the actor fiber until a message arrives (cooperates with cancellation).
4. Examples (`examples/actors/*.flx`) + VM/native parity.

**Explicitly not in scope (→ 0.0.8+):** typed per-message mailbox protocols (0143 Phase B), supervision trees / restart strategies (Phase C), the M:N scheduler upgrade (Phase D), and any [0162](../proposals/0162_unified_effect_handler_runtime.md) evidence-passing rework.

**Validation:** `tests/integration/actor_mvp.rs` (+ native twin): ping/pong, counter, fan-out; deterministic-scheduler ordering tests; parity sweep.

---

## Out of Scope for v0.0.7

Explicitly deferred:

- **0174 Phase 4** (TLS + PostgreSQL) and **Phase 5** (`io_uring`) — I/O breadth, after the substrate is proven solid.
- **0162** unified handler runtime (Koka evidence passing) — deep architectural bet, slated ~0.0.9; not a prerequisite for the Actor MVP.
- **0143 Phases B–F** — typed mailboxes, supervision, scheduler upgrade, move optimization.
- **Non-concurrency backlog** — module-scoped classes (0151), `NonZero` (0135 Ph2), diagnostics Ph2/3 (0126), package workflow (0015). These remain open; they are the natural 0.0.8 "consolidation" candidates.

---

## Risks and Mitigations

- **Concurrency bugs are environment-sensitive.** Several 0.0.6 issues reproduced only under load or on macOS/Linux, not on Windows. Mitigation: the **deterministic test scheduler** (M1) is the headline mitigation — it makes races reproducible and assertions stable across platforms; stress tests run in CI on all three OSes.
- **Actor scope creep.** Phase A easily balloons into mailboxes + supervision. Mitigation: the MVP is `spawn`/`send`/`receive` over the M2 channel only; typed mailboxes and supervision are named out-of-scope above.
- **Hardening has no obvious "done."** Mitigation: gate on the stress harness running clean N×100 plus the full async parity sweep, not on a subjective bar.
- **Pulling Actor Phase A earlier than the 0143 table (which slates it for 0.0.8).** Mitigation: M1–M3 land the solid substrate *first*; if hardening overruns, the Actor MVP slips to 0.0.8 and 0.0.7 ships as a pure solidity release — still a coherent theme.

---

## Exit Criteria

v0.0.7 ships when:

- M1–M3 delivered: deterministic test scheduler in place, stress harness green on VM + native across CI OSes, Phase 2 slices 2-ii/2-iii/2-vi landed, async parity wired into `release_check.sh`.
- M4 delivered **or** explicitly rescheduled to 0.0.8 with a one-line rationale in this file.
- `cargo test --all --all-features` green; `parity-check tests/parity --ways vm,llvm` at 100%; full `examples/` parity holds.
- Proposal statuses corrected in [`0000_index.md`](../proposals/0000_index.md) (0175/0176/0083/0152) and `roadmap_to_1_0_0.md` table refreshed.
- Changelog fragments in `changes/` per release procedure.

---

## Post-v0.0.7 — What Becomes Next

- **0.0.8 — Actor maturity + language closeout:** 0143 Phase B (typed mailboxes) and the slipped 0.0.6 language work (0151 module-scoped classes, 0135 NonZero, 0126 diagnostics, 0015 package workflow).
- **0.0.9 — Effect/runtime depth:** 0162 Phase 1/2 (evidence passing + monomorphic State/Reader), VM/SSA perf (0109/0112).
- **0174 Phase 4** (TLS + PostgreSQL) slots wherever the HTTP-microservice target pulls it forward.
