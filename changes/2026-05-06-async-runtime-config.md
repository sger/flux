### Added
- `Async.RuntimeConfig`, `Async.default_runtime_config()`, `Async.with_worker_count(n)`, and `Async.run_async_with(cfg, action)` (proposal 0174 Phase 2 slice 2-vii) — explicit per-`run_async` knobs for `worker_count`, `fs_pool_size`, `dns_pool_size`.
- New `CorePrimOp::FiberRunAsyncWith = 179` (arity 4) wired through VM dispatch in `src/vm/core_dispatch.rs` (with thread-local `PendingRunConfig` consulted by `enter_run_async`), `flux_fiber_run_async_with` C shim in `runtime/c/tasks.c` over `flux_async_run_root_with` extern in `src/runtime/async/native_abi.rs`, LLVM emit-name and 4-arg signature in `src/lir/emit_llvm.rs`.
- `FLUX_WORKERS` env-var fallback for the VM scheduler, parsed once via `OnceLock` and overridden by explicit `RuntimeConfig`.
- Introspection helper `vm_fibers::current_num_workers()` for in-process tests.
- `tests/integration/vm_runtime_config.rs` — three tests covering the explicit-config path, the default-config path, and the env-var path.

### Changed
- `enter_run_async` in `src/vm/core_dispatch.rs` no longer hardcodes 2 logical workers; it resolves the worker count from (in order) the pending `RuntimeConfig`, `FLUX_WORKERS`, then the default of 2.
- Updated proposal 0174 slice 2-vii body to reflect the actual landed surface (with `with_worker_count` builder) and document the current native-side limitation (worker_count ignored on native pending a runtime-config refactor).
