//! Fiber data structures for Phase 1b M:N cooperative scheduling.
//!
//! A **fiber** is a lightweight cooperative task: it has its own
//! [`EffectContext`] and a captured continuation, but shares an OS thread with
//! other fibers on the same worker. Fibers suspend at `perform Suspend` points
//! (I/O awaits, `yield_now`, `sleep`) and resume when a backend completion is
//! delivered.
//!
//! The no-fiber-migration invariant (Eio's model) holds: a fiber always resumes
//! on the same worker it started on (`home_worker`). Cross-worker parallelism
//! comes from the HTTP listener distributing connections across workers, not
//! from fiber migration.
//!
//! ## Phase status
//!
//! This module defines the data types only. The fiber scheduler
//! (`src/runtime/async/scheduler.rs`) that creates, suspends, and resumes
//! fibers is populated in Slice 1b-iv.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::runtime::r#async::context::{CancelScope, EffectContext, WorkerId};
use crate::runtime::continuation::Continuation;
use crate::runtime::value::Value;

// ── FiberId ───────────────────────────────────────────────────────────────

/// Monotonically-increasing fiber identifier. Unique within a scheduler
/// instance; used to route backend completions back to the correct fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FiberId(pub u64);

static NEXT_FIBER_ID: AtomicU64 = AtomicU64::new(1);

impl FiberId {
    /// Allocate a fresh fiber ID. Guaranteed unique across all threads within
    /// a process lifetime.
    pub fn fresh() -> Self {
        FiberId(NEXT_FIBER_ID.fetch_add(1, Ordering::Relaxed))
    }
}

// ── FiberState ────────────────────────────────────────────────────────────

/// Lifecycle state of a fiber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiberState {
    /// The fiber is queued in a worker's ready queue and may be scheduled.
    Ready,
    /// The fiber is waiting for a backend completion keyed by `request_id`.
    Suspended { request_id: u64 },
    /// The fiber's body returned successfully.
    Done,
    /// The fiber's enclosing scope was cancelled.
    Cancelled,
}

// ── Fiber ─────────────────────────────────────────────────────────────────

/// A schedulable unit of concurrent work.
///
/// Fibers are owned by the [`FiberQueue`] of their home worker. When a fiber
/// suspends, it is moved to a `HashMap<u64, Fiber>` keyed by `request_id`;
/// when the completion arrives it is pushed back onto the ready queue.
pub struct Fiber {
    /// Unique identifier — used to key the suspension registry.
    pub id: FiberId,
    /// Current lifecycle state.
    pub state: FiberState,
    /// Per-fiber effect/continuation context. Shared with the scheduler for
    /// cancellation propagation.
    pub context: EffectContext,
    /// Worker this fiber must run on (no-migration invariant).
    pub home_worker: WorkerId,
    /// The continuation closure to invoke when this fiber is resumed.
    /// `None` for newly-created fibers (their first "resume" is the body call).
    ///
    /// Phase 1b-v populates this with a boxed closure that captures the
    /// Flux continuation value; for now it is typed as `Box<dyn FnOnce() + Send>`
    /// to unblock the data structure definition.
    pub resume: Option<Box<dyn FnOnce() + Send>>,

    /// Body closure to invoke on first dispatch (proposal 0174 Phase 1b-vi-b₂).
    /// Set when a fresh fiber is spawned via `FiberFork`/`FiberBoth`/`FiberRace`;
    /// the dispatch loop takes it on first run, leaving `None` thereafter.
    pub body: Option<Value>,

    /// Captured delimited continuation for a parked fiber (Phase 1b-vi-b₂).
    /// Populated on `FiberSuspend`/`FiberSleep` park; consumed when the
    /// dispatch loop resumes the fiber after its completion arrives.
    pub parked: Option<Rc<RefCell<Continuation>>>,

    /// Request id that just woke this fiber (proposal 0174 Phase 1b-vi-b₂.2).
    /// Set by `FiberScheduler::complete` at the suspended → ready transition;
    /// the dispatch loop reads it to look up the synthetic resume value
    /// (e.g. the tuple built for `FiberBoth`) before invoking
    /// `resume_from_dispatch`. Cleared by the dispatch loop after read.
    pub last_completion_req: Option<u64>,
}

impl std::fmt::Debug for Fiber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fiber")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("home_worker", &self.home_worker)
            .field("resume", &self.resume.is_some())
            .finish()
    }
}

impl Fiber {
    /// Create a new fiber in the `Ready` state with a fresh ID.
    pub fn new(home_worker: WorkerId) -> Self {
        Fiber {
            id: FiberId::fresh(),
            state: FiberState::Ready,
            context: EffectContext::new(),
            home_worker,
            resume: None,
            body: None,
            parked: None,
            last_completion_req: None,
        }
    }

    /// True if the fiber's cancel scope has been marked.
    pub fn is_cancelled(&self) -> bool {
        self.context.is_cancelled()
    }

    /// Mark the fiber's cancel scope as cancelled.
    pub fn cancel(&mut self) {
        self.context.cancel_scope = CancelScope::Cancelled;
    }
}

// ── FiberQueue ────────────────────────────────────────────────────────────

/// Per-worker FIFO queue of ready fibers.
///
/// The scheduler pops from the front and pushes completions to the back,
/// giving simple FIFO scheduling within a worker. Phase 1b does not implement
/// work-stealing; fibers are pinned to their home worker.
#[derive(Debug, Default)]
pub struct FiberQueue {
    inner: VecDeque<Fiber>,
}

impl FiberQueue {
    pub fn new() -> Self {
        FiberQueue {
            inner: VecDeque::new(),
        }
    }

    /// Push a ready fiber onto the back of the queue.
    pub fn push(&mut self, fiber: Fiber) {
        self.inner.push_back(fiber);
    }

    /// Pop the next ready fiber from the front of the queue.
    pub fn pop(&mut self) -> Option<Fiber> {
        self.inner.pop_front()
    }

    /// True when no fibers are ready.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Number of fibers currently ready.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn contains(&self, id: FiberId) -> bool {
        self.inner.iter().any(|fiber| fiber.id == id)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiber_ids_are_unique() {
        let a = FiberId::fresh();
        let b = FiberId::fresh();
        let c = FiberId::fresh();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn new_fiber_is_ready_and_not_cancelled() {
        let f = Fiber::new(WorkerId(0));
        assert_eq!(f.state, FiberState::Ready);
        assert!(!f.is_cancelled());
        assert!(f.resume.is_none());
    }

    #[test]
    fn cancel_makes_fiber_report_cancelled() {
        let mut f = Fiber::new(WorkerId(0));
        assert!(!f.is_cancelled());
        f.cancel();
        assert!(f.is_cancelled());
        // Idempotent.
        f.cancel();
        assert!(f.is_cancelled());
    }

    #[test]
    fn fiber_queue_is_fifo() {
        let mut q = FiberQueue::new();
        assert!(q.is_empty());

        let f1 = Fiber::new(WorkerId(0));
        let f2 = Fiber::new(WorkerId(0));
        let id1 = f1.id;
        let id2 = f2.id;

        q.push(f1);
        q.push(f2);
        assert_eq!(q.len(), 2);
        assert!(!q.is_empty());

        let popped1 = q.pop().unwrap();
        let popped2 = q.pop().unwrap();
        assert_eq!(popped1.id, id1, "FIFO: first in, first out");
        assert_eq!(popped2.id, id2);
        assert!(q.pop().is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn home_worker_is_preserved() {
        let f = Fiber::new(WorkerId(7));
        assert_eq!(f.home_worker, WorkerId(7));
    }
}
