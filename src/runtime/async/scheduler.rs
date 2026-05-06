//! Fiber scheduler for Phase 1b M:N cooperative concurrency (proposal 0174).
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
//! ## Phase status
//!
//! Slice 1b-iv provides the data structure skeleton and the public API.  The
//! actual fiber-switching (stack-switching or green-thread trampoline) is
//! deferred to Slice 1b-v/vi when the `FiberSuspend` / `FiberFork` primops
//! are wired into the VM and C runtime.  Until then, `run_fiber_body` is a
//! thin stub that runs the fiber body synchronously (same semantics as
//! Phase 1a task spawn — sequentially equivalent, correct for non-concurrent
//! tests).
//!
//! ## No-fiber-migration invariant
//!
//! Following Eio's model, a fiber always resumes on the same worker it started
//! on (`home_worker`).  Cross-worker parallelism comes from the HTTP listener
//! distributing accepted connections across workers, not from fiber migration.

use std::collections::HashMap;

use super::backend::RequestId;
use super::context::WorkerId;
use super::fiber::{Fiber, FiberId, FiberQueue, FiberState};

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

/// M:N fiber scheduler.
///
/// One instance is created per `Async.run_async` boundary (Phase 1b-vi).
/// Phase 1b-iv provides the data-structure skeleton; the actual fiber
/// switch is wired in Phase 1b-v.
pub struct FiberScheduler {
    workers: Vec<WorkerState>,
}

impl FiberScheduler {
    /// Create a scheduler with `num_workers` logical worker slots.
    ///
    /// In Phase 1b the workers are virtual — no OS threads are created here.
    /// Phase 1b-vi may add real thread spawning; for now all dispatch runs
    /// on the calling OS thread.
    pub fn new(num_workers: usize) -> Self {
        assert!(num_workers >= 1, "need at least one worker");
        let workers = (0..num_workers).map(|_| WorkerState::new()).collect();
        FiberScheduler { workers }
    }

    /// Spawn a new fiber on the given worker and push it onto the ready queue.
    ///
    /// Returns the new fiber's id, which callers can use to cancel it.
    pub fn spawn(&mut self, home_worker: WorkerId) -> FiberId {
        let worker_idx = home_worker.0 as usize;
        assert!(worker_idx < self.workers.len(), "invalid worker id");
        let fiber = Fiber::new(home_worker);
        let id = fiber.id;
        self.workers[worker_idx].ready.push(fiber);
        id
    }

    /// Push a pre-existing fiber onto its home worker's ready queue (proposal
    /// 0174 Phase 1b-vi-b₂.1). Used by the dispatch loop when re-queuing a
    /// fiber whose body or parked-state has been updated externally.
    pub fn spawn_existing(&mut self, fiber: Fiber) {
        let worker_idx = fiber.home_worker.0 as usize;
        assert!(worker_idx < self.workers.len(), "invalid worker id");
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
        fiber.state = FiberState::Suspended {
            request_id: request_id.0,
        };
        let worker_idx = fiber.home_worker.0 as usize;
        self.workers[worker_idx]
            .suspended
            .insert(request_id.0, fiber);
    }

    /// Deliver a backend completion: move the matching fiber back to ready.
    ///
    /// Returns `true` if the fiber was found and re-queued, `false` if the
    /// request was abandoned (no fiber was waiting for it).
    pub fn complete(&mut self, worker: WorkerId, request_id: RequestId) -> bool {
        let idx = worker.0 as usize;
        if let Some(mut fiber) = self.workers[idx].suspended.remove(&request_id.0) {
            fiber.state = FiberState::Ready;
            // Record which request woke us so the dispatch loop can look
            // up the synthetic resume value (proposal 0174 Phase 1b-vi-b₂.2).
            fiber.last_completion_req = Some(request_id.0);
            self.workers[idx].ready.push(fiber);
            true
        } else {
            false
        }
    }

    /// Cancel all suspended fibers whose `FiberId` matches the given set.
    ///
    /// Cancelled fibers are moved back onto the ready queue in `Cancelled`
    /// state; when the dispatch loop next dequeues them it delivers the
    /// `AsyncError.Canceled` error (Phase 1b-vi).
    pub fn cancel_fibers(&mut self, ids: &[FiberId]) {
        let id_set: std::collections::HashSet<u64> = ids.iter().map(|f| f.0).collect();
        for worker in &mut self.workers {
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
                    fiber.state = FiberState::Cancelled;
                    worker.ready.push(fiber);
                }
            }
        }
    }

    /// Pop the next ready fiber from a worker's queue.
    pub fn next_ready(&mut self, worker: WorkerId) -> Option<Fiber> {
        let idx = worker.0 as usize;
        self.workers[idx].ready.pop()
    }

    /// Number of fibers ready to run on a given worker.
    pub fn ready_count(&self, worker: WorkerId) -> usize {
        self.workers[worker.0 as usize].ready.len()
    }

    /// Number of fibers suspended on a given worker.
    pub fn suspended_count(&self, worker: WorkerId) -> usize {
        self.workers[worker.0 as usize].suspended.len()
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
}
