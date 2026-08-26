# Make `Flow.Fs` async-aware

`Flow.Fs` now schedules filesystem work on the async runtime's blocking pool
when called from `Async.run_async`. Existing APIs, effect rows, `IoError`
values, directory-entry behavior, and synchronous calls outside an async
boundary are unchanged.

The VM and LLVM/native backends share host-owned request and completion
payloads. Cancellation suppresses completion delivery without attempting to
interrupt the underlying operating-system call. `RuntimeConfig.fs_pool_size`
and `FLUX_FS_THREADS` configure the pool, with a bounded platform default.

Coverage includes VM/native parity for reads, predicates, mutations, listing,
metadata, concurrent fibers, and regression behavior outside async boundaries.
