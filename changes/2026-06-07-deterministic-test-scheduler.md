### Added
- **Deterministic test scheduler** (proposal 0177 T1.1). `Async.run_async_with`
  now accepts a `RuntimeConfig` with a deterministic-scheduler seed, selected via
  the new builder `with_deterministic_scheduler(seed)`. It runs the fiber
  scheduler on a single logical worker with a seedable, dependency-free
  SplitMix64 policy, so a concurrent program's fiber interleaving is reproducible
  across runs and operating systems — letting concurrency tests assert a fixed
  schedule instead of racing on `sleep` timers. `seed == 0` is strict FIFO; a
  non-zero seed selects a reproducible interleaving (sweep seeds to explore
  schedules). The guarantee covers cooperative/`yield_now`-only programs; it does
  not yet virtualize timers or I/O completion order.

### Changed
- `RuntimeConfig` gains a `det_seed: Option<Int>` field, and the
  `fiber_run_async_with` primop is now arity 5 (`workers, fs, dns, det_seed,
  action`). The native/LLVM backend threads the seed through its ABI but ignores
  it for now (deterministic native scheduling is a later milestone), keeping VM
  and native primop arities in lockstep.

### Tests
- Native ABI-smoke coverage for the arity-5 path
  (`native_runtime_config_tests::native_deterministic_scheduler_seed_is_accepted_and_runs`):
  runs the same four-fiber `with_deterministic_scheduler(seed)` program as the VM
  end-to-end test through the LLVM backend and asserts it drains a valid
  permutation of all four tags for several seeds — guarding the seed-threading
  without asserting the VM-only reproducibility guarantee.
