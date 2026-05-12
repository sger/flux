### Changed
- VM child-fiber spawn placement now matches the native backend: fresh fibers land on the
  least-loaded logical worker queue (shortest ready queue, ties broken by lowest worker id)
  instead of always going through round-robin. `FLUX_WORK_STEALING=0` (or `false`/`off`)
  restores the original round-robin placement as a diagnostic escape hatch — the same flag the
  native backend already honours. `FiberScheduler` gains `spawn_child()` (the policy-aware
  entry point) alongside the existing `spawn_child_round_robin` / `spawn_child_least_loaded`
  primitives, and a shared `runtime::async::scheduler::work_stealing_enabled()` helper. VM
  cross-worker fiber *stealing* remains deferred — `Value` / continuations are not yet safe to
  migrate across worker execution contexts; only spawn placement is brought to parity here.

### Fixed
- Module-graph integration tests (`flow_prelude_module_tests`, `cross_module_function_tests`,
  `module_linking_integration_tests`) now include the process id in their `target/tmp/...`
  fixture paths, so concurrent `cargo test` invocations no longer clobber each other's fixture
  files (previously a per-process counter alone left the paths colliding across processes).

### Docs
- Updated the `runtime/async/scheduler.rs` "Backend shape" notes to describe load-aware VM
  spawn placement and the `FLUX_WORK_STEALING` escape hatch.
