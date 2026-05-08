//! Native/LLVM C ABI shims for proposal 0174 Phase 1b-vi-d.
//!
//! These symbols are linked into LLVM-generated native binaries and provide
//! the narrow entry surface from the C runtime into the Rust async backend.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::slice;
use std::str;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

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
    promote: unsafe extern "C" fn(i64),
    enter_worker_thread: unsafe extern "C" fn(),
    make_string: unsafe extern "C" fn(*const u8, usize) -> i64,
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

#[derive(Clone, Copy)]
struct Fiber {
    id: u64,
    home_worker: usize,
    work: Work,
}

enum AwaitKind {
    Both {
        left: u64,
        right: u64,
        left_value: Option<FiberOutcome>,
        right_value: Option<FiberOutcome>,
    },
    Try {
        child: u64,
        ok_ctor_tag: i32,
        err_ctor_tag: i32,
    },
    Race {
        children: Vec<u64>,
        completed: Vec<(u64, FiberOutcome)>,
    },
    FirstOf {
        children: Vec<(u64, usize)>,
        completed: Vec<(u64, usize, FiberOutcome)>,
    },
    Timeout {
        body: u64,
    },
}

struct NativeRun {
    root: u64,
    ready: Vec<VecDeque<Fiber>>,
    next_child_worker: usize,
    suspended: HashMap<u64, Fiber>,
    pending_wakes: HashMap<u64, FiberOutcome>,
    cancelled_requests: HashSet<u64>,
    fiber_request: HashMap<u64, u64>,
    awaits: HashMap<u64, AwaitKind>,
    awaiter_index: HashMap<u64, Vec<u64>>,
    scopes: HashMap<u64, HashSet<u64>>,
    fiber_scope: HashMap<u64, u64>,
    cancelled_fibers: HashSet<u64>,
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
        });
        Self {
            root,
            ready,
            next_child_worker: if worker_count > 1 { 1 } else { 0 },
            suspended: HashMap::new(),
            pending_wakes: HashMap::new(),
            cancelled_requests: HashSet::new(),
            fiber_request: HashMap::new(),
            awaits: HashMap::new(),
            awaiter_index: HashMap::new(),
            scopes: HashMap::new(),
            fiber_scope: HashMap::new(),
            cancelled_fibers: HashSet::new(),
            panicked_ctor_tag: 0,
            root_result: None,
            running: 0,
            shutdown: false,
        }
    }

    fn next_child_worker(&mut self) -> usize {
        let worker = self.next_child_worker;
        self.next_child_worker = (self.next_child_worker + 1) % self.ready.len();
        worker
    }

    fn push_ready(&mut self, fiber: Fiber) {
        if self.cancelled_fibers.contains(&fiber.id) {
            release_cancelled_work(fiber.work);
            self.forget_cancelled_fiber(fiber.id);
            return;
        }
        self.ready[fiber.home_worker].push_back(fiber);
    }

    fn pop_ready_for_worker(&mut self, worker: usize) -> Option<Fiber> {
        let fiber = self.ready[worker].pop_front()?;
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
        self.awaiter_index.remove(&id);
    }

    fn release_discarded_fiber(&mut self, fiber: Fiber) {
        release_cancelled_work(fiber.work);
        self.forget_cancelled_fiber(fiber.id);
    }

    fn has_live_work(&self) -> bool {
        self.running > 0
            || !self.suspended.is_empty()
            || self.ready.iter().any(|queue| !queue.is_empty())
    }

    fn is_ready(&self, fiber_id: u64) -> bool {
        self.ready
            .iter()
            .any(|queue| queue.iter().any(|fiber| fiber.id == fiber_id))
    }

    fn spawn_child(&mut self, closure: i64) -> u64 {
        let id = next_fiber_id();
        let home_worker = self.next_child_worker();
        self.spawn_child_on(home_worker, id, closure)
    }

    fn spawn_child_on(&mut self, home_worker: usize, id: u64, closure: i64) -> u64 {
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
            release_cancelled_work(Work::Resume {
                continuation,
                value: FLUX_NONE,
            });
            if let Some(value) = self.pending_wakes.remove(&req) {
                release_outcome(value);
            }
            self.forget_cancelled_fiber(fiber.id);
            return;
        }
        promote_value(continuation);
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

        let parent_reqs = self
            .awaiter_index
            .get(&fiber_id)
            .cloned()
            .unwrap_or_default();
        for parent_req in parent_reqs {
            match self.awaits.remove(&parent_req) {
                Some(AwaitKind::Race {
                    children,
                    completed,
                }) => self.resolve_race(parent_req, children, completed),
                Some(AwaitKind::FirstOf {
                    children,
                    completed,
                }) => self.resolve_first_of(parent_req, children, completed),
                Some(other) => {
                    self.awaits.insert(parent_req, other);
                }
                None => {}
            }
        }
    }

    fn wake(&mut self, req: u64, outcome: FiberOutcome) {
        if let Some(mut fiber) = self.suspended.remove(&req) {
            self.fiber_request.remove(&fiber.id);
            if self.is_cancelled(fiber.id) {
                release_cancelled_work(fiber.work);
                release_outcome(outcome);
                self.forget_cancelled_fiber(fiber.id);
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

        let Some(parent_reqs) = self.awaiter_index.remove(&id) else {
            return;
        };

        for parent_req in parent_reqs {
            let Some(await_kind) = self.awaits.remove(&parent_req) else {
                continue;
            };
            match await_kind {
                AwaitKind::Both {
                    left,
                    right,
                    mut left_value,
                    mut right_value,
                } => {
                    if id == left {
                        left_value = Some(outcome);
                    } else if id == right {
                        right_value = Some(outcome);
                    }

                    if let FiberOutcome::Error(err) = outcome {
                        let loser = if id == left { right } else { left };
                        self.wake(parent_req, FiberOutcome::Error(err));
                        self.cancel_fibers(&[loser]);
                        continue;
                    }

                    match (left_value, right_value) {
                        (Some(l), Some(r)) => match (l, r) {
                            (FiberOutcome::Value(l), FiberOutcome::Value(r)) => {
                                if let Some(cb) = callbacks() {
                                    let tuple = unsafe { (cb.make_tuple2)(l, r) };
                                    promote_value(tuple);
                                    self.wake(parent_req, FiberOutcome::Value(tuple));
                                }
                            }
                            (FiberOutcome::Error(err), _) | (_, FiberOutcome::Error(err)) => {
                                self.wake(parent_req, FiberOutcome::Error(err));
                            }
                        },
                        (left_value, right_value) => {
                            self.awaits.insert(
                                parent_req,
                                AwaitKind::Both {
                                    left,
                                    right,
                                    left_value,
                                    right_value,
                                },
                            );
                        }
                    }
                }
                AwaitKind::Try {
                    child,
                    ok_ctor_tag,
                    err_ctor_tag,
                } => {
                    if id == child {
                        if let Some(cb) = callbacks() {
                            let result = match outcome {
                                FiberOutcome::Value(v) => unsafe { (cb.make_adt1)(ok_ctor_tag, v) },
                                FiberOutcome::Error(err) => unsafe {
                                    (cb.make_adt1)(err_ctor_tag, err)
                                },
                            };
                            promote_value(result);
                            self.wake(parent_req, FiberOutcome::Value(result));
                        } else {
                            self.wake(parent_req, FiberOutcome::Value(FLUX_NONE));
                        }
                    }
                }
                AwaitKind::Race {
                    children,
                    mut completed,
                } => {
                    completed.push((id, outcome));
                    self.resolve_race(parent_req, children, completed);
                }
                AwaitKind::FirstOf {
                    children,
                    mut completed,
                } => {
                    let Some((_, index)) = children.iter().find(|(child, _)| *child == id) else {
                        release_outcome(outcome);
                        self.awaits.insert(
                            parent_req,
                            AwaitKind::FirstOf {
                                children,
                                completed,
                            },
                        );
                        continue;
                    };
                    completed.push((id, *index, outcome));
                    self.resolve_first_of(parent_req, children, completed);
                }
                AwaitKind::Timeout { body } => {
                    if id == body {
                        backend().cancel(RequestId(parent_req));
                        self.cancelled_requests.insert(parent_req);
                        match outcome {
                            FiberOutcome::Value(value) => {
                                if let Some(cb) = callbacks() {
                                    let some = unsafe { (cb.wrap_some)(value) };
                                    promote_value(some);
                                    self.wake(parent_req, FiberOutcome::Value(some));
                                }
                            }
                            FiberOutcome::Error(err) => {
                                self.wake(parent_req, FiberOutcome::Error(err));
                            }
                        }
                    }
                }
            }
        }
    }

    fn resolve_race(
        &mut self,
        parent_req: u64,
        children: Vec<u64>,
        completed: Vec<(u64, FiberOutcome)>,
    ) {
        let winner = children.iter().copied().find(|child| {
            completed
                .iter()
                .any(|(completed_child, _)| completed_child == child)
        });

        let Some(winner) = winner else {
            self.awaits.insert(
                parent_req,
                AwaitKind::Race {
                    children,
                    completed,
                },
            );
            return;
        };

        let blocked_by_earlier_ready = children
            .iter()
            .take_while(|child| **child != winner)
            .any(|child| self.is_ready(*child));

        if blocked_by_earlier_ready {
            self.awaits.insert(
                parent_req,
                AwaitKind::Race {
                    children,
                    completed,
                },
            );
            return;
        }

        let mut winner_outcome = FiberOutcome::Value(FLUX_NONE);
        for (completed_child, completed_value) in completed {
            if completed_child == winner {
                winner_outcome = completed_value;
            } else {
                release_outcome(completed_value);
            }
        }

        self.wake(parent_req, winner_outcome);
        let losers: Vec<u64> = children
            .into_iter()
            .filter(|child| *child != winner)
            .collect();
        self.cancel_fibers(&losers);
    }

    fn resolve_first_of(
        &mut self,
        parent_req: u64,
        children: Vec<(u64, usize)>,
        completed: Vec<(u64, usize, FiberOutcome)>,
    ) {
        let winner = children.iter().copied().find(|(child, _)| {
            completed
                .iter()
                .any(|(completed_child, _, _)| completed_child == child)
        });

        let Some((winner, index)) = winner else {
            self.awaits.insert(
                parent_req,
                AwaitKind::FirstOf {
                    children,
                    completed,
                },
            );
            return;
        };

        let blocked_by_earlier_ready = children
            .iter()
            .take_while(|(child, _)| *child != winner)
            .any(|(child, _)| self.is_ready(*child));

        if blocked_by_earlier_ready {
            self.awaits.insert(
                parent_req,
                AwaitKind::FirstOf {
                    children,
                    completed,
                },
            );
            return;
        }

        let mut winner_outcome = FiberOutcome::Value(FLUX_NONE);
        for (completed_child, _, completed_value) in completed {
            if completed_child == winner {
                winner_outcome = completed_value;
            } else {
                release_outcome(completed_value);
            }
        }

        match winner_outcome {
            FiberOutcome::Value(winner_value) => {
                if let Some(cb) = callbacks() {
                    let tuple = unsafe { (cb.make_tuple2)(tag_int(index as u64), winner_value) };
                    promote_value(tuple);
                    self.wake(parent_req, FiberOutcome::Value(tuple));
                } else {
                    release_completion_value(winner_value);
                    self.wake(parent_req, FiberOutcome::Value(FLUX_NONE));
                }
            }
            FiberOutcome::Error(err) => self.wake(parent_req, FiberOutcome::Error(err)),
        }

        let losers: Vec<u64> = children
            .into_iter()
            .map(|(child, _)| child)
            .filter(|child| *child != winner)
            .collect();
        self.cancel_fibers(&losers);
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
                    release_cancelled_work(fiber.work);
                    discarded.push(fiber.id);
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
                    release_cancelled_work(fiber.work);
                    self.forget_cancelled_fiber(fiber.id);
                }
            }
        }
    }

    fn cancel_scope(&mut self, scope: u64) {
        let children: Vec<u64> = self
            .scopes
            .remove(&scope)
            .map(|children| children.into_iter().collect())
            .unwrap_or_default();
        self.cancel_fibers(&children);
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

        if let Some(AwaitKind::Timeout { body }) = self.awaits.remove(&req) {
            self.wake(req, FiberOutcome::Value(FLUX_NONE));
            self.cancel_fibers(&[body]);
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

fn execute_fiber(handle: &RunHandle, worker: usize, fiber: Fiber, cb: FluxAsyncCallbacks) {
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
        let executed_work = fiber.work;
        state.park(request, fiber, continuation);
        state.worker_finished();
        drop(state);
        release_executed_work(executed_work);
        handle.notify_all();
        return;
    }

    release_executed_work(fiber.work);
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
                if let Some(fiber) = state.pop_ready_for_worker(worker) {
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
    flux_async_run_root_configured(root_closure, DEFAULT_LOGICAL_WORKERS)
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
            } else if let Some(fiber) = state.pop_ready_for_worker(0) {
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
        run.awaiter_index
            .entry(left_id)
            .or_default()
            .push(parent_req);
        run.awaiter_index
            .entry(right_id)
            .or_default()
            .push(parent_req);
        run.awaits.insert(
            parent_req,
            AwaitKind::Both {
                left: left_id,
                right: right_id,
                left_value: None,
                right_value: None,
            },
        );
        parent_req
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_fiber_race(left: i64, right: i64) -> u64 {
    with_run(|run| {
        let parent_req = next_request_id();
        let left_id = run.spawn_child_on(current_worker(), next_fiber_id(), left);
        let right_id = run.spawn_child(right);
        run.awaiter_index
            .entry(left_id)
            .or_default()
            .push(parent_req);
        run.awaiter_index
            .entry(right_id)
            .or_default()
            .push(parent_req);
        run.awaits.insert(
            parent_req,
            AwaitKind::Race {
                children: vec![left_id, right_id],
                completed: Vec::new(),
            },
        );
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
        let mut child_ids = Vec::with_capacity(closures.len());
        for (idx, closure) in closures.iter().copied().enumerate() {
            let id = if idx == 0 {
                run.spawn_child_on(current_worker(), next_fiber_id(), closure)
            } else {
                run.spawn_child(closure)
            };
            run.awaiter_index.entry(id).or_default().push(parent_req);
            child_ids.push((id, idx));
        }
        run.awaits.insert(
            parent_req,
            AwaitKind::FirstOf {
                children: child_ids,
                completed: Vec::new(),
            },
        );
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
        run.awaiter_index
            .entry(child_id)
            .or_default()
            .push(parent_req);
        run.awaits.insert(
            parent_req,
            AwaitKind::Try {
                child: child_id,
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
        run.awaiter_index
            .entry(body_id)
            .or_default()
            .push(parent_req);
        run.awaits
            .insert(parent_req, AwaitKind::Timeout { body: body_id });
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
        DEFAULT_LOGICAL_WORKERS
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
