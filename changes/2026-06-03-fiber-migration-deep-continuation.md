### Fixed
- VM fiber migration (`FLUX_FIBER_MIGRATION=1`) no longer intermittently fails
  with `AsyncError: Panicked("resumed continuation exited without return")` when
  a background worker steals a parked fiber that had been running on worker 0's
  main VM. A continuation records *absolute* frame/stack indices relative to the
  VM that captured it; worker 0 reuses the caller's several-frames-deep main VM,
  whereas background workers run on shallow baseline-`(0, 0)` VMs, so a deep
  continuation could not be rebased onto a shallow VM and resumed into padded
  placeholder frames. The work-stealing scheduler now only steals a parked fiber
  whose continuation was captured at the shallow baseline (`Fiber::is_migratable`);
  deep worker-0 continuations stay on their home worker. Surfaced as a flake in
  `vm_fiber_migration::migration_enabled_completes_parked_work` under thread
  contention.
