- Clarified VM vs native async scheduling docs: VM fibers are cooperative and
  single-OS-threaded within each `run_async` boundary, while native fibers can
  run on OS workers. `worker_count` on the VM now documents logical scheduler
  queues rather than CPU parallelism.
- Documented the current CPU parallelism path for the VM: use `Task.spawn` /
  `Task.await`, which run task bodies in isolated worker VMs across a
  `Sendable` transfer boundary.
- Added the deferred VM fiber OS-worker dispatch design blockers: `Rc<Value>`
  and VM continuations are not `Send`, effect/frame state is VM-local, and real
  VM fiber parallelism requires a value-promotion/thread-safety design first.
- Phase 1 (VM multithreading): Added `ArcValue` — a thread-safe `Arc<T>` mirror
  of `Value` — with `promote_value` / `demote_value` for cross-worker-boundary
  value transfer. Added `ArcClosure`, `ArcAdtValue`, `ArcConsCell`,
  `ArcHamtNode`, `ArcContinuation`, `ArcHandlerFrame`, `ArcEvidence` companions.
  Added `EvidenceVector::entries()` / `EvidenceVector::from_entries()` accessors.
- Phase 2 (VM multithreading): `Fiber` now implements `Send` via `unsafe impl`
  with the documented no-concurrent-access invariant (home-worker exclusivity).
  `FiberScheduler` is now transitively `Send`, enabling `Arc<Mutex<FiberScheduler>>`
  sharing across OS threads (Phase 4).
- Phase 3 (VM multithreading): Added `VM::new_for_worker` for creating per-worker
  execution contexts sharing constants/globals from a parent VM. Added `FiberExecState`
  to document the scheduler-level fiber execution context.
- Phase 4 (VM multithreading): Added `SharedDispatchState` (cross-thread scheduler,
  condvar wakeups, await coordinator, scope/cancellation registries). Added
  `worker_dispatch_loop` (background OS worker threads) and `worker_0_dispatch_loop`
  (caller's thread, drives mio backend). Added `WorkerVmSnapshot` serialization and
  `vm_from_worker_snapshot` for bootstrapping per-worker VMs. `FiberRunAsync` and
  `FiberRunAsyncWith` now branch: `n_workers == 1` uses the unchanged single-threaded
  `dispatch_loop`; `n_workers > 1` calls `enter_run_async_multi` which spawns real
  OS worker threads.
