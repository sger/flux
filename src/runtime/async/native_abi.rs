//! Native/LLVM C ABI shims for proposal 0174 Phase 1b-vi-d.
//!
//! These symbols are linked into LLVM-generated native binaries and provide
//! the narrow entry surface from the C runtime into the Rust async backend.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::backend::{AsyncBackend, CompletionPayload, RequestId};
use super::backends::mio::MioBackend;

static BACKEND: OnceLock<MioBackend> = OnceLock::new();
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_FIBER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SCOPE_ID: AtomicU64 = AtomicU64::new(1);
static READY_TIMERS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();

const FLUX_NONE: i64 = 0;

thread_local! {
    static NATIVE_RUN: RefCell<Option<NativeRun>> = const { RefCell::new(None) };
    static CURRENT_FIBER: Cell<u64> = const { Cell::new(0) };
}

unsafe extern "C" {
    fn flux_async_call0(closure: i64) -> i64;
    fn flux_async_resume1(continuation: i64, value: i64) -> i64;
    fn flux_async_retain(value: i64);
    fn flux_async_release(value: i64);
    fn flux_async_make_tuple2(left: i64, right: i64) -> i64;
    fn flux_async_wrap_some(value: i64) -> i64;

    fn flux_async_suspend(request_id: i64, resume_value: i64) -> i64;
    fn flux_async_is_suspended() -> i32;
    fn flux_async_current_request() -> i64;
    fn flux_async_clear_suspend();
    fn flux_compose_conts() -> i64;
}

#[derive(Clone, Copy)]
enum Work {
    Closure { closure: i64, owned: bool },
    Resume { continuation: i64, value: i64 },
}

#[derive(Clone, Copy)]
struct Fiber {
    id: u64,
    work: Work,
}

enum AwaitKind {
    Both {
        left: u64,
        right: u64,
        left_value: Option<i64>,
        right_value: Option<i64>,
    },
    Race {
        children: Vec<u64>,
    },
    Timeout {
        body: u64,
    },
}

struct NativeRun {
    root: u64,
    ready: VecDeque<Fiber>,
    suspended: HashMap<u64, Fiber>,
    fiber_request: HashMap<u64, u64>,
    awaits: HashMap<u64, AwaitKind>,
    awaiter_index: HashMap<u64, Vec<u64>>,
    scopes: HashMap<u64, HashSet<u64>>,
    fiber_scope: HashMap<u64, u64>,
    root_result: Option<i64>,
}

impl NativeRun {
    fn new(root_closure: i64) -> Self {
        let root = next_fiber_id();
        let mut ready = VecDeque::new();
        ready.push_back(Fiber {
            id: root,
            work: Work::Closure {
                closure: root_closure,
                owned: false,
            },
        });
        Self {
            root,
            ready,
            suspended: HashMap::new(),
            fiber_request: HashMap::new(),
            awaits: HashMap::new(),
            awaiter_index: HashMap::new(),
            scopes: HashMap::new(),
            fiber_scope: HashMap::new(),
            root_result: None,
        }
    }

    fn spawn_child(&mut self, closure: i64) -> u64 {
        let id = next_fiber_id();
        unsafe {
            flux_async_retain(closure);
        }
        self.ready.push_back(Fiber {
            id,
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
        fiber.work = Work::Resume {
            continuation,
            value: FLUX_NONE,
        };
        self.fiber_request.insert(fiber.id, req);
        self.suspended.insert(req, fiber);
    }

    fn wake(&mut self, req: u64, value: i64) {
        if let Some(mut fiber) = self.suspended.remove(&req) {
            self.fiber_request.remove(&fiber.id);
            if let Work::Resume { continuation, .. } = fiber.work {
                fiber.work = Work::Resume {
                    continuation,
                    value,
                };
            }
            self.ready.push_back(fiber);
        }
    }

    fn complete_fiber(&mut self, id: u64, value: i64) {
        if id == self.root {
            self.root_result = Some(value);
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
                        left_value = Some(value);
                    } else if id == right {
                        right_value = Some(value);
                    }

                    match (left_value, right_value) {
                        (Some(l), Some(r)) => {
                            let tuple = unsafe { flux_async_make_tuple2(l, r) };
                            self.wake(parent_req, tuple);
                        }
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
                AwaitKind::Race { children } => {
                    self.wake(parent_req, value);
                    let losers: Vec<u64> =
                        children.into_iter().filter(|child| *child != id).collect();
                    self.cancel_fibers(&losers);
                }
                AwaitKind::Timeout { body } => {
                    if id == body {
                        backend().cancel(RequestId(parent_req));
                        let some = unsafe { flux_async_wrap_some(value) };
                        self.wake(parent_req, some);
                    }
                }
            }
        }
    }

    fn cancel_fibers(&mut self, fiber_ids: &[u64]) {
        if fiber_ids.is_empty() {
            return;
        }

        let losers: HashSet<u64> = fiber_ids.iter().copied().collect();
        let mut kept = VecDeque::new();
        while let Some(fiber) = self.ready.pop_front() {
            if losers.contains(&fiber.id) {
                release_cancelled_work(fiber.work);
            } else {
                kept.push_back(fiber);
            }
        }
        self.ready = kept;

        for id in fiber_ids {
            if let Some(scope) = self.fiber_scope.remove(id)
                && let Some(children) = self.scopes.get_mut(&scope)
            {
                children.remove(id);
            }
            if let Some(req) = self.fiber_request.remove(id) {
                backend().cancel(RequestId(req));
                if let Some(fiber) = self.suspended.remove(&req) {
                    release_cancelled_work(fiber.work);
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
        if let Some(AwaitKind::Timeout { body }) = self.awaits.remove(&req) {
            self.wake(req, FLUX_NONE);
            self.cancel_fibers(&[body]);
            return;
        }

        match payload {
            CompletionPayload::Unit => self.wake(req, FLUX_NONE),
            CompletionPayload::Error(_) => self.wake(req, FLUX_NONE),
            CompletionPayload::TcpHandle(_) | CompletionPayload::Bytes(_) => {
                self.wake(req, FLUX_NONE)
            }
        }
    }
}

fn release_executed_work(work: Work) {
    unsafe {
        match work {
            Work::Closure {
                closure,
                owned: true,
            } => flux_async_release(closure),
            Work::Closure { owned: false, .. } => {}
            Work::Resume { continuation, .. } => flux_async_release(continuation),
        }
    }
}

fn release_cancelled_work(work: Work) {
    unsafe {
        match work {
            Work::Closure {
                closure,
                owned: true,
            } => flux_async_release(closure),
            Work::Closure { owned: false, .. } => {}
            Work::Resume {
                continuation,
                value,
            } => {
                flux_async_release(continuation);
                flux_async_release(value);
            }
        }
    }
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

fn with_run<R>(f: impl FnOnce(&mut NativeRun) -> R) -> Option<R> {
    NATIVE_RUN.with(|run| run.borrow_mut().as_mut().map(f))
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

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_run_root(root_closure: i64) -> i64 {
    if flux_async_runtime_init() != 0 {
        return FLUX_NONE;
    }

    NATIVE_RUN.with(|run| {
        *run.borrow_mut() = Some(NativeRun::new(root_closure));
    });

    loop {
        let next = with_run(|run| run.ready.pop_front()).flatten();
        if let Some(fiber) = next {
            CURRENT_FIBER.with(|current| current.set(fiber.id));
            let result = unsafe {
                match fiber.work {
                    Work::Closure { closure, .. } => flux_async_call0(closure),
                    Work::Resume {
                        continuation,
                        value,
                    } => flux_async_resume1(continuation, value),
                }
            };
            CURRENT_FIBER.with(|current| current.set(0));

            if unsafe { flux_async_is_suspended() } != 0 {
                let request = untag_int(unsafe { flux_async_current_request() });
                let continuation = unsafe { flux_compose_conts() };
                unsafe {
                    flux_async_clear_suspend();
                }
                release_executed_work(fiber.work);
                let _ = with_run(|run| run.park(request, fiber, continuation));
                continue;
            }

            release_executed_work(fiber.work);
            let _ = with_run(|run| run.complete_fiber(fiber.id, result));
            if let Some(done) = with_run(|run| run.root_result).flatten() {
                NATIVE_RUN.with(|run| {
                    *run.borrow_mut() = None;
                });
                return done;
            }
            continue;
        }

        if let Some(done) = with_run(|run| run.root_result).flatten() {
            NATIVE_RUN.with(|run| {
                *run.borrow_mut() = None;
            });
            return done;
        }

        let suspended = with_run(|run| run.suspended.is_empty()).unwrap_or(true);
        if suspended {
            NATIVE_RUN.with(|run| {
                *run.borrow_mut() = None;
            });
            return FLUX_NONE;
        }

        if let Some(completion) = backend().next_completion() {
            let req = completion.request_id.0;
            let payload = completion.payload;
            let _ = with_run(|run| run.route_backend_completion(req, payload));
        } else {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
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
            AwaitKind::Race {
                children: vec![left_id, right_id],
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

#[unsafe(no_mangle)]
pub extern "C" fn flux_async_suspend_request(request_id: u64) -> i64 {
    if request_id == 0 {
        return FLUX_NONE;
    }
    unsafe { flux_async_suspend(tag_int(request_id), FLUX_NONE) }
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
                CompletionPayload::Unit => {
                    if let Ok(mut ready) = ready_timers().lock() {
                        ready.insert(completion.request_id.0);
                    }
                }
                CompletionPayload::Bytes(_)
                | CompletionPayload::TcpHandle(_)
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
