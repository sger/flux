//! Native/LLVM C ABI shims for proposal 0174 Phase 1b-vi-d.
//!
//! These symbols are linked into LLVM-generated native binaries and provide
//! the narrow entry surface from the C runtime into the Rust async backend.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::net::SocketAddr;
use std::slice;
use std::str;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use super::await_coordinator::{AwaitCoordinator, AwaitEvent};
use super::backend::{AsyncBackend, CompletionPayload, IoHandle, RequestId};
use super::backends::mio::{MioBackend, configure_default_dns_pool_size};

static BACKEND: OnceLock<MioBackend> = OnceLock::new();
static CALLBACKS: OnceLock<FluxAsyncCallbacks> = OnceLock::new();
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_FIBER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SCOPE_ID: AtomicU64 = AtomicU64::new(1);
static READY_TIMERS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
static ACTIVE_RUN: OnceLock<Mutex<Option<RunHandle>>> = OnceLock::new();

const FLUX_NONE: i64 = 0;
const DEFAULT_LOGICAL_WORKERS: usize = 2;

/// Resolve the worker count when the caller passes the "use default"
/// sentinel (`worker_count <= 0`). Precedence:
///   1. `FLUX_WORKERS` env var (parsed once, positive integer).
///   2. `std::thread::available_parallelism()`.
///   3. `DEFAULT_LOGICAL_WORKERS` (2) as ultimate fallback.
///
/// Mirrors the VM's `resolved_worker_count` in `vm/core_dispatch.rs` so
/// VM and native give the same default sizing on multi-core machines.
fn resolve_default_worker_count() -> usize {
    static ENV_WORKERS: OnceLock<Option<usize>> = OnceLock::new();
    let env = *ENV_WORKERS.get_or_init(|| {
        std::env::var("FLUX_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
    });
    env.or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(DEFAULT_LOGICAL_WORKERS)
}

/// Work-stealing + least-loaded-queue spawn placement on by default.
/// `FLUX_WORK_STEALING=0` (or `false`/`off`) restores the original
/// per-worker FIFO + round-robin spawn placement — useful for strict
/// owner-only debugging or as a regression escape hatch.
fn work_stealing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        match std::env::var("FLUX_WORK_STEALING").ok().as_deref() {
            Some("0") | Some("false") | Some("FALSE") | Some("off") | Some("OFF") => false,
            _ => true,
        }
    })
}

thread_local! {
    static CURRENT_FIBER: Cell<u64> = const { Cell::new(0) };
    static CURRENT_WORKER: Cell<usize> = const { Cell::new(0) };
    static LAST_RUN_FAILED: Cell<bool> = const { Cell::new(false) };
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FluxAsyncCallbacks {
    call0: unsafe extern "C" fn(i64) -> i64,
    resume1: unsafe extern "C" fn(i64, i64) -> i64,
    try_call0: unsafe extern "C" fn(i64, *mut i64) -> i32,
    try_resume1: unsafe extern "C" fn(i64, i64, *mut i64) -> i32,
    retain: unsafe extern "C" fn(i64),
    release: unsafe extern "C" fn(i64),
    make_tuple2: unsafe extern "C" fn(i64, i64) -> i64,
    wrap_some: unsafe extern "C" fn(i64) -> i64,
    make_adt0: unsafe extern "C" fn(i32) -> i64,
    make_adt1: unsafe extern "C" fn(i32, i64) -> i64,
    suspend: unsafe extern "C" fn(i64, i64) -> i64,
    is_suspended: unsafe extern "C" fn() -> i32,
    current_request: unsafe extern "C" fn() -> i64,
    clear_suspend: unsafe extern "C" fn(),
    compose_conts: unsafe extern "C" fn() -> i64,
    capture_effect_context: unsafe extern "C" fn() -> *mut c_void,
    restore_effect_context: unsafe extern "C" fn(*mut c_void),
    reset_effect_context: unsafe extern "C" fn(),
    release_effect_context: unsafe extern "C" fn(*mut c_void),
    promote: unsafe extern "C" fn(i64),
    enter_worker_thread: unsafe extern "C" fn(),
    make_string: unsafe extern "C" fn(*const u8, usize) -> i64,
    task_spawn: unsafe extern "C" fn(i64) -> i64,
    task_cancel: unsafe extern "C" fn(i64) -> i64,
    register_root_task: unsafe extern "C" fn(i64),
    deregister_root_task: unsafe extern "C" fn(i64),
}

fn callbacks() -> Option<&'static FluxAsyncCallbacks> {
    CALLBACKS.get()
}

#[derive(Clone, Copy)]
enum Work {
    Closure { closure: i64, owned: bool },
    Resume { continuation: i64, value: i64 },
}

#[derive(Clone, Copy)]
enum FiberOutcome {
    Value(i64),
    Error(i64),
}

struct EffectSnapshot(*mut c_void);

unsafe impl Send for EffectSnapshot {}

impl EffectSnapshot {
    fn capture(cb: &FluxAsyncCallbacks) -> Self {
        Self(unsafe { (cb.capture_effect_context)() })
    }

    fn null() -> Self {
        Self(std::ptr::null_mut())
    }

    fn restore(&self, cb: &FluxAsyncCallbacks) {
        unsafe { (cb.restore_effect_context)(self.0) };
    }

    fn reset_thread(cb: &FluxAsyncCallbacks) {
        unsafe { (cb.reset_effect_context)() };
    }

    fn release(self, cb: &FluxAsyncCallbacks) {
        unsafe { (cb.release_effect_context)(self.0) };
    }
}

struct Fiber {
    id: u64,
    /// Queue that receives normal wakeups. Native idle workers may still run
    /// this fiber by stealing it from that queue.
    home_worker: usize,
    work: Work,
    effect_context: EffectSnapshot,
    /// False for immediate `race` / `first_of` candidates that participate in
    /// source-order tie-breaking. `park` flips this on after first suspend.
    stealable: bool,
}

#[derive(Clone, Copy, Debug)]
struct TryMeta {
    ok_ctor_tag: i32,
    err_ctor_tag: i32,
}

struct NativeRun {
    root: u64,
    ready: Vec<VecDeque<Fiber>>,
    next_child_worker: usize,
    suspended: HashMap<u64, Fiber>,
    pending_wakes: HashMap<u64, FiberOutcome>,
    cancelled_requests: HashSet<u64>,
    fiber_request: HashMap<u64, u64>,
    awaits: AwaitCoordinator<u64, FiberOutcome, TryMeta>,
    scopes: HashMap<u64, HashSet<u64>>,
    fiber_scope: HashMap<u64, u64>,
    cancelled_fibers: HashSet<u64>,
    scope_tasks: HashMap<u64, Vec<i64>>,
    /// Tasks spawned via detached `Task.spawn` while this run is active.
    /// Cancelled at root teardown; deregistered when the user awaits, joins,
    /// or explicitly cancels the handle. Closes the OS-thread leak hole for
    /// `Task.spawn` invoked inside `run_async` without a matching await.
    root_tasks: HashSet<i64>,
    panicked_ctor_tag: i32,
    root_result: Option<FiberOutcome>,
    running: usize,
    shutdown: bool,
}

#[derive(Clone)]
struct RunHandle {
    shared: Arc<RunShared>,
}

struct RunShared {
    state: Mutex<NativeRun>,
    cvar: Condvar,
}

impl NativeRun {
    fn new(root_closure: i64, worker_count: usize) -> Self {
        let root = next_fiber_id();
        let worker_count = worker_count.max(1);
        let mut ready = (0..worker_count)
            .map(|_| VecDeque::new())
            .collect::<Vec<_>>();
        ready[0].push_back(Fiber {
            id: root,
            home_worker: 0,
            work: Work::Closure {
                closure: root_closure,
                owned: false,
            },
            effect_context: callbacks()
                .map(EffectSnapshot::capture)
                .unwrap_or_else(EffectSnapshot::null),
            stealable: false,
        });
        Self {
            root,
            ready,
            next_child_worker: if worker_count > 1 { 1 } else { 0 },
            suspended: HashMap::new(),
            pending_wakes: HashMap::new(),
            cancelled_requests: HashSet::new(),
            fiber_request: HashMap::new(),
            awaits: AwaitCoordinator::new(),
            scopes: HashMap::new(),
            fiber_scope: HashMap::new(),
            cancelled_fibers: HashSet::new(),
            scope_tasks: HashMap::new(),
            root_tasks: HashSet::new(),
            panicked_ctor_tag: 0,
            root_result: None,
            running: 0,
            shutdown: false,
        }
    }

    /// Pick the worker for a fresh spawn.
    ///
    /// When work-stealing is enabled (the default), use **least-loaded-queue**
    /// placement: argmin len(self.ready[w]), tie-broken by lowest worker id
    /// for deterministic test behavior. This proactively keeps queues
    /// balanced so the steal path handles uneven fiber durations rather
    /// than compensating for avoidable spawn imbalance.
    ///
    /// When `FLUX_WORK_STEALING=0`, fall back to the original round-robin
    /// counter. The `next_child_worker` field stays so the round-robin path
    /// still has stable state.
    fn pick_next_worker(&mut self) -> usize {
        if work_stealing_enabled() {
            self.ready
                .iter()
                .enumerate()
                .min_by_key(|(idx, q)| (q.len(), *idx))
                .map(|(idx, _)| idx)
                .unwrap_or(0)
        } else {
            let worker = self.next_child_worker;
            self.next_child_worker = (self.next_child_worker + 1) % self.ready.len();
            worker
        }
    }

    fn push_ready(&mut self, fiber: Fiber) {
        if self.cancelled_fibers.contains(&fiber.id) {
            let fiber_id = fiber.id;
            release_cancelled_fiber(fiber);
            self.forget_cancelled_fiber(fiber_id);
            return;
        }
        self.ready[fiber.home_worker].push_back(fiber);
    }

    /// Pop the next fiber to execute on `worker`.
    ///
    /// Local queues are FIFO. When the local queue is empty and
    /// `FLUX_WORK_STEALING` is enabled, idle workers steal from the back of
    /// another worker's queue. Victim scan order is deterministic so tests
    /// and diagnostics stay reproducible.
    fn pop_ready_or_steal(&mut self, worker: usize) -> Option<Fiber> {
        let fiber = self.ready[worker].pop_front().or_else(|| {
            if !work_stealing_enabled() || self.ready.len() <= 1 {
                return None;
            }
            for offset in 1..self.ready.len() {
                let victim = (worker + offset) % self.ready.len();
                let Some(pos) = self.ready[victim]
                    .iter()
                    .rposition(|fiber| fiber.id != self.root && fiber.stealable)
                else {
                    continue;
                };
                if let Some(fiber) = self.ready[victim].remove(pos) {
                    return Some(fiber);
                }
            }
            None
        })?;
        self.running += 1;
        Some(fiber)
    }

    fn worker_finished(&mut self) {
        self.running = self.running.saturating_sub(1);
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.cancelled_fibers.contains(&id)
    }

    fn forget_cancelled_fiber(&mut self, id: u64) {
        self.cancelled_fibers.remove(&id);
        self.awaits.remove_child(id);
    }

    fn release_discarded_fiber(&mut self, fiber: Fiber) {
        let fiber_id = fiber.id;
        release_cancelled_fiber(fiber);
        self.forget_cancelled_fiber(fiber_id);
    }

    fn has_live_work(&self) -> bool {
        self.running > 0
            || !self.suspended.is_empty()
            || self.ready.iter().any(|queue| !queue.is_empty())
    }

    fn spawn_child(&mut self, closure: i64) -> u64 {
        let id = next_fiber_id();
        let home_worker = self.pick_next_worker();
        self.spawn_child_on(home_worker, id, closure)
    }

    fn spawn_child_on(&mut self, home_worker: usize, id: u64, closure: i64) -> u64 {
        self.spawn_child_on_with_stealable(home_worker, id, closure, true)
    }

    fn spawn_child_on_with_stealable(
        &mut self,
        home_worker: usize,
        id: u64,
        closure: i64,
        stealable: bool,
    ) -> u64 {
        if let Some(cb) = callbacks() {
            unsafe {
                (cb.retain)(closure);
                (cb.promote)(closure);
            }
        }
        self.push_ready(Fiber {
            id,
            home_worker,
            work: Work::Closure {
                closure,
                owned: true,
            },
            effect_context: callbacks()
                .map(EffectSnapshot::capture)
                .unwrap_or_else(EffectSnapshot::null),
            stealable,
        });
        id
    }

    fn new_scope(&mut self) -> u64 {
        let id = next_scope_id();
        self.scopes.entry(id).or_default();
        id
    }

    fn spawn_scoped_child(&mut self, scope: u64, closure: i64) {
        if !self.scopes.contains_key(&scope) {
            self.scopes.insert(scope, HashSet::new());
        }
        let id = self.spawn_child(closure);
        self.scopes.entry(scope).or_default().insert(id);
        self.fiber_scope.insert(id, scope);
    }

    fn park(&mut self, req: u64, mut fiber: Fiber, continuation: i64) {
        if self.is_cancelled(fiber.id) {
            let fiber_id = fiber.id;
            release_cancelled_work(Work::Resume {
                continuation,
                value: FLUX_NONE,
            });
            release_cancelled_fiber(fiber);
            if let Some(value) = self.pending_wakes.remove(&req) {
                release_outcome(value);
            }
            self.forget_cancelled_fiber(fiber_id);
            return;
        }
        promote_value(continuation);
        fiber.stealable = true;
        fiber.work = Work::Resume {
            continuation,
            value: FLUX_NONE,
        };
        if let Some(outcome) = self.pending_wakes.remove(&req) {
            match outcome {
                FiberOutcome::Value(value) => {
                    if let Work::Resume { continuation, .. } = fiber.work {
                        fiber.work = Work::Resume {
                            continuation,
                            value,
                        };
                    }
                    self.push_ready(fiber);
                }
                FiberOutcome::Error(err) => {
                    self.complete_fiber(fiber.id, FiberOutcome::Error(err));
                }
            }
            return;
        }
        self.fiber_request.insert(fiber.id, req);
        let fiber_id = fiber.id;
        self.suspended.insert(req, fiber);

        let ready = self.ready_fiber_ids();
        let events = self
            .awaits
            .record_suspended(fiber_id, |child| ready.contains(&child));
        self.apply_await_events(events);
    }

    fn wake(&mut self, req: u64, outcome: FiberOutcome) {
        if let Some(mut fiber) = self.suspended.remove(&req) {
            self.fiber_request.remove(&fiber.id);
            if self.is_cancelled(fiber.id) {
                let fiber_id = fiber.id;
                release_cancelled_fiber(fiber);
                release_outcome(outcome);
                self.forget_cancelled_fiber(fiber_id);
                return;
            }
            match outcome {
                FiberOutcome::Value(value) => {
                    if fiber.home_worker != current_worker() {
                        promote_value(value);
                    }
                    if let Work::Resume { continuation, .. } = fiber.work {
                        fiber.work = Work::Resume {
                            continuation,
                            value,
                        };
                    }
                    self.push_ready(fiber);
                }
                FiberOutcome::Error(err) => {
                    self.complete_fiber(fiber.id, FiberOutcome::Error(err));
                }
            }
        } else if self.cancelled_requests.remove(&req) {
            release_outcome(outcome);
        } else {
            if let FiberOutcome::Value(value) = outcome {
                promote_value(value);
            }
            self.pending_wakes.insert(req, outcome);
        }
    }

    fn complete_fiber(&mut self, id: u64, outcome: FiberOutcome) {
        if self.is_cancelled(id) {
            release_outcome(outcome);
            self.forget_cancelled_fiber(id);
            return;
        }

        if id == self.root {
            self.root_result = Some(outcome);
            self.shutdown = true;
        }

        if let Some(scope) = self.fiber_scope.remove(&id)
            && let Some(children) = self.scopes.get_mut(&scope)
        {
            children.remove(&id);
        }

        let ready = self.ready_fiber_ids();
        let events = self.awaits.record_completed(
            id,
            outcome,
            |child| ready.contains(&child),
            |outcome| matches!(outcome, FiberOutcome::Error(_)),
        );
        self.apply_await_events(events);
    }

    fn ready_fiber_ids(&self) -> HashSet<u64> {
        self.ready
            .iter()
            .flat_map(|queue| queue.iter().map(|fiber| fiber.id))
            .collect()
    }

    fn apply_await_events(&mut self, events: Vec<AwaitEvent<u64, FiberOutcome, TryMeta>>) {
        for event in events {
            match event {
                AwaitEvent::BothReady {
                    request,
                    left,
                    right,
                } => match (left, right) {
                    (FiberOutcome::Value(left), FiberOutcome::Value(right)) => {
                        if let Some(cb) = callbacks() {
                            let tuple = unsafe { (cb.make_tuple2)(left, right) };
                            promote_value(tuple);
                            self.wake(request, FiberOutcome::Value(tuple));
                        }
                    }
                    (FiberOutcome::Error(err), other) | (other, FiberOutcome::Error(err)) => {
                        release_outcome(other);
                        self.wake(request, FiberOutcome::Error(err));
                    }
                },
                AwaitEvent::BothError {
                    request,
                    error,
                    loser,
                    discarded,
                } => {
                    for outcome in discarded {
                        release_outcome(outcome);
                    }
                    self.wake(request, error);
                    self.cancel_fibers(&[loser]);
                }
                AwaitEvent::TryReady {
                    request,
                    outcome,
                    meta,
                } => {
                    if let Some(cb) = callbacks() {
                        let result = match outcome {
                            FiberOutcome::Value(v) => unsafe {
                                (cb.make_adt1)(meta.ok_ctor_tag, v)
                            },
                            FiberOutcome::Error(err) => unsafe {
                                (cb.make_adt1)(meta.err_ctor_tag, err)
                            },
                        };
                        promote_value(result);
                        self.wake(request, FiberOutcome::Value(result));
                    } else {
                        release_outcome(outcome);
                        self.wake(request, FiberOutcome::Value(FLUX_NONE));
                    }
                }
                AwaitEvent::RaceReady {
                    request,
                    outcome,
                    losers,
                    discarded,
                } => {
                    for outcome in discarded {
                        release_outcome(outcome);
                    }
                    self.wake(request, outcome);
                    self.cancel_fibers(&losers);
                }
                AwaitEvent::FirstOfReady {
                    request,
                    index,
                    outcome,
                    losers,
                    discarded,
                } => {
                    for outcome in discarded {
                        release_outcome(outcome);
                    }
                    match outcome {
                        FiberOutcome::Value(winner_value) => {
                            if let Some(cb) = callbacks() {
                                let tuple = unsafe {
                                    (cb.make_tuple2)(tag_int(index as u64), winner_value)
                                };
                                promote_value(tuple);
                                self.wake(request, FiberOutcome::Value(tuple));
                            } else {
                                release_completion_value(winner_value);
                                self.wake(request, FiberOutcome::Value(FLUX_NONE));
                            }
                        }
                        FiberOutcome::Error(err) => self.wake(request, FiberOutcome::Error(err)),
                    }
                    self.cancel_fibers(&losers);
                }
                AwaitEvent::TimeoutBodyReady { request, outcome } => {
                    backend().cancel(RequestId(request));
                    self.cancelled_requests.insert(request);
                    match outcome {
                        FiberOutcome::Value(value) => {
                            if let Some(cb) = callbacks() {
                                let some = unsafe { (cb.wrap_some)(value) };
                                promote_value(some);
                                self.wake(request, FiberOutcome::Value(some));
                            } else {
                                release_completion_value(value);
                                self.wake(request, FiberOutcome::Value(FLUX_NONE));
                            }
                        }
                        FiberOutcome::Error(err) => {
                            self.wake(request, FiberOutcome::Error(err));
                        }
                    }
                }
                AwaitEvent::TimeoutTimerReady { request, body } => {
                    self.wake(request, FiberOutcome::Value(FLUX_NONE));
                    self.cancel_fibers(&[body]);
                }
            }
        }
    }

    fn cancel_fibers(&mut self, fiber_ids: &[u64]) {
        if fiber_ids.is_empty() {
            return;
        }

        let losers: HashSet<u64> = fiber_ids.iter().copied().collect();
        self.cancelled_fibers.extend(losers.iter().copied());
        let mut discarded = Vec::new();
        for queue in &mut self.ready {
            let mut kept = VecDeque::new();
            while let Some(fiber) = queue.pop_front() {
                if losers.contains(&fiber.id) {
                    let fiber_id = fiber.id;
                    release_cancelled_fiber(fiber);
                    discarded.push(fiber_id);
                } else {
                    kept.push_back(fiber);
                }
            }
            *queue = kept;
        }
        for id in discarded {
            self.forget_cancelled_fiber(id);
        }

        for id in fiber_ids {
            if let Some(scope) = self.fiber_scope.remove(id)
                && let Some(children) = self.scopes.get_mut(&scope)
            {
                children.remove(id);
            }
            if let Some(req) = self.fiber_request.remove(id) {
                backend().cancel(RequestId(req));
                self.cancelled_requests.insert(req);
                if let Some(fiber) = self.suspended.remove(&req) {
                    let fiber_id = fiber.id;
                    release_cancelled_fiber(fiber);
                    self.forget_cancelled_fiber(fiber_id);
                }
            }
        }
    }

    fn register_task_in_scope(&mut self, scope: u64, task_id: i64) {
        self.scope_tasks.entry(scope).or_default().push(task_id);
    }

    fn cancel_scope(&mut self, scope: u64) {
        let children: Vec<u64> = self
            .scopes
            .remove(&scope)
            .map(|children| children.into_iter().collect())
            .unwrap_or_default();
        self.cancel_fibers(&children);
        // Also cancel any tasks registered under this scope via spawn_scoped.
        // flux_task_cancel is idempotent — no-op for already-completed tasks.
        let task_ids = self.scope_tasks.remove(&scope).unwrap_or_default();
        if !task_ids.is_empty()
            && let Some(cb) = callbacks()
        {
            for task_id in task_ids {
                unsafe { (cb.task_cancel)(task_id) };
            }
        }
    }

    fn route_backend_completion(&mut self, req: u64, payload: CompletionPayload) {
        if self.cancelled_requests.remove(&req) {
            return;
        }

        if let CompletionPayload::AddressList(addrs) = payload {
            if let Some(addr) = addrs.first().copied() {
                backend().tcp_connect(RequestId(req), addr);
            } else {
                self.wake(req, FiberOutcome::Value(FLUX_NONE));
            }
            return;
        }

        if let Some(event) = self.awaits.route_timeout_timer(req) {
            self.apply_await_events(vec![event]);
            return;
        }

        self.wake(req, FiberOutcome::Value(completion_payload_value(payload)));
    }
}

fn completion_payload_value(payload: CompletionPayload) -> i64 {
    match payload {
        CompletionPayload::Unit | CompletionPayload::Error(_) => FLUX_NONE,
        CompletionPayload::TcpHandle(handle) => tag_int(handle.0),
        CompletionPayload::Bytes(bytes) => callbacks()
            .map(|cb| unsafe { (cb.make_string)(bytes.as_ptr(), bytes.len()) })
            .unwrap_or(FLUX_NONE),
        CompletionPayload::AddressList(_) => FLUX_NONE,
    }
}

fn release_executed_work(work: Work) {
    if let Some(cb) = callbacks() {
        unsafe {
            match work {
                Work::Closure {
                    closure,
                    owned: true,
                } => (cb.release)(closure),
                Work::Closure { owned: false, .. } => {}
                Work::Resume { continuation, .. } => (cb.release)(continuation),
            }
        }
    }
}

fn release_effect_context(snapshot: EffectSnapshot) {
    if let Some(cb) = callbacks() {
        snapshot.release(cb);
    }
}

fn release_finished_fiber(fiber: Fiber) {
    release_executed_work(fiber.work);
    release_effect_context(fiber.effect_context);
}

fn release_cancelled_fiber(fiber: Fiber) {
    release_cancelled_work(fiber.work);
    release_effect_context(fiber.effect_context);
}

fn release_completion_value(value: i64) {
    if value == FLUX_NONE {
        return;
    }
    if let Some(cb) = callbacks() {
        unsafe {
            (cb.release)(value);
        }
    }
}

fn release_outcome(outcome: FiberOutcome) {
    match outcome {
        FiberOutcome::Value(value) | FiberOutcome::Error(value) => release_completion_value(value),
    }
}

impl RunHandle {
    fn new(root_closure: i64, worker_count: usize) -> Self {
        Self {
            shared: Arc::new(RunShared {
                state: Mutex::new(NativeRun::new(root_closure, worker_count)),
                cvar: Condvar::new(),
            }),
        }
    }

    fn notify_all(&self) {
        self.shared.cvar.notify_all();
    }
}

fn release_cancelled_work(work: Work) {
    if let Some(cb) = callbacks() {
        unsafe {
            match work {
                Work::Closure {
                    closure,
                    owned: true,
                } => (cb.release)(closure),
                Work::Closure { owned: false, .. } => {}
                Work::Resume {
                    continuation,
                    value,
                } => {
                    (cb.release)(continuation);
                    (cb.release)(value);
                }
            }
        }
    }
}

fn promote_value(value: i64) {
    if let Some(cb) = callbacks() {
        unsafe {
            (cb.promote)(value);
        }
    }
}

fn current_worker() -> usize {
    CURRENT_WORKER.with(Cell::get)
}

fn backend() -> &'static MioBackend {
    BACKEND.get_or_init(MioBackend::new)
}

fn ready_timers() -> &'static Mutex<HashSet<u64>> {
    READY_TIMERS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_fiber_id() -> u64 {
    NEXT_FIBER_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_scope_id() -> u64 {
    NEXT_SCOPE_ID.fetch_add(1, Ordering::Relaxed)
}

fn tag_int(raw: u64) -> i64 {
    ((raw as i64) << 1) | 1
}

fn untag_int(value: i64) -> u64 {
    (value >> 1) as u64
}

fn is_adt_value(value: i64) -> bool {
    if (value & 1) != 0 || (value as u64) < 12 {
        return false;
    }
    unsafe {
        let ptr = value as *const u8;
        *ptr.offset(-3) == 0xF2
    }
}

fn active_run_slot() -> &'static Mutex<Option<RunHandle>> {
    ACTIVE_RUN.get_or_init(|| Mutex::new(None))
}

fn active_run() -> Option<RunHandle> {
    active_run_slot().lock().ok()?.clone()
}

fn set_active_run(run: Option<RunHandle>) {
    if let Ok(mut slot) = active_run_slot().lock() {
        *slot = run;
    }
}

fn with_run<R>(f: impl FnOnce(&mut NativeRun) -> R) -> Option<R> {
    let handle = active_run()?;
    let mut state = handle.shared.state.lock().ok()?;
    let result = f(&mut state);
    drop(state);
    handle.notify_all();
    Some(result)
}

fn execute_fiber(handle: &RunHandle, worker: usize, mut fiber: Fiber, cb: FluxAsyncCallbacks) {
    {
        let mut state = handle
            .shared
            .state
            .lock()
            .expect("native async state poisoned");
        if state.is_cancelled(fiber.id) {
            state.release_discarded_fiber(fiber);
            state.worker_finished();
            drop(state);
            handle.notify_all();
            return;
        }
    }

    CURRENT_WORKER.with(|current| current.set(worker));
    CURRENT_FIBER.with(|current| current.set(fiber.id));
    fiber.effect_context.restore(&cb);
    let mut result = FLUX_NONE;
    let ok = unsafe {
        match fiber.work {
            Work::Closure { closure, .. } => (cb.try_call0)(closure, &mut result as *mut i64) != 0,
            Work::Resume {
                continuation,
                value,
            } => (cb.try_resume1)(continuation, value, &mut result as *mut i64) != 0,
        }
    };
    CURRENT_FIBER.with(|current| current.set(0));

    let mut state = handle
        .shared
        .state
        .lock()
        .expect("native async state poisoned");
    if unsafe { (cb.is_suspended)() } != 0 {
        let request = untag_int(unsafe { (cb.current_request)() });
        let continuation = unsafe { (cb.compose_conts)() };
        unsafe {
            (cb.clear_suspend)();
        }
        let previous_context =
            std::mem::replace(&mut fiber.effect_context, EffectSnapshot::capture(&cb));
        previous_context.release(&cb);
        EffectSnapshot::reset_thread(&cb);
        let executed_work = fiber.work;
        state.park(request, fiber, continuation);
        state.worker_finished();
        drop(state);
        release_executed_work(executed_work);
        handle.notify_all();
        return;
    }

    EffectSnapshot::reset_thread(&cb);
    let outcome = if ok {
        FiberOutcome::Value(result)
    } else if is_adt_value(result) {
        FiberOutcome::Error(result)
    } else if state.panicked_ctor_tag != 0 {
        let err = unsafe { (cb.make_adt1)(state.panicked_ctor_tag, result) };
        promote_value(err);
        FiberOutcome::Error(err)
    } else {
        FiberOutcome::Error(result)
    };
    if ok && fiber.id != state.root {
        promote_value(result);
    }
    state.complete_fiber(fiber.id, outcome);
    state.worker_finished();
    drop(state);
    release_finished_fiber(fiber);
    handle.notify_all();
}

fn worker_loop(handle: RunHandle, worker: usize, cb: FluxAsyncCallbacks) {
    unsafe {
        (cb.enter_worker_thread)();
    }
    CURRENT_WORKER.with(|current| current.set(worker));
    loop {
        let fiber = {
            let mut state = handle
                .shared
                .state
                .lock()
                .expect("native async state poisoned");
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(fiber) = state.pop_ready_or_steal(worker) {
                    break fiber;
                }
                state = handle
                    .shared
                    .cvar
                    .wait(state)
                    .expect("native async state poisoned");
            }
        };
        execute_fiber(&handle, worker, fiber, cb);
    }
}

fn spawn_workers(
    handle: &RunHandle,
    cb: FluxAsyncCallbacks,
    worker_count: usize,
) -> Vec<JoinHandle<()>> {
    (1..worker_count.max(1))
        .map(|worker| {
            let handle = handle.clone();
            std::thread::Builder::new()
                .name(format!("flux-async-worker-{worker}"))
                .spawn(move || worker_loop(handle, worker, cb))
                .expect("spawn native async worker")
        })
        .collect()
}

/// Register C-runtime callbacks used by the Rust async scheduler.
///
/// The `flux` Rust library is also linked into ordinary Rust test binaries
/// that do not link `runtime/c`. Keeping these callbacks in a C-provided
/// table avoids unresolved externals in those binaries while preserving the
/// native executable path, where `runtime/c/tasks.c` registers the real
/// callback set before entering `flux_async_run_root`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_set_callbacks(callbacks: *const FluxAsyncCallbacks) -> i32 {
    if callbacks.is_null() {
        return -1;
    }
    let callbacks = unsafe { *callbacks };
    let _ = CALLBACKS.set(callbacks);
    0
}

/// Initialize the process-global native async backend.
///
/// Returns 0 on success, -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_runtime_init() -> i32 {
    match backend().start() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Shut down the process-global native async backend.
///
/// The current native driver runs a short-lived executable per Flux program,
/// so this is mostly a test/diagnostic hook. It is intentionally idempotent.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_shutdown() -> i32 {
    match backend().shutdown() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Allocate a timer request and submit it to the Rust `mio` backend.
///
/// `ms` is an untagged millisecond count from the C runtime. Returns the raw
/// request id, or 0 on failure. RequestId(0) is reserved as a sentinel.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_timer_start(ms: i64) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let req = next_request_id();
    let delay = if ms < 0 { 0 } else { ms as u64 };
    backend().timer_start(RequestId(req), delay);
    req
}

fn socket_addr_from_raw(
    host: *const u8,
    host_len: usize,
    port: i64,
    listen: bool,
) -> Option<SocketAddr> {
    let host = string_from_raw(host, host_len)?;
    socket_addr_from_host(&host, port, listen)
}

fn string_from_raw(data: *const u8, len: usize) -> Option<String> {
    if data.is_null() {
        return None;
    }
    let bytes = unsafe { slice::from_raw_parts(data, len) };
    str::from_utf8(bytes).ok().map(str::to_string)
}

fn socket_addr_from_host(host: &str, port: i64, listen: bool) -> Option<SocketAddr> {
    let host = if listen && host.is_empty() {
        "0.0.0.0"
    } else {
        host
    };
    format!("{host}:{port}").parse().ok()
}

fn bytes_from_raw(data: *const u8, len: usize) -> Option<Vec<u8>> {
    if data.is_null() && len != 0 {
        return None;
    }
    if len == 0 {
        return Some(Vec::new());
    }
    Some(unsafe { slice::from_raw_parts(data, len) }.to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_connect(host: *const u8, host_len: usize, port: i64) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let Some(host) = string_from_raw(host, host_len) else {
        return 0;
    };
    let req = next_request_id();
    if let Some(addr) = socket_addr_from_host(&host, port, false) {
        backend().tcp_connect(RequestId(req), addr);
        return req;
    }
    let Ok(port) = u16::try_from(port) else {
        return 0;
    };
    backend().dns_resolve(RequestId(req), host, port);
    req
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_listen(host: *const u8, host_len: usize, port: i64) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let Some(addr) = socket_addr_from_raw(host, host_len, port, true) else {
        return 0;
    };
    let req = next_request_id();
    backend().tcp_listen(RequestId(req), addr);
    req
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_read(handle: u64, max: usize) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let req = next_request_id();
    let max = if max > 0 && max <= (1 << 24) {
        max
    } else {
        4096
    };
    backend().tcp_read(RequestId(req), IoHandle(handle), max);
    req
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_write_all(handle: u64, data: *const u8, len: usize) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let Some(bytes) = bytes_from_raw(data, len) else {
        return 0;
    };
    let req = next_request_id();
    backend().tcp_write(RequestId(req), IoHandle(handle), bytes);
    req
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_accept(handle: u64) -> u64 {
    if flux_async_runtime_init() != 0 {
        return 0;
    }
    let req = next_request_id();
    backend().tcp_accept(RequestId(req), IoHandle(handle));
    req
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_tcp_close(handle: u64) -> i32 {
    if flux_async_runtime_init() != 0 {
        return -1;
    }
    backend().tcp_close(IoHandle(handle));
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_run_root(root_closure: i64) -> i64 {
    flux_async_run_root_configured(root_closure, resolve_default_worker_count())
}

fn flux_async_run_root_configured(root_closure: i64, worker_count: usize) -> i64 {
    let Some(cb) = callbacks().copied() else {
        return FLUX_NONE;
    };

    if flux_async_runtime_init() != 0 {
        return FLUX_NONE;
    }
    LAST_RUN_FAILED.with(|failed| failed.set(false));

    let previous_run = active_run();
    let handle = RunHandle::new(root_closure, worker_count);
    set_active_run(Some(handle.clone()));
    let workers = spawn_workers(&handle, cb, worker_count);
    let mut result = FLUX_NONE;

    loop {
        let next = {
            let mut state = handle
                .shared
                .state
                .lock()
                .expect("native async state poisoned");
            if let Some(done) = state.root_result {
                result = match done {
                    FiberOutcome::Value(value) => {
                        LAST_RUN_FAILED.with(|failed| failed.set(false));
                        value
                    }
                    FiberOutcome::Error(value) => {
                        LAST_RUN_FAILED.with(|failed| failed.set(true));
                        value
                    }
                };
                state.shutdown = true;
                None
            } else if let Some(fiber) = state.pop_ready_or_steal(0) {
                Some(fiber)
            } else if !state.has_live_work() {
                state.shutdown = true;
                result = FLUX_NONE;
                None
            } else {
                None
            }
        };

        if let Some(fiber) = next {
            execute_fiber(&handle, 0, fiber, cb);
            continue;
        }

        {
            let state = handle
                .shared
                .state
                .lock()
                .expect("native async state poisoned");
            if state.shutdown {
                break;
            }
        }

        if let Some(completion) = backend().next_completion() {
            let req = completion.request_id.0;
            let payload = completion.payload;
            {
                let mut state = handle
                    .shared
                    .state
                    .lock()
                    .expect("native async state poisoned");
                state.route_backend_completion(req, payload);
            }
            handle.notify_all();
        } else {
            let state = handle
                .shared
                .state
                .lock()
                .expect("native async state poisoned");
            let _ = handle
                .shared
                .cvar
                .wait_timeout(state, Duration::from_millis(1))
                .expect("native async state poisoned");
        }
    }

    {
        let mut state = handle
            .shared
            .state
            .lock()
            .expect("native async state poisoned");
        state.shutdown = true;
    }
    handle.notify_all();
    for worker in workers {
        let _ = worker.join();
    }

    // Drain the root-task safety net: any task spawned via detached
    // `Task.spawn` during this run that the user never awaited / joined /
    // cancelled gets cancelled here. flux_task_cancel is idempotent for
    // already-completed tasks, so the call is safe even when the task
    // finished on its own. Bounds Task.spawn lifetime to run_async.
    let leaked: Vec<i64> = {
        let mut state = handle
            .shared
            .state
            .lock()
            .expect("native async state poisoned");
        state.root_tasks.drain().collect()
    };
    for task_id in leaked {
        unsafe { (cb.task_cancel)(task_id) };
    }

    set_active_run(previous_run);
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_last_run_failed() -> i32 {
    LAST_RUN_FAILED.with(|failed| if failed.get() { 1 } else { 0 })
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_both(left: i64, right: i64) -> u64 {
    with_run(|run| {
        let parent_req = next_request_id();
        let left_id = run.spawn_child(left);
        let right_id = run.spawn_child(right);
        run.awaits.register_both(parent_req, left_id, right_id);
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_race(left: i64, right: i64) -> u64 {
    with_run(|run| {
        let parent_req = next_request_id();
        let worker = current_worker();
        // Launch race candidates on the caller's worker in source order so
        // immediate completions have deterministic FIFO tie-breaking. Once a
        // child suspends, backend completions still race normally.
        let left_id = run.spawn_child_on_with_stealable(worker, next_fiber_id(), left, false);
        let right_id = run.spawn_child_on_with_stealable(worker, next_fiber_id(), right, false);
        run.awaits
            .register_race(parent_req, vec![left_id, right_id]);
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_first_of(children: *const i64, len: usize) -> u64 {
    if children.is_null() || len == 0 {
        return 0;
    }
    let closures = unsafe { slice::from_raw_parts(children, len) };
    with_run(|run| {
        let parent_req = next_request_id();
        let worker = current_worker();
        let mut child_ids = Vec::with_capacity(closures.len());
        for (idx, closure) in closures.iter().copied().enumerate() {
            let id = run.spawn_child_on_with_stealable(worker, next_fiber_id(), closure, false);
            child_ids.push((id, idx));
        }
        run.awaits.register_first_of(parent_req, child_ids);
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_try(
    ok_ctor_tag: i32,
    err_ctor_tag: i32,
    panicked_ctor_tag: i32,
    body: i64,
) -> u64 {
    with_run(|run| {
        run.panicked_ctor_tag = panicked_ctor_tag;
        let parent_req = next_request_id();
        let child_id = run.spawn_child(body);
        run.awaits.register_try(
            parent_req,
            child_id,
            TryMeta {
                ok_ctor_tag,
                err_ctor_tag,
            },
        );
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_timeout(ms: i64, body: i64) -> u64 {
    with_run(|run| {
        let parent_req = next_request_id();
        let body_id = run.spawn_child(body);
        run.awaits.register_timeout(parent_req, body_id);
        let delay = if ms < 0 { 0 } else { ms as u64 };
        backend().timer_start(RequestId(parent_req), delay);
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_scope_new() -> u64 {
    with_run(|run| run.new_scope()).unwrap_or_else(next_scope_id)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fork_scoped(scope: u64, body: i64) -> i32 {
    with_run(|run| run.spawn_scoped_child(scope, body))
        .map(|_| 0)
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_cancel_scope(scope: u64) -> i32 {
    with_run(|run| run.cancel_scope(scope))
        .map(|_| 0)
        .unwrap_or(-1)
}

/// Register a detached task with the active run's root task set so it is
/// cancelled at `run_async` teardown if never awaited / joined / cancelled.
/// No-op if no run is active (preserves detached semantics outside `run_async`).
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_register_root_task(task_id: i64) {
    if task_id <= 0 {
        return;
    }
    let _ = with_run(|run| run.root_tasks.insert(task_id));
}

/// Remove a task from the active run's root task set. Called when the user
/// owns the handle (await, blocking_join, cancel) — the safety net is no
/// longer needed. No-op if no run is active or the id isn't tracked.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_deregister_root_task(task_id: i64) {
    if task_id <= 0 {
        return;
    }
    let _ = with_run(|run| run.root_tasks.remove(&task_id));
}

/// Spawn a task under a fiber scope. The task runs on an OS thread; when
/// `cancel(scope)` is called, the task is cancelled alongside any forked
/// fibers. Returns the task id (positive), or a negative error code.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_task_spawn_scoped(scope: u64, closure: i64) -> i64 {
    let Some(cb) = callbacks() else {
        return -1;
    };
    let task_id = unsafe { (cb.task_spawn)(closure) };
    if task_id <= 0 {
        return task_id;
    }
    // The C `flux_task_spawn` adds the id to `root_tasks` as a safety net.
    // A scoped task already has an explicit owner, so move it from
    // root_tasks → scope_tasks to avoid double-cancel-on-teardown.
    with_run(|run| {
        run.root_tasks.remove(&task_id);
        run.register_task_in_scope(scope, task_id);
    });
    task_id
}

/// Phase 2 slice 2-vii: `flux_async_run_root` with an explicit
/// `RuntimeConfig` (worker_count, fs_pool_size, dns_pool_size).
///
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_run_root_with(
    worker_count: i64,
    _fs_pool_size: i64,
    dns_pool_size: i64,
    root_closure: i64,
) -> i64 {
    if dns_pool_size > 0 {
        configure_default_dns_pool_size(dns_pool_size as usize);
    }
    let worker_count = if worker_count > 0 {
        worker_count as usize
    } else {
        resolve_default_worker_count()
    };
    flux_async_run_root_configured(root_closure, worker_count)
}

/// Phase 2 slice 2-iv: poll whether the *current* fiber's enclosing scope
/// has been cancelled.
///
/// Returns `1` if cancelled, `0` otherwise. Callers (the C shim
/// `flux_fiber_check_cancelled`) convert this to a tagged `Bool` for
/// the Flux source layer.
///
/// Outside `Async.run_async` (no active runtime, or `CURRENT_FIBER == 0`),
/// returns `0` — there is no scope to be cancelled.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_check_cancelled() -> i32 {
    let id = CURRENT_FIBER.with(|c| c.get());
    if id == 0 {
        return 0;
    }
    match with_run(|run| run.is_cancelled(id)) {
        Some(true) => 1,
        _ => 0,
    }
}

/// Slice 2-vii follow-up: report the worker count of the active
/// `Async.run_async` boundary. Returns the native scheduler's
/// configured worker count, or `0` outside any active run.
///
/// Mirrors `vm_fibers::current_num_workers` on the VM path. The
/// C shim `flux_fiber_current_worker_count` boxes this into a
/// tagged Flux `Int`.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_current_worker_count() -> i32 {
    with_run(|run| run.ready.len() as i32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_worker_steals_from_back_of_another_ready_queue() {
        let mut run = NativeRun::new(0, 2);
        let first = run.spawn_child_on(0, next_fiber_id(), 0);
        let second = run.spawn_child_on(0, next_fiber_id(), 0);

        let stolen = run.pop_ready_or_steal(1).expect("worker 1 should steal");
        assert_eq!(stolen.id, second);
        assert_ne!(stolen.id, run.root);

        release_cancelled_fiber(stolen);
        run.cancel_fibers(&[first]);
        while let Some(fiber) = run.ready[0].pop_front() {
            release_cancelled_fiber(fiber);
        }
    }

    #[test]
    fn stealing_skips_order_sensitive_ready_fibers() {
        let mut run = NativeRun::new(0, 2);
        let ordered = run.spawn_child_on_with_stealable(0, next_fiber_id(), 0, false);

        assert!(run.pop_ready_or_steal(1).is_none());
        let local = run.pop_ready_or_steal(0).expect("worker 0 keeps root first");
        assert_eq!(local.id, run.root);
        release_cancelled_fiber(local);
        let local = run
            .pop_ready_or_steal(0)
            .expect("worker 0 runs ordered child");
        assert_eq!(local.id, ordered);
        release_cancelled_fiber(local);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_suspend_request(request_id: u64) -> i64 {
    if request_id == 0 {
        return FLUX_NONE;
    }
    let Some(cb) = callbacks() else {
        return FLUX_NONE;
    };
    unsafe { (cb.suspend)(tag_int(request_id), FLUX_NONE) }
}

/// Allocate a scheduler request for native `Task.await`.
///
/// Returns 0 outside an active `run_async` boundary so the C task shim can
/// fail loudly instead of silently falling back to a blocking join.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_task_await_request() -> u64 {
    if active_run().is_some() {
        next_request_id()
    } else {
        0
    }
}

/// Publish a native task completion into the currently active fiber scheduler.
///
/// `value` is the Flux-level `Option<a>` produced by the C task table:
/// `Some(result)` on success, `None` when cancellation wins.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_task_complete(request_id: u64, value: i64) {
    if request_id == 0 {
        release_completion_value(value);
        return;
    }
    let Some(handle) = active_run() else {
        release_completion_value(value);
        return;
    };

    promote_value(value);
    {
        let mut state = handle
            .shared
            .state
            .lock()
            .expect("native async state poisoned");
        state.wake(request_id, FiberOutcome::Value(value));
    }
    handle.notify_all();
}

/// Poll the backend dispatch path until `req` has completed.
///
/// Returns:
/// - 1 when the request completed successfully,
/// - 0 when no matching completion is currently available,
/// - -1 when the request completed with an error or the backend failed.
#[unsafe(no_mangle)]
pub extern "C" fn flux_async_poll_dispatch(req: u64) -> i32 {
    if req == 0 {
        return -1;
    }

    if let Ok(mut ready) = ready_timers().lock()
        && ready.remove(&req)
    {
        return 1;
    }

    if flux_async_runtime_init() != 0 {
        return -1;
    }

    loop {
        if let Some(completion) = backend().next_completion() {
            match completion.payload {
                CompletionPayload::Unit if completion.request_id.0 == req => return 1,
                CompletionPayload::Error(_) if completion.request_id.0 == req => return -1,
                CompletionPayload::AddressList(addrs) if completion.request_id.0 == req => {
                    let Some(addr) = addrs.first().copied() else {
                        return -1;
                    };
                    backend().tcp_connect(RequestId(req), addr);
                }
                CompletionPayload::Unit => {
                    if let Ok(mut ready) = ready_timers().lock() {
                        ready.insert(completion.request_id.0);
                    }
                }
                CompletionPayload::Bytes(_)
                | CompletionPayload::TcpHandle(_)
                | CompletionPayload::AddressList(_)
                | CompletionPayload::Error(_) => {
                    if completion.request_id.0 == req {
                        return -1;
                    }
                }
            }
        } else {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}
