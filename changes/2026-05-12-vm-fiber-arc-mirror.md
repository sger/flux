### Added
- Groundwork for cross-OS-worker VM fiber migration (proposal 0174 §"VM
  cross-worker fiber dispatch"): a parked/yielded fiber can now be converted to
  and from a genuinely `Send` form. `Fiber::promote(self) -> Result<ArcFiber, _>`
  deep-copies a fiber's body, parked continuation, and effect/evidence context
  into `Arc`-backed mirrors (`ArcFiber`, `ArcEffectContext`, `ArcYieldState`,
  reusing the existing `ArcValue` / `ArcContinuation` infrastructure);
  `ArcFiber::demote(self) -> Fiber` rebuilds a thread-local fiber on the worker
  that will run it. Added `runtime::async::config::fiber_migration_enabled()`
  (env flag `FLUX_FIBER_MIGRATION`, default off) — the gate the cross-worker
  steal path will check once it lands. `promote_effect_context` /
  `demote_effect_context` are new public helpers in `runtime::value`.

### Changed
- The `unsafe impl Send for Fiber` safety comment now states the real, narrow
  invariant: a `Fiber` is only ever *moved* as a sequential hand-off (never
  shared), and a cross-worker hand-off goes through `Fiber::promote` /
  `ArcFiber::demote` rather than sending the `Rc`-bearing `Fiber` itself.

### Docs
- proposal 0174's "VM cross-worker fiber dispatch" section notes that the
  "make resumable fiber state `Send`" blocker is now addressed by
  promote-on-share (`Fiber ⇄ ArcFiber`); wiring it into a cross-worker steal
  path remains the outstanding follow-up.
