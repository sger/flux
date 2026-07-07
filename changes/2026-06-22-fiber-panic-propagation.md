### Fixed
- **Fiber panic propagation** (proposal 0177 T2.1). A genuine Rust `panic!`
  inside a fiber body — a runtime fault such as a bad `unwrap`, an arithmetic
  overflow, or an internal invariant break, as opposed to a Flux `panic`
  (already a catchable `Err`) — previously unwound the OS worker thread: on a
  background worker it **poisoned the thread**, and on the single-threaded
  dispatch loop it aborted the whole `run_async`. The VM dispatch now wraps both
  fiber-body invocation sites (`dispatch_loop` and the multi-worker
  `run_one_fiber`) in `catch_unwind`, resets the reused worker VM back to its
  pre-tick stack/frame boundary, and surfaces the fault as a catchable
  `AsyncError.Panicked(message)` at the enclosing scope — identical to how a
  performed failure propagates. The `FiberFork` inline-child path needs no
  separate guard: it runs within the parent fiber's tick and is covered by the
  parent's `catch_unwind`.

### Changed
- `RuntimeContext` gains an `unwind_to_boundary(frame_index, sp)` hook (default
  no-op; implemented by the VM via the existing `unwind_invoke_error` teardown)
  so the dispatch loop can restore a consistent VM stack after catching a fiber
  panic. `invoke_value`/`resume_from_dispatch` only unwind the VM stack on an
  `Err` return; a Rust panic skips that cleanup, so the reset is explicit.

### Tests
- New `core_dispatch` unit test
  (`panic_propagation_tests::rust_panic_in_fiber_body_becomes_catchable_error_not_thread_poison`):
  drives `run_one_fiber` with a context whose body Rust-`panic!`s and asserts the
  result is `WorkerFiberResult::Error(Panicked("…"))` with the worker intact —
  the panic never escapes the dispatch tick.
- New parity fixture `tests/parity/async_both_child_panic.flx`: a child forked by
  `both` panics; the failure propagates to the parent await and is caught by
  `try` as `Err(Panicked("child boom"))`, byte-identical on VM and native. The
  pre-existing `tests/parity/async_try_panic.flx` continues to cover the directly
  `try`-wrapped panic case.
