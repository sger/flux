//! Direct CorePrimOp dispatch for the VM (Proposal 0133 Step 5).
//!
//! Replaces the old PrimOp → execute_primop() path with a single dispatch
//! keyed by CorePrimOp.  Same Rust implementations, no translation layer.

use std::fs;
use std::io::Read as IoRead;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use crate::core::CorePrimOp;
use crate::runtime::RuntimeContext;
use crate::runtime::r#async::backend::AsyncBackend;
use crate::runtime::r#async::fiber_trace::{self, FiberEvent};
use crate::runtime::hamt as rc_hamt;
use crate::runtime::hash_key::HashKey;
use crate::runtime::value::{Value, format_value};

// ── TCP handle table (proposal 0174 Phase 1b-vii) ────────────────────────────
// DEPRECATED: Replaced by async mio backend in Phase 1b-vi-e.
// Old blocking implementation kept for reference only — no longer used.
#[allow(dead_code)]
mod vm_tcp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    enum TcpHandle {
        Stream(TcpStream),
        Listener(TcpListener),
    }

    thread_local! {
        static TCP_HANDLES: RefCell<HashMap<i64, TcpHandle>> = RefCell::new(HashMap::new());
        static NEXT_ID: RefCell<i64> = RefCell::new(1);
    }

    fn alloc_id() -> i64 {
        NEXT_ID.with(|n| {
            let id = *n.borrow();
            *n.borrow_mut() = id + 1;
            id
        })
    }

    pub fn tcp_connect(host: &str, port: i64) -> Result<i64, String> {
        let addr = format!("{}:{}", host, port);
        let stream =
            TcpStream::connect(&addr).map_err(|e| format!("tcp_connect {}: {}", addr, e))?;
        let id = alloc_id();
        TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Stream(stream)));
        Ok(id)
    }

    pub fn tcp_read(handle: i64, max: i64) -> Result<String, String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&handle) {
                Some(TcpHandle::Stream(stream)) => {
                    let cap = if max > 0 && max <= (1 << 24) {
                        max as usize
                    } else {
                        4096
                    };
                    let mut buf = vec![0u8; cap];
                    let n = stream
                        .read(&mut buf)
                        .map_err(|e| format!("tcp_read {}: {}", handle, e))?;
                    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
                }
                _ => Err(format!("tcp_read: invalid handle {}", handle)),
            }
        })
    }

    pub fn tcp_write_all(handle: i64, data: &str) -> Result<(), String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&handle) {
                Some(TcpHandle::Stream(stream)) => stream
                    .write_all(data.as_bytes())
                    .map_err(|e| format!("tcp_write_all {}: {}", handle, e)),
                _ => Err(format!("tcp_write_all: invalid handle {}", handle)),
            }
        })
    }

    pub fn tcp_close(handle: i64) {
        TCP_HANDLES.with(|h| {
            h.borrow_mut().remove(&handle);
        });
    }

    pub fn tcp_listen(host: &str, port: i64) -> Result<i64, String> {
        let addr = format!("{}:{}", host, port);
        let listener =
            TcpListener::bind(&addr).map_err(|e| format!("tcp_listen {}: {}", addr, e))?;
        let id = alloc_id();
        TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Listener(listener)));
        Ok(id)
    }

    pub fn tcp_accept(listener: i64) -> Result<i64, String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&listener) {
                Some(TcpHandle::Listener(l)) => {
                    let (stream, _) = l
                        .accept()
                        .map_err(|e| format!("tcp_accept {}: {}", listener, e))?;
                    let id = NEXT_ID.with(|n| {
                        let id = *n.borrow();
                        *n.borrow_mut() = id + 1;
                        id
                    });
                    drop(map);
                    TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Stream(stream)));
                    Ok(id)
                }
                _ => Err(format!("tcp_accept: invalid listener {}", listener)),
            }
        })
    }
}

mod vm_http {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    #[derive(Debug)]
    struct ServerState {
        listener: i64,
        scope: u64,
        config: crate::runtime::http::BlockingServerConfig,
        active: HashSet<i64>,
        shutting_down: bool,
        force_shutdown: bool,
        stopped: bool,
    }

    #[derive(Debug, Default)]
    pub struct ShutdownSnapshot {
        pub listener: Option<i64>,
        pub scope: Option<u64>,
        pub active: Vec<i64>,
    }

    thread_local! {
        static SERVERS: RefCell<HashMap<i64, ServerState>> = RefCell::new(HashMap::new());
        static NEXT_ID: RefCell<i64> = RefCell::new(1);
    }

    pub fn register(
        listener: i64,
        scope: u64,
        config: crate::runtime::http::BlockingServerConfig,
    ) -> i64 {
        let id = NEXT_ID.with(|n| {
            let id = *n.borrow();
            *n.borrow_mut() = id + 1;
            id
        });
        SERVERS.with(|servers| {
            servers.borrow_mut().insert(
                id,
                ServerState {
                    listener,
                    scope,
                    config,
                    active: HashSet::new(),
                    shutting_down: false,
                    force_shutdown: false,
                    stopped: false,
                },
            );
        });
        id
    }

    pub fn register_connection(server: i64, conn: i64) {
        SERVERS.with(|servers| {
            if let Some(state) = servers.borrow_mut().get_mut(&server)
                && !state.shutting_down
            {
                state.active.insert(conn);
            }
        });
    }

    pub fn unregister_connection(server: i64, conn: i64) {
        SERVERS.with(|servers| {
            if let Some(state) = servers.borrow_mut().get_mut(&server) {
                state.active.remove(&conn);
            }
        });
    }

    pub fn active_count(server: i64) -> usize {
        SERVERS.with(|servers| {
            servers
                .borrow()
                .get(&server)
                .map(|state| state.active.len())
                .unwrap_or(0)
        })
    }

    pub fn config(server: i64) -> Option<crate::runtime::http::BlockingServerConfig> {
        SERVERS.with(|servers| servers.borrow().get(&server).map(|state| state.config))
    }

    pub fn is_shutting_down(server: i64) -> bool {
        SERVERS.with(|servers| {
            servers
                .borrow()
                .get(&server)
                .map(|state| state.shutting_down)
                .unwrap_or(true)
        })
    }

    pub fn mark_stopped(server: i64) {
        SERVERS.with(|servers| {
            if let Some(state) = servers.borrow_mut().get_mut(&server) {
                state.stopped = true;
            }
        });
    }

    pub fn is_stopped(server: i64) -> bool {
        SERVERS.with(|servers| {
            servers
                .borrow()
                .get(&server)
                .map(|state| state.stopped)
                .unwrap_or(true)
        })
    }

    pub fn shutdown(server: i64, force: bool) -> ShutdownSnapshot {
        SERVERS.with(|servers| {
            let mut servers = servers.borrow_mut();
            let Some(state) = servers.get_mut(&server) else {
                return ShutdownSnapshot::default();
            };
            state.shutting_down = true;
            state.force_shutdown |= force;
            let active = state.active.iter().copied().collect::<Vec<_>>();
            if force {
                state.active.clear();
            }
            ShutdownSnapshot {
                listener: Some(state.listener),
                scope: Some(state.scope),
                active,
            }
        })
    }
}

// ── Async backend handle (proposal 0174 Phase 1b-vi-a) ───────────────────────
// Lazy process-global `MioBackend` for fiber primops that need real reactor
// roundtrips (`FiberSleep` today; `Async.timeout` and TCP-suspending ops
// follow in 1b-vi-b/c). The reactor thread starts on first use and lives
// for the rest of the process — `MioBackend` is intentionally leaked via
// `Box::leak` rather than parked behind a `thread_local!` Drop. Reason:
// on Windows, TLS destructors fire during `RtlExitUserProcess`, which has
// already started killing other threads, so `JoinHandle::join` from
// `MioBackend::Drop` panics with "threads should not terminate
// unexpectedly". OS process exit reaps the reactor thread anyway, so we
// give up the (unused) graceful-shutdown semantics. With one fiber the
// OS-thread call stack *is* the continuation; this module deliberately
// does NOT involve the FiberScheduler or capture continuations — that's
// 1b-vi-b/c work.
mod vm_async {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::runtime::r#async::backend::{AsyncBackend, RequestId};
    use crate::runtime::r#async::backends::mio::{MioBackend, configure_default_dns_pool_size};

    static BACKEND: OnceLock<&'static MioBackend> = OnceLock::new();
    static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

    pub fn backend() -> Result<&'static MioBackend, String> {
        if let Some(b) = BACKEND.get() {
            return Ok(*b);
        }
        let owned = Box::leak(Box::new(MioBackend::new()));
        owned
            .start()
            .map_err(|e| format!("MioBackend::start failed: {e}"))?;
        // Race-tolerant: if another thread set this first, our leaked box
        // is harmlessly orphaned (process-lifetime anyway).
        Ok(*BACKEND.get_or_init(|| owned))
    }

    pub fn alloc_request_id() -> RequestId {
        RequestId(NEXT_REQ.fetch_add(1, Ordering::Relaxed))
    }

    pub fn configure_dns_pool_size(size: usize) {
        configure_default_dns_pool_size(size);
    }
}

// ── Fiber registry (proposal 0174 Phase 1b-vi-b₁) ────────────────────────────
// Plumbing-only: a per-thread `FiberScheduler` lives for the duration of an
// `Async.run_async` boundary, with a depth counter so nested boundaries share
// one instance. Each `FiberFork` goes through `scheduler.spawn` to allocate a
// real `FiberId`, and `with_current` tracks which fiber id the OS thread is
// currently executing so b₂ can target wakeups correctly. Execution order is
// unchanged — `FiberFork`'s body still runs inline to completion. The point
// is to land the registry shape without behaviour change so b₂ can replace
// the inline call with a real park/resume cycle without touching bookkeeping.
mod vm_fibers {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;
    use std::sync::{Arc, Condvar, Mutex};

    use std::sync::atomic::{AtomicU64, Ordering};

    use super::vm_async;
    use crate::runtime::RuntimeContext;
    use crate::runtime::r#async::await_coordinator::{AwaitCoordinator, AwaitEvent};
    use crate::runtime::r#async::backend::{AsyncBackend, RequestId};
    use crate::runtime::r#async::context::WorkerId;
    use crate::runtime::r#async::fiber::{Fiber, FiberId, FiberState};
    use crate::runtime::r#async::fiber_trace::{self, FiberEvent};
    use crate::runtime::r#async::scheduler::FiberScheduler;
    use crate::runtime::value::Value;

    thread_local! {
        static SCHED: RefCell<Option<FiberScheduler>> = const { RefCell::new(None) };
        static DEPTH: Cell<u32> = const { Cell::new(0) };
        static CURRENT: Cell<Option<FiberId>> = const { Cell::new(None) };
        // (frame_index, sp) recorded at the outermost FiberRunAsync entry —
        // the boundary that FiberSleep captures continuations down to
        // (proposal 0174 Phase 1b-vi-b₂.1).
        static BOUNDARY: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
        // Park request signalled by FiberSleep / FiberSuspend.  The dispatch
        // loop reads-and-clears this after each fiber tick to learn whether
        // the fiber suspended or finished.  Stores (request_id, captured cont
        // as a Value::Continuation).
        static PENDING_PARK: RefCell<Option<(u64, Value)>> = const { RefCell::new(None) };
        // Cooperative yield signalled by FiberYieldNow (Proposal 0174 slice
        // 2-vi).  Unlike PENDING_PARK there is no backend request: the
        // dispatch loop reads-and-clears this, re-enqueues the fiber at the
        // tail of its run-queue, and resumes it with `Unit`.  Stores the
        // captured continuation as a Value::Continuation.
        static PENDING_YIELD: RefCell<Option<Value>> = const { RefCell::new(None) };
        // Root fiber id for the current FiberRunAsync boundary; the dispatch
        // loop uses this to recognise root-fiber completion and propagate the
        // result.  None when no run_async is active.
        static ROOT: Cell<Option<FiberId>> = const { Cell::new(None) };
        // Synthetic-await coordination (Phase 1b-vi-b₂.2).
        static AWAITS: RefCell<AwaitCoordinator<FiberId, FiberOutcome>> =
            RefCell::new(AwaitCoordinator::new());
        static RESUME_OUTCOMES: RefCell<HashMap<u64, FiberOutcome>> = RefCell::new(HashMap::new());
        static PENDING_FIBER_ERROR: RefCell<Option<Value>> = const { RefCell::new(None) };
        // Scope-cancellation registry (Phase 1b-vi-c): scope_id → Vec<FiberId>.
        static SCOPE_REGISTRY: RefCell<HashMap<u64, Vec<FiberId>>> =
            RefCell::new(HashMap::new());
        // Set of fiber ids whose enclosing scope has been cancelled
        // (Phase 2 slice 2-iv). Populated by `cancel_losers` /
        // `FiberCancelScope`; queried by `FiberCheckCancelled` from a fiber
        // that may currently be executing (and therefore is not in the
        // scheduler's `suspended` map). Cleared at outermost run_async exit.
        static CANCELLED_IDS: RefCell<HashSet<FiberId>> =
            RefCell::new(HashSet::new());
        // Pending RuntimeConfig knobs for the next outermost FiberRunAsync
        // boundary (Phase 2 slice 2-vii). Set by `FiberRunAsyncWith` before
        // calling `enter_run_async`; consumed by `enter_run_async` and
        // cleared at outermost exit. `None` means "use defaults".
        static PENDING_RUN_CONFIG: Cell<Option<PendingRunConfig>> =
            const { Cell::new(None) };
        // Set by signal_park when the current fiber is already cancelled —
        // tells the call site to propagate cancellation rather than parking.
        // Cleared by check_and_clear_park_cancelled immediately after.
        static PARK_CANCELLED: Cell<bool> = const { Cell::new(false) };
        // Scoped task registry: scope_id → Vec<task_id>. Populated by
        // TaskSpawnScoped; cleared by FiberCancelScope and run_async exit.
        static SCOPE_TASK_REGISTRY: RefCell<HashMap<u64, Vec<i64>>> =
            RefCell::new(HashMap::new());
        // Root-task safety net for detached `Task.spawn` invoked while a
        // run_async is active. `Some` only between enter_run_async and exit.
        // Populated by TaskSpawn; deregistered by TaskAwait /
        // TaskBlockingJoin / TaskCancel; drained (and cancelled) at outermost
        // run_async exit so unawaited tasks cannot leak past the run.
        static RUN_ROOT_TASKS: RefCell<Option<HashSet<i64>>> =
            const { RefCell::new(None) };
        // Set to Some(...) by every OS worker (both worker 0 and background
        // workers) while a multi-worker run_async is executing.  All fiber
        // primop helpers (spawn_child_with_body, register_*_await, etc.) check
        // this first; if set they route through the shared state instead of
        // their per-thread TLS equivalents.  Cleared when the worker's dispatch
        // loop exits.
        static ACTIVE_SHARED: RefCell<Option<Arc<SharedDispatchState>>> =
            const { RefCell::new(None) };
    }

    /// RuntimeConfig knobs threaded from `FiberRunAsyncWith` into
    /// `enter_run_async` (Phase 2 slice 2-vii). Each field stored in raw
    /// form (a value of 0 means "default"). The library wrapper translates
    /// the source-level `Option<Int>` and ints into this.
    #[derive(Debug, Clone, Copy)]
    pub struct PendingRunConfig {
        pub worker_count: u32,
        // Accepted by RuntimeConfig for native/VM parity, but the VM does not
        // have a filesystem blocking pool to configure yet.
        #[allow(dead_code)]
        pub fs_pool_size: u32,
        pub dns_pool_size: u32,
    }

    #[derive(Clone)]
    pub enum FiberOutcome {
        Value(Value),
        Error(Value),
    }

    fn adt1(name: &str, value: Value) -> Value {
        use crate::runtime::value::{AdtFields, AdtValue};
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new(name.to_string()),
            fields: AdtFields::One(value),
        }))
    }

    pub fn async_panicked(message: impl Into<String>) -> Value {
        adt1("Panicked", Value::String(Rc::new(message.into())))
    }

    pub fn async_canceled() -> Value {
        Value::AdtUnit(Rc::new("Canceled".to_string()))
    }

    /// Signal an automatic cancellation error for the current fiber.
    /// Pair with `Err("__fiber_error__")` at the call site.
    pub fn signal_cancel_error() {
        signal_fiber_error(async_canceled());
    }

    /// Remove an orphaned await registration when a parent fiber aborts
    /// at park time due to cancellation.
    pub fn deregister_await(parent_req: u64) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner()).remove(parent_req);
        } else {
            AWAITS.with(|a| a.borrow_mut().remove(parent_req));
        }
    }

    fn result_ok(value: Value) -> Value {
        adt1("Ok", value)
    }

    fn result_err(err: Value) -> Value {
        adt1("Err", err)
    }

    pub fn signal_fiber_error(err: Value) {
        PENDING_FIBER_ERROR.with(|p| *p.borrow_mut() = Some(err));
    }

    fn take_fiber_error() -> Option<Value> {
        PENDING_FIBER_ERROR.with(|p| p.borrow_mut().take())
    }

    fn error_to_string(err: &Value) -> String {
        format!("AsyncError: {:?}", err)
    }

    /// Set the pending RuntimeConfig for the next outermost
    /// `FiberRunAsync`/`FiberRunAsyncWith` entry. No-op for nested entries
    /// (depth > 0): the outermost config wins.
    pub fn set_pending_run_config(cfg: PendingRunConfig) {
        PENDING_RUN_CONFIG.with(|c| c.set(Some(cfg)));
    }

    /// Resolve the worker count for `enter_run_async`. Order of precedence:
    ///   1. Explicit `PendingRunConfig.worker_count` (non-zero), set by
    ///      `FiberRunAsyncWith`.
    ///   2. `FLUX_WORKERS` env var, parsed once.
    ///   3. `std::thread::available_parallelism()` — the documented
    ///      default per `core/mod.rs::FiberRunAsyncWith` (proposal 0174
    ///      slice 2-vii). Mirrors `native_abi::resolve_default_worker_count`.
    ///   4. Hardcoded fallback of 2 logical workers when parallelism cannot
    ///      be determined (matches the Phase 1b-vi-c default).
    fn resolved_worker_count() -> usize {
        if let Some(cfg) = PENDING_RUN_CONFIG.with(|c| c.get())
            && cfg.worker_count > 0
        {
            return cfg.worker_count as usize;
        }
        if let Some(n) = env_workers_once() {
            return n;
        }
        1
    }

    /// Parse `FLUX_WORKERS` once per process; return `Some(n)` for a
    /// positive integer, `None` otherwise.
    fn env_workers_once() -> Option<usize> {
        use std::sync::OnceLock;
        static CACHED: OnceLock<Option<usize>> = OnceLock::new();
        *CACHED.get_or_init(|| {
            std::env::var("FLUX_WORKERS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| *n > 0)
        })
    }

    /// Process-global scope ID counter (1b-vi-c). Scopes across all boundaries
    /// and threads get unique IDs because this is atomic.
    static NEXT_SCOPE_ID: AtomicU64 = AtomicU64::new(1);

    /// Enter an `Async.run_async` boundary: lazy-init the scheduler on the
    /// outermost entry, spawn the root fiber, and make it current. Returns
    /// the root fiber id so the caller can pair this with `exit_run_async`.
    pub fn enter_run_async() -> FiberId {
        let depth = DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n
        });
        if depth == 0 {
            // Phase 2 slice 2-vii: respect the pending `RuntimeConfig`
            // knobs set by `FiberRunAsyncWith`. `fs_pool_size` is stored for
            // API parity but has no VM filesystem pool to configure yet.
            if let Some(cfg) = PENDING_RUN_CONFIG.with(|c| c.get())
                && cfg.dns_pool_size > 0
            {
                vm_async::configure_dns_pool_size(cfg.dns_pool_size as usize);
            }
            let n_workers = resolved_worker_count().max(1);
            SCHED.with(|s| {
                *s.borrow_mut() = Some(FiberScheduler::new(n_workers));
            });
            // Phase 2: arm the root-task safety net for `Task.spawn` calls
            // made inside this run_async. Cleared at outermost exit.
            RUN_ROOT_TASKS.with(|r| *r.borrow_mut() = Some(HashSet::new()));
        }
        let root = SCHED.with(|s| {
            s.borrow_mut()
                .as_mut()
                .expect("scheduler must be initialised after enter_run_async")
                .spawn(WorkerId(0))
        });
        CURRENT.with(|c| c.set(Some(root)));
        root
    }

    /// Pop the matching fiber off the ready queue (it was placed there by
    /// `spawn` and never executed via the scheduler — b₁ runs bodies inline).
    /// On outermost exit, tear down the scheduler.
    pub fn exit_run_async(_root: FiberId) {
        // Cancel all outstanding suspended-fiber backend requests so mio
        // reactor timers don't linger after run_async returns (1b-vi-c).
        let outstanding_reqs: Vec<u64> = SCHED.with(|s| {
            s.borrow()
                .as_ref()
                .map(|sched| {
                    (0..sched.num_workers())
                        .flat_map(|w| sched.all_suspended_reqs(WorkerId(w as u32)))
                        .collect()
                })
                .unwrap_or_default()
        });
        if !outstanding_reqs.is_empty() {
            if let Ok(backend) = vm_async::backend() {
                for req in outstanding_reqs {
                    backend.cancel(RequestId(req));
                }
            }
        }
        // Drain any leftover ready fibers (b₁ FiberFork bookkeeping
        // artifacts; race losers that finished after parent woke; etc).
        SCHED.with(|s| {
            if let Some(sched) = s.borrow_mut().as_mut() {
                while sched.next_ready_any().is_some() {}
            }
        });
        let new_depth = DEPTH.with(|d| {
            let n = d.get().saturating_sub(1);
            d.set(n);
            n
        });
        if new_depth == 0 {
            SCHED.with(|s| {
                *s.borrow_mut() = None;
            });
            CURRENT.with(|c| c.set(None));
            // Phase 1b-vi-b₂.2: also tear down await coordination state
            // so a second top-level run_async on the same thread starts fresh.
            clear_await_state();
            // Phase 2 slice 2-iv: drop the cancelled-id set so a second
            // top-level run_async on the same thread starts fresh.
            CANCELLED_IDS.with(|c| c.borrow_mut().clear());
            // Phase 2 slice 2-vii: drop the pending config so the next
            // top-level run_async picks up its own (or falls back to env/default).
            PENDING_RUN_CONFIG.with(|c| c.set(None));
            // Tear down the root-task safety net. Cancellation of any leftover
            // tasks happens at the dispatch site (which has the RuntimeContext)
            // via `take_all_root_tasks`; reaching this point should leave the
            // set empty.
            RUN_ROOT_TASKS.with(|r| *r.borrow_mut() = None);
        }
    }

    /// True if the current thread is at the outermost `run_async` boundary
    /// (depth == 1 — i.e., the next `exit_run_async` will tear down the
    /// scheduler). Used by the dispatch site to decide whether to drain the
    /// root-task safety net.
    pub fn at_outermost_run_async() -> bool {
        DEPTH.with(|d| d.get() == 1)
    }

    /// Drain the root-task safety net. Returns every task id still registered
    /// (i.e., spawned via detached `Task.spawn` and never awaited / joined /
    /// cancelled). Caller should `vm_task_cancel` each id before
    /// `exit_run_async` so unawaited tasks cannot leak past the run.
    pub fn take_all_root_tasks() -> Vec<i64> {
        RUN_ROOT_TASKS.with(|r| {
            r.borrow_mut()
                .as_mut()
                .map(|set| set.drain().collect())
                .unwrap_or_default()
        })
    }

    /// Add a task id to the root-task safety net if a run_async is active.
    /// No-op outside `run_async` (preserves detached semantics for
    /// `Task.spawn` invoked from synchronous code).
    pub fn register_root_task(task_id: i64) {
        RUN_ROOT_TASKS.with(|r| {
            if let Some(set) = r.borrow_mut().as_mut() {
                set.insert(task_id);
            }
        });
    }

    /// Remove a task id from the root-task safety net once the user owns
    /// the handle (await / blocking_join / cancel). No-op if the set is
    /// inactive or the id isn't tracked.
    pub fn deregister_root_task(task_id: i64) {
        RUN_ROOT_TASKS.with(|r| {
            if let Some(set) = r.borrow_mut().as_mut() {
                set.remove(&task_id);
            }
        });
    }

    /// Allocate a child fiber via the scheduler and return its id. Must be
    /// called inside an active `enter_run_async` / `exit_run_async` window.
    pub fn spawn_child() -> FiberId {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            return shared.sched.lock().unwrap_or_else(|e| e.into_inner())
                .spawn_child();
        }
        SCHED.with(|s| {
            s.borrow_mut()
                .as_mut()
                .expect("FiberFork outside Async.run_async — scheduler missing")
                .spawn_child()
        })
    }

    /// Run `f` with `id` recorded as the current fiber, restoring the previous
    /// value on return (including on early-return via `?`). The guard's
    /// `Drop` ensures CURRENT is always restored.
    pub fn with_current<F, R>(id: FiberId, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        struct Guard(Option<FiberId>);
        impl Drop for Guard {
            fn drop(&mut self) {
                CURRENT.with(|c| c.set(self.0));
            }
        }
        let prev = CURRENT.with(|c| c.replace(Some(id)));
        let _g = Guard(prev);
        f()
    }

    // ── Phase 1b-vi-b₂.1: park/resume + dispatch loop ───────────────────────

    /// Set the FiberRunAsync boundary, returning the previous value so the
    /// caller can restore it on exit (supports nested run_async).
    pub fn set_boundary(frame_index: usize, sp: usize) -> Option<(usize, usize)> {
        BOUNDARY.with(|b| b.replace(Some((frame_index, sp))))
    }

    pub fn restore_boundary(prev: Option<(usize, usize)>) {
        BOUNDARY.with(|b| b.set(prev));
    }

    pub fn boundary() -> Option<(usize, usize)> {
        BOUNDARY.with(|b| b.get())
    }

    /// FiberSleep / FiberSuspend signal that the current fiber has captured
    /// its continuation and parked on `req`.  The dispatch loop will see
    /// this on the next tick boundary.
    ///
    /// If the current fiber is already cancelled, `cont` is dropped and
    /// `PARK_CANCELLED` is set instead.  The call site must check
    /// `check_and_clear_park_cancelled()` immediately after and propagate
    /// the cancel error rather than returning `__fiber_park__`.
    pub fn signal_park(req: RequestId, cont: Value) {
        if is_current_cancelled() {
            drop(cont);
            PARK_CANCELLED.with(|c| c.set(true));
            return;
        }
        PENDING_PARK.with(|p| *p.borrow_mut() = Some((req.0, cont)));
    }

    /// Returns true and clears the flag if `signal_park` aborted due to
    /// cancellation.  Call immediately after `signal_park`.
    pub fn check_and_clear_park_cancelled() -> bool {
        PARK_CANCELLED.with(|c| {
            let v = c.get();
            c.set(false);
            v
        })
    }

    pub fn take_park() -> Option<(u64, Value)> {
        PENDING_PARK.with(|p| p.borrow_mut().take())
    }

    /// FiberYieldNow signals that the current fiber has captured its
    /// continuation and wants to be rescheduled behind any other ready
    /// fibers (Proposal 0174 slice 2-vi).  The caller has already verified
    /// the fiber is not cancelled (yield_now raises before reaching here).
    pub fn signal_yield(cont: Value) {
        PENDING_YIELD.with(|p| *p.borrow_mut() = Some(cont));
    }

    pub fn take_yield() -> Option<Value> {
        PENDING_YIELD.with(|p| p.borrow_mut().take())
    }

    /// Mark `id` as the root fiber for the current run_async boundary, and
    /// stash the body closure on the matching ready-queue Fiber so the
    /// dispatch loop knows to invoke it on first dispatch.
    pub fn set_root_with_body(id: FiberId, body: Value) {
        ROOT.with(|r| r.set(Some(id)));
        attach_body_to_ready_fiber(id, body);
    }

    fn attach_body_to_ready_fiber(id: FiberId, body: Value) {
        // Multi-worker path: use the shared scheduler.
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            let mut sched = shared.sched.lock().unwrap_or_else(|e| e.into_inner());
            let mut buf: Vec<Fiber> = Vec::new();
            while let Some((_worker, f)) = sched.next_ready_any() {
                buf.push(f);
            }
            for mut f in buf {
                if f.id == id {
                    f.body = Some(body.clone());
                }
                sched.spawn_existing(f);
            }
            return;
        }
        // Single-worker path: use thread-local scheduler.
        SCHED.with(|s| {
            if let Some(sched) = s.borrow_mut().as_mut() {
                let mut buf: Vec<Fiber> = Vec::new();
                while let Some((_worker, f)) = sched.next_ready_any() {
                    buf.push(f);
                }
                for mut f in buf {
                    if f.id == id {
                        f.body = Some(body.clone());
                    }
                    sched.spawn_existing(f);
                }
            }
        });
    }

    pub fn root() -> Option<FiberId> {
        ROOT.with(|r| r.get())
    }

    pub fn clear_root() {
        ROOT.with(|r| r.set(None));
    }

    /// Allocate a child fiber and attach `body` so the dispatch loop will
    /// invoke it on first run (proposal 0174 Phase 1b-vi-b₂.2). Mirror of
    /// `set_root_with_body` but for child fibers.
    pub fn spawn_child_with_body(body: Value) -> FiberId {
        let id = if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.sched.lock().unwrap_or_else(|e| e.into_inner())
                .spawn_child()
        } else {
            SCHED.with(|s| {
                s.borrow_mut()
                    .as_mut()
                    .expect("spawn_child_with_body outside Async.run_async")
                    .spawn_child()
            })
        };
        attach_body_to_ready_fiber(id, body);
        id
    }

    /// Register a `FiberBoth` await: parent_req fires when both children finish.
    pub fn register_both_await(parent_req: u64, left: FiberId, right: FiberId) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .register_both(parent_req, left, right);
        } else {
            AWAITS.with(|a| a.borrow_mut().register_both(parent_req, left, right));
        }
    }

    pub fn register_try_await(parent_req: u64, child: FiberId) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .register_try(parent_req, child, ());
        } else {
            AWAITS.with(|a| a.borrow_mut().register_try(parent_req, child, ()));
        }
    }

    /// Register a `FiberTimeout` await: parent_req fires when either the
    /// body child completes (delivers `Some(result)`) or a backend timer
    /// keyed on `parent_req` fires (delivers `None`).
    pub fn register_timeout_await(parent_req: u64, body_child: FiberId) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .register_timeout(parent_req, body_child);
        } else {
            AWAITS.with(|a| a.borrow_mut().register_timeout(parent_req, body_child));
        }
    }

    /// Called by the backend completion pump just before `scheduler.complete`.
    /// If the request id matches a Timeout await, set the parent's resume
    /// value to `None` and discard the await metadata so that a later body
    /// completion is dropped silently.  Returns `Some(body_child)` if this
    /// was a Timeout-routed completion — the body_child fiber should be
    /// cancelled by the caller (1b-vi-c).  Returns `None` for non-Timeout
    /// completions (caller may still call `scheduler.complete`).
    pub fn try_route_timer_for_timeout(req: u64) -> Option<FiberId> {
        let event = if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .route_timeout_timer(req)
        } else {
            AWAITS.with(|a| a.borrow_mut().route_timeout_timer(req))
        };
        match event {
            Some(AwaitEvent::TimeoutTimerReady { body, .. }) => {
                set_resume_outcome(req, FiberOutcome::Value(Value::None));
                Some(body)
            }
            _ => None,
        }
    }

    /// Register a `FiberRace` await: parent_req fires when any child finishes.
    pub fn register_race_await(parent_req: u64, children: Vec<FiberId>) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .register_race(parent_req, children);
        } else {
            AWAITS.with(|a| a.borrow_mut().register_race(parent_req, children));
        }
    }

    /// Register a `FiberFirstOf` await: parent_req fires when the first child
    /// in source-order tie-break semantics finishes.
    pub fn register_first_of_await(parent_req: u64, children: Vec<(FiberId, usize)>) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                .register_first_of(parent_req, children);
        } else {
            AWAITS.with(|a| a.borrow_mut().register_first_of(parent_req, children));
        }
    }

    pub fn set_resume_outcome(req: u64, outcome: FiberOutcome) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.resume_outcomes.lock().unwrap_or_else(|e| e.into_inner())
                .insert(req, outcome);
        } else {
            RESUME_OUTCOMES.with(|r| r.borrow_mut().insert(req, outcome));
        }
    }

    pub fn take_resume_outcome(req: u64) -> Option<FiberOutcome> {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.resume_outcomes.lock().unwrap_or_else(|e| e.into_inner())
                .remove(&req)
        } else {
            RESUME_OUTCOMES.with(|r| r.borrow_mut().remove(&req))
        }
    }

    /// Outcome of a child fiber finishing (proposal 0174 Phase 1b-vi-c).
    pub struct FiberDoneOutcome {
        /// `(parent_req, resume_val)` pairs to flush via `set_resume_value` +
        /// `scheduler.complete`.
        pub completions: Vec<(u64, FiberOutcome)>,
        /// Fibers that became losers as a result of this completion and should
        /// be cancelled by the caller (race losers, timeout body child).
        pub losers: Vec<FiberId>,
    }

    /// A child fiber finished with `value`. Stash the result, walk awaiters,
    /// determine which parent requests are now satisfiable, build their
    /// resume values, and return the outcome.  The caller (dispatch loop) is
    /// responsible for storing each resume value via `set_resume_value`,
    /// calling `scheduler.complete(parent_req)` to wake the parent, and
    /// calling `cancel_losers` for the returned losers — all done outside
    /// the AWAITS borrow to avoid re-entrant `RefCell` panics.
    pub fn on_fiber_done(id: FiberId, value: Value) -> FiberDoneOutcome {
        on_fiber_done_outcome(id, FiberOutcome::Value(value))
    }

    pub fn on_fiber_error(id: FiberId, err: Value) -> FiberDoneOutcome {
        on_fiber_done_outcome(id, FiberOutcome::Error(err))
    }

    fn on_fiber_done_outcome(id: FiberId, outcome: FiberOutcome) -> FiberDoneOutcome {
        let events = AWAITS.with(|a| {
            a.borrow_mut()
                .record_completed(id, outcome, is_ready, |outcome| {
                    matches!(outcome, FiberOutcome::Error(_))
                })
        });
        events_to_done_outcome(events)
    }

    /// Re-evaluate deferred `first_of` awaits when a child parks. This is what
    /// lets a later completed child win once all earlier source-order siblings
    /// have either completed or reached a suspension point.
    pub fn on_fiber_suspended(id: FiberId) -> FiberDoneOutcome {
        let events = AWAITS.with(|a| a.borrow_mut().record_suspended(id, is_ready));
        events_to_done_outcome(events)
    }

    fn is_ready(id: FiberId) -> bool {
        SCHED.with(|s| {
            s.borrow()
                .as_ref()
                .map(|sched| sched.is_ready(id))
                .unwrap_or(false)
        })
    }

    fn events_to_done_outcome(
        events: Vec<AwaitEvent<FiberId, FiberOutcome, ()>>,
    ) -> FiberDoneOutcome {
        let mut completions = Vec::new();
        let mut losers = Vec::new();

        for event in events {
            match event {
                AwaitEvent::BothReady {
                    request,
                    left,
                    right,
                } => match (left, right) {
                    (FiberOutcome::Value(left), FiberOutcome::Value(right)) => {
                        let tuple = Value::Tuple(Rc::new(vec![left, right]));
                        completions.push((request, FiberOutcome::Value(tuple)));
                    }
                    (FiberOutcome::Error(err), other) | (other, FiberOutcome::Error(err)) => {
                        drop(other);
                        completions.push((request, FiberOutcome::Error(err)));
                    }
                },
                AwaitEvent::BothError {
                    request,
                    error,
                    loser,
                    discarded,
                } => {
                    for value in discarded {
                        drop(value);
                    }
                    completions.push((request, error));
                    losers.push(loser);
                }
                AwaitEvent::TryReady {
                    request, outcome, ..
                } => {
                    let result = match outcome {
                        FiberOutcome::Value(v) => result_ok(v),
                        FiberOutcome::Error(err) => result_err(err),
                    };
                    completions.push((request, FiberOutcome::Value(result)));
                }
                AwaitEvent::RaceReady {
                    request,
                    outcome,
                    losers: event_losers,
                    discarded,
                } => {
                    for value in discarded {
                        drop(value);
                    }
                    completions.push((request, outcome));
                    losers.extend(event_losers);
                }
                AwaitEvent::FirstOfReady {
                    request,
                    index,
                    outcome,
                    losers: event_losers,
                    discarded,
                } => {
                    for value in discarded {
                        drop(value);
                    }
                    match outcome {
                        FiberOutcome::Value(result) => {
                            let tuple =
                                Value::Tuple(Rc::new(vec![Value::Integer(index as i64), result]));
                            completions.push((request, FiberOutcome::Value(tuple)));
                        }
                        FiberOutcome::Error(err) => {
                            completions.push((request, FiberOutcome::Error(err)));
                        }
                    }
                    losers.extend(event_losers);
                }
                AwaitEvent::TimeoutBodyReady { request, outcome } => match outcome {
                    FiberOutcome::Value(result) => {
                        completions
                            .push((request, FiberOutcome::Value(Value::Some(Rc::new(result)))));
                    }
                    FiberOutcome::Error(err) => {
                        completions.push((request, FiberOutcome::Error(err)));
                    }
                },
                AwaitEvent::TimeoutTimerReady { .. } => {}
            }
        }

        FiberDoneOutcome {
            completions,
            losers,
        }
    }

    /// Cancel a set of fibers: look up each fiber's pending backend request,
    /// call `backend.cancel` to stop I/O work, then mark the fiber `Cancelled`
    /// in the scheduler so the dispatch loop resumes it with `AsyncError.Canceled`
    /// (proposal 0174 1b-vi-c).  Always cancel the backend request *before*
    /// moving the fiber to `Cancelled` so a late-arriving completion cannot
    /// re-queue a fiber that is about to be dropped.
    pub fn cancel_losers(
        loser_ids: &[FiberId],
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            cancel_losers_shared(loser_ids, backend, &shared);
            return;
        }
        let reqs: Vec<u64> = SCHED.with(|s| {
            s.borrow()
                .as_ref()
                .map(|sc| {
                    loser_ids
                        .iter()
                        .filter_map(|id| sc.find_request_for_fiber(*id))
                        .collect()
                })
                .unwrap_or_default()
        });
        for req in reqs {
            backend.cancel(RequestId(req));
        }
        SCHED.with(|s| {
            if let Some(sc) = s.borrow_mut().as_mut() {
                sc.cancel_fibers(loser_ids);
            }
        });
        mark_cancelled(loser_ids);
    }

    /// Record `ids` as cancelled so a fiber that is currently *executing*
    /// (not in the scheduler's `suspended` map) can observe its scope's
    /// cancellation via `is_current_cancelled()` (Phase 2 slice 2-iv).
    pub fn mark_cancelled(ids: &[FiberId]) {
        if ids.is_empty() {
            return;
        }
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            let mut set = shared.cancelled_ids.lock().unwrap_or_else(|e| e.into_inner());
            for id in ids {
                set.insert(*id);
            }
            return;
        }
        CANCELLED_IDS.with(|c| {
            let mut set = c.borrow_mut();
            for id in ids {
                set.insert(*id);
            }
        });
    }

    /// True if the current fiber's enclosing scope has been cancelled.
    /// Returns `false` outside any `Async.run_async` boundary.
    pub fn is_current_cancelled() -> bool {
        let id = match CURRENT.with(|c| c.get()) {
            Some(id) => id,
            None => return false,
        };
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            return shared.cancelled_ids.lock().unwrap_or_else(|e| e.into_inner())
                .contains(&id);
        }
        CANCELLED_IDS.with(|c| c.borrow().contains(&id))
    }

    /// Return the current fiber id as a raw u64 (0 when outside run_async).
    /// Used only for fiber-trace event attribution.
    pub fn current_fiber_id() -> u64 {
        CURRENT.with(|c| c.get().map(|id| id.0).unwrap_or(0))
    }

    /// Report the worker count of the currently active `FiberScheduler`.
    /// Returns 0 when no scheduler is active (i.e., outside `run_async`).
    /// Exposed to user code via `Async.current_worker_count` and the
    /// `FiberCurrentWorkerCount` primop.
    pub fn current_num_workers() -> usize {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            return shared.sched.lock().unwrap_or_else(|e| e.into_inner()).num_workers();
        }
        SCHED.with(|s| s.borrow().as_ref().map(|sc| sc.num_workers()).unwrap_or(0))
    }

    // ── Scope helpers (1b-vi-c) ─────────────────────────────────────────

    /// Allocate a fresh scope ID and register an empty fiber list for it.
    pub fn new_scope() -> u64 {
        let id = NEXT_SCOPE_ID.fetch_add(1, Ordering::Relaxed);
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.scope_registry.lock().unwrap_or_else(|e| e.into_inner())
                .insert(id, Vec::new());
        } else {
            SCOPE_REGISTRY.with(|r| r.borrow_mut().insert(id, Vec::new()));
        }
        id
    }

    /// Register a fiber under a scope so it can be cancelled with the scope.
    pub fn register_fiber_in_scope(scope_id: u64, fiber_id: FiberId) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.scope_registry.lock().unwrap_or_else(|e| e.into_inner())
                .entry(scope_id).or_default().push(fiber_id);
        } else {
            SCOPE_REGISTRY.with(|r| {
                r.borrow_mut().entry(scope_id).or_default().push(fiber_id);
            });
        }
    }

    /// Remove and return all fibers registered under a scope.
    pub fn take_scope_fibers(scope_id: u64) -> Vec<FiberId> {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.scope_registry.lock().unwrap_or_else(|e| e.into_inner())
                .remove(&scope_id).unwrap_or_default()
        } else {
            SCOPE_REGISTRY.with(|r| r.borrow_mut().remove(&scope_id).unwrap_or_default())
        }
    }

    pub fn register_task_in_scope(scope_id: u64, task_id: i64) {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.scope_task_registry.lock().unwrap_or_else(|e| e.into_inner())
                .entry(scope_id).or_default().push(task_id);
        } else {
            SCOPE_TASK_REGISTRY.with(|r| {
                r.borrow_mut().entry(scope_id).or_default().push(task_id);
            });
        }
    }

    pub fn take_scope_tasks(scope_id: u64) -> Vec<i64> {
        if let Some(shared) = ACTIVE_SHARED.with(|s| s.borrow().clone()) {
            shared.scope_task_registry.lock().unwrap_or_else(|e| e.into_inner())
                .remove(&scope_id).unwrap_or_default()
        } else {
            SCOPE_TASK_REGISTRY.with(|r| r.borrow_mut().remove(&scope_id).unwrap_or_default())
        }
    }

    /// Tear down the await-coordination state at run_async exit
    /// (Phase 1b-vi-b₂.2). Avoids leaks across nested boundaries.
    pub fn clear_await_state() {
        AWAITS.with(|a| a.borrow_mut().clear());
        RESUME_OUTCOMES.with(|r| r.borrow_mut().clear());
        SCOPE_REGISTRY.with(|r| r.borrow_mut().clear());
        SCOPE_TASK_REGISTRY.with(|r| r.borrow_mut().clear());
        // Defense in depth: the dispatch site is supposed to drain
        // RUN_ROOT_TASKS before exit_run_async. If anything bypasses that
        // path, drop the leftover ids here so they don't bleed across runs.
        RUN_ROOT_TASKS.with(|r| {
            if let Some(set) = r.borrow_mut().as_mut() {
                set.clear();
            }
        });
    }

    /// Drive the FiberRunAsync dispatch loop until all fibers are done or
    /// the root fiber returns a value.  Invokes fiber bodies and resumes
    /// parked fibers via the supplied `ctx`; pumps the mio backend when no
    /// fiber is ready.
    pub fn dispatch_loop(
        ctx: &mut dyn RuntimeContext,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> Result<Value, String> {
        let root_id = root().expect("dispatch_loop called outside FiberRunAsync");
        let mut root_result: Option<Value> = None;

        loop {
            // Drain ready queue.
            loop {
                let next = SCHED.with(|s| {
                    s.borrow_mut()
                        .as_mut()
                        .expect("scheduler missing in dispatch_loop")
                        .next_ready_any()
                });
                let Some((_worker, mut fiber)) = next else {
                    break;
                };

                // Skip fibers with no work (b₁ FiberFork pushes a fiber but
                // also runs its body inline; the fiber on the ready queue is
                // a bookkeeping artifact with no body and no parked cont).
                if fiber.body.is_none() && fiber.parked.is_none() {
                    continue;
                }

                // Cancelled fibers (1b-vi-c): resume their parked continuation
                // with `AsyncError.Canceled` so that `bracket`/`finally`
                // cleanup arms run before the fiber exits.  If there is no
                // continuation (body never ran), the fiber is dropped silently.
                if fiber.state == FiberState::Cancelled {
                    if let Some(cont) = fiber.parked.take() {
                        // Resume with Unit (None) rather than Canceled so that
                        // intermediate operation return-type contracts are not
                        // violated (e.g. sleep -> Unit).  The fiber completes
                        // normally: bracket's release arm still runs since
                        // bracket is sequential (body → release).
                        let cancel_val = Value::None;
                        let fid = fiber.id;
                        let outcome = with_current(fid, || {
                            ctx.resume_from_dispatch(Value::Continuation(cont), cancel_val)
                        });
                        // The fiber may park again (e.g. inside a release arm
                        // that does async I/O).  Handle exactly like normal park.
                        if let Some((req, cont_val)) = take_park() {
                            let cont_rc =
                                match cont_val {
                                    Value::Continuation(rc) => rc,
                                    _ => return Err(
                                        "cancelled fiber re-park: non-Continuation in PENDING_PARK"
                                            .into(),
                                    ),
                                };
                            fiber.parked = Some(cont_rc);
                            fiber.state = FiberState::Suspended { request_id: req };
                            let home_worker = fiber.home_worker;
                            fiber_trace::emit(FiberEvent::Suspend {
                                fid: fid.0,
                                worker: home_worker.0,
                                request_id: req,
                            });
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .insert_suspended(home_worker, req, fiber);
                            });
                            let done = on_fiber_suspended(fid);
                            if !done.losers.is_empty() {
                                cancel_losers(&done.losers, backend);
                            }
                            for (pr, rv) in done.completions {
                                set_resume_outcome(pr, rv);
                                SCHED.with(|s| {
                                    s.borrow_mut()
                                        .as_mut()
                                        .expect("scheduler missing")
                                        .complete_request(RequestId(pr));
                                });
                            }
                            continue;
                        }
                        // Fiber ran to completion after cleanup.
                        if let Ok(v) = outcome {
                            let done = on_fiber_done(fid, v);
                            if !done.losers.is_empty() {
                                cancel_losers(&done.losers, backend);
                            }
                            for (pr, rv) in done.completions {
                                set_resume_outcome(pr, rv);
                                SCHED.with(|s| {
                                    s.borrow_mut()
                                        .as_mut()
                                        .expect("scheduler missing")
                                        .complete_request(RequestId(pr));
                                });
                            }
                        }
                        // Errors from cancelled fibers' cleanup are swallowed.
                    }
                    // No continuation → bookkeeping artifact; drop silently.
                    continue;
                }

                let fiber_id = fiber.id;
                let fiber_home_worker = fiber.home_worker.0;
                // Resume value: if the wakeup was caused by a synthetic await
                // (FiberBoth / FiberRace), the dispatch loop stored a value
                // keyed by the request id when the children finished.
                // Default for backend-timer wakeups is Value::None.
                let resume_outcome = fiber
                    .last_completion_req
                    .take()
                    .and_then(take_resume_outcome)
                    .unwrap_or(FiberOutcome::Value(Value::None));
                let resume_val = match resume_outcome {
                    FiberOutcome::Value(v) => v,
                    FiberOutcome::Error(err) => {
                        if fiber_id == root_id {
                            return Err(error_to_string(&err));
                        }
                        let done = on_fiber_error(fiber_id, err);
                        if !done.losers.is_empty() {
                            cancel_losers(&done.losers, backend);
                        }
                        for (pr, rv) in done.completions {
                            set_resume_outcome(pr, rv);
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .complete_request(RequestId(pr));
                            });
                        }
                        continue;
                    }
                };
                let outcome = with_current(fiber_id, || {
                    if let Some(cont) = fiber.parked.take() {
                        ctx.resume_from_dispatch(Value::Continuation(cont), resume_val)
                    } else if let Some(body) = fiber.body.take() {
                        ctx.invoke_value(body, vec![])
                    } else {
                        unreachable!("checked above")
                    }
                });

                // Did the fiber park during this tick?  FiberSleep/FiberSuspend
                // capture+unwind+set PENDING_PARK, then return Err so the
                // outcome we observe here is Err with the park signal staged.
                if let Some((req, cont_val)) = take_park() {
                    let cont_rc = match cont_val {
                        Value::Continuation(rc) => rc,
                        _ => {
                            return Err(
                                "dispatch_loop: PENDING_PARK contained non-Continuation value"
                                    .into(),
                            );
                        }
                    };
                    fiber.parked = Some(cont_rc);
                    fiber.state = FiberState::Suspended { request_id: req };
                    let home_worker = fiber.home_worker;
                    fiber_trace::emit(FiberEvent::Suspend {
                        fid: fiber_id.0,
                        worker: home_worker.0,
                        request_id: req,
                    });
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .insert_suspended(home_worker, req, fiber);
                    });
                    let done = on_fiber_suspended(fiber_id);
                    if !done.losers.is_empty() {
                        cancel_losers(&done.losers, backend);
                    }
                    for (pr, rv) in done.completions {
                        set_resume_outcome(pr, rv);
                        SCHED.with(|s| {
                            s.borrow_mut()
                                .as_mut()
                                .expect("scheduler missing")
                                .complete_request(RequestId(pr));
                        });
                    }
                    // The Err that surfaced was the park-unwind signal, not
                    // a real error; ignore it.
                    let _ = outcome;
                    continue;
                }

                // Did the fiber yield during this tick?  FiberYieldNow
                // captures its continuation, sets PENDING_YIELD, and returns
                // Err with no backend request.  Re-enqueue at the tail of the
                // run-queue so any other ready fibers run first, then resume
                // this one with Unit (last_completion_req stays None →
                // resume_outcome defaults to Value::None).
                if let Some(cont_val) = take_yield() {
                    let cont_rc = match cont_val {
                        Value::Continuation(rc) => rc,
                        _ => {
                            return Err(
                                "dispatch_loop: PENDING_YIELD contained non-Continuation value"
                                    .into(),
                            );
                        }
                    };
                    fiber.parked = Some(cont_rc);
                    fiber.state = FiberState::Ready;
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .spawn_existing(fiber);
                    });
                    // The Err that surfaced was the yield-unwind signal.
                    let _ = outcome;
                    continue;
                }

                // No park: outcome is the fiber's actual result or a real error.
                match outcome {
                    Ok(v) => {
                        if fiber_id == root_id {
                            root_result = Some(v.clone());
                        }
                        fiber_trace::emit(FiberEvent::Complete {
                            fid: fiber_id.0,
                            worker: fiber_home_worker,
                        });
                        // Synthetic-await coordination: record the result and
                        // wake any parents whose await condition is now met.
                        // Set each resume value *before* calling
                        // scheduler.complete so the resumed parent fiber sees
                        // its expected resume value.  Also cancel any losers
                        // (race losers, timeout body child) — 1b-vi-c.
                        let outcome = on_fiber_done(fiber_id, v);
                        if !outcome.losers.is_empty() {
                            cancel_losers(&outcome.losers, backend);
                        }
                        for (parent_req, resume_val) in outcome.completions {
                            set_resume_outcome(parent_req, resume_val);
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .complete_request(RequestId(parent_req));
                            });
                        }
                    }
                    Err(e) => {
                        let err = take_fiber_error().unwrap_or_else(|| async_panicked(e));
                        if fiber_id == root_id {
                            return Err(error_to_string(&err));
                        }
                        let outcome = on_fiber_error(fiber_id, err);
                        if !outcome.losers.is_empty() {
                            cancel_losers(&outcome.losers, backend);
                        }
                        for (parent_req, resume_val) in outcome.completions {
                            set_resume_outcome(parent_req, resume_val);
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .complete_request(RequestId(parent_req));
                            });
                        }
                    }
                }
            }

            // Exit as soon as the root fiber is done. Any remaining suspended
            // fibers (e.g. race losers not yet cancelled) will have their
            // backend requests cancelled by exit_run_async (1b-vi-c).
            if root_result.is_some() {
                break;
            }
            // No more ready fibers.  If nothing is suspended either, we're done.
            let suspended = SCHED.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sched| sched.total_suspended_count())
                    .unwrap_or(0)
            });
            if suspended == 0 {
                break;
            }

            // Pump the backend until a completion arrives, then route it.
            loop {
                if let Some((request_id, result)) = super::super::task::try_recv_completion() {
                    let outcome = match result {
                        Ok(value) => FiberOutcome::Value(Value::Some(Rc::new(value))),
                        Err(err) if err == "TaskCancelled" => FiberOutcome::Value(Value::None),
                        Err(err) => FiberOutcome::Error(async_panicked(err)),
                    };
                    set_resume_outcome(request_id, outcome);
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .complete_request(RequestId(request_id));
                    });
                    break;
                }
                if let Some((request_id, result)) = super::super::channel::try_recv_completion() {
                    let outcome = match result {
                        Ok(value) => FiberOutcome::Value(value),
                        Err(err) => FiberOutcome::Error(async_panicked(err)),
                    };
                    set_resume_outcome(request_id, outcome);
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .complete_request(RequestId(request_id));
                    });
                    break;
                }
                if let Some(request_id) = super::super::event::try_recv_completion() {
                    set_resume_outcome(request_id, FiberOutcome::Value(Value::None));
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .complete_request(RequestId(request_id));
                    });
                    break;
                }
                if let Some(c) = backend.next_completion() {
                    // Route TCP payloads as resume values before waking the fiber.
                    use crate::runtime::r#async::backend::CompletionPayload;
                    if let CompletionPayload::AddressList(addrs) = &c.payload {
                        if let Some(addr) = addrs.first() {
                            backend.tcp_connect(c.request_id, *addr);
                        } else {
                            set_resume_outcome(
                                c.request_id.0,
                                FiberOutcome::Error(async_panicked(
                                    "dns resolve returned no addresses",
                                )),
                            );
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .complete_request(c.request_id);
                            });
                            break;
                        }
                        continue;
                    }
                    match &c.payload {
                        CompletionPayload::TcpHandle(h) => {
                            set_resume_outcome(
                                c.request_id.0,
                                FiberOutcome::Value(Value::Integer(h.0 as i64)),
                            );
                        }
                        CompletionPayload::Bytes(buf) => {
                            let s = String::from_utf8_lossy(buf).into_owned();
                            set_resume_outcome(
                                c.request_id.0,
                                FiberOutcome::Value(Value::String(Rc::new(s))),
                            );
                        }
                        CompletionPayload::Unit => {}
                        CompletionPayload::Error(e) => {
                            set_resume_outcome(
                                c.request_id.0,
                                FiberOutcome::Error(async_panicked(e.clone())),
                            );
                        }
                        CompletionPayload::AddressList(_) => unreachable!("handled above"),
                    }
                    // If this completion is the timer half of a FiberTimeout
                    // await, set the parent's resume value to None and cancel
                    // the body child fiber (1b-vi-c).
                    if let Some(body_child) = try_route_timer_for_timeout(c.request_id.0) {
                        cancel_losers(&[body_child], backend);
                    }
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .complete_request(c.request_id);
                    });
                    break;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        }

        Ok(root_result.unwrap_or(Value::None))
    }

    // ── Phase 4: Multi-OS-thread worker dispatch ──────────────────────────────
    //
    // When `n_workers > 1`, `enter_run_async_multi` is called instead of
    // running `dispatch_loop` on a single OS thread.  The entry point:
    //   1. Builds a `SharedDispatchState` wrapping the cross-thread scheduler
    //      and coordination state behind `Arc<Mutex<_>>`.
    //   2. Spawns `n_workers - 1` OS threads, each running `worker_dispatch_loop`.
    //   3. Runs `worker_0_dispatch_loop` on the caller's thread (worker 0).
    //
    // The existing single-threaded `dispatch_loop` is unchanged and remains the
    // fast path for `n_workers == 1`.

    /// Per-worker condvar for parking when the ready queue is empty.
    pub struct WorkerWakeup {
        pub has_work: Mutex<bool>,
        pub cv: Condvar,
    }

    impl WorkerWakeup {
        pub fn new() -> Arc<Self> {
            Arc::new(WorkerWakeup {
                has_work: Mutex::new(false),
                cv: Condvar::new(),
            })
        }

        /// Wake this worker: set the flag and notify one waiter.
        pub fn wake(&self) {
            let mut guard = self.has_work.lock().unwrap_or_else(|e| e.into_inner());
            *guard = true;
            self.cv.notify_one();
        }

        /// Park until work arrives or `timeout` elapses.
        pub fn park_timeout(&self, timeout: std::time::Duration) {
            let guard = self.has_work.lock().unwrap_or_else(|e| e.into_inner());
            let (mut guard, _) = self.cv.wait_timeout(guard, timeout).unwrap_or_else(|e| e.into_inner());
            *guard = false;
        }
    }

    /// Outcome of a completed fiber, sent back to the coordination layer.
    #[allow(dead_code)]
    pub enum WorkerFiberResult {
        /// Fiber completed normally with a value.
        Done { fiber_id: FiberId, value: Value },
        /// Fiber parked: captured continuation + request id.
        Parked { fiber_id: FiberId, request_id: u64, cont: Value, home_worker: WorkerId },
        /// Fiber yielded cooperatively.
        Yielded { fiber_id: FiberId, cont: Value, home_worker: WorkerId },
        /// Fiber errored.
        Error { fiber_id: FiberId, err_value: Value },
    }

    /// State shared across all OS worker threads during a multi-worker run.
    ///
    /// All fields that are accessed from multiple workers are behind `Arc<Mutex<_>>`.
    /// Per-worker fields (like the wakeup condvars) are indexed by worker id.
    ///
    /// # Safety
    ///
    /// `SharedDispatchState` contains `FiberOutcome` (which wraps `Value`) inside
    /// `Mutex`-guarded fields. `Value` is not `Send` because it uses `Rc` for
    /// reference counting. However, all `Value` data inside `SharedDispatchState`
    /// is protected by `Mutex`: no two OS threads can access the same `Value`
    /// concurrently. Values are moved into the mutex by the producing worker and
    /// moved out by the consuming worker; they are never accessed simultaneously.
    ///
    /// The `no-migration` invariant means each `Value` in `resume_outcomes` was
    /// produced by the completing fiber's home worker and consumed by the parent
    /// fiber's home worker — always under mutex lock, never concurrently.
    pub struct SharedDispatchState {
        /// The fiber scheduler: owns the per-worker ready queues and suspended map.
        pub sched: Arc<Mutex<FiberScheduler>>,
        /// Synthetic-await coordination (FiberBoth / FiberRace / etc.).
        pub awaits: Arc<Mutex<AwaitCoordinator<FiberId, FiberOutcome>>>,
        /// Resume outcomes keyed by request id (parent-fiber wakeup values).
        pub resume_outcomes: Arc<Mutex<HashMap<u64, FiberOutcome>>>,
        /// Scope-cancellation registry: scope_id → Vec<FiberId>.
        /// Reserved for future Phase 4+ scope cancellation across workers.
        #[allow(dead_code)]
        pub scope_registry: Arc<Mutex<HashMap<u64, Vec<FiberId>>>>,
        /// Set of fiber ids whose enclosing scope has been cancelled.
        pub cancelled_ids: Arc<Mutex<HashSet<FiberId>>>,
        /// Scoped-task registry: scope_id → Vec<task_id>.
        /// Reserved for future Phase 4+ scoped task tracking across workers.
        #[allow(dead_code)]
        pub scope_task_registry: Arc<Mutex<HashMap<u64, Vec<i64>>>>,
        /// Root-task safety net for detached Task.spawn inside run_async.
        /// Reserved for future Phase 4+ root-task tracking across workers.
        #[allow(dead_code)]
        pub run_root_tasks: Arc<Mutex<Option<HashSet<i64>>>>,
        /// Per-worker condvars for park/wake signalling.
        pub worker_wakeups: Vec<Arc<WorkerWakeup>>,
        /// Root fiber id for this run_async boundary.
        pub root_id: FiberId,
        /// Root result (written by worker 0 when the root fiber completes;
        /// stored as a `String` error or a flag for success, then retrieved
        /// directly from `worker_0_dispatch_loop`'s return value).
        pub root_error: Arc<Mutex<Option<String>>>,
        /// Whether the overall run is finished (written true when root result is set).
        pub finished: Arc<Mutex<bool>>,
    }

    // SAFETY: See doc comment on `SharedDispatchState`. All `Value` data is
    // guarded by `Mutex`; no concurrent access to any `Value` occurs.
    unsafe impl Send for SharedDispatchState {}
    unsafe impl Sync for SharedDispatchState {}

    impl SharedDispatchState {
        fn new(sched: FiberScheduler, root_id: FiberId, n_workers: usize) -> Arc<Self> {
            let worker_wakeups = (0..n_workers).map(|_| WorkerWakeup::new()).collect();
            Arc::new(SharedDispatchState {
                sched: Arc::new(Mutex::new(sched)),
                awaits: Arc::new(Mutex::new(AwaitCoordinator::new())),
                resume_outcomes: Arc::new(Mutex::new(HashMap::new())),
                scope_registry: Arc::new(Mutex::new(HashMap::new())),
                cancelled_ids: Arc::new(Mutex::new(HashSet::new())),
                scope_task_registry: Arc::new(Mutex::new(HashMap::new())),
                run_root_tasks: Arc::new(Mutex::new(Some(HashSet::new()))),
                worker_wakeups,
                root_id,
                root_error: Arc::new(Mutex::new(None)),
                finished: Arc::new(Mutex::new(false)),
            })
        }

        /// Deliver a completion: store the resume value and wake the fiber's home worker.
        fn deliver_completion(
            &self,
            request_id: u64,
            outcome: FiberOutcome,
        ) {
            self.resume_outcomes.lock().unwrap_or_else(|e| e.into_inner())
                .insert(request_id, outcome);
            let mut sched = self.sched.lock().unwrap_or_else(|e| e.into_inner());
            let home_worker = sched.complete_request(RequestId(request_id));
            drop(sched);
            if let Some(w) = home_worker {
                let idx = w.0 as usize;
                if idx < self.worker_wakeups.len() {
                    self.worker_wakeups[idx].wake();
                }
            }
        }

        /// Check whether the run is complete.
        fn is_finished(&self) -> bool {
            *self.finished.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Mark the run as finished and wake all workers so they can exit.
        fn set_finished(&self) {
            *self.finished.lock().unwrap_or_else(|e| e.into_inner()) = true;
            for wu in &self.worker_wakeups {
                wu.wake();
            }
        }
    }

    // Type alias for the worker snapshot from task.rs.
    use super::super::task::WorkerVmSnapshot;

    /// Run one fiber on a worker VM, returning the structured outcome.
    ///
    /// This is the per-fiber execution primitive used by both worker 0 (running
    /// on the caller's OS thread with the main `ctx`) and by background OS workers
    /// (running with their own freshly-created VM).
    ///
    /// Per-fiber thread-locals (`CURRENT`, `PENDING_PARK`, `PENDING_YIELD`,
    /// `PENDING_FIBER_ERROR`) are set/cleared within this call so that each
    /// fiber's execution is isolated to the running OS thread.
    fn run_one_fiber(
        fiber: &mut Fiber,
        ctx: &mut dyn RuntimeContext,
        resume_val: Value,
    ) -> WorkerFiberResult {
        let fiber_id = fiber.id;
        let home_worker = fiber.home_worker;

        // Set the current fiber id for this OS thread's per-fiber locals.
        let prev_current = CURRENT.with(|c| c.replace(Some(fiber_id)));

        // In multi-worker mode (ACTIVE_SHARED is set), BOUNDARY may not be
        // initialized on background worker threads.  Set it to the current
        // VM frame/sp so that fiber primops (FiberSleep, FiberBoth, etc.)
        // see a valid boundary and capture the correct continuation slice.
        let prev_boundary = if ACTIVE_SHARED.with(|s| s.borrow().is_some()) {
            Some(set_boundary(ctx.current_frame_index(), ctx.current_sp()))
        } else {
            None
        };

        let outcome = if let Some(cont) = fiber.parked.take() {
            ctx.resume_from_dispatch(Value::Continuation(cont), resume_val)
        } else if let Some(body) = fiber.body.take() {
            ctx.invoke_value(body, vec![])
        } else {
            // No body, no parked: bookkeeping artifact.
            if let Some(prev) = prev_boundary { restore_boundary(prev); }
            CURRENT.with(|c| c.set(prev_current));
            return WorkerFiberResult::Done { fiber_id, value: Value::None };
        };

        // Restore the previous current fiber and boundary.
        if let Some(prev) = prev_boundary { restore_boundary(prev); }
        CURRENT.with(|c| c.set(prev_current));

        // Check for park signal.
        if let Some((req, cont_val)) = take_park() {
            return WorkerFiberResult::Parked {
                fiber_id,
                request_id: req,
                cont: cont_val,
                home_worker,
            };
        }

        // Check for yield signal.
        if let Some(cont_val) = take_yield() {
            return WorkerFiberResult::Yielded {
                fiber_id,
                cont: cont_val,
                home_worker,
            };
        }

        match outcome {
            Ok(v) => WorkerFiberResult::Done { fiber_id, value: v },
            Err(_e) => {
                let err = take_fiber_error().unwrap_or_else(|| async_panicked(_e));
                WorkerFiberResult::Error { fiber_id, err_value: err }
            }
        }
    }

    /// Process a `WorkerFiberResult` against the shared state, issuing completions
    /// and cancellations as needed.  Returns `true` if the root fiber completed
    /// (the caller should exit its dispatch loop).
    ///
    /// `backend` is used for cancellation of I/O requests.
    fn process_fiber_result(
        result: WorkerFiberResult,
        fiber: Fiber,
        shared: &Arc<SharedDispatchState>,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> bool {
        match result {
            WorkerFiberResult::Done { fiber_id, value } => {
                fiber_trace::emit(FiberEvent::Complete {
                    fid: fiber_id.0,
                    worker: fiber.home_worker.0,
                });
                // Check for root fiber completion (value case).
                // Worker 0 owns the root fiber and handles success return directly;
                // background workers should not own the root (root is on WorkerId(0)).
                if fiber_id == shared.root_id {
                    shared.set_finished();
                    return true;
                }
                // Compute synthetic-await completions.
                let (completions, losers) = {
                    let events = shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                        .record_completed(
                            fiber_id,
                            FiberOutcome::Value(value),
                            |id| shared.sched.lock().unwrap_or_else(|e| e.into_inner()).is_ready(id),
                            |o| matches!(o, FiberOutcome::Error(_)),
                        );
                    events_to_completions_and_losers(events)
                };
                // Cancel losers.
                cancel_losers_shared(&losers, backend, shared);
                // Deliver parent completions.
                for (pr, rv) in completions {
                    shared.deliver_completion(pr, rv);
                }
                false
            }
            WorkerFiberResult::Error { fiber_id, err_value } => {
                if fiber_id == shared.root_id {
                    // Store error string so worker 0 can return it.
                    *shared.root_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(error_to_string(&err_value));
                    shared.set_finished();
                    return true;
                }
                let (completions, losers) = {
                    let events = shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                        .record_completed(
                            fiber_id,
                            FiberOutcome::Error(err_value),
                            |id| shared.sched.lock().unwrap_or_else(|e| e.into_inner()).is_ready(id),
                            |o| matches!(o, FiberOutcome::Error(_)),
                        );
                    events_to_completions_and_losers(events)
                };
                cancel_losers_shared(&losers, backend, shared);
                for (pr, rv) in completions {
                    shared.deliver_completion(pr, rv);
                }
                false
            }
            WorkerFiberResult::Parked { fiber_id, request_id, cont, home_worker } => {
                let cont_rc = match cont {
                    Value::Continuation(rc) => rc,
                    _ => {
                        // Malformed park — treat as error.
                        return false;
                    }
                };
                let mut f = fiber;
                f.parked = Some(cont_rc);
                f.state = FiberState::Suspended { request_id };
                fiber_trace::emit(FiberEvent::Suspend {
                    fid: fiber_id.0,
                    worker: home_worker.0,
                    request_id,
                });
                shared.sched.lock().unwrap_or_else(|e| e.into_inner())
                    .insert_suspended(home_worker, request_id, f);
                // Re-evaluate suspended-fiber awaits for first_of.
                let (completions, losers) = {
                    let events = shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                        .record_suspended(
                            fiber_id,
                            |id| shared.sched.lock().unwrap_or_else(|e| e.into_inner()).is_ready(id),
                        );
                    events_to_completions_and_losers(events)
                };
                cancel_losers_shared(&losers, backend, shared);
                for (pr, rv) in completions {
                    shared.deliver_completion(pr, rv);
                }
                false
            }
            WorkerFiberResult::Yielded { fiber_id: _, cont, home_worker } => {
                let cont_rc = match cont {
                    Value::Continuation(rc) => rc,
                    _ => return false,
                };
                let mut f = fiber;
                f.parked = Some(cont_rc);
                f.state = FiberState::Ready;
                let mut sched = shared.sched.lock().unwrap_or_else(|e| e.into_inner());
                sched.spawn_existing(f);
                drop(sched);
                // Wake the home worker since there's now work in its queue.
                let idx = home_worker.0 as usize;
                if idx < shared.worker_wakeups.len() {
                    shared.worker_wakeups[idx].wake();
                }
                false
            }
        }
    }

    /// Extract `(completions, losers)` from a vec of `AwaitEvent`s.
    fn events_to_completions_and_losers(
        events: Vec<AwaitEvent<FiberId, FiberOutcome, ()>>,
    ) -> (Vec<(u64, FiberOutcome)>, Vec<FiberId>) {
        let outcome = events_to_done_outcome(events);
        (outcome.completions, outcome.losers)
    }

    /// Cancel losers using shared state instead of thread-locals.
    fn cancel_losers_shared(
        loser_ids: &[FiberId],
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
        shared: &Arc<SharedDispatchState>,
    ) {
        if loser_ids.is_empty() {
            return;
        }
        let reqs: Vec<u64> = {
            let sched = shared.sched.lock().unwrap_or_else(|e| e.into_inner());
            loser_ids.iter()
                .filter_map(|id| sched.find_request_for_fiber(*id))
                .collect()
        };
        for req in reqs {
            backend.cancel(RequestId(req));
        }
        let mut sched = shared.sched.lock().unwrap_or_else(|e| e.into_inner());
        sched.cancel_fibers(loser_ids);
        drop(sched);
        let mut cancelled = shared.cancelled_ids.lock().unwrap_or_else(|e| e.into_inner());
        for id in loser_ids {
            cancelled.insert(*id);
        }
    }

    /// Pop the next ready fiber assigned to `worker_id` from the shared scheduler.
    fn pop_ready_for_worker(
        shared: &Arc<SharedDispatchState>,
        worker_id: WorkerId,
    ) -> Option<Fiber> {
        shared.sched.lock().unwrap_or_else(|e| e.into_inner())
            .next_ready(worker_id)
    }

    /// Get the resume value for a fiber that was woken by a completion.
    fn take_resume_outcome_shared(
        shared: &Arc<SharedDispatchState>,
        req: u64,
    ) -> Option<FiberOutcome> {
        shared.resume_outcomes.lock().unwrap_or_else(|e| e.into_inner())
            .remove(&req)
    }

    /// Dispatch loop for worker threads 1..n (background OS threads).
    ///
    /// Each such thread:
    ///   1. Pops ready fibers pinned to its `worker_id` from the shared scheduler.
    ///   2. Runs each fiber using its own `ctx`.
    ///   3. Handles park / yield / done results through the shared state.
    ///   4. Parks via `Condvar` when its queue is empty, waking when the
    ///      `WorkerWakeup` is signalled by a completion.
    ///   5. Exits when `SharedDispatchState::is_finished()` is set.
    fn worker_dispatch_loop(
        worker_id: WorkerId,
        shared: Arc<SharedDispatchState>,
        snapshot: WorkerVmSnapshot,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) {
        // Build a standalone VM from the snapshot.
        let mut vm = match super::super::task::vm_from_worker_snapshot(snapshot) {
            Ok(vm) => vm,
            Err(e) => {
                // Cannot create worker VM — signal an error and exit.
                if !shared.is_finished() {
                    *shared.root_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(format!("worker_dispatch_loop: failed to reconstruct VM: {e}"));
                    shared.set_finished();
                }
                return;
            }
        };

        let wakeup = shared.worker_wakeups[worker_id.0 as usize].clone();

        // Set ACTIVE_SHARED so all fiber primop helpers on this thread route
        // through the shared state rather than the per-thread TLS slots.
        ACTIVE_SHARED.with(|s| *s.borrow_mut() = Some(Arc::clone(&shared)));

        loop {
            if shared.is_finished() {
                break;
            }

            // Drain all ready fibers assigned to this worker.
            loop {
                let Some(mut fiber) = pop_ready_for_worker(&shared, worker_id) else {
                    break;
                };

                if fiber.body.is_none() && fiber.parked.is_none() {
                    // Bookkeeping artifact — skip.
                    continue;
                }

                // Cancelled fibers: resume with None so cleanup arms run.
                if fiber.state == FiberState::Cancelled {
                    if fiber.parked.is_some() {
                        let resume_val = Value::None;
                        let result = run_one_fiber(&mut fiber, &mut vm, resume_val);
                        let done = process_fiber_result(result, fiber, &shared, backend);
                        if done { break; }
                    }
                    continue;
                }

                // Determine resume value.
                let resume_outcome = fiber.last_completion_req
                    .take()
                    .and_then(|req| take_resume_outcome_shared(&shared, req))
                    .unwrap_or(FiberOutcome::Value(Value::None));

                let resume_val = match resume_outcome {
                    FiberOutcome::Value(v) => v,
                    FiberOutcome::Error(err) => {
                        // Propagate error through await coordination.
                        // Background workers should not own the root fiber, but
                        // handle defensively: store in root_error.
                        if fiber.id == shared.root_id {
                            *shared.root_error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error_to_string(&err));
                            shared.set_finished();
                            break;
                        }
                        let (completions, losers) = {
                            let events = shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                                .record_completed(
                                    fiber.id,
                                    FiberOutcome::Error(err),
                                    |id| shared.sched.lock().unwrap_or_else(|e| e.into_inner()).is_ready(id),
                                    |o| matches!(o, FiberOutcome::Error(_)),
                                );
                            events_to_completions_and_losers(events)
                        };
                        cancel_losers_shared(&losers, backend, &shared);
                        for (pr, rv) in completions {
                            shared.deliver_completion(pr, rv);
                        }
                        continue;
                    }
                };

                let result = run_one_fiber(&mut fiber, &mut vm, resume_val);
                let done = process_fiber_result(result, fiber, &shared, backend);
                if done {
                    break;
                }
            }

            if shared.is_finished() {
                break;
            }

            // Park until woken by a completion or the finished flag is set.
            wakeup.park_timeout(std::time::Duration::from_millis(1));
        }
        ACTIVE_SHARED.with(|s| *s.borrow_mut() = None);
    }

    /// Dispatch loop for worker 0 (the caller's OS thread) in multi-worker mode.
    ///
    /// This is structurally similar to `worker_dispatch_loop` but uses the
    /// existing `ctx: &mut dyn RuntimeContext` (the main VM) so that the
    /// caller's VM stack is reused rather than reconstructed.  It also drives
    /// the mio backend completion pump when no fibers are ready, routing
    /// completions to the appropriate worker queue via `SharedDispatchState`.
    fn worker_0_dispatch_loop(
        ctx: &mut dyn RuntimeContext,
        shared: Arc<SharedDispatchState>,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> Result<Value, String> {
        let worker_id = WorkerId(0);
        let wakeup = shared.worker_wakeups[0].clone();

        loop {
            if shared.is_finished() {
                break;
            }

            // Drain all ready fibers assigned to worker 0.
            let mut did_work = false;
            loop {
                let Some(mut fiber) = pop_ready_for_worker(&shared, worker_id) else {
                    break;
                };
                did_work = true;

                if fiber.body.is_none() && fiber.parked.is_none() {
                    continue;
                }

                if fiber.state == FiberState::Cancelled {
                    if fiber.parked.is_some() {
                        let resume_val = Value::None;
                        let result = run_one_fiber(&mut fiber, ctx, resume_val);
                        let done = process_fiber_result(result, fiber, &shared, backend);
                        if done { break; }
                    }
                    continue;
                }

                let resume_outcome = fiber.last_completion_req
                    .take()
                    .and_then(|req| take_resume_outcome_shared(&shared, req))
                    .unwrap_or(FiberOutcome::Value(Value::None));

                let resume_val = match resume_outcome {
                    FiberOutcome::Value(v) => v,
                    FiberOutcome::Error(err) => {
                        if fiber.id == shared.root_id {
                            // Root fiber got an error resume — signal and exit.
                            *shared.root_error.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(error_to_string(&err));
                            shared.set_finished();
                            break;
                        }
                        let (completions, losers) = {
                            let events = shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
                                .record_completed(
                                    fiber.id,
                                    FiberOutcome::Error(err),
                                    |id| shared.sched.lock().unwrap_or_else(|e| e.into_inner()).is_ready(id),
                                    |o| matches!(o, FiberOutcome::Error(_)),
                                );
                            events_to_completions_and_losers(events)
                        };
                        cancel_losers_shared(&losers, backend, &shared);
                        for (pr, rv) in completions {
                            shared.deliver_completion(pr, rv);
                        }
                        continue;
                    }
                };

                let is_root = fiber.id == shared.root_id;
                let result = run_one_fiber(&mut fiber, ctx, resume_val);
                // For the root fiber completion case, capture the value before
                // handing to process_fiber_result (which discards it).
                let root_value = if is_root {
                    if let WorkerFiberResult::Done { value: ref v, .. } = result {
                        Some(v.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                let done = process_fiber_result(result, fiber, &shared, backend);
                if done {
                    if let Some(rv) = root_value {
                        // Root completed successfully — return the value directly.
                        shared.set_finished();
                        return Ok(rv);
                    }
                    break;
                }
            }

            if shared.is_finished() {
                break;
            }

            // Check if we're done (root completed or no more fibers anywhere).
            let (total_ready, total_suspended) = {
                let sched = shared.sched.lock().unwrap_or_else(|e| e.into_inner());
                (sched.total_ready_count(), sched.total_suspended_count())
            };

            if total_ready == 0 && total_suspended == 0 && !did_work {
                break;
            }

            if total_ready == 0 {
                // Pump the backend for completions.
                let pumped = pump_backend_completions(&shared, backend);
                if !pumped {
                    wakeup.park_timeout(std::time::Duration::from_millis(1));
                }
            }
        }

        // Check if a background worker set root_error.
        if let Some(err) = shared.root_error.lock().unwrap_or_else(|e| e.into_inner()).take() {
            return Err(err);
        }
        Ok(Value::None)
    }

    /// Pump one round of backend completions, routing each to the correct worker.
    /// Returns `true` if at least one completion was processed.
    fn pump_backend_completions(
        shared: &Arc<SharedDispatchState>,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> bool {
        // Check task completions.
        if let Some((request_id, result)) = super::super::task::try_recv_completion() {
            let outcome = match result {
                Ok(value) => FiberOutcome::Value(Value::Some(Rc::new(value))),
                Err(err) if err == "TaskCancelled" => FiberOutcome::Value(Value::None),
                Err(err) => FiberOutcome::Error(async_panicked(err)),
            };
            shared.deliver_completion(request_id, outcome);
            return true;
        }
        // Check channel completions.
        if let Some((request_id, result)) = super::super::channel::try_recv_completion() {
            let outcome = match result {
                Ok(value) => FiberOutcome::Value(value),
                Err(err) => FiberOutcome::Error(async_panicked(err)),
            };
            shared.deliver_completion(request_id, outcome);
            return true;
        }
        // Check event completions.
        if let Some(request_id) = super::super::event::try_recv_completion() {
            shared.deliver_completion(request_id, FiberOutcome::Value(Value::None));
            return true;
        }
        // Check mio backend.
        if let Some(c) = backend.next_completion() {
            use crate::runtime::r#async::backend::CompletionPayload;
            if let CompletionPayload::AddressList(addrs) = &c.payload {
                if let Some(addr) = addrs.first() {
                    backend.tcp_connect(c.request_id, *addr);
                } else {
                    shared.deliver_completion(
                        c.request_id.0,
                        FiberOutcome::Error(async_panicked("dns resolve returned no addresses")),
                    );
                }
                return true;
            }
            let outcome = match &c.payload {
                CompletionPayload::TcpHandle(h) => {
                    FiberOutcome::Value(Value::Integer(h.0 as i64))
                }
                CompletionPayload::Bytes(buf) => {
                    let s = String::from_utf8_lossy(buf).into_owned();
                    FiberOutcome::Value(Value::String(Rc::new(s)))
                }
                CompletionPayload::Unit => FiberOutcome::Value(Value::None),
                CompletionPayload::Error(e) => FiberOutcome::Error(async_panicked(e.clone())),
                CompletionPayload::AddressList(_) => unreachable!("handled above"),
            };
            // Timeout routing.
            if let Some(body_child) =
                try_route_timer_for_timeout_shared(c.request_id.0, shared)
            {
                cancel_losers_for_timer_shared(&[body_child], backend, shared);
            }
            shared.deliver_completion(c.request_id.0, outcome);
            return true;
        }
        false
    }

    /// Route a timer completion for `FiberTimeout` using shared state.
    fn try_route_timer_for_timeout_shared(
        req: u64,
        shared: &Arc<SharedDispatchState>,
    ) -> Option<FiberId> {
        use crate::runtime::r#async::await_coordinator::AwaitEvent;
        match shared.awaits.lock().unwrap_or_else(|e| e.into_inner())
            .route_timeout_timer(req)
        {
            Some(AwaitEvent::TimeoutTimerReady { body, .. }) => {
                shared.resume_outcomes.lock().unwrap_or_else(|e| e.into_inner())
                    .insert(req, FiberOutcome::Value(Value::None));
                Some(body)
            }
            _ => None,
        }
    }

    /// Cancel timer losers using shared state.
    fn cancel_losers_for_timer_shared(
        loser_ids: &[FiberId],
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
        shared: &Arc<SharedDispatchState>,
    ) {
        cancel_losers_shared(loser_ids, backend, shared);
    }

    /// Enter a multi-OS-thread `run_async` (n_workers > 1).
    ///
    /// Creates a `SharedDispatchState`, takes a VM snapshot via
    /// `ctx.vm_worker_snapshot()` for each background worker thread,
    /// spawns `n_workers - 1` OS threads, and runs worker 0's dispatch
    /// loop on the caller's thread.
    ///
    /// Called from the `FiberRunAsync` / `FiberRunAsyncWith` dispatch arms
    /// when `resolved_worker_count() > 1`.
    pub fn enter_run_async_multi(
        ctx: &mut dyn RuntimeContext,
        n_workers: usize,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> Result<Value, String> {
        // The scheduler and root fiber were already set up by `enter_run_async`.
        let shared = SCHED.with(|s| {
            let mut borrow = s.borrow_mut();
            let sched = borrow.take().expect("scheduler not initialised before enter_run_async_multi");
            let root_id = ROOT.with(|r| r.get()).expect("ROOT not set before enter_run_async_multi");
            SharedDispatchState::new(sched, root_id, n_workers)
        });

        // Collect per-worker VM snapshots from the main ctx.  If the context
        // doesn't support snapshots (e.g. test contexts), we still spawn threads
        // but they will construct minimal VMs.
        let mut worker_handles = Vec::with_capacity(n_workers - 1);

        for w in 1..n_workers {
            let worker_id = WorkerId(w as u32);
            let shared_w = Arc::clone(&shared);
            // Attempt to extract a WorkerVmSnapshot from the runtime context.
            let snap: WorkerVmSnapshot = ctx
                .vm_worker_snapshot()
                .and_then(|any| {
                    any.downcast::<super::super::task::WorkerVmSnapshot>().ok().map(|b| *b)
                })
                .unwrap_or_else(|| super::super::task::WorkerVmSnapshot {
                    constants: vec![],
                    globals: vec![],
                });
            let handle = std::thread::Builder::new()
                .name(format!("flux-fiber-worker-{}", w))
                .spawn(move || {
                    worker_dispatch_loop(worker_id, shared_w, snap, backend);
                })
                .map_err(|e| format!("failed to spawn fiber worker thread {}: {}", w, e))?;
            worker_handles.push(handle);
        }

        // Worker 0 runs on the caller's thread.  Set ACTIVE_SHARED so all
        // fiber primop helpers route through the shared state.
        ACTIVE_SHARED.with(|s| *s.borrow_mut() = Some(Arc::clone(&shared)));
        let result = worker_0_dispatch_loop(ctx, Arc::clone(&shared), backend);
        ACTIVE_SHARED.with(|s| *s.borrow_mut() = None);

        // Ensure the finished flag is set so background workers exit.
        shared.set_finished();

        // Join all background workers, propagating panics.
        for handle in worker_handles {
            if let Err(e) = handle.join() {
                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                    format!("fiber worker panicked: {s}")
                } else if let Some(s) = e.downcast_ref::<String>() {
                    format!("fiber worker panicked: {s}")
                } else {
                    "fiber worker panicked (unknown payload)".to_string()
                };
                // If we already have a good result, prefer reporting the
                // worker panic.  If we already have an error, keep the first.
                if result.is_ok() {
                    return Err(msg);
                }
            }
        }

        result
    }
}

/// Park a TCP primop operation until its completion is available.
/// If inside Async.run_async, captures the continuation and parks the fiber.
/// Otherwise, synchronously pumps the backend (single-fiber fallback path).
fn park_tcp_op(
    ctx: &mut dyn RuntimeContext,
    req: crate::runtime::r#async::backend::RequestId,
) -> Result<Value, String> {
    if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
        // Automatic cancel checkpoint: if the fiber is already cancelled,
        // cancel the backend request we just issued and abort immediately.
        if vm_fibers::is_current_cancelled() {
            if let Ok(backend) = vm_async::backend() {
                use crate::runtime::r#async::backend::AsyncBackend;
                backend.cancel(req);
            }
            vm_fibers::signal_cancel_error();
            return Err("__fiber_error__".to_string());
        }
        let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
        vm_fibers::signal_park(req, cont);
        if let Err(e) = propagate_if_park_cancelled() {
            // signal_park detected cancellation after cont capture —
            // cancel the backend request that was already issued.
            if let Ok(backend) = vm_async::backend() {
                use crate::runtime::r#async::backend::AsyncBackend;
                backend.cancel(req);
            }
            return Err(e);
        }
        Err("__fiber_park__".to_string())
    } else {
        // No fiber boundary: pump backend synchronously (single-fiber fallback).
        let backend = vm_async::backend()?;
        loop {
            if let Some(c) = backend.next_completion() {
                if c.request_id == req {
                    use crate::runtime::r#async::backend::CompletionPayload;
                    return match c.payload {
                        CompletionPayload::TcpHandle(h) => Ok(Value::Integer(h.0 as i64)),
                        CompletionPayload::Bytes(buf) => Ok(Value::String(Rc::new(
                            String::from_utf8_lossy(&buf).into_owned(),
                        ))),
                        CompletionPayload::Unit => Ok(Value::None),
                        CompletionPayload::Error(e) => Err(e),
                        CompletionPayload::AddressList(addrs) => {
                            let Some(addr) = addrs.first() else {
                                return Err("dns resolve returned no addresses".into());
                            };
                            backend.tcp_connect(req, *addr);
                            continue;
                        }
                    };
                }
            }
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
}

/// Called immediately after `signal_park`. If the fiber was already cancelled
/// and `signal_park` set `PARK_CANCELLED` instead of storing the park,
/// signals the cancel error and returns `Err` so the call site can propagate
/// it without returning `__fiber_park__`.
#[inline]
fn propagate_if_park_cancelled() -> Result<(), String> {
    if vm_fibers::check_and_clear_park_cancelled() {
        vm_fibers::signal_cancel_error();
        Err("__fiber_error__".to_string())
    } else {
        Ok(())
    }
}

/// Execute a `CorePrimOp` with the given arguments.
///
/// This is the single dispatch point for all `OpPrimOp` instructions in the VM.
/// Each arm matches a `CorePrimOp` variant and runs the corresponding Rust
/// implementation inline (no sub-dispatch through `PrimOp`).
pub fn execute_core_primop(
    ctx: &mut dyn RuntimeContext,
    op: CorePrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    use CorePrimOp::*;

    match op {
        // ── Typed integer arithmetic ──────────────────────────────────
        IAdd => int2(&args, |a, b| Value::Integer(a + b), "iadd"),
        ISub => int2(&args, |a, b| Value::Integer(a - b), "isub"),
        IMul => int2(&args, |a, b| Value::Integer(a * b), "imul"),
        IDiv => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("division by zero".into())
                } else {
                    Ok(Value::Integer(a / b))
                }
            },
            "idiv",
        ),
        IMod => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("modulo by zero".into())
                } else {
                    Ok(Value::Integer(a % b))
                }
            },
            "imod",
        ),

        // ── Safe arithmetic (Proposal 0135) ──────────────────────────
        SafeDiv => safe_arith_div(&args),
        SafeMod => safe_arith_mod(&args),

        // ── Typed float arithmetic ────────────────────────────────────
        FAdd => float2(&args, |a, b| Value::Float(a + b), "fadd"),
        FSub => float2(&args, |a, b| Value::Float(a - b), "fsub"),
        FMul => float2(&args, |a, b| Value::Float(a * b), "fmul"),
        FDiv => float2(&args, |a, b| Value::Float(a / b), "fdiv"),
        FSqrt => float1(&args, |a| Value::Float(a.sqrt()), "fsqrt"),
        FSin => float1(&args, |a| Value::Float(a.sin()), "fsin"),
        FCos => float1(&args, |a| Value::Float(a.cos()), "fcos"),
        FExp => float1(&args, |a| Value::Float(a.exp()), "fexp"),
        FLog => float1(&args, |a| Value::Float(a.ln()), "flog"),
        FFloor => float1(&args, |a| Value::Float(a.floor()), "ffloor"),
        FCeil => float1(&args, |a| Value::Float(a.ceil()), "fceil"),
        FRound => float1(&args, |a| Value::Float(a.round()), "fround"),
        FTan => float1(&args, |a| Value::Float(a.tan()), "ftan"),
        FAsin => float1(&args, |a| Value::Float(a.asin()), "fasin"),
        FAcos => float1(&args, |a| Value::Float(a.acos()), "facos"),
        FAtan => float1(&args, |a| Value::Float(a.atan()), "fatan"),
        FSinh => float1(&args, |a| Value::Float(a.sinh()), "fsinh"),
        FCosh => float1(&args, |a| Value::Float(a.cosh()), "fcosh"),
        FTanh => float1(&args, |a| Value::Float(a.tanh()), "ftanh"),
        FTruncate => float1(&args, |a| Value::Float(a.trunc()), "ftruncate"),
        BitAnd => int2(&args, |a, b| Value::Integer(a & b), "bit_and"),
        BitOr => int2(&args, |a, b| Value::Integer(a | b), "bit_or"),
        BitXor => int2(&args, |a, b| Value::Integer(a ^ b), "bit_xor"),
        BitShl => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shl(masked_shift_amount(b))),
            "bit_shl",
        ),
        BitShr => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shr(masked_shift_amount(b))),
            "bit_shr",
        ),

        // ── Numeric helpers ───────────────────────────────────────────
        Abs => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(v.abs())),
            Value::Float(v) => Ok(Value::Float(v.abs())),
            other => Err(terr("abs", "Number", other)),
        },
        Min => numeric_min_max(&args, "min", true),
        Max => numeric_min_max(&args, "max", false),
        Neg => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(-v)),
            Value::Float(v) => Ok(Value::Float(-v)),
            other => Err(terr("neg", "Number", other)),
        },

        // ── Typed integer comparisons ─────────────────────────────────
        ICmpEq => int_cmp(&args, |a, b| a == b, "icmp_eq"),
        ICmpNe => int_cmp(&args, |a, b| a != b, "icmp_ne"),
        ICmpLt => int_cmp(&args, |a, b| a < b, "icmp_lt"),
        ICmpLe => int_cmp(&args, |a, b| a <= b, "icmp_le"),
        ICmpGt => int_cmp(&args, |a, b| a > b, "icmp_gt"),
        ICmpGe => int_cmp(&args, |a, b| a >= b, "icmp_ge"),

        // ── Typed float comparisons ───────────────────────────────────
        FCmpEq => float_cmp(&args, |a, b| a == b, "fcmp_eq"),
        FCmpNe => float_cmp(&args, |a, b| a != b, "fcmp_ne"),
        FCmpLt => float_cmp(&args, |a, b| a < b, "fcmp_lt"),
        FCmpLe => float_cmp(&args, |a, b| a <= b, "fcmp_le"),
        FCmpGt => float_cmp(&args, |a, b| a > b, "fcmp_gt"),
        FCmpGe => float_cmp(&args, |a, b| a >= b, "fcmp_ge"),

        // ── Deep structural comparison ────────────────────────────────
        CmpEq => Ok(Value::Boolean(args[0] == args[1])),
        CmpNe => Ok(Value::Boolean(args[0] != args[1])),

        // ── Array operations ──────────────────────────────────────────
        ArrayLen => match &args[0] {
            Value::Array(items) => Ok(Value::Integer(items.len() as i64)),
            other => Err(terr("array_len", "Array", other)),
        },
        ArrayGet => {
            let index = eint(&args[1], "array_get")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        Ok(Value::None)
                    } else {
                        Ok(items[index as usize].clone())
                    }
                }
                other => Err(terr("array_get", "Array", other)),
            }
        }
        ArraySet => {
            let index = eint(&args[1], "array_set")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        return Err(format!(
                            "array_set: index {} out of bounds for length {}",
                            index,
                            items.len()
                        ));
                    }
                    let mut items = items.clone();
                    Rc::make_mut(&mut items)[index as usize] = args[2].clone();
                    Ok(Value::Array(items))
                }
                other => Err(terr("array_set", "Array", other)),
            }
        }
        ArrayPush => {
            let mut args = args;
            let elem = args.swap_remove(1);
            let arr_obj = args.swap_remove(0);
            match arr_obj {
                Value::Array(mut arr) => {
                    Rc::make_mut(&mut arr).push(elem);
                    Ok(Value::Array(arr))
                }
                other => Err(terr("push", "Array", &other)),
            }
        }
        ArrayConcat => {
            let left = earr(&args[0], "concat")?;
            let right = earr(&args[1], "concat")?;
            let mut out = left.clone();
            Rc::make_mut(&mut out).extend(right.iter().cloned());
            Ok(Value::Array(out))
        }
        ArraySlice => {
            let arr = earr(&args[0], "slice")?;
            let start = eint(&args[1], "slice")?;
            let end = eint(&args[2], "slice")?;
            let len = arr.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= arr.len() {
                Ok(Value::Array(vec![].into()))
            } else {
                Ok(Value::Array(arr[start..end].to_vec().into()))
            }
        }

        // ── HAMT operations ───────────────────────────────────────────
        HamtGet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("get", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => match rc_hamt::hamt_lookup(node, &key) {
                    Some(value) => Ok(Value::Some(Rc::new(value))),
                    None => Ok(Value::None),
                },
                other => Err(terr("get", "Map", other)),
            }
        }
        HamtSet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("put", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => Ok(Value::HashMap(rc_hamt::hamt_insert(
                    node,
                    key,
                    args[2].clone(),
                ))),
                other => Err(terr("put", "Map", other)),
            }
        }
        HamtContains => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("has_key", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => {
                    Ok(Value::Boolean(rc_hamt::hamt_lookup(node, &key).is_some()))
                }
                other => Err(terr("has_key", "Map", other)),
            }
        }
        HamtDelete => {
            let node = ehamt(&args[0], "delete")?;
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("delete", &args[1]))?;
            Ok(Value::HashMap(rc_hamt::hamt_delete(node, &key)))
        }
        HamtKeys => {
            let node = ehamt(&args[0], "keys")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs
                    .iter()
                    .map(|(k, _)| hash_key_to_value(k))
                    .collect::<Vec<_>>()
                    .into(),
            ))
        }
        HamtValues => {
            let node = ehamt(&args[0], "values")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs.into_iter().map(|(_, v)| v).collect::<Vec<_>>().into(),
            ))
        }
        HamtMerge => {
            let node1 = ehamt(&args[0], "merge")?;
            let node2 = ehamt(&args[1], "merge")?;
            let pairs = rc_hamt::hamt_iter(node2);
            let mut result = Rc::clone(node1);
            for (k, v) in pairs {
                result = rc_hamt::hamt_insert(&result, k, v);
            }
            Ok(Value::HashMap(result))
        }
        HamtSize => {
            let node = ehamt(&args[0], "size")?;
            Ok(Value::Integer(rc_hamt::hamt_len(node) as i64))
        }

        // ── String operations ─────────────────────────────────────────
        StringLength => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            other => Err(terr("string_length", "String", other)),
        },
        StringConcat => match (&args[0], &args[1]) {
            (Value::String(l), Value::String(r)) => Ok(Value::String(format!("{}{}", l, r).into())),
            (l, r) => Err(format!(
                "string_concat expects (String, String), got ({}, {})",
                l.type_name(),
                r.type_name()
            )),
        },
        StringSlice | Substring => {
            let s = estr(&args[0], "string_slice")?;
            let start = eint(&args[1], "string_slice")?;
            let end = eint(&args[2], "string_slice")?;
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end < 0 {
                0
            } else if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= chars.len() {
                Ok(Value::String(String::new().into()))
            } else {
                Ok(Value::String(
                    chars[start..end].iter().collect::<String>().into(),
                ))
            }
        }
        ToString => Ok(Value::String(format_value(&args[0]).into())),
        Split => {
            let s = estr(&args[0], "split")?;
            let delim = estr(&args[1], "split")?;
            let parts: Vec<Value> = if delim.is_empty() {
                s.chars()
                    .map(|c| Value::String(c.to_string().into()))
                    .collect()
            } else {
                s.split(delim)
                    .map(|p| Value::String(p.to_string().into()))
                    .collect()
            };
            Ok(Value::Array(parts.into()))
        }
        Trim => Ok(Value::String(
            estr(&args[0], "trim")?.trim().to_string().into(),
        )),
        Upper => Ok(Value::String(
            estr(&args[0], "upper")?.to_uppercase().into(),
        )),
        Lower => Ok(Value::String(
            estr(&args[0], "lower")?.to_lowercase().into(),
        )),
        Replace => Ok(Value::String(
            estr(&args[0], "replace")?
                .replace(estr(&args[1], "replace")?, estr(&args[2], "replace")?)
                .into(),
        )),

        // ── Type tag inspection ───────────────────────────────────────
        IsInt => Ok(Value::Boolean(matches!(args[0], Value::Integer(_)))),
        IsFloat => Ok(Value::Boolean(matches!(args[0], Value::Float(_)))),
        IsString => Ok(Value::Boolean(matches!(args[0], Value::String(_)))),
        IsBool => Ok(Value::Boolean(matches!(args[0], Value::Boolean(_)))),
        IsArray => Ok(Value::Boolean(matches!(args[0], Value::Array(_)))),
        IsNone => Ok(Value::Boolean(matches!(args[0], Value::None))),
        IsSome => Ok(Value::Boolean(matches!(args[0], Value::Some(_)))),
        IsList => Ok(Value::Boolean(matches!(
            args[0],
            Value::None | Value::EmptyList | Value::Cons(_)
        ))),
        IsMap => Ok(Value::Boolean(matches!(args[0], Value::HashMap(_)))),
        TypeOf => {
            let name = match &args[0] {
                Value::Cons(_) => "List",
                Value::HashMap(_) => "Map",
                other => other.type_name(),
            };
            Ok(Value::String(name.to_string().into()))
        }

        // ── I/O ───────────────────────────────────────────────────────
        Print => {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{}", format_value(arg));
            }
            println!();
            Ok(Value::None)
        }
        Println => {
            println!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        DebugTrace => {
            // Debug output goes to stderr so program stdout stays clean for
            // piping to other tools. Matches GHC `Debug.Trace`, Rust `dbg!`,
            // Python's default logging behavior. The argument is expected to
            // be a pre-formatted string (the Flow.Debug wrappers call
            // `show()` on values before perform-ing the effect operation).
            eprintln!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        ReadFile => {
            let path = estr(&args[0], "read_file")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_file failed for '{}': {}", path, e))?;
            Ok(Value::String(content.into()))
        }
        WriteFile => {
            let path = estr(&args[0], "write_file")?;
            let content = estr(&args[1], "write_file")?;
            fs::write(path, content)
                .map_err(|e| format!("write_file failed for '{}': {}", path, e))?;
            Ok(Value::None)
        }
        ReadStdin => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("read_stdin failed: {}", e))?;
            Ok(Value::String(input.into()))
        }
        ReadLines => {
            let path = estr(&args[0], "read_lines")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_lines failed for '{}': {}", path, e))?;
            let lines = content
                .lines()
                .map(|line| Value::String(line.trim_end_matches('\r').to_string().into()))
                .collect::<Vec<_>>();
            Ok(Value::Array(lines.into()))
        }

        // ── Control ───────────────────────────────────────────────────
        Unwrap => match &args[0] {
            Value::None => Err("unwrap called on None".into()),
            other => Ok(other.clone()),
        },
        Panic => {
            if vm_fibers::boundary().is_some() {
                vm_fibers::signal_fiber_error(vm_fibers::async_panicked(args[0].to_string_value()));
                Err("__fiber_error__".to_string())
            } else {
                Err(format!("panic: {}", args[0].to_string_value()))
            }
        }
        ClockNow => {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|e| format!("clock_now failed: {}", e))?;
            Ok(Value::Integer(now.as_millis() as i64))
        }
        Time => {
            let start = Instant::now();
            let _ = ctx
                .invoke_value(args[0].clone(), vec![])
                .map_err(|e| format!("time: callback error: {}", e))?;
            let elapsed_ms = start.elapsed().as_millis();
            Ok(Value::Integer(elapsed_ms.min(i64::MAX as u128) as i64))
        }
        Try => match ctx.invoke_value(args[0].clone(), vec![]) {
            Ok(val) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("ok".to_string().into()),
                val,
            ]))),
            Err(msg) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("error".to_string().into()),
                Value::String(Rc::new(msg)),
            ]))),
        },
        AssertThrows => {
            let expected_msg: Option<&str> = if args.len() >= 2 {
                match &args[1] {
                    Value::String(s) => Some(s.as_ref()),
                    _ => None,
                }
            } else {
                None
            };
            match ctx.invoke_value(args[0].clone(), vec![]) {
                Ok(_) => Err("assert_throws failed: function completed without error".into()),
                Err(msg) => match expected_msg {
                    Some(expected) if msg.contains(expected) => Ok(Value::None),
                    Some(expected) => Err(format!(
                        "assert_throws failed\n  expected error containing: {}\n  actual error: {}",
                        expected, msg
                    )),
                    None => Ok(Value::None),
                },
            }
        }

        // ── Parsing ───────────────────────────────────────────────────
        ParseInt => {
            let text = estr(&args[0], "parse_int")?;
            let parsed = text
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("parse_int: could not parse '{}' as Int", text))?;
            Ok(Value::Integer(parsed))
        }
        // ── Polymorphic length ────────────────────────────────────────
        Len => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
            Value::Tuple(t) => Ok(Value::Integer(t.len() as i64)),
            Value::None | Value::EmptyList => Ok(Value::Integer(0)),
            Value::Cons(_) => {
                let mut count: i64 = 0;
                let mut cur = &args[0];
                loop {
                    match cur {
                        Value::None | Value::EmptyList => break,
                        Value::Cons(cell) => {
                            count += 1;
                            cur = &cell.tail;
                        }
                        _ => break,
                    }
                }
                Ok(Value::Integer(count))
            }
            Value::HashMap(node) => Ok(Value::Integer(rc_hamt::hamt_len(node) as i64)),
            other => Err(terr("len", "String, Array, Tuple, or Map", other)),
        },

        // ── Effect handler ops (native-only, Koka-style yield model) ────
        EvvGet | EvvSet | FreshMarker | EvvInsert | YieldTo | YieldExtend | YieldPrompt
        | IsYielding | PerformDirect => Err(format!(
            "CorePrimOp {:?} is native-backend only (Koka yield model)",
            op
        )),

        // ── Concurrency (proposal 0174 D5-a) ─────────────────────────
        //
        // VM tasks run on the Rust TaskScheduler through `src/vm/task.rs`.
        // That layer deep-copies sendable values into an isolated worker VM
        // and rehydrates the result back into the caller's normal Rc-backed
        // value graph. The main VM stack, continuations, and handler state do
        // not cross OS-thread boundaries.
        TaskSpawn | TaskSpawnMove => {
            let mut args = args;
            let action = args.swap_remove(0);
            let id = if op == TaskSpawnMove {
                ctx.vm_task_spawn_move(action)?
            } else {
                ctx.vm_task_spawn(action)?
            };
            // Bound the task's lifetime to the enclosing run_async: if the
            // user never awaits / joins / cancels, the dispatch-site drain
            // at run_async exit will cancel it. No-op outside run_async.
            vm_fibers::register_root_task(id);
            fiber_trace::emit(FiberEvent::TaskSpawn {
                tid: id,
                parent_fid: vm_fibers::current_fiber_id(),
            });
            Ok(Value::Integer(id))
        }
        TaskSpawnScoped | TaskSpawnScopedMove => {
            let scope_id = match &args[0] {
                Value::Adt(a) if a.constructor.as_ref() == "Scope" => match &a.fields {
                    crate::runtime::value::AdtFields::One(Value::Integer(n)) => *n as u64,
                    _ => return Err("task_spawn_scoped: malformed Scope ADT".to_string()),
                },
                other => return Err(terr("task_spawn_scoped", "Scope", other)),
            };
            let mut args = args;
            let action = args.swap_remove(1);
            let id = if op == TaskSpawnScopedMove {
                ctx.vm_task_spawn_move(action)?
            } else {
                ctx.vm_task_spawn(action)?
            };
            vm_fibers::register_task_in_scope(scope_id, id);
            Ok(Value::Integer(id))
        }
        TaskBlockingJoin => match &args[0] {
            Value::Integer(id) => {
                // User has taken ownership — drop the root-task safety net.
                vm_fibers::deregister_root_task(*id);
                ctx.vm_task_blocking_join(*id)
            }
            other => Err(terr("task_blocking_join", "Int", other)),
        },
        TaskCancel => match &args[0] {
            Value::Integer(id) => {
                vm_fibers::deregister_root_task(*id);
                ctx.vm_task_cancel(*id)?;
                fiber_trace::emit(FiberEvent::TaskCancel { tid: *id });
                Ok(Value::None)
            }
            other => Err(terr("task_cancel", "Int", other)),
        },

        // ── Fiber / structured concurrency (proposal 0174 Phase 1b) ──
        //
        // VM path: sequentially-equivalent semantics (same model as Phase 1a
        // TaskSpawn). No OS-thread yields or stack-switching happen here.
        // Real M:N fiber switching lives in the native backend (Phase 1b-vi+).

        // run_async: install the Async boundary and drive the dispatch loop
        // (proposal 0174 Phase 1b-vi-b₂.1). The root fiber's body is the
        // `action` closure; FiberSleep parks the current fiber on a backend
        // request, the loop pumps completions and resumes parked fibers.
        // Behaviour is observably identical to b₁ for single-fiber programs;
        // multi-fiber overlap arrives in 1b-vi-b₂.2.
        FiberRunAsync => {
            let backend = vm_async::backend()?;
            let root = vm_fibers::enter_run_async();
            let prev_boundary =
                vm_fibers::set_boundary(ctx.current_frame_index(), ctx.current_sp());
            vm_fibers::set_root_with_body(root, args[0].clone());

            // Phase 4: branch on worker count.
            //   n_workers == 1 → existing single-threaded dispatch_loop (unchanged).
            //   n_workers > 1  → enter_run_async_multi spawns real OS workers.
            let n_workers = vm_fibers::current_num_workers();
            let result = if n_workers > 1 {
                vm_fibers::enter_run_async_multi(ctx, n_workers, backend)
            } else {
                vm_fibers::dispatch_loop(ctx, backend)
            };

            vm_fibers::clear_root();
            vm_fibers::restore_boundary(prev_boundary);
            // Drain the root-task safety net before tearing down the run:
            // any task spawned via detached `Task.spawn` that the user never
            // awaited / joined / cancelled gets cancelled here so it cannot
            // leak past run_async. Idempotent for already-completed tasks.
            if vm_fibers::at_outermost_run_async() {
                for id in vm_fibers::take_all_root_tasks() {
                    let _ = ctx.vm_task_cancel(id);
                }
            }
            vm_fibers::exit_run_async(root);
            result
        }

        // fiber_run_async_with: `FiberRunAsync` plus explicit RuntimeConfig
        // knobs (proposal 0174 Phase 2 slice 2-vii). Args:
        //   args[0] = worker_count   (Int; 0 means "default")
        //   args[1] = fs_pool_size   (Int; 0 means "default"; consulted by 2-viii)
        //   args[2] = dns_pool_size  (Int; 0 means "default"; consulted by 2-viii)
        //   args[3] = action closure
        // Sets the pending RuntimeConfig before entering the boundary so
        // `enter_run_async` picks it up; the rest of the path is identical
        // to `FiberRunAsync`.
        FiberRunAsyncWith => {
            let workers = match &args[0] {
                Value::Integer(n) if *n >= 0 => *n as u32,
                Value::Integer(_) => 0,
                other => return Err(terr("fiber_run_async_with(workers)", "Int", other)),
            };
            let fs_pool = match &args[1] {
                Value::Integer(n) if *n >= 0 => *n as u32,
                Value::Integer(_) => 0,
                other => return Err(terr("fiber_run_async_with(fs)", "Int", other)),
            };
            let dns_pool = match &args[2] {
                Value::Integer(n) if *n >= 0 => *n as u32,
                Value::Integer(_) => 0,
                other => return Err(terr("fiber_run_async_with(dns)", "Int", other)),
            };
            vm_fibers::set_pending_run_config(vm_fibers::PendingRunConfig {
                worker_count: workers,
                fs_pool_size: fs_pool,
                dns_pool_size: dns_pool,
            });

            let backend = vm_async::backend()?;
            let root = vm_fibers::enter_run_async();
            let prev_boundary =
                vm_fibers::set_boundary(ctx.current_frame_index(), ctx.current_sp());
            vm_fibers::set_root_with_body(root, args[3].clone());

            // Phase 4: branch on worker count (same as FiberRunAsync above).
            let n_workers = vm_fibers::current_num_workers();
            let result = if n_workers > 1 {
                vm_fibers::enter_run_async_multi(ctx, n_workers, backend)
            } else {
                vm_fibers::dispatch_loop(ctx, backend)
            };

            vm_fibers::clear_root();
            vm_fibers::restore_boundary(prev_boundary);
            // Same root-task safety-net drain as `FiberRunAsync`. See comment there.
            if vm_fibers::at_outermost_run_async() {
                for id in vm_fibers::take_all_root_tasks() {
                    let _ = ctx.vm_task_cancel(id);
                }
            }
            vm_fibers::exit_run_async(root);
            result
        }

        // yield_now: cooperative reschedule (proposal 0174 slice 2-vi) — and
        // automatic cancel checkpoint. If the enclosing scope is cancelled,
        // raise here (a cancelled fiber never reaches `signal_yield`). Inside
        // a FiberRunAsync boundary, capture the continuation, hand it to the
        // dispatch loop via PENDING_YIELD, and bail out with the park signal;
        // the loop re-enqueues this fiber at the tail of its run-queue and
        // resumes it with Unit. Outside a boundary (`main() with Async` host
        // handler) there is nothing to reschedule against — harmless no-op.
        FiberYieldNow => {
            if vm_fibers::is_current_cancelled() {
                vm_fibers::signal_cancel_error();
                return Err("__fiber_error__".to_string());
            }
            if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                vm_fibers::signal_yield(cont);
                Err("__fiber_park__".to_string())
            } else {
                Ok(Value::None)
            }
        }

        // fiber_check_cancelled: returns true iff the current fiber's
        // enclosing scope has been cancelled (proposal 0174 Phase 2 slice
        // 2-iv). Scheduler-flag read; no suspend, no backend round-trip.
        // Composes with Async.fail when the caller wants to raise; slice
        // 2-vi makes that raise catchable.
        FiberCheckCancelled => Ok(Value::Boolean(vm_fibers::is_current_cancelled())),

        // fiber_current_worker_count: report the worker count of the
        // currently active FiberScheduler (proposal 0174 slice 2-vii
        // follow-up). Returns 0 outside `run_async`. Scheduler-state
        // read; no suspend.
        FiberCurrentWorkerCount => Ok(Value::Integer(vm_fibers::current_num_workers() as i64)),

        // sleep: capture the current fiber's continuation back to the
        // FiberRunAsync boundary, register a timer with the mio backend,
        // signal park-pending, then return Err to bail out to the dispatch
        // loop (proposal 0174 Phase 1b-vi-b₂.1). The dispatch loop reads
        // PENDING_PARK, moves the fiber into the suspended map, and resumes
        // it when the backend's completion arrives. Single-fiber timing is
        // unchanged from 1b-vi-a; multi-fiber overlap lands in b₂.2.
        FiberSleep => match &args[0] {
            Value::Integer(ms) if *ms >= 0 => {
                use crate::runtime::r#async::backend::AsyncBackend;
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.timer_start(req, *ms as u64);
                if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                    // Inside Async.run_async: capture continuation, signal
                    // park; dispatch loop resumes us when timer fires.
                    let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                    vm_fibers::signal_park(req, cont);
                    if let Err(e) = propagate_if_park_cancelled() {
                        // Timer was already armed — cancel it.
                        backend.cancel(req);
                        return Err(e);
                    }
                    Err("__fiber_park__".to_string())
                } else {
                    // No boundary (e.g. `main() with Async` is the host
                    // handler). Fall back to blocking pump — same shape as
                    // 1b-vi-a. The OS-thread call stack is the continuation.
                    loop {
                        if let Some(c) = backend.next_completion() {
                            if c.request_id == req {
                                break;
                            }
                            return Err(format!(
                                "fiber_sleep: unexpected completion {:?}",
                                c.request_id
                            ));
                        }
                        std::thread::park_timeout(std::time::Duration::from_millis(1));
                    }
                    Ok(Value::None)
                }
            }
            Value::Integer(_) => Err("fiber_sleep: ms must be non-negative".to_string()),
            other => Err(terr("fiber_sleep", "Int", other)),
        },

        // fiber_suspend: on the VM path the "setup closure" is never needed —
        // there is no suspension, so we just return immediately. The closure is
        // discarded (Aether RC drops it). The caller (yield_now / sleep) has
        // already done the real work above via dedicated primops.
        FiberSuspend => Ok(Value::None),

        // fiber_fork: allocate a child fiber via the scheduler, then run its
        // body inline (preserving sequential semantics — child runs to
        // completion before parent resumes). b₂ replaces the inline call
        // with a real park/resume cycle.
        FiberFork => {
            let child = vm_fibers::spawn_child();
            vm_fibers::with_current(child, || ctx.invoke_value(args[0].clone(), vec![]))?;
            Ok(Value::None)
        }

        // fiber_both / fiber_race (proposal 0174 Phase 1b-vi-b₂.2): spawn
        // two child fibers, park the parent on a synthetic completion,
        // bail out to the dispatch loop. When children finish, the loop
        // routes synthetic completions through `on_fiber_done`, builds the
        // resume value (tuple for both, winner for race), and wakes the
        // parent.
        FiberBoth => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_both called outside Async.run_async — no boundary set".to_string()
            })?;
            let child_a = vm_fibers::spawn_child_with_body(args[0].clone());
            let child_b = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_both_await(req.0, child_a, child_b);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            if let Err(e) = propagate_if_park_cancelled() {
                vm_fibers::deregister_await(req.0);
                let backend = vm_async::backend()?;
                vm_fibers::cancel_losers(&[child_a, child_b], backend);
                return Err(e);
            }
            Err("__fiber_park__".to_string())
        }

        FiberRace => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_race called outside Async.run_async — no boundary set".to_string()
            })?;
            let child_a = vm_fibers::spawn_child_with_body(args[0].clone());
            let child_b = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_race_await(req.0, vec![child_a, child_b]);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            if let Err(e) = propagate_if_park_cancelled() {
                vm_fibers::deregister_await(req.0);
                let backend = vm_async::backend()?;
                vm_fibers::cancel_losers(&[child_a, child_b], backend);
                return Err(e);
            }
            Err("__fiber_park__".to_string())
        }

        FiberFirstOf => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_first_of called outside Async.run_async — no boundary set".to_string()
            })?;
            let bodies = collect_list_values(&args[0]).map_err(|got| {
                format!("fiber_first_of expected non-empty List of async thunks, got {got}")
            })?;
            if bodies.is_empty() {
                return Err("Async.first_of called on empty list".to_string());
            }
            let children: Vec<_> = bodies
                .into_iter()
                .enumerate()
                .map(|(idx, body)| (vm_fibers::spawn_child_with_body(body), idx))
                .collect();
            let req = vm_async::alloc_request_id();
            vm_fibers::register_first_of_await(req.0, children.clone());
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            if let Err(e) = propagate_if_park_cancelled() {
                vm_fibers::deregister_await(req.0);
                let child_ids: Vec<_> = children.iter().map(|(id, _)| *id).collect();
                let backend = vm_async::backend()?;
                vm_fibers::cancel_losers(&child_ids, backend);
                return Err(e);
            }
            Err("__fiber_park__".to_string())
        }

        FiberTry => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_try called outside Async.run_async — no boundary set".to_string()
            })?;
            let child = vm_fibers::spawn_child_with_body(args[0].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_try_await(req.0, child);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            if let Err(e) = propagate_if_park_cancelled() {
                vm_fibers::deregister_await(req.0);
                let backend = vm_async::backend()?;
                vm_fibers::cancel_losers(&[child], backend);
                return Err(e);
            }
            Err("__fiber_park__".to_string())
        }

        // fiber_timeout (proposal 0174 Phase 1b-vi-b₂.2 follow-up): bound
        // `f` by `ms` ms. Spawns the body fiber, registers a backend timer
        // keyed on the SAME request id as the parent's await; whichever
        // fires first delivers the resume value (Some(body_result) or
        // None). Loser is left to its fate (cancellation is 1b-vi-c).
        FiberTimeout => {
            use crate::runtime::r#async::backend::AsyncBackend;
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_timeout called outside Async.run_async — no boundary set".to_string()
            })?;
            let ms = match &args[0] {
                Value::Integer(n) if *n >= 0 => *n as u64,
                Value::Integer(_) => {
                    return Err("fiber_timeout: ms must be non-negative".to_string());
                }
                other => return Err(terr("fiber_timeout", "Int", other)),
            };
            let body_child = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_timeout_await(req.0, body_child);
            // Register the backend timer keyed on the SAME request id as
            // the parent's await — when the timer fires, the dispatch
            // loop's pump observes c.request_id == req, calls
            // try_route_timer_for_timeout (which sets resume = None),
            // then scheduler.complete wakes the parent.
            let backend = vm_async::backend()?;
            backend.timer_start(req, ms);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            if let Err(e) = propagate_if_park_cancelled() {
                vm_fibers::deregister_await(req.0);
                backend.cancel(req);
                vm_fibers::cancel_losers(&[body_child], backend);
                return Err(e);
            }
            Err("__fiber_park__".to_string())
        }

        // fiber_get_context: return a dummy integer context handle. Not
        // observable by user code on the VM path (used by scheduler internals).
        FiberGetContext => Ok(Value::Integer(0)),

        // fiber_fail: raise a catchable async error in the current fiber.
        FiberFail => {
            vm_fibers::signal_fiber_error(args[0].clone());
            Err("__fiber_error__".to_string())
        }

        // ── Scope / cancel primops (proposal 0174 Phase 1b-vi-c) ──────

        // fiber_new_scope: allocate a real cancellation boundary.
        // Returns Scope(id) as an ADT value.
        FiberNewScope => {
            use crate::runtime::value::{AdtFields, AdtValue};
            let id = vm_fibers::new_scope();
            Ok(Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("Scope".to_string()),
                fields: AdtFields::One(Value::Integer(id as i64)),
            })))
        }

        // fiber_fork_scoped: spawn a child fiber and register it under the scope.
        FiberForkScoped => {
            let scope_id = match &args[0] {
                Value::Adt(a) if a.constructor.as_ref() == "Scope" => match &a.fields {
                    crate::runtime::value::AdtFields::One(Value::Integer(n)) => *n as u64,
                    _ => return Err("fiber_fork_scoped: malformed Scope ADT".to_string()),
                },
                other => return Err(terr("fiber_fork_scoped", "Scope", other)),
            };
            let child_id = vm_fibers::spawn_child_with_body(args[1].clone());
            vm_fibers::register_fiber_in_scope(scope_id, child_id);
            fiber_trace::emit(FiberEvent::ScopeCreate {
                scope_id,
                fid: child_id.0,
            });
            Ok(Value::None)
        }

        // fiber_cancel_scope: cancel all fibers and scoped tasks registered
        // under the scope.
        FiberCancelScope => {
            let scope_id = match &args[0] {
                Value::Adt(a) if a.constructor.as_ref() == "Scope" => match &a.fields {
                    crate::runtime::value::AdtFields::One(Value::Integer(n)) => *n as u64,
                    _ => return Err("fiber_cancel_scope: malformed Scope ADT".to_string()),
                },
                other => return Err(terr("fiber_cancel_scope", "Scope", other)),
            };
            fiber_trace::emit(FiberEvent::ScopeCancel { scope_id });
            let fiber_ids = vm_fibers::take_scope_fibers(scope_id);
            if !fiber_ids.is_empty() {
                let backend = vm_async::backend()?;
                vm_fibers::cancel_losers(&fiber_ids, backend);
            }
            // Also cancel any tasks registered under this scope via spawn_scoped.
            // vm_task_cancel is idempotent — no-op for already-completed tasks.
            let task_ids = vm_fibers::take_scope_tasks(scope_id);
            for task_id in task_ids {
                let _ = ctx.vm_task_cancel(task_id);
            }
            Ok(Value::None)
        }

        // task_await: inside Async.run_async, park the current fiber and let a
        // waiter thread publish the task result back into the VM scheduler.
        // Outside an async boundary, fall back to blocking join.
        TaskAwait => match &args[0] {
            Value::Integer(id) => {
                // User has taken ownership — drop the root-task safety net.
                vm_fibers::deregister_root_task(*id);
                fiber_trace::emit(FiberEvent::TaskAwait {
                    tid: *id,
                    fid: vm_fibers::current_fiber_id(),
                });
                if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                    if vm_fibers::is_current_cancelled() {
                        vm_fibers::signal_cancel_error();
                        return Err("__fiber_error__".to_string());
                    }
                    let req = vm_async::alloc_request_id();
                    let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                    super::task::start_await(*id, req.0)?;
                    vm_fibers::signal_park(req, cont);
                    Err("__flux_fiber_park__".to_string())
                } else {
                    let value = ctx.vm_task_blocking_join(*id)?;
                    Ok(Value::Some(Rc::new(value)))
                }
            }
            other => Err(terr("task_await", "Int", other)),
        },

        // ── Channel primops (cross-fiber producer/consumer queues) ───
        ChanMake => match &args[0] {
            Value::Integer(capacity) => super::channel::make(*capacity).map(Value::Integer),
            other => Err(terr("chan_make", "Int", other)),
        },
        ChanSend | ChanSendMove => match &args[0] {
            Value::Integer(id) => {
                let id = *id;
                let move_send = op == ChanSendMove;
                if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                    if vm_fibers::is_current_cancelled() {
                        vm_fibers::signal_cancel_error();
                        return Err("__fiber_error__".to_string());
                    }
                    let req = vm_async::alloc_request_id();
                    let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                    if move_send {
                        let mut args = args;
                        super::channel::start_send_move(id, args.swap_remove(1), req.0)?;
                    } else {
                        super::channel::start_send(id, &args[1], req.0)?;
                    }
                    fiber_trace::emit(FiberEvent::ChanSend {
                        ch: id,
                        fid: vm_fibers::current_fiber_id(),
                        backpressure: true,
                    });
                    vm_fibers::signal_park(req, cont);
                    Err("__flux_fiber_park__".to_string())
                } else {
                    if move_send {
                        let mut args = args;
                        super::channel::send_move(id, args.swap_remove(1))?;
                    } else {
                        super::channel::send(id, &args[1])?;
                    }
                    fiber_trace::emit(FiberEvent::ChanSend {
                        ch: id,
                        fid: vm_fibers::current_fiber_id(),
                        backpressure: false,
                    });
                    Ok(Value::None)
                }
            }
            other => Err(terr("chan_send", "Int", other)),
        },
        ChanRecv => match &args[0] {
            Value::Integer(id) => {
                if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                    if vm_fibers::is_current_cancelled() {
                        vm_fibers::signal_cancel_error();
                        return Err("__fiber_error__".to_string());
                    }
                    let req = vm_async::alloc_request_id();
                    let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                    super::channel::start_recv(*id, req.0)?;
                    fiber_trace::emit(FiberEvent::ChanRecv {
                        ch: *id,
                        fid: vm_fibers::current_fiber_id(),
                        was_empty: true,
                    });
                    vm_fibers::signal_park(req, cont);
                    Err("__flux_fiber_park__".to_string())
                } else {
                    let result = super::channel::recv(*id);
                    fiber_trace::emit(FiberEvent::ChanRecv {
                        ch: *id,
                        fid: vm_fibers::current_fiber_id(),
                        was_empty: false,
                    });
                    result
                }
            }
            other => Err(terr("chan_recv", "Int", other)),
        },
        ChanTrySend | ChanTrySendMove => match &args[0] {
            Value::Integer(id) => {
                let id = *id;
                if op == ChanTrySendMove {
                    let mut args = args;
                    super::channel::try_send_move(id, args.swap_remove(1)).map(Value::Boolean)
                } else {
                    super::channel::try_send(id, &args[1]).map(Value::Boolean)
                }
            }
            other => Err(terr("chan_try_send", "Int", other)),
        },
        ChanTryRecv => match &args[0] {
            Value::Integer(id) => super::channel::try_recv(*id),
            other => Err(terr("chan_try_recv", "Int", other)),
        },
        ChanClose => match &args[0] {
            Value::Integer(id) => {
                super::channel::close(*id)?;
                fiber_trace::emit(FiberEvent::ChanClose { ch: *id });
                Ok(Value::None)
            }
            other => Err(terr("chan_close", "Int", other)),
        },
        ChanLen => match &args[0] {
            Value::Integer(id) => super::channel::len(*id).map(Value::Integer),
            other => Err(terr("chan_len", "Int", other)),
        },
        ChanCap => match &args[0] {
            Value::Integer(id) => super::channel::cap(*id).map(Value::Integer),
            other => Err(terr("chan_cap", "Int", other)),
        },
        ChanIsClosed => match &args[0] {
            Value::Integer(id) => super::channel::is_closed(*id).map(Value::Boolean),
            other => Err(terr("chan_is_closed", "Int", other)),
        },
        EventRecv => match &args[0] {
            Value::Integer(id) => Ok(Value::Integer(super::event::recv(*id))),
            other => Err(terr("event_recv", "Int", other)),
        },
        EventSend | EventSendMove => match &args[0] {
            Value::Integer(id) => {
                let id = *id;
                if op == EventSendMove {
                    let mut args = args;
                    super::event::send_move(id, args.swap_remove(1)).map(Value::Integer)
                } else {
                    Ok(Value::Integer(super::event::send(id, args[1].clone())))
                }
            }
            other => Err(terr("event_send", "Int", other)),
        },
        EventAfter => match &args[0] {
            Value::Integer(ms) => super::event::after(*ms).map(Value::Integer),
            other => Err(terr("event_after", "Int", other)),
        },
        EventAlways => Ok(Value::Integer(super::event::always(args[0].clone()))),
        EventNever => Ok(Value::Integer(super::event::never())),
        EventChoose => {
            let ids = super::event::collect_ids(&args[0])?;
            super::event::choose(ids).map(Value::Integer)
        }
        EventWrap => match &args[0] {
            Value::Integer(id) => Ok(Value::Integer(super::event::wrap(*id, args[1].clone()))),
            other => Err(terr("event_wrap", "Int", other)),
        },
        EventSync => match &args[0] {
            Value::Integer(_) => {
                Err("event_sync primop is deprecated; Flow.Event.sync uses event_poll".to_string())
            }
            other => Err(terr("event_sync", "Int", other)),
        },
        EventPoll => match &args[0] {
            Value::Integer(id) => super::event::poll_value(ctx, *id),
            other => Err(terr("event_poll", "Int", other)),
        },
        EventWait => match &args[0] {
            Value::Integer(id) => {
                let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() else {
                    return Err("event_wait: requires Async.run_async boundary".to_string());
                };
                if vm_fibers::is_current_cancelled() {
                    vm_fibers::signal_cancel_error();
                    return Err("__fiber_error__".to_string());
                }
                let req = vm_async::alloc_request_id();
                let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                super::event::wait(*id, req.0)?;
                vm_fibers::signal_park(req, cont);
                Err("__flux_fiber_park__".to_string())
            }
            other => Err(terr("event_wait", "Int", other)),
        },

        // ── TCP primops (proposal 0174 Phase 1b-vii) ────────────────
        // Async-aware: park on the fiber scheduler, execute non-blocking
        // via the mio backend. Returns integer handle IDs on success.
        TcpConnect => match (&args[0], &args[1]) {
            (Value::String(host), Value::Integer(port)) => {
                use crate::runtime::r#async::backend::AsyncBackend;
                use std::net::SocketAddr;
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                let target = format!("{}:{}", host, port);
                if let Ok(addr) = target.parse::<SocketAddr>() {
                    backend.tcp_connect(req, addr);
                } else {
                    let port: u16 = (*port)
                        .try_into()
                        .map_err(|_| format!("tcp_connect: bad port: {port}"))?;
                    backend.dns_resolve(req, host.to_string(), port);
                }
                park_tcp_op(ctx, req)
            }
            _ => Err(format!("tcp_connect: expected (String, Int)")),
        },

        TcpRead => match (&args[0], &args[1]) {
            (Value::Integer(handle), Value::Integer(max)) => {
                use crate::runtime::r#async::backend::{AsyncBackend, IoHandle};
                let h = IoHandle(*handle as u64);
                let max = if *max > 0 && *max <= (1 << 24) {
                    *max as usize
                } else {
                    4096
                };
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.tcp_read(req, h, max);
                park_tcp_op(ctx, req)
            }
            _ => Err(format!("tcp_read: expected (Int, Int)")),
        },

        TcpWriteAll => match (&args[0], &args[1]) {
            (Value::Integer(handle), Value::String(data)) => {
                use crate::runtime::r#async::backend::{AsyncBackend, IoHandle};
                let h = IoHandle(*handle as u64);
                let bytes = data.as_bytes().to_vec();
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.tcp_write(req, h, bytes);
                park_tcp_op(ctx, req)
            }
            _ => Err(format!("tcp_write_all: expected (Int, String)")),
        },

        TcpClose => match &args[0] {
            Value::Integer(handle) => {
                use crate::runtime::r#async::backend::{AsyncBackend, IoHandle};
                let h = IoHandle(*handle as u64);
                if let Ok(backend) = vm_async::backend() {
                    backend.tcp_close(h);
                }
                Ok(Value::None)
            }
            other => Err(terr("tcp_close", "Int", other)),
        },

        TcpListen => match (&args[0], &args[1]) {
            (Value::String(host), Value::Integer(port)) => {
                use std::net::SocketAddr;
                let addr: SocketAddr = format!("{}:{}", host, port)
                    .parse()
                    .map_err(|e| format!("tcp_listen: bad address: {e}"))?;
                use crate::runtime::r#async::backend::AsyncBackend;
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.tcp_listen(req, addr);
                park_tcp_op(ctx, req)
            }
            _ => Err(format!("tcp_listen: expected (String, Int)")),
        },

        TcpAccept => match &args[0] {
            Value::Integer(listener_handle) => {
                use crate::runtime::r#async::backend::{AsyncBackend, IoHandle};
                let h = IoHandle(*listener_handle as u64);
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.tcp_accept(req, h);
                park_tcp_op(ctx, req)
            }
            other => Err(terr("tcp_accept", "Int", other)),
        },

        // ── HTTP server-manager reserved primops (proposal 0174 Phase 3a) ──
        //
        // The first `Flow.Http` server slice is source-level over Flow.Tcp so
        // VM closures remain on the owning fiber. These primops track the
        // detached accept-fiber lifecycle and connection set while the parser
        // and writer stay Rust-owned.
        HttpServeConfig => vm_http_serve_config(ctx, &args),
        HttpShutdown => vm_http_shutdown(&args, false),
        HttpShutdownNow => vm_http_shutdown(&args, true),
        HttpParseRequest => vm_http_parse_request(&args),
        HttpWriteResponse => vm_http_write_response(&args),
        HttpWriteChunkedHead => vm_http_write_chunked_head(&args),
        HttpWriteChunk => vm_http_write_chunk(&args),
        HttpWriteChunkedEnd => vm_http_write_chunked_end(&args),
        HttpParseUrl => vm_http_parse_url(&args),
        HttpWriteRequest => vm_http_write_request(&args),
        HttpParseResponse => vm_http_parse_response(&args),
        JsonParse => vm_json_parse(&args),
        JsonStringify => vm_json_stringify(&args),
        HttpRegisterConnection => {
            let server = eint(&args[0], "http_register_connection")?;
            let conn = eint(&args[1], "http_register_connection")?;
            vm_http::register_connection(server, conn);
            Ok(Value::None)
        }
        HttpUnregisterConnection => {
            let server = eint(&args[0], "http_unregister_connection")?;
            let conn = eint(&args[1], "http_unregister_connection")?;
            vm_http::unregister_connection(server, conn);
            Ok(Value::None)
        }
        HttpActiveConnectionCount => {
            let server = eint(&args[0], "http_active_connection_count")?;
            Ok(Value::Integer(vm_http::active_count(server) as i64))
        }
        HttpIsShuttingDown => {
            let server = eint(&args[0], "http_is_shutting_down")?;
            Ok(Value::Boolean(vm_http::is_shutting_down(server)))
        }
        HttpServerStopped => {
            let server = eint(&args[0], "http_server_stopped")?;
            vm_http::mark_stopped(server);
            Ok(Value::None)
        }
        HttpIsServerStopped => {
            let server = eint(&args[0], "http_is_server_stopped")?;
            Ok(Value::Boolean(vm_http::is_stopped(server)))
        }

        // ── Generic/structural ops (never emitted as OpPrimOp) ───────
        Add | Sub | Mul | Div | Mod | Not | Eq | NEq | Lt | Le | Gt | Ge | And | Or | Concat
        | Interpolate | MakeList | MakeArray | MakeTuple | MakeHash | Index => Err(format!(
            "CorePrimOp {:?} should not appear in OpPrimOp bytecode",
            op
        )),
    }
}

// ── Compact helper functions ─────────────────────────────────────────────────

fn terr(op: &str, expected: &str, got: &Value) -> String {
    format!(
        "primop {} expected {}, got {}",
        op,
        expected,
        got.type_name()
    )
}

fn collect_list_values(value: &Value) -> Result<Vec<Value>, &'static str> {
    let mut out = Vec::new();
    let mut current = value.clone();
    loop {
        match current {
            Value::EmptyList | Value::None => return Ok(out),
            Value::Cons(cell) => {
                out.push(cell.head.clone());
                current = cell.tail.clone();
            }
            other => return Err(other.type_name()),
        }
    }
}

fn hkey_err(op: &str, v: &Value) -> String {
    format!(
        "primop {} expects hashable key (String, Int, Bool), got {}",
        op,
        v.type_name()
    )
}

fn estr<'a>(v: &'a Value, op: &str) -> Result<&'a str, String> {
    match v {
        Value::String(s) => Ok(s.as_ref()),
        other => Err(terr(op, "String", other)),
    }
}

fn eint(v: &Value, op: &str) -> Result<i64, String> {
    match v {
        Value::Integer(n) => Ok(*n),
        other => Err(terr(op, "Int", other)),
    }
}

fn efloat(v: &Value, op: &str) -> Result<f64, String> {
    match v {
        Value::Float(n) => Ok(*n),
        other => Err(terr(op, "Float", other)),
    }
}

fn earr<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<Vec<Value>>, String> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(terr(op, "Array", other)),
    }
}

fn ehamt<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<rc_hamt::HamtNode>, String> {
    match v {
        Value::HashMap(n) => Ok(n),
        other => Err(terr(op, "Map", other)),
    }
}

fn int2(args: &[Value], f: impl FnOnce(i64, i64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(eint(&args[0], op)?, eint(&args[1], op)?))
}

fn int2_result(
    args: &[Value],
    f: impl FnOnce(i64, i64) -> Result<Value, String>,
    op: &str,
) -> Result<Value, String> {
    f(eint(&args[0], op)?, eint(&args[1], op)?)
}

fn float2(args: &[Value], f: impl FnOnce(f64, f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?, efloat(&args[1], op)?))
}

fn float1(args: &[Value], f: impl FnOnce(f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?))
}

fn masked_shift_amount(value: i64) -> u32 {
    (value as u64 & 63) as u32
}

fn int_cmp(args: &[Value], f: impl FnOnce(i64, i64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(eint(&args[0], op)?, eint(&args[1], op)?)))
}

fn float_cmp(args: &[Value], f: impl FnOnce(f64, f64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(
        efloat(&args[0], op)?,
        efloat(&args[1], op)?,
    )))
}

fn numeric_min_max(args: &[Value], op: &str, is_min: bool) -> Result<Value, String> {
    let (a_num, b_num) = match (&args[0], &args[1]) {
        (Value::Integer(x), Value::Integer(y)) => (*x as f64, *y as f64),
        (Value::Integer(x), Value::Float(y)) => (*x as f64, *y),
        (Value::Float(x), Value::Integer(y)) => (*x, *y as f64),
        (Value::Float(x), Value::Float(y)) => (*x, *y),
        (l, r) => {
            return Err(format!(
                "primop {} expects (Number, Number), got ({}, {})",
                op,
                l.type_name(),
                r.type_name()
            ));
        }
    };
    let result = if is_min {
        a_num.min(b_num)
    } else {
        a_num.max(b_num)
    };
    match (&args[0], &args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

fn hash_key_to_value(key: &HashKey) -> Value {
    match key {
        HashKey::Integer(v) => Value::Integer(*v),
        HashKey::Boolean(v) => Value::Boolean(*v),
        HashKey::String(v) => Value::String(v.clone().into()),
    }
}

// ── Safe arithmetic (Proposal 0135) ─────────────────────────────────────────

fn safe_arith_div(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a / b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 / b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_div expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn safe_arith_mod(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a % b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a % b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 % b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a % *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_mod expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn vm_http_serve_config(ctx: &mut dyn RuntimeContext, args: &[Value]) -> Result<Value, String> {
    let _ = ctx;
    let listener = eint(&args[0], "http_serve_config(listener)")?;
    let scope = eint(&args[1], "http_serve_config(scope)")?;
    let config = http_server_config(&args[2])?;
    Ok(Value::Integer(vm_http::register(
        listener,
        scope as u64,
        config,
    )))
}

fn http_server_config(value: &Value) -> Result<crate::runtime::http::BlockingServerConfig, String> {
    let Value::Adt(adt) = value else {
        return Err(format!(
            "http_serve_config: config expected ServerConfig, got {}",
            value.type_name()
        ));
    };
    if adt.constructor.as_ref() != "ServerConfig" {
        return Err(format!(
            "http_serve_config: config expected ServerConfig, got {}",
            adt.constructor
        ));
    }
    let fields = match &adt.fields {
        crate::runtime::value::AdtFields::One(Value::Adt(inner))
            if inner.constructor.as_ref() == "ServerConfig" =>
        {
            &inner.fields
        }
        fields => fields,
    };

    let max_connections = config_usize_field(fields, 0, "max_connections")?;
    let max_header_bytes = config_usize_field(fields, 1, "max_header_bytes")?;
    let max_body_bytes = config_usize_field(fields, 2, "max_body_bytes")?;
    let request_timeout_ms = config_usize_field(fields, 3, "request_timeout_ms")?;
    let worker_count = config_optional_usize_field(fields, 4, "worker_count")?;

    Ok(crate::runtime::http::BlockingServerConfig {
        max_connections,
        limits: crate::runtime::http::ParseLimits {
            max_header_bytes,
            max_body_bytes,
        },
        request_timeout_ms,
        worker_count,
    })
}

fn vm_http_shutdown(args: &[Value], force: bool) -> Result<Value, String> {
    use crate::runtime::r#async::backend::{AsyncBackend, IoHandle};

    let server = eint(&args[0], "http_shutdown")?;
    let snapshot = vm_http::shutdown(server, force);
    if let Ok(backend) = vm_async::backend() {
        if let Some(listener) = snapshot.listener {
            backend.tcp_close(IoHandle(listener as u64));
        }
        if force {
            for conn in snapshot.active {
                backend.tcp_close(IoHandle(conn as u64));
            }
            if let Some(scope) = snapshot.scope {
                let fibers = vm_fibers::take_scope_fibers(scope);
                if !fibers.is_empty() {
                    vm_fibers::cancel_losers(&fibers, backend);
                    vm_fibers::mark_cancelled(&fibers);
                }
            }
        }
    }
    Ok(Value::None)
}

fn vm_http_parse_request(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::{HttpError, ParseLimits, parse_request};
    use crate::runtime::value::{AdtFields, AdtValue};

    let raw = estr(&args[0], "http_parse_request")?;
    let server = eint(&args[1], "http_parse_request(server)")?;
    let config = vm_http::config(server).unwrap_or_default();
    let limits = ParseLimits {
        max_header_bytes: config.limits.max_header_bytes,
        max_body_bytes: config.limits.max_body_bytes,
    };
    match parse_request(raw.as_bytes(), limits) {
        Ok((req, used)) => {
            let request_value = http_request_value(&req);
            Ok(Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("HttpParsed".to_string()),
                fields: AdtFields::from_vec(vec![
                    request_value,
                    Value::Integer(used as i64),
                    Value::Boolean(req.keep_alive),
                ]),
            })))
        }
        Err(HttpError::NeedMore) => Ok(Value::AdtUnit(Rc::new("HttpNeedMore".to_string()))),
        Err(HttpError::PayloadTooLarge(msg)) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpParseFailure".to_string()),
            fields: AdtFields::Two(Value::Integer(413), Value::String(Rc::new(msg))),
        }))),
        Err(HttpError::BadRequest(msg)) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpParseFailure".to_string()),
            fields: AdtFields::Two(Value::Integer(400), Value::String(Rc::new(msg))),
        }))),
    }
}

fn vm_http_write_response(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::{HttpResponse, write_response};

    let (status, body) = response_parts(&args[0])?;
    let keep_alive = match &args[1] {
        Value::Boolean(b) => *b,
        other => return Err(terr("http_write_response", "Bool", other)),
    };
    let status_u16: u16 = status
        .try_into()
        .map_err(|_| format!("Response.status out of HTTP range: {status}"))?;
    let wire = write_response(&HttpResponse {
        status: status_u16,
        reason: http_reason(status),
        headers: vec![(
            "Connection".into(),
            if keep_alive { "keep-alive" } else { "close" }.into(),
        )],
        body: body.into_bytes(),
    });
    Ok(Value::String(
        String::from_utf8_lossy(&wire).to_string().into(),
    ))
}

fn vm_http_write_chunked_head(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::write_chunked_head;

    let (status, headers) = stream_response_head_parts(&args[0])?;
    let status_u16: u16 = status
        .try_into()
        .map_err(|_| format!("StreamResponse.status out of HTTP range: {status}"))?;
    let wire = write_chunked_head(status_u16, &http_reason(status), &headers);
    Ok(Value::String(
        String::from_utf8_lossy(&wire).to_string().into(),
    ))
}

fn vm_http_write_chunk(args: &[Value]) -> Result<Value, String> {
    let chunk = estr(&args[0], "http_write_chunk")?;
    let wire = crate::runtime::http::write_chunk(chunk.as_bytes());
    Ok(Value::String(
        String::from_utf8_lossy(&wire).to_string().into(),
    ))
}

fn vm_http_write_chunked_end(_args: &[Value]) -> Result<Value, String> {
    let wire = crate::runtime::http::write_chunked_end();
    Ok(Value::String(
        String::from_utf8_lossy(&wire).to_string().into(),
    ))
}

fn vm_http_parse_url(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::{HttpError, parse_url};
    use crate::runtime::value::{AdtFields, AdtValue};

    let url = estr(&args[0], "http_parse_url")?;
    match parse_url(url) {
        Ok(parsed) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpUrlParsed".to_string()),
            fields: AdtFields::from_vec(vec![
                Value::String(parsed.host.into()),
                Value::Integer(parsed.port as i64),
                Value::String(parsed.target.into()),
            ]),
        }))),
        Err(HttpError::BadRequest(msg) | HttpError::PayloadTooLarge(msg)) => {
            Ok(Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("HttpUrlFailure".to_string()),
                fields: AdtFields::Two(Value::Integer(0), Value::String(msg.into())),
            })))
        }
        Err(HttpError::NeedMore) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpUrlFailure".to_string()),
            fields: AdtFields::Two(
                Value::Integer(0),
                Value::String("incomplete URL".to_string().into()),
            ),
        }))),
    }
}

fn vm_http_write_request(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::write_request;

    let method = http_method_name(&args[0])?;
    let host = estr(&args[1], "http_write_request(host)")?;
    let target = estr(&args[2], "http_write_request(target)")?;
    let headers = http_header_pairs(&args[3], "http_write_request(headers)")?;
    let body = estr(&args[4], "http_write_request(body)")?;
    let wire = write_request(method, host, target, &headers, body.as_bytes());
    Ok(Value::String(
        String::from_utf8_lossy(&wire).to_string().into(),
    ))
}

fn vm_http_parse_response(args: &[Value]) -> Result<Value, String> {
    use crate::runtime::http::{HttpError, ParseLimits, parse_response};
    use crate::runtime::value::{AdtFields, AdtValue};

    let raw = estr(&args[0], "http_parse_response")?;
    match parse_response(raw.as_bytes(), ParseLimits::default()) {
        Ok((resp, consumed)) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpResponseParsed".to_string()),
            fields: AdtFields::Two(http_response_value(&resp), Value::Integer(consumed as i64)),
        }))),
        Err(HttpError::NeedMore) => Ok(Value::AdtUnit(Rc::new("HttpResponseNeedMore".to_string()))),
        Err(HttpError::PayloadTooLarge(msg)) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpResponseFailure".to_string()),
            fields: AdtFields::Two(Value::Integer(413), Value::String(msg.into())),
        }))),
        Err(HttpError::BadRequest(msg)) => Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("HttpResponseFailure".to_string()),
            fields: AdtFields::Two(Value::Integer(0), Value::String(msg.into())),
        }))),
    }
}

fn vm_json_parse(args: &[Value]) -> Result<Value, String> {
    let raw = estr(&args[0], "json_parse")?;
    Ok(match crate::runtime::json::parse(raw) {
        Ok(value) => json_ok_value(json_to_value(&value)),
        Err(err) => json_err_value(&err),
    })
}

fn vm_json_stringify(args: &[Value]) -> Result<Value, String> {
    let value = value_to_json(&args[0])?;
    Ok(Value::String(
        crate::runtime::json::stringify(&value).into(),
    ))
}

fn json_ok_value(value: Value) -> Value {
    use crate::runtime::value::{AdtFields, AdtValue};

    Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("JsonOk".to_string()),
        fields: AdtFields::One(value),
    }))
}

fn json_err_value(err: &crate::runtime::json::JsonError) -> Value {
    use crate::runtime::value::{AdtFields, AdtValue};

    let error = Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("JsonError".to_string()),
        fields: AdtFields::Two(
            Value::String(err.path.clone().into()),
            Value::String(err.message.clone().into()),
        ),
    }));
    Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("JsonErr".to_string()),
        fields: AdtFields::One(error),
    }))
}

fn json_to_value(value: &crate::runtime::json::JsonValue) -> Value {
    use crate::runtime::json::{JsonNumber, JsonValue};
    use crate::runtime::value::{AdtFields, AdtValue};

    match value {
        JsonValue::Null => Value::AdtUnit(Rc::new("JsonNull".to_string())),
        JsonValue::Bool(v) => Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("JsonBool".to_string()),
            fields: AdtFields::One(Value::Boolean(*v)),
        })),
        JsonValue::Number(number) => {
            let payload = match number {
                JsonNumber::Int(v) => Value::Adt(Rc::new(AdtValue {
                    constructor: Rc::new("JsonInt".to_string()),
                    fields: AdtFields::One(Value::Integer(*v)),
                })),
                JsonNumber::Float(v) => Value::Adt(Rc::new(AdtValue {
                    constructor: Rc::new("JsonFloat".to_string()),
                    fields: AdtFields::One(Value::Float(*v)),
                })),
            };
            Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("JsonNumber".to_string()),
                fields: AdtFields::One(payload),
            }))
        }
        JsonValue::String(v) => Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("JsonString".to_string()),
            fields: AdtFields::One(Value::String(v.clone().into())),
        })),
        JsonValue::Array(values) => Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("JsonArray".to_string()),
            fields: AdtFields::One(Value::Array(Rc::new(
                values.iter().map(json_to_value).collect(),
            ))),
        })),
        JsonValue::Object(values) => {
            let mut map = rc_hamt::hamt_empty();
            for (key, item) in values {
                map = rc_hamt::hamt_insert(&map, HashKey::String(key.clone()), json_to_value(item));
            }
            Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new("JsonObject".to_string()),
                fields: AdtFields::One(Value::HashMap(map)),
            }))
        }
    }
}

fn value_to_json(value: &Value) -> Result<crate::runtime::json::JsonValue, String> {
    use crate::runtime::json::{JsonNumber, JsonValue};

    match value {
        Value::AdtUnit(name) if name.as_ref() == "JsonNull" => Ok(JsonValue::Null),
        Value::Adt(adt) => match adt.constructor.as_str() {
            "JsonBool" => match adt.fields.get(0) {
                Some(Value::Boolean(v)) => Ok(JsonValue::Bool(*v)),
                Some(other) => Err(terr("json_stringify(JsonBool)", "Bool", other)),
                None => Err("JsonBool missing value".into()),
            },
            "JsonNumber" => match adt.fields.get(0) {
                Some(Value::Adt(payload)) if payload.constructor.as_str() == "JsonInt" => {
                    match payload.fields.get(0) {
                        Some(Value::Integer(v)) => Ok(JsonValue::Number(JsonNumber::Int(*v))),
                        Some(other) => Err(terr("json_stringify(JsonInt)", "Int", other)),
                        None => Err("JsonInt missing value".into()),
                    }
                }
                Some(Value::Adt(payload)) if payload.constructor.as_str() == "JsonFloat" => {
                    match payload.fields.get(0) {
                        Some(Value::Float(v)) => Ok(JsonValue::Number(JsonNumber::Float(*v))),
                        Some(other) => Err(terr("json_stringify(JsonFloat)", "Float", other)),
                        None => Err("JsonFloat missing value".into()),
                    }
                }
                Some(Value::Float(v)) => Ok(JsonValue::Number(JsonNumber::Float(*v))),
                Some(Value::Integer(v)) => Ok(JsonValue::Number(JsonNumber::Int(*v))),
                Some(other) => Err(terr("json_stringify(JsonNumber)", "JsonNumber", other)),
                None => Err("JsonNumber missing value".into()),
            },
            "JsonString" => match adt.fields.get(0) {
                Some(Value::String(v)) => Ok(JsonValue::String(v.to_string())),
                Some(other) => Err(terr("json_stringify(JsonString)", "String", other)),
                None => Err("JsonString missing value".into()),
            },
            "JsonArray" => match adt.fields.get(0) {
                Some(Value::Array(items)) => {
                    let values = items
                        .iter()
                        .map(value_to_json)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(JsonValue::Array(values))
                }
                Some(other) => Err(terr("json_stringify(JsonArray)", "Array", other)),
                None => Err("JsonArray missing value".into()),
            },
            "JsonObject" => match adt.fields.get(0) {
                Some(Value::HashMap(map)) => {
                    let mut values = std::collections::BTreeMap::new();
                    for (key, item) in rc_hamt::hamt_iter(map) {
                        let HashKey::String(key) = key else {
                            return Err("json_stringify(JsonObject): key must be String".into());
                        };
                        values.insert(key, value_to_json(&item)?);
                    }
                    Ok(JsonValue::Object(values))
                }
                Some(other) => Err(terr("json_stringify(JsonObject)", "Map", other)),
                None => Err("JsonObject missing value".into()),
            },
            other => Err(format!("json_stringify expected Json, got {other}")),
        },
        other => Err(terr("json_stringify", "Json", other)),
    }
}

fn http_request_value(req: &crate::runtime::http::HttpRequest) -> Value {
    use crate::runtime::value::{AdtFields, AdtValue};
    Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("Request".to_string()),
        fields: AdtFields::from_vec(vec![
            Value::AdtUnit(Rc::new(method_constructor(&req.method).to_string())),
            Value::String(req.target.clone().into()),
            Value::HashMap(rc_hamt::hamt_empty()),
            Value::String(String::from_utf8_lossy(&req.body).to_string().into()),
        ]),
    }))
}

fn http_response_value(resp: &crate::runtime::http::HttpResponse) -> Value {
    use crate::runtime::value::{AdtFields, AdtValue};

    let mut headers = rc_hamt::hamt_empty();
    for (name, value) in &resp.headers {
        headers = rc_hamt::hamt_insert(
            &headers,
            HashKey::String(name.clone()),
            Value::String(value.clone().into()),
        );
    }
    Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new("Response".to_string()),
        fields: AdtFields::from_vec(vec![
            Value::Integer(resp.status as i64),
            Value::HashMap(headers),
            Value::String(String::from_utf8_lossy(&resp.body).to_string().into()),
        ]),
    }))
}

fn http_method_name(value: &Value) -> Result<&'static str, String> {
    let name = match value {
        Value::AdtUnit(name) => name.as_ref(),
        Value::Adt(adt) if adt.fields.len() == 0 => adt.constructor.as_ref(),
        other => return Err(terr("http_write_request(method)", "Method", other)),
    };
    Ok(match name.as_str() {
        "Post" => "POST",
        "Put" => "PUT",
        "Delete" => "DELETE",
        "Patch" => "PATCH",
        "Head" => "HEAD",
        "Options" => "OPTIONS",
        _ => "GET",
    })
}

fn http_header_pairs(value: &Value, op: &str) -> Result<Vec<(String, String)>, String> {
    let node = ehamt(value, op)?;
    let mut out = Vec::new();
    for (key, value) in rc_hamt::hamt_iter(node) {
        let HashKey::String(name) = key else {
            return Err(format!("{op}: header name must be String"));
        };
        let Value::String(header_value) = value else {
            return Err(format!("{op}: header value must be String"));
        };
        out.push((name, header_value.to_string()));
    }
    Ok(out)
}

fn config_usize_field(
    fields: &crate::runtime::value::AdtFields,
    index: usize,
    name: &str,
) -> Result<usize, String> {
    match fields.get(index) {
        Some(Value::Integer(n)) if *n >= 0 => Ok(*n as usize),
        Some(Value::Integer(n)) => Err(format!(
            "http_serve_config: ServerConfig.{name} must be non-negative, got {n}"
        )),
        Some(other) => Err(format!(
            "http_serve_config: ServerConfig.{name} expected Int, got {}",
            other.type_name()
        )),
        None => Err(format!("http_serve_config: ServerConfig missing {name}")),
    }
}

fn config_optional_usize_field(
    fields: &crate::runtime::value::AdtFields,
    index: usize,
    name: &str,
) -> Result<Option<usize>, String> {
    match fields.get(index) {
        Some(Value::None) => Ok(None),
        Some(Value::Some(inner)) => match inner.as_ref() {
            Value::Integer(n) if *n >= 0 => Ok(Some(*n as usize)),
            Value::Integer(n) => Err(format!(
                "http_serve_config: ServerConfig.{name} must be non-negative, got {n}"
            )),
            other => Err(format!(
                "http_serve_config: ServerConfig.{name} expected Option<Int>, got Some({})",
                other.type_name()
            )),
        },
        Some(other) => Err(format!(
            "http_serve_config: ServerConfig.{name} expected Option<Int>, got {}",
            other.type_name()
        )),
        None => Err(format!("http_serve_config: ServerConfig missing {name}")),
    }
}

fn method_constructor(method: &str) -> &'static str {
    match method {
        "POST" => "Post",
        "PUT" => "Put",
        "DELETE" => "Delete",
        "PATCH" => "Patch",
        "HEAD" => "Head",
        "OPTIONS" => "Options",
        _ => "Get",
    }
}

fn response_parts(value: &Value) -> Result<(i64, String), String> {
    let Value::Adt(adt) = value else {
        return Err(format!(
            "http handler returned {}, expected Response",
            value.type_name()
        ));
    };
    if adt.constructor.as_ref() != "Response" {
        return Err(format!(
            "http handler returned {}, expected Response",
            adt.constructor
        ));
    }
    let status = match adt.fields.get(0) {
        Some(Value::Integer(n)) => *n,
        Some(other) => {
            return Err(format!(
                "Response.status expected Int, got {}",
                other.type_name()
            ));
        }
        None => return Err("Response missing status field".into()),
    };
    let body = match adt.fields.get(2) {
        Some(Value::String(s)) => s.to_string(),
        Some(other) => {
            return Err(format!(
                "Response.body expected String, got {}",
                other.type_name()
            ));
        }
        None => return Err("Response missing body field".into()),
    };
    Ok((status, body))
}

fn stream_response_head_parts(value: &Value) -> Result<(i64, Vec<(String, String)>), String> {
    let Value::Adt(adt) = value else {
        return Err(format!(
            "http streaming handler returned {}, expected StreamResponse",
            value.type_name()
        ));
    };
    if adt.constructor.as_ref() != "StreamResponse" {
        return Err(format!(
            "http streaming handler returned {}, expected StreamResponse",
            adt.constructor
        ));
    }
    let status = match adt.fields.get(0) {
        Some(Value::Integer(n)) => *n,
        Some(other) => {
            return Err(format!(
                "StreamResponse.status expected Int, got {}",
                other.type_name()
            ));
        }
        None => return Err("StreamResponse missing status field".into()),
    };
    let headers = match adt.fields.get(1) {
        Some(value) => http_header_pairs(value, "http_write_chunked_head")?,
        None => return Err("StreamResponse missing headers field".into()),
    };
    Ok((status, headers))
}

fn http_reason(status: i64) -> String {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        504 => "Gateway Timeout",
        _ => "OK",
    }
    .to_string()
}

#[cfg(test)]
mod http_config_tests {
    use super::*;
    use crate::runtime::value::{AdtFields, AdtValue};

    fn server_config_value(worker_count: Value) -> Value {
        Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("ServerConfig".to_string()),
            fields: AdtFields::from_vec(vec![
                Value::Integer(11),
                Value::Integer(22),
                Value::Integer(33),
                Value::Integer(44),
                worker_count,
            ]),
        }))
    }

    #[test]
    fn http_server_config_parses_worker_count_option() {
        let parsed = http_server_config(&server_config_value(Value::Some(Rc::new(
            Value::Integer(3),
        ))))
        .expect("parse ServerConfig");

        assert_eq!(parsed.max_connections, 11);
        assert_eq!(parsed.limits.max_header_bytes, 22);
        assert_eq!(parsed.limits.max_body_bytes, 33);
        assert_eq!(parsed.request_timeout_ms, 44);
        assert_eq!(parsed.worker_count, Some(3));
    }

    #[test]
    fn http_server_config_rejects_negative_worker_count() {
        let err = http_server_config(&server_config_value(Value::Some(Rc::new(Value::Integer(
            -1,
        )))))
        .expect_err("negative worker_count should fail");

        assert!(
            err.contains("ServerConfig.worker_count must be non-negative"),
            "{err}"
        );
    }
}
