# Concurrency Model & Race Audit

> **Proposal:** [0177 — Concurrency Reliability & Actor MVP](../proposals/0177_concurrency_reliability_and_actor_mvp.md) (T1.3)
> **Substrate:** [0174 — Async Effect & Concurrency Roadmap](../proposals/0174_async_effect_concurrency.md) (Phases 0–3)
> **Runtime source:** [`src/runtime/async/`](../../src/runtime/async/)
> **Related internals:** [async_syntax.md](async_syntax.md) §18 ("Under the hood")

This document is the written record of the invariants that keep the Flux fiber
scheduler correct. It exists because every concurrency bug shaken out late in the
v0.0.6 cycle clustered at the same three boundaries — **cancellation**,
**cross-worker stealing**, and **continuation migration** — and each was
point-patched without a stated invariant or a regression test. The goal here is
the inverse: for every race-prone or `unsafe` boundary, write down *what must be
true*, *what breaks if it isn't*, and *which test pins it*.

Read this before touching anything under [`src/runtime/async/`](../../src/runtime/async/).

## 1. The model in one paragraph

Flux runs an **M:N cooperative fiber scheduler**: M Flux fibers multiplexed over N
OS worker threads. There is exactly **one** `FiberScheduler`, shared as
`Arc<Mutex<FiberScheduler>>`; all scheduler mutation happens under that lock, so
the scheduler's own data structures (ready queues, the suspended map, the
reserved slot, the await coordinator) are **never** accessed by two threads at
once. The genuinely concurrent actors are the **worker threads**, which take the
lock to pull/return fibers and otherwise run fiber bodies independently. The hard
problems live precisely at the hand-off points between a worker and the scheduler,
and at the seam where a fiber's `Rc`-based value graph crosses an OS-thread
boundary.

Three behaviours are env-gated and change which race windows are even reachable:

| Env var | Default | Effect |
| --- | --- | --- |
| `FLUX_WORKERS` | core count | number of OS worker threads (1 ⇒ no stealing/migration is reachable) |
| `FLUX_WORK_STEALING` | on | allow a worker to steal a ready fiber from another worker's queue |
| `FLUX_FIBER_MIGRATION` | off | allow a *parked* fiber to resume on a different worker's VM |
| `FLUX_FIBER_TRACE` | off | emit `FiberEvent` trace records (used to debug interleavings) |

The deterministic test scheduler (proposal 0177 T1.1) forces a single logical
worker, so **every steal/migration boundary below is structurally unreachable in
deterministic mode** — that is what makes deterministic tests trustworthy.

## 2. The continuation-portability invariant (the keystone)

Everything about migration follows from one fact: **a captured `Continuation`
stores absolute frame/stack indices relative to the VM that captured it.**

- Background worker VMs always capture at the **baseline** `(entry_frame_index ==
  0, entry_sp == 0)`. Their continuations splice cleanly onto any *other*
  background VM.
- Worker 0 reuses the **caller's deep main VM**, whose stack is already several
  frames deep. A continuation captured there records a non-zero
  `entry_frame_index`/`entry_sp` and **cannot** be rebased onto a shallow
  background VM — doing so resumes into padded placeholder frames and fails with
  *"resumed continuation exited without return."*

Therefore **only baseline-`(0,0)` continuations are portable across workers.** This
single rule gates work-stealing migration. It is enforced by
[`Fiber::is_migratable`](../../src/runtime/async/fiber.rs#L186):

```rust
pub fn is_migratable(&self) -> bool {
    match &self.parked {
        Some(cont) => {
            let cont = cont.borrow();
            cont.entry_frame_index == 0 && cont.entry_sp == 0
        }
        None => true,   // not-yet-started fibers run from scratch at the receiver's baseline
    }
}
```

A not-yet-started fiber (a `body` with no parked continuation) is always
migratable — it runs from scratch via `invoke_value` at the receiver's baseline.

> **If you change how continuations capture stack indices, this invariant and
> every steal path below must be re-audited.** The native/LLVM backend sidesteps
> the issue entirely by registering its awaiter before spawning; see §6.

## 3. `unsafe impl Send for Fiber`

[`fiber.rs:127`](../../src/runtime/async/fiber.rs#L127) — `Fiber` holds
`Rc`/`RefCell` (in `parked`, `body`, `context`) and is therefore not auto-`Send`.
The blanket `unsafe impl Send` is sound **only** under this invariant:

> A `Fiber` is only ever *moved* across an OS-thread boundary as a **sequential
> hand-off** between the scheduler (behind `Arc<Mutex<…>>`) and exactly one worker
> thread at a time — never shared, never accessed concurrently.

Two regimes preserve it:

- **Migration off:** the receiving worker is always the fiber's `home_worker`, so
  the inner `Rc` graphs stay thread-local in practice.
- **Migration on:** the cross-worker hand-off goes through
  [`Fiber::promote`](../../src/runtime/async/fiber.rs#L197) /
  `ArcFiber::demote`, which deep-copy the body, parked continuation, and effect
  context into `Arc`-backed structures and **consume the original** — so no
  `Fiber`'s `Rc`s are ever touched by two threads.

**Race window if violated:** any code path that clones a `Fiber` and hands both
copies to different workers, or that resumes a `Fiber` on a thread other than its
owner without going through `promote`, reintroduces concurrent `Rc` refcount
mutation → use-after-free. A planned hardening (noted in the source) is to replace
the blanket `unsafe impl` with a `FiberCarrier` enum (`Local(Fiber)` /
`Migrated(ArcFiber)`) so only the honestly-`Send` `ArcFiber` crosses threads.

**Tests:** `promote_demote_round_trips_a_parked_fiber`
([fiber.rs](../../src/runtime/async/fiber.rs)); migration paths exercised by
[vm_fiber_migration.rs](../../tests/integration/vm_fiber_migration.rs) and the
stress harness [async_stress.rs](../../tests/integration/async_stress.rs).

## 4. Reserve → register → park → activate (synthetic-await spawn)

[`scheduler.rs`](../../src/runtime/async/scheduler.rs):
[`reserve_child`](../../src/runtime/async/scheduler.rs#L291) (L291),
[`activate_child`](../../src/runtime/async/scheduler.rs#L316) (L316),
[`discard_reserved`](../../src/runtime/async/scheduler.rs#L340) (L340), backed by
the `reserved: HashMap<u64, Fiber>` slot
([scheduler.rs:150](../../src/runtime/async/scheduler.rs#L150)).

**Shared state:** the per-worker `ready` queues, the scheduler's `reserved` slot,
and the `AwaitCoordinator`'s `awaits`/`awaiter_index` maps.

**Invariant:** a child fiber spawned by a synthetic-await combinator (`both`,
`race`, `first_of`, `timeout`, `try`) must be in exactly one of three states, and
must **never be runnable before its parent has registered the await and parked**:

1. **Reserved** — created by `reserve_child`, lives only in `reserved`, in no
   ready queue, unreachable to workers.
2. **Activated** — moved into `workers[home].ready` by `activate_child`, *only
   after* the parent has registered its await **and** suspended.
3. **Discarded** — removed from `reserved` by `discard_reserved` if the parent's
   park aborted (e.g. cancelled before suspending).

**Race window (the deadlock this fixed — 0177 T1.2):** the old `spawn_child`
enqueued children *immediately*. A child could complete on another worker and call
`AwaitCoordinator::record_completed` **before** the parent registered its await,
so the completion found no awaiter and was silently dropped — the parent then
parked forever waiting for a completion that had already happened. A deep
`both`-tree fan-out deadlocked ~80% of runs under `FLUX_FIBER_MIGRATION=1` +
`FLUX_WORK_STEALING=1`, with no cancellation involved. The reserve→activate
sequence closes both sub-windows (no-awaiter-yet and not-yet-suspended) by making
children structurally unreachable until activation.

> The native/LLVM backend already registered its awaiter before spawning children
> and was never affected — the VM now matches that ordering.

**Tests:** `reserved_child_is_not_runnable_until_activated` and
`discard_reserved_drops_child_without_running_it`
([scheduler.rs:678,708](../../src/runtime/async/scheduler.rs#L678)); end-to-end
under chaos in [async_stress.rs](../../tests/integration/async_stress.rs) (the
fan-out fixture went 12/15 failing → 25/25 passing with this fix).

## 5. `cancel_fibers` — cancellation racing delivery (the TOCTOU)

[`cancel_fibers`](../../src/runtime/async/scheduler.rs#L442) (scheduler.rs:442).

**Shared state:** each worker's `suspended: HashMap<req, Fiber>` and `ready`
queue. Both are under the scheduler lock, but a *delivery completion*
(`complete`) and a *cancellation* (`cancel_fibers`) are two separate critical
sections that can interleave: `complete` removes a fiber from `suspended` and then
pushes it to `ready` as `Ready`.

**Invariant:** every fiber whose ID is being cancelled must end up marked
`Cancelled` — whether it is currently in `suspended` **or** has just been moved to
`ready` by a racing delivery. `cancel_fibers` guarantees this with a **two-pass
scan per worker**:

1. Scan `suspended`, move every matching fiber to `ready` as `Cancelled`.
2. **Re-scan `ready`** and downgrade any matching fiber still in `Ready` state to
   `Cancelled`.

```rust
// A delivery-completion racing with this cancellation may have already
// moved the fiber from `suspended` into the ready queue as `Ready`.
// Re-scan ready and downgrade those entries so the dispatch loop handles
// them as cancelled rather than live.
for fiber in worker.ready.iter_mut() {
    if id_set.contains(&fiber.id.0) && fiber.state == FiberState::Ready {
        fiber.state = FiberState::Cancelled;
    }
}
```

**Race window if the second pass is removed:** delivery removes the fiber from
`suspended` (pass 1 misses it) and pushes it to `ready` as `Ready` *after* pass 1
already scanned ready — the fiber is never marked `Cancelled` and the dispatch
loop resumes a fiber that should have been killed. This was the original
late-0.0.6 TOCTOU bug.

**Tests:** `cancel_fibers_moves_suspended_fiber_to_ready_cancelled`,
`cancel_fibers_ignores_unknown_fiber_id`
([scheduler.rs:760,775](../../src/runtime/async/scheduler.rs#L760));
`racing_cancel_under_migration_never_loses_a_winner` (1024 concurrent
`race(slow, fast)` loser-cancellations) in
[async_stress.rs](../../tests/integration/async_stress.rs).

## 6. Work-stealing & migration — `next_ready_or_steal`

[`next_ready_or_steal`](../../src/runtime/async/scheduler.rs#L498) (L498) →
`next_ready_or_steal_inner` ([scheduler.rs:506](../../src/runtime/async/scheduler.rs#L506)).

**Shared state:** the victim worker's `ready` queue and the thief's
`pending_drop` queue.

**Invariants:**

1. A fiber is stolen **only** if `fiber.stealable && fiber.is_migratable()`.
   `stealable` (set by `mark_parked`) means the fiber has crossed at least one
   scheduling boundary; `is_migratable()` is the §2 baseline-`(0,0)` check. The
   root fiber and not-yet-parked fibers are never stolen.
2. The original `Fiber` (with its `Rc` graph) is pushed to the victim's
   `pending_drop` and dropped **on the victim worker** — the only thread that ever
   touched those `Rc`s — never on the thief.
3. `home_worker` is rewritten to the thief at the moment of the steal, so
   subsequent re-queues land on the new owner.

**Race window:** the scheduler is single-threaded under its lock, so the
scan-then-remove on the victim queue cannot TOCTOU against another thief. The real
hazard is invariant (1): stealing a **deep-baseline** continuation would splice it
onto a shallow background VM and crash per §2. That is exactly what
`is_migratable()` prevents.

**Tests:** `next_ready_or_steal_steals_from_back`,
`next_ready_or_steal_skips_root_and_non_stealable`,
`next_ready_or_steal_skips_deep_baseline_continuations`,
`next_ready_or_steal_respects_disabled_migration`
([scheduler.rs:969–1071](../../src/runtime/async/scheduler.rs#L969));
[vm_fiber_migration.rs](../../tests/integration/vm_fiber_migration.rs).

## 7. `AwaitCoordinator` registration ordering

[`record_completed`](../../src/runtime/async/await_coordinator.rs#L225) (L225).

**Shared state:** `awaits: HashMap<req, AwaitKind>` (the per-parent
state machine) and `awaiter_index: HashMap<child, Vec<req>>`. Accessed **only**
from the single scheduler thread.

**Invariant:** a child's completion must find its parent request already in
`awaits`. This is *guaranteed upstream* by the §4 reserve→activate sequence: the
parent registers (`register_both`/`register_race`/…) before the child can become
runnable, so by the time `record_completed` fires, the awaiter exists. A child not
found in `awaiter_index` means the parent aborted before parking (a legitimate
discard), not a lost completion.

**Race window:** none *as long as §4 holds*. This boundary is the consumer of §4's
guarantee — if you weaken the reserve→activate ordering, the silent-drop deadlock
resurfaces here.

**Tests:** exercised end-to-end by the combinator integration tests
([vm_fiber_first_of.rs](../../tests/integration/vm_fiber_first_of.rs),
[vm_fiber_timeout.rs](../../tests/integration/vm_fiber_timeout.rs),
[vm_fiber_overlap.rs](../../tests/integration/vm_fiber_overlap.rs)) and the stress
harness [async_stress.rs](../../tests/integration/async_stress.rs).

## 8. Lower-risk boundaries (atomics & task join)

These are race-prone surfaces that are already correct by construction; recorded
so a future change doesn't quietly break them.

- **FiberId allocation** — [`fiber.rs:39`](../../src/runtime/async/fiber.rs#L39),
  `static NEXT_FIBER_ID: AtomicU64`, `fetch_add(1, Relaxed)`. *Invariant:* IDs are
  unique per process. Monotonic `fetch_add` is sufficient — uniqueness is the only
  contract, no ordering against other state is implied. Test:
  `fiber_ids_are_unique`.
- **Task outcome / join** — `task_scheduler.rs`, `TaskState` =
  `Mutex<Option<TaskOutcome>>` + `Condvar` + `AtomicBool cancelled`. *Invariant:*
  outcome written once by the worker, read once by the joiner; `cancelled` is an
  advisory flag checked before the body runs. Setting `cancelled` after the body
  has started is a no-op (Phase-1a tasks observe cancellation only at yield
  points). Tests: `cancel_before_pickup_short_circuits`,
  `cancel_after_completion_is_a_noop`, `spawn_many_concurrent_tasks_all_complete`.
- **Home-worker pinning (migration off)** — `Fiber.home_worker`
  ([fiber.rs:80](../../src/runtime/async/fiber.rs#L80)) is immutable while
  migration is off; the scheduler pins spawn/next/complete to it, so no
  cross-worker hand-off code runs at all. Tests: `multi_worker_fibers_are_isolated`,
  `migration_disabled_keeps_existing_behavior`.
- **Deterministic scheduler PRNG** —
  [`SplitMix64`](../../src/runtime/async/scheduler.rs#L60) +
  [`SchedPolicy`](../../src/runtime/async/scheduler.rs#L94),
  [`new_deterministic`](../../src/runtime/async/scheduler.rs#L183). *Invariant:* a
  single logical worker, pure integer RNG (no floats, no pointer hashing, no
  clock), so a seed reproduces byte-identically across platforms; `seed == 0` is
  strict FIFO. Tests: `splitmix64_pins_first_outputs_for_seed_42`,
  `deterministic_seed_zero_is_fifo`,
  `deterministic_seed_nonzero_picks_pinned_permutation`,
  `deterministic_same_seed_is_reproducible`,
  [vm_deterministic_scheduler.rs](../../tests/integration/vm_deterministic_scheduler.rs).

## 9. Audit summary

| # | Boundary | Anchor | Invariant | Guards | Regression test |
| --- | --- | --- | --- | --- | --- |
| 2 | Continuation portability | `fiber.rs:186` | only baseline-`(0,0)` continuations migrate | `is_migratable()` | `…skips_deep_baseline_continuations` |
| 3 | `unsafe impl Send` | `fiber.rs:127` | sequential hand-off; `Rc`s never shared | scheduler lock + `promote`/`demote` | `promote_demote_round_trips_a_parked_fiber` |
| 4 | Synthetic-await spawn | `scheduler.rs:291` | child unreachable until parent registers+parks | `reserved` slot, reserve→activate | `reserved_child_is_not_runnable_until_activated` |
| 5 | Cancel vs delivery | `scheduler.rs:442` | cancel catches fibers in `suspended` **and** `ready` | two-pass scan | `cancel_fibers_moves_suspended_fiber_to_ready_cancelled` |
| 6 | Work-steal / migrate | `scheduler.rs:498` | steal only `stealable && migratable`; drop on victim | `is_migratable()`, `pending_drop` | `next_ready_or_steal_skips_deep_baseline_continuations` |
| 7 | Await registration | `await_coordinator.rs:225` | awaiter exists before completion | consumes §4's guarantee | combinator integration tests |
| 8 | FiberId / task join / pinning / PRNG | various | see §8 | atomics / mutex / structural | per-row tests in §8 |

When you add a new scheduler operation, add its row here: state the invariant, name
the guard, and cite the test. A boundary without a written invariant and a test is
the exact shape of bug this document exists to prevent.
