### Added
- **Async stress/soak harness** (proposal 0177 T1.2). New
  `tests/integration/async_stress.rs` and its native twin
  `tests/native_llvm/native_async_stress_tests.rs` spawn thousands of fibers
  (up to a 4096-leaf `both`-tree) under forced migration and work-stealing
  (`FLUX_FIBER_MIGRATION=1`, `FLUX_WORK_STEALING=1`), hammering the
  spawn / steal / cancel / timeout paths simultaneously. Three fixtures cover
  the cancel × steal × completion boundary where the late-0.0.6 bugs clustered:
  a pure fan-out tree, 1024 concurrent `race(slow, fast)` loser-cancellations,
  and 512 concurrent `timeout` body-cancellations. Each folds N fibers into one
  integer total with a known-correct value, so a lost completion undershoots and
  a double-resumed continuation overshoots — both caught by an exact-equals
  assertion. Process success doubles as the "no panic / no leaked continuation"
  check, and each run is spawned with a hard wall-clock kill deadline so a
  deadlock fails the test loudly instead of hanging CI. Soak coverage is the
  looped binary:
  `for i in $(seq 1 100); do cargo test --test async_stress --quiet || break; done`.

### Fixed
- **VM fiber scheduler lost-completion deadlock under migration + work-stealing**
  (found by the new T1.2 harness). A deep `both`-tree fan-out deadlocked ~80% of
  runs with `FLUX_FIBER_MIGRATION=1` + `FLUX_WORK_STEALING=1` — no cancellation
  needed. The synthetic-await combinators (`both`/`race`/`first_of`/`try`/
  `timeout`) spawned their children **runnable before** the parent registered its
  await and suspended, so a child completing on another worker could (a) hit
  `AwaitCoordinator::record_completed` with no registered awaiter and have its
  outcome silently dropped, or (b) fire `complete_request` before the parent was
  in the suspended map — either way the parent waited forever. Fixed by a
  reserve → register → park → activate sequence: children are held in a new
  non-runnable `reserved` slot on `FiberScheduler` and only enqueued (activated)
  after the parent is registered **and** suspended, so neither race window can
  open. This matches the native/LLVM backend, which already registered before
  spawning and was never affected.

### Changed
- The VM deterministic test scheduler's seeded interleavings shifted (e.g. seed
  42 now drains `CADB` rather than `ADBC`) because synthetic-await children now
  enter the ready queue after the parent parks. Determinism, reproducibility, and
  strict-FIFO for `seed == 0` (`ABCD`) are unchanged; the pinned permutations in
  `vm_deterministic_scheduler.rs` were updated.

### Tests
- Both stress harnesses pass on the VM and native (LLVM) backends with exact
  totals (4096 / 1024 / 512); the VM fan-out fixture, which deadlocked 12/15
  runs before the fix, now passes 25/25 (and 4 concurrent instances all succeed).
