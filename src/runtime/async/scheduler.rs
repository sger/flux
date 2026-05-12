//! Fiber scheduler for Phase 1b cooperative concurrency (proposal 0174).
//!
//! The [`FiberScheduler`] owns per-worker ready queues and the suspension
//! registry. It drives the fiber lifecycle:
//!
//! 1. **Spawn** — create a `Fiber`, push onto the home worker's ready queue.
//! 2. **Dispatch** — pop the next ready fiber, invoke its resume closure.
//! 3. **Suspend** — fiber calls `suspend(request_id)`; scheduler moves it from
//!    the ready queue to the suspension map and yields the OS thread to the
//!    next ready fiber.
//! 4. **Complete** — backend delivers a completion; scheduler looks up the
//!    suspended fiber by `request_id`, pushes it back onto the home worker's
//!    ready queue.
//! 5. **Cancel** — mark a fiber's cancel scope; at the next yield point the
//!    fiber observes `is_cancelled()` and unwinds.
//!
//! ## Backend shape
//!
//! The scheduler owns logical worker queues on both backends. Native `run_async`
//! runs those queues on OS worker threads and may allow cross-worker stealing.
//! The VM uses the same logical queue model — including load-aware spawn
//! placement (see [`FiberScheduler::spawn_child`]) — but drains each worker's
//! queue without stealing, since VM `Value`/continuations are not yet safe to
//! migrate across worker execution contexts. Both backends honour
//! `FLUX_WORK_STEALING=0` to fall back to plain round-robin spawn placement
//! (and, on native, owner-only dispatch).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::backend::RequestId;
use super::context::WorkerId;
use super::fiber::{Fiber, FiberId, FiberQueue, FiberState};
use super::fiber_trace::{self, FiberEvent};

/// Load-aware spawn placement (and, on native, cross-worker work stealing) is
/// on by default. `FLUX_WORK_STEALING=0` (or `false`/`off`) restores the
/// original round-robin spawn placement — a diagnostic escape hatch.
///
/// Mirrors `work_stealing_enabled` in `runtime/async/native_abi.rs`; kept in
/// sync deliberately (the native copy lives behind `#[cfg(feature = "llvm")]`).
/// The value is read from the environment once and cached for the process.
pub fn work_stealing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("FLUX_WORK_STEALING").ok().as_deref(),
            Some("0") | Some("false") | Some("FALSE") | Some("off") | Some("OFF")
        )
    })
}

// ── WorkerState ───────────────────────────────────────────────────────────────

/// Per-worker state owned by the scheduler.
struct WorkerState {
    /// FIFO queue of fibers ready to run on this worker.
    ready: FiberQueue,
    /// Fibers suspended waiting for a backend completion, keyed by request id.
    suspended: HashMap<u64, Fiber>,
}

impl WorkerState {
    fn new() -> Self {
        WorkerState {
            ready: FiberQueue::new(),
            suspended: HashMap::new(),
        }
    }
}

// ── FiberScheduler ────────────────────────────────────────────────────────────

/// Cooperative fiber scheduler.
///
/// One instance is created per `Async.run_async` boundary (Phase 1b-vi).
/// Phase 1b-iv provides the data-structure skeleton; the actual fiber
/// switch is wired in Phase 1b-v.
pub struct FiberScheduler {
    workers: Vec<WorkerState>,
    next_child_worker: usize,
    next_ready_worker: usize,
}

impl FiberScheduler {
    /// Create a scheduler with `num_workers` logical worker slots.
    ///
    /// This constructor only creates logical queues. Native runtime code binds
    /// those queues to OS workers; VM dispatch drains them on the caller OS
    /// thread.
    pub fn new(num_workers: usize) -> Self {
        assert!(num_workers >= 1, "need at least one worker");
        let workers = (0..num_workers).map(|_| WorkerState::new()).collect();
        FiberScheduler {
            workers,
            // Root fibers stay on worker 0. Child round-robin starts at
            // worker 1 when available so two-worker runs exercise both queues
            // immediately while preserving the no-migration invariant.
            next_child_worker: if num_workers > 1 { 1 } else { 0 },
            next_ready_worker: 0,
        }
    }

    /// Spawn a new fiber on the given worker and push it onto the ready queue.
    ///
    /// Returns the new fiber's id, which callers can use to cancel it.
    pub fn spawn(&mut self, home_worker: WorkerId) -> FiberId {
        let worker_idx = home_worker.0 as usize;
        assert!(worker_idx < self.workers.len(), "invalid worker id");
        let fiber = Fiber::new(home_worker);
        let id = fiber.id;
        if self.total_ready_count() == 0 {
            self.next_ready_worker = worker_idx;
        }
        self.workers[worker_idx].ready.push(fiber);
        fiber_trace::emit(FiberEvent::Spawn { fid: id.0, worker: home_worker.0 });
        id
    }

    /// Return the logical worker the next child fiber should be pinned to.
    ///
    /// This does not create OS threads; it only assigns stable home-worker
    /// affinity for the current single-threaded dispatch loop and future
    /// multi-worker execution.
    pub fn next_child_worker(&mut self) -> WorkerId {
        let worker = WorkerId(self.next_child_worker as u32);
        self.next_child_worker = (self.next_child_worker + 1) % self.workers.len();
        worker
    }

    /// Spawn a child fiber, choosing its home worker by the active placement
    /// policy: least-loaded queue when `FLUX_WORK_STEALING` is enabled (the
    /// default), plain round-robin otherwise. This is the entry point VM/native
    /// child-spawn sites should call; the two `spawn_child_*` variants below are
    /// the explicit primitives (used directly by tests).
    pub fn spawn_child(&mut self) -> FiberId {
        if work_stealing_enabled() {
            self.spawn_child_least_loaded()
        } else {
            self.spawn_child_round_robin()
        }
    }

    /// Spawn a child fiber on the next logical worker.
    pub fn spawn_child_round_robin(&mut self) -> FiberId {
        let worker = self.next_child_worker();
        self.spawn(worker)
    }

    /// Spawn a child fiber on the least-loaded logical worker.
    ///
    /// Picks the worker with the shortest ready queue; breaks ties by lowest
    /// worker ID for determinism. Mirrors `pick_next_worker` in native_abi.rs.
    pub fn spawn_child_least_loaded(&mut self) -> FiberId {
        let worker = self
            .workers
            .iter()
            .enumerate()
            .min_by_key(|(idx, w)| (w.ready.len(), *idx))
            .map(|(idx, _)| WorkerId(idx as u32))
            .unwrap_or(WorkerId(0));
        self.spawn(worker)
    }

    /// Push a pre-existing fiber onto its home worker's ready queue (proposal
    /// 0174 Phase 1b-vi-b₂.1). Used by the dispatch loop when re-queuing a
    /// fiber whose body or parked-state has been updated externally.
    pub fn spawn_existing(&mut self, fiber: Fiber) {
        let worker_idx = fiber.home_worker.0 as usize;
        assert!(worker_idx < self.workers.len(), "invalid worker id");
        if self.total_ready_count() == 0 {
            self.next_ready_worker = worker_idx;
        }
        self.workers[worker_idx].ready.push(fiber);
    }

    /// Insert an externally-prepared parked fiber into the suspended map
    /// (proposal 0174 Phase 1b-vi-b₂.1). Companion to `suspend(fiber, req)`
    /// for the case where the fiber's `parked` continuation was captured
    /// outside the scheduler.
    pub fn insert_suspended(&mut self, worker: WorkerId, request_id: u64, fiber: Fiber) {
        let idx = worker.0 as usize;
        self.workers[idx].suspended.insert(request_id, fiber);
    }

    /// Suspend a fiber that was just dequeued from the ready queue.
    ///
    /// Moves the fiber into the suspended map keyed by `request_id` so that
    /// when the backend delivers its completion we can route the wakeup.
    pub fn suspend(&mut self, mut fiber: Fiber, request_id: RequestId) {
        let worker = fiber.home_worker.0;
        let fid = fiber.id.0;
        fiber.state = FiberState::Suspended {
            request_id: request_id.0,
        };
        let worker_idx = fiber.home_worker.0 as usize;
        self.workers[worker_idx]
            .suspended
            .insert(request_id.0, fiber);
        fiber_trace::emit(FiberEvent::Suspend { fid, worker, request_id: request_id.0 });
    }

    /// Deliver a backend completion: move the matching fiber back to ready.
    ///
    /// Returns `true` if the fiber was found and re-queued, `false` if the
    /// request was abandoned (no fiber was waiting for it).
    pub fn complete(&mut self, worker: WorkerId, request_id: RequestId) -> bool {
        let idx = worker.0 as usize;
        if let Some(mut fiber) = self.workers[idx].suspended.remove(&request_id.0) {
            let fid = fiber.id.0;
            fiber.state = FiberState::Ready;
            // Record which request woke us so the dispatch loop can look
            // up the synthetic resume value (proposal 0174 Phase 1b-vi-b₂.2).
            fiber.last_completion_req = Some(request_id.0);
            if self.total_ready_count() == 0 {
                self.next_ready_worker = idx;
            }
            self.workers[idx].ready.push(fiber);
            fiber_trace::emit(FiberEvent::Resume { fid, worker: idx as u32, request_id: request_id.0 });
            true
        } else {
            false
        }
    }

    /// Deliver a backend completion without requiring the caller to know the
    /// fiber's home worker.
    ///
    /// This is the scheduler-side half of proposal 0174 1b-vi-c's
    /// home-worker completion routing. The reactor/dispatch owner only knows
    /// the request id; the scheduler owns the request → parked-fiber map and
    /// re-enqueues the fiber onto the worker it was originally pinned to.
    pub fn complete_request(&mut self, request_id: RequestId) -> Option<WorkerId> {
        for worker_idx in 0..self.workers.len() {
            let worker = WorkerId(worker_idx as u32);
            if self.complete(worker, request_id) {
                return Some(worker);
            }
        }
        None
    }

    /// Cancel all suspended fibers whose `FiberId` matches the given set.
    ///
    /// Cancelled fibers are moved back onto the ready queue in `Cancelled`
    /// state; when the dispatch loop next dequeues them it delivers the
    /// `AsyncError.Canceled` error (Phase 1b-vi).
    pub fn cancel_fibers(&mut self, ids: &[FiberId]) {
        let id_set: std::collections::HashSet<u64> = ids.iter().map(|f| f.0).collect();
        let mut ready_was_empty = self.total_ready_count() == 0;
        for (worker_idx, worker) in self.workers.iter_mut().enumerate() {
            let to_cancel: Vec<u64> = worker
                .suspended
                .keys()
                .filter(|&&req| {
                    worker
                        .suspended
                        .get(&req)
                        .map(|f| id_set.contains(&f.id.0))
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            for req in to_cancel {
                if let Some(mut fiber) = worker.suspended.remove(&req) {
                    let fid = fiber.id.0;
                    fiber.state = FiberState::Cancelled;
                    if ready_was_empty {
                        self.next_ready_worker = worker_idx;
                        ready_was_empty = false;
                    }
                    worker.ready.push(fiber);
                    fiber_trace::emit(FiberEvent::Cancel { fid, worker: worker_idx as u32 });
                }
            }
        }
    }

    /// Pop the next ready fiber from a worker's queue.
    pub fn next_ready(&mut self, worker: WorkerId) -> Option<Fiber> {
        let idx = worker.0 as usize;
        self.workers[idx].ready.pop()
    }

    /// Pop the next ready fiber from the first non-empty logical worker.
    ///
    /// This keeps the current single-OS-thread VM/native loops simple while
    /// letting tests and future dispatch loops exercise multiple logical
    /// worker queues.
    pub fn next_ready_any(&mut self) -> Option<(WorkerId, Fiber)> {
        for offset in 0..self.workers.len() {
            let worker_idx = (self.next_ready_worker + offset) % self.workers.len();
            let worker = WorkerId(worker_idx as u32);
            if let Some(fiber) = self.next_ready(worker) {
                self.next_ready_worker = (worker_idx + 1) % self.workers.len();
                return Some((worker, fiber));
            }
        }
        None
    }

    /// Number of fibers ready to run on a given worker.
    pub fn ready_count(&self, worker: WorkerId) -> usize {
        self.workers[worker.0 as usize].ready.len()
    }

    /// True when `id` is still queued for execution on any logical worker.
    pub fn is_ready(&self, id: FiberId) -> bool {
        self.workers.iter().any(|worker| worker.ready.contains(id))
    }

    /// Total ready fibers across all logical workers.
    pub fn total_ready_count(&self) -> usize {
        self.workers.iter().map(|w| w.ready.len()).sum()
    }

    /// Number of fibers suspended on a given worker.
    pub fn suspended_count(&self, worker: WorkerId) -> usize {
        self.workers[worker.0 as usize].suspended.len()
    }

    /// Total suspended fibers across all logical workers.
    pub fn total_suspended_count(&self) -> usize {
        self.workers.iter().map(|w| w.suspended.len()).sum()
    }

    /// True when a worker has no ready or suspended fibers.
    pub fn is_idle(&self, worker: WorkerId) -> bool {
        let w = &self.workers[worker.0 as usize];
        w.ready.is_empty() && w.suspended.is_empty()
    }

    /// Number of workers in this scheduler.
    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    /// Return the `request_id` that a fiber is currently suspended on, if any.
    pub fn find_request_for_fiber(&self, id: FiberId) -> Option<u64> {
        for worker in &self.workers {
            for (req, fiber) in &worker.suspended {
                if fiber.id == id {
                    return Some(*req);
                }
            }
        }
        None
    }

    /// Return all request IDs currently in the suspended map for a worker.
    /// Used by `exit_run_async` to cancel outstanding backend requests on teardown.
    pub fn all_suspended_reqs(&self, worker: WorkerId) -> Vec<u64> {
        self.workers[worker.0 as usize]
            .suspended
            .keys()
            .copied()
            .collect()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn w0() -> WorkerId {
        WorkerId(0)
    }

    #[test]
    fn spawn_pushes_fiber_onto_ready_queue() {
        let mut sched = FiberScheduler::new(1);
        assert_eq!(sched.ready_count(w0()), 0);

        let id = sched.spawn(w0());
        assert_eq!(sched.ready_count(w0()), 1);

        let fiber = sched.next_ready(w0()).unwrap();
        assert_eq!(fiber.id, id);
        assert_eq!(fiber.state, FiberState::Ready);
        assert_eq!(sched.ready_count(w0()), 0);
    }

    #[test]
    fn suspend_moves_fiber_to_suspended_map() {
        let mut sched = FiberScheduler::new(1);
        sched.spawn(w0());
        let fiber = sched.next_ready(w0()).unwrap();
        let req = RequestId(42);

        sched.suspend(fiber, req);
        assert_eq!(sched.ready_count(w0()), 0);
        assert_eq!(sched.suspended_count(w0()), 1);
    }

    #[test]
    fn complete_moves_fiber_back_to_ready() {
        let mut sched = FiberScheduler::new(1);
        sched.spawn(w0());
        let fiber = sched.next_ready(w0()).unwrap();
        let req = RequestId(7);
        sched.suspend(fiber, req);

        let found = sched.complete(w0(), req);
        assert!(found);
        assert_eq!(sched.suspended_count(w0()), 0);
        assert_eq!(sched.ready_count(w0()), 1);

        let resumed = sched.next_ready(w0()).unwrap();
        assert_eq!(resumed.state, FiberState::Ready);
    }

    #[test]
    fn complete_returns_false_for_unknown_request() {
        let mut sched = FiberScheduler::new(1);
        let found = sched.complete(w0(), RequestId(99));
        assert!(!found);
    }

    #[test]
    fn cancel_fibers_moves_suspended_fiber_to_ready_cancelled() {
        let mut sched = FiberScheduler::new(1);
        let id = sched.spawn(w0());
        let fiber = sched.next_ready(w0()).unwrap();
        sched.suspend(fiber, RequestId(1));

        sched.cancel_fibers(&[id]);
        assert_eq!(sched.suspended_count(w0()), 0);
        assert_eq!(sched.ready_count(w0()), 1);

        let f = sched.next_ready(w0()).unwrap();
        assert_eq!(f.state, FiberState::Cancelled);
    }

    #[test]
    fn cancel_fibers_ignores_unknown_fiber_id() {
        let mut sched = FiberScheduler::new(1);
        let bogus = FiberId(999_999);
        sched.cancel_fibers(&[bogus]);
        assert!(sched.is_idle(w0()));
    }

    #[test]
    fn multi_worker_fibers_are_isolated() {
        let mut sched = FiberScheduler::new(2);
        let w1 = WorkerId(1);

        let id0 = sched.spawn(w0());
        let id1 = sched.spawn(w1);

        assert_ne!(id0, id1);
        assert_eq!(sched.ready_count(w0()), 1);
        assert_eq!(sched.ready_count(w1), 1);

        // Completing on worker 0 does not affect worker 1.
        let f0 = sched.next_ready(w0()).unwrap();
        sched.suspend(f0, RequestId(10));
        sched.complete(w0(), RequestId(10));
        assert_eq!(sched.ready_count(w0()), 1);
        assert_eq!(sched.suspended_count(w1), 0);
        assert_eq!(sched.ready_count(w1), 1);
    }

    #[test]
    fn complete_request_routes_to_fiber_home_worker() {
        let mut sched = FiberScheduler::new(2);
        let w1 = WorkerId(1);
        let id = sched.spawn(w1);
        let fiber = sched.next_ready(w1).unwrap();
        sched.suspend(fiber, RequestId(77));

        let routed = sched.complete_request(RequestId(77));
        assert_eq!(routed, Some(w1));
        assert_eq!(sched.suspended_count(w1), 0);
        assert_eq!(sched.ready_count(w1), 1);

        let resumed = sched.next_ready(w1).unwrap();
        assert_eq!(resumed.id, id);
        assert_eq!(resumed.home_worker, w1);
        assert_eq!(resumed.last_completion_req, Some(77));
    }

    #[test]
    fn complete_request_returns_none_for_abandoned_request() {
        let mut sched = FiberScheduler::new(2);
        assert_eq!(sched.complete_request(RequestId(404)), None);
    }

    #[test]
    fn next_ready_any_starts_with_first_ready_worker() {
        let mut sched = FiberScheduler::new(2);
        let w1 = WorkerId(1);
        let id1 = sched.spawn(w1);
        let id0 = sched.spawn(w0());

        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, w1);
        assert_eq!(fiber.id, id1);

        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, w0());
        assert_eq!(fiber.id, id0);

        assert!(sched.next_ready_any().is_none());
    }

    #[test]
    fn child_round_robin_assigns_logical_workers() {
        let mut sched = FiberScheduler::new(2);

        let a = sched.spawn_child_round_robin();
        let b = sched.spawn_child_round_robin();
        let c = sched.spawn_child_round_robin();

        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, WorkerId(1));
        assert_eq!(fiber.id, a);
        assert_eq!(fiber.home_worker, WorkerId(1));

        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, w0());
        assert_eq!(fiber.id, b);
        assert_eq!(fiber.home_worker, w0());

        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, WorkerId(1));
        assert_eq!(fiber.id, c);
        assert_eq!(fiber.home_worker, WorkerId(1));
    }

    #[test]
    fn child_round_robin_single_worker_stays_on_worker_zero() {
        let mut sched = FiberScheduler::new(1);
        let id = sched.spawn_child_round_robin();
        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, w0());
        assert_eq!(fiber.id, id);
        assert_eq!(fiber.home_worker, w0());
    }

    #[test]
    fn is_idle_when_no_fibers() {
        let sched = FiberScheduler::new(1);
        assert!(sched.is_idle(w0()));
    }

    #[test]
    fn is_idle_false_when_ready_or_suspended() {
        let mut sched = FiberScheduler::new(1);
        sched.spawn(w0());
        assert!(!sched.is_idle(w0()));

        let fiber = sched.next_ready(w0()).unwrap();
        sched.suspend(fiber, RequestId(1));
        assert!(!sched.is_idle(w0()));

        sched.complete(w0(), RequestId(1));
        sched.next_ready(w0()); // drain
        assert!(sched.is_idle(w0()));
    }

    #[test]
    fn ready_queue_is_fifo() {
        let mut sched = FiberScheduler::new(1);
        let a = sched.spawn(w0());
        let b = sched.spawn(w0());
        let c = sched.spawn(w0());

        assert_eq!(sched.next_ready(w0()).unwrap().id, a);
        assert_eq!(sched.next_ready(w0()).unwrap().id, b);
        assert_eq!(sched.next_ready(w0()).unwrap().id, c);
        assert!(sched.next_ready(w0()).is_none());
    }

    #[test]
    fn spawn_child_least_loaded_picks_shortest_queue() {
        let mut sched = FiberScheduler::new(3);
        // Worker queues: w0 = 2, w1 = 1, w2 = 0 ready fibers.
        sched.spawn(WorkerId(0));
        sched.spawn(WorkerId(0));
        sched.spawn(WorkerId(1));

        let id = sched.spawn_child_least_loaded();
        assert_eq!(sched.ready_count(WorkerId(2)), 1);

        let fiber = sched.next_ready(WorkerId(2)).unwrap();
        assert_eq!(fiber.id, id);
        assert_eq!(fiber.home_worker, WorkerId(2));
    }

    #[test]
    fn spawn_child_least_loaded_breaks_ties_by_lowest_worker_id() {
        let mut sched = FiberScheduler::new(3);
        // All queues empty → tie → lowest worker id (0).
        let id = sched.spawn_child_least_loaded();
        let fiber = sched.next_ready(w0()).unwrap();
        assert_eq!(fiber.id, id);
        assert_eq!(fiber.home_worker, w0());
    }

    #[test]
    fn spawn_child_least_loaded_single_worker_uses_worker_zero() {
        let mut sched = FiberScheduler::new(1);
        let id = sched.spawn_child_least_loaded();
        let (worker, fiber) = sched.next_ready_any().unwrap();
        assert_eq!(worker, w0());
        assert_eq!(fiber.id, id);
        assert_eq!(fiber.home_worker, w0());
    }
}
