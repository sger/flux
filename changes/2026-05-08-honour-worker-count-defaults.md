### Changed
- Proposal 0174 slice 2-vii cleanup: native default-fallback for `RuntimeConfig.worker_count` now mirrors the documented `core/mod.rs::FiberRunAsyncWith` contract instead of returning the hardcoded `2`. New `native_abi::resolve_default_worker_count` resolves `FLUX_WORKERS` env → `std::thread::available_parallelism()` → `2` and is used by both `flux_async_run_root` and `flux_async_run_root_with`. Worker count was already honoured per call (sized ready queues + spawned threads); only the sentinel-replacement default was wrong.
- VM `core_dispatch::resolved_worker_count` gains the same `available_parallelism()` rung between the `FLUX_WORKERS` env var and the hardcoded `2`, bringing VM and native to identical default sizing on multi-core machines.

### Added
- `Async.current_worker_count() -> Int with Async` (proposal 0174 slice 2-vii follow-up) — reports the worker count of the active `run_async` scheduler, returning 0 outside any active boundary. Backs the new self-asserting native tests and gives users a way to verify the runtime is honouring their `RuntimeConfig`.
- New `CorePrimOp::FiberCurrentWorkerCount = 201` with VM dispatch in `src/vm/core_dispatch.rs` (over the existing `vm_fibers::current_num_workers`), `flux_fiber_current_worker_count` C shim in `runtime/c/tasks.c`, `flux_async_current_worker_count` extern in `src/runtime/async/native_abi.rs`, and LLVM emit-name in `src/lir/emit_llvm.rs`.
- `tests/native_llvm/native_runtime_config_tests.rs` — native equivalent of `tests/integration/vm_runtime_config.rs`, covering explicit `worker_count`, default-config, plain `run_async`, the `FLUX_WORKERS` env-var fallback, and exact-count assertions via the new primop.
- `tests/parity/async_run_async_with_workers.flx` — vm/llvm parity fixture exercising `run_async_with_workers`, `with_worker_count`, `default_runtime_config`, and plain `run_async` in one program.
- `tests/parity/async_current_worker_count.flx` — vm/llvm parity fixture asserting `Async.current_worker_count` reports the configured count on both backends.
- `examples/async/16_current_worker_count.flx` — runnable demonstration of the new introspection primop.

### Documentation
- `docs/proposals/0174_async_effect_concurrency.md` slice 2-vii row updated: `worker_count` is honoured on both backends; the prior caveat about native ignoring the field is removed.
- `docs/internals/async_syntax.md` §13 `RuntimeConfig` table updated; §14.6 ("worker_count ignored on native") removed — no longer a limitation.
- `examples/async/11_runtime_config.flx` header comment rewritten.
