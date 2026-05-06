### Added
- `Async.check_cancelled() -> Bool with Async` and `Async.bail_if_cancelled() -> Unit with Async` (proposal 0174 Phase 2 slice 2-iv) — fibers in long pure compute loops between `await` points can poll the cancel flag instead of running to completion under a cancelled scope.
- New `CorePrimOp::FiberCheckCancelled = 178` with VM dispatch in `src/vm/core_dispatch.rs`, `flux_fiber_check_cancelled` C shim in `runtime/c/tasks.c` over `flux_async_check_cancelled` extern in `src/runtime/async/native_abi.rs`, LLVM emit-name in `src/lir/emit_llvm.rs`.
- Per-thread `vm_fibers::CANCELLED_IDS` set tracking fibers whose enclosing scope was cancelled, queryable from a currently-executing fiber (the scheduler's `suspended` map only covers suspended ones).
- `tests/integration/vm_fiber_check_cancelled.rs` — `check_cancelled` returns false for a non-cancelled fiber; `check_cancelled` returns true in a `timeout(20, body)`-cancelled body's post-sleep run-through.
- `tests/parity/async_check_cancelled_false_when_not_cancelled.flx` — vm/llvm parity fixture for the no-cancel case.
- Recursive-ADT regression test added to `tests/type_inference/sendable_tests.rs` (proposal 0174 Phase 2 slice 2-x — confirms `synthesize_sendable_instances` already handles recursive user ADTs).

### Changed
- Restructured proposal 0174 (revision 9) — inserted Phase 2 (concurrency closeout + runtime gaps) between Phase 1b and Phase 3 (HTTP/JSON/Streams); renumbered TLS+DB to Phase 4 and io_uring to Phase 5. The HTTP parser is no longer vendored — it is scratch-built in Rust under `src/runtime/http/` over the existing `mio` TCP substrate. JSON design split into manual-instances-first / synthesised-deriving-second sub-slices.
- Pinned `Http.serve` production knobs in proposal 0174 (Phase 2 slice 2-v): `ServerConfig`, `ServerHandle`, `serve_config`, `shutdown` (graceful drain), `shutdown_now` (cancel in-flight). API spec only; Phase 3 implements against this signature.
- Documented `Flow.Channel` deferral (Phase 2 slice 2-iii) — cross-worker communication for Phases 2-4 uses `Task.spawn` / `Task.await` only; the `module Flow.Channel { ... }` block in the Sendable example is flagged as illustrative only.
