### Changed
- **De-flaked the VM fiber concurrency tests** (proposal 0177 T1.4) — no
  concurrency test gates correctness on a tight elapsed-time margin anymore. The
  old assertions (e.g. `vm_fiber_cancel_loser` asserting `elapsed < 1800ms` over
  a 2s loser sleep) had only a few hundred ms of headroom over a slow
  `--no-cache` startup and flaked under CI load. Two replacements, both
  load-insensitive:
  - **Cancellation / timeout / race tests** now use a *wide-gap deadlock guard*:
    the branch that should be cancelled or skipped sleeps a large fixed amount
    (30s) and the test asserts completion well under it (8s). A working run
    finishes in compile + ~50ms regardless of load; a regression that fails to
    cancel/skip blocks ~30s and trips the guard unmistakably. Applied to
    `vm_fiber_cancel_loser`, `vm_fiber_cancel_timeout`, `vm_fiber_bracket_cancel`,
    `vm_fiber_scope_cancel`, `vm_fiber_timeout`, `vm_fiber_first_of`, and the
    `vm_fiber_overlap` / `vm_fiber_multiworker` race fixtures.
  - **Overlap / parallelism tests** (`both`) now prove concurrency
    *semantically* via a channel **rendezvous** instead of comparing elapsed time
    to a sequential baseline: each child announces itself and waits for the
    other over capacity-1 `Flow.Channel`s, so both can only complete if they are
    alive simultaneously — sequential execution deadlocks. No sleeps, so a
    passing run is near-instant. Applied to `vm_fiber_overlap::both_overlap_runs_in_parallel`
    and `vm_fiber_multiworker::multiworker_sleep_both_overlaps`.
  As a side effect the converted binaries now run in ~0.12s each (down from
  0.5–3s) because the timer sleeps are cancelled rather than waited on. A fully
  time-free form for the timer-cancellation cases awaits the virtual-time
  scheduler backend (a T1.1 follow-up); until then the wide-gap guards are robust
  deadlock checks, not timing races. Rationale documented in
  `docs/internals/concurrency_model.md` §1.
