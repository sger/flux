### Docs
- **New `docs/internals/concurrency_model.md`** (proposal 0177 T1.3) — the race
  audit + invariants record for the async fiber scheduler. Documents every
  race-prone and `unsafe` boundary under `src/runtime/async/` with its invariant,
  the race window opened if violated, and the regression test that pins it: the
  continuation-portability keystone (`is_migratable`, baseline-`(0,0)` only),
  `unsafe impl Send for Fiber`, the reserve→register→park→activate synthetic-await
  sequence (the lost-completion deadlock fix), the `cancel_fibers` two-pass
  TOCTOU scan, work-stealing/migration in `next_ready_or_steal`, await-coordinator
  registration ordering, and the lower-risk atomics/task-join/PRNG surfaces. Every
  cited invariant carries a file:line anchor and a named test; §9 is a summary
  table to extend when adding a scheduler operation.
