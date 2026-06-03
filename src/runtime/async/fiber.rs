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
use crate::runtime::value::{
    ArcEffectContext, ArcValue, Value, demote_effect_context, demote_value, promote_effect_context,
    promote_value,
};

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

    /// Whether this fiber may be stolen by another OS worker.
    ///
    /// Fresh race/first_of candidates start non-stealable so immediate
    /// completions preserve source-order FIFO ties. Once a fiber parks or
    /// yields, it has crossed a scheduling boundary and can migrate safely via
    /// the Arc mirror path.
    pub stealable: bool,
}

// SAFETY: `Fiber` contains `Rc`/`RefCell` (in `parked`, `body`, and `context`),
// so it is not auto-`Send`. It is nonetheless only ever *moved* across an
// OS-thread boundary as a sequential hand-off between the scheduler (behind
// `Arc<Mutex<FiberScheduler>>`) and exactly one worker thread at a time — never
// shared, never accessed concurrently. While `FLUX_FIBER_MIGRATION` is off the
// receiving worker is always the fiber's `home_worker`, so the inner `Rc`
// graphs stay thread-local in practice; when migration is on, the cross-worker
// hand-off goes through [`Fiber::promote`] / [`ArcFiber::demote`] instead, so a
// `Fiber` value still never has its `Rc`s touched by two threads. (A future
// refactor may replace this blanket `unsafe impl` with a `FiberCarrier` enum —
// `Local(Fiber)` / `Migrated(ArcFiber)` — so only the honestly-`Send`
// `ArcFiber` ever crosses threads; see proposal 0174.)
unsafe impl Send for Fiber {}

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
            stealable: false,
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

    /// Mark that this fiber has crossed its first scheduling boundary and can
    /// now be considered for cross-worker stealing.
    pub fn mark_parked(&mut self) {
        self.stealable = true;
    }

    /// True if this fiber can be safely resumed on a *different* worker's VM.
    ///
    /// A parked fiber's continuation stores absolute frame/stack indices
    /// relative to the VM that captured it. Background worker VMs always capture
    /// at the baseline `(frame_index 0, sp 0)`, so their continuations splice
    /// cleanly onto any other background VM. Worker 0 reuses the caller's main
    /// VM, whose stack is several frames deep; a continuation captured there
    /// records a non-zero `entry_frame_index`/`entry_sp` and cannot be rebased
    /// onto a shallow background VM (doing so resumes into padded placeholder
    /// frames and fails with "resumed continuation exited without return").
    ///
    /// A not-yet-started fiber (body only, no parked continuation) is always
    /// migratable: it runs from scratch via `invoke_value` at the receiver's
    /// baseline. See proposal 0174 §"VM cross-worker fiber dispatch".
    pub fn is_migratable(&self) -> bool {
        match &self.parked {
            Some(cont) => {
                let cont = cont.borrow();
                cont.entry_frame_index == 0 && cont.entry_sp == 0
            }
            None => true,
        }
    }

    /// Promote this fiber into its `Send` [`ArcFiber`] mirror — used the instant
    /// a fiber's ownership crosses to another OS worker (cross-worker steal,
    /// gated by `FLUX_FIBER_MIGRATION`). Deep-copies the body, parked
    /// continuation, and effect context into `Arc`-backed structures, consuming
    /// the original. Errors only if a contained value fails to promote, or if
    /// the legacy (always-unset) `resume` closure is present.
    ///
    /// See proposal 0174 §"VM cross-worker fiber dispatch".
    pub fn promote(self) -> Result<ArcFiber, String> {
        self.promote_ref()
    }

    /// Promote this fiber into its `Send` [`ArcFiber`] mirror without consuming
    /// the original. Used by work stealing so the original `Fiber` can be moved
    /// to its home worker's pending-drop queue and have its `Rc`s dropped on
    /// the owning worker.
    pub fn promote_ref(&self) -> Result<ArcFiber, String> {
        if self.resume.is_some() {
            return Err("Fiber::promote: legacy `resume` closure is not migratable".to_string());
        }
        Ok(ArcFiber {
            id: self.id,
            state: self.state.clone(),
            home_worker: self.home_worker,
            last_completion_req: self.last_completion_req,
            stealable: self.stealable,
            body: match &self.body {
                Some(v) => Some(promote_value(v)?),
                None => None,
            },
            parked: match &self.parked {
                Some(rc) => Some(promote_value(&Value::Continuation(rc.clone()))?),
                None => None,
            },
            context: promote_effect_context(&self.context)?,
        })
    }
}

// ── ArcFiber ──────────────────────────────────────────────────────────────

/// `Send` mirror of [`Fiber`] for cross-OS-worker hand-off.
///
/// Every `Rc` inside a `Fiber` (the parked continuation, the body value, the
/// effect/evidence context) becomes an `Arc` here, so an `ArcFiber` can be
/// moved to another worker thread without the `unsafe impl Send` caveat that
/// `Fiber` carries. The receiving worker calls [`ArcFiber::demote`] to rebuild
/// a thread-local `Fiber` (fresh `Rc` wrappers) before running it. Built by
/// [`Fiber::promote`]. Constructed only when `FLUX_FIBER_MIGRATION` is on and a
/// fiber is actually stolen across workers — never on the single-worker hot
/// path.
pub struct ArcFiber {
    pub id: FiberId,
    pub state: FiberState,
    pub home_worker: WorkerId,
    pub last_completion_req: Option<u64>,
    pub stealable: bool,
    /// Body thunk for a not-yet-started fiber. `ArcValue::Closure` in practice.
    pub body: Option<ArcValue>,
    /// Parked continuation for a suspended/yielded fiber. Always
    /// `ArcValue::Continuation(_)` when `Some`.
    pub parked: Option<ArcValue>,
    pub context: ArcEffectContext,
}

// (Auto-`Send`: every field is `Send` — `Copy` scalars or `Arc`-backed via
// `ArcValue` / `ArcEffectContext`. The compile-time `arc_fiber_is_send` test
// below pins this.)

impl ArcFiber {
    /// Rebuild a thread-local [`Fiber`] from this `Send` mirror — the inverse
    /// of [`Fiber::promote`], called on the worker that will run the fiber.
    /// Allocates fresh `Rc` wrappers from the `Arc` data.
    pub fn demote(self) -> Fiber {
        Fiber {
            id: self.id,
            state: self.state,
            context: demote_effect_context(self.context),
            home_worker: self.home_worker,
            resume: None,
            body: self.body.map(demote_value),
            parked: match self.parked.map(demote_value) {
                Some(Value::Continuation(rc)) => Some(rc),
                None => None,
                Some(other) => unreachable!(
                    "ArcFiber::parked must be a continuation, got {}",
                    other.type_name()
                ),
            },
            last_completion_req: self.last_completion_req,
            stealable: self.stealable,
        }
    }
}

impl std::fmt::Debug for ArcFiber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcFiber")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("home_worker", &self.home_worker)
            .field("parked", &self.parked.is_some())
            .field("body", &self.body.is_some())
            .finish()
    }
}

// ── FiberExecState ────────────────────────────────────────────────────────
//
// Captures the per-fiber VM execution state at park time so that a worker VM
// on a (possibly different) OS thread can restore exactly the right context
// when resuming. On the VM backend the stack/frame state is encoded inside the
// `Continuation` value stored in `Fiber::parked`; `FiberExecState` carries the
// scheduler-level fields that are *not* part of the continuation (worker
// context, completion-request id).
//
// Phase 3 defines the type. Phase 4 uses it to hand fibers across OS worker
// threads together with a freshly-created `VM::new_for_worker` instance.

/// Scheduler-level execution context for a parked fiber.
///
/// Contains only the fields that the fiber scheduler needs to route the fiber
/// to its home worker on resume. The VM-level stack/frame state is captured in
/// `Fiber::parked` as a `Value::Continuation`.
#[derive(Debug)]
pub struct FiberExecState {
    /// Worker this fiber must resume on (no-migration invariant).
    pub home_worker: WorkerId,
    /// Request id the fiber is waiting on, if suspended.
    pub suspended_on: Option<u64>,
}

impl FiberExecState {
    pub fn new(home_worker: WorkerId) -> Self {
        FiberExecState {
            home_worker,
            suspended_on: None,
        }
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

    /// Pop the newest ready fiber from the back of the queue.
    pub fn pop_back(&mut self) -> Option<Fiber> {
        self.inner.pop_back()
    }

    /// Iterate over queued fibers from front to back.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Fiber> {
        self.inner.iter()
    }

    /// Iterate mutably over queued fibers from front to back.
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Fiber> {
        self.inner.iter_mut()
    }

    /// Remove a queued fiber by id, preserving relative order of the rest.
    pub fn remove(&mut self, id: FiberId) -> Option<Fiber> {
        let idx = self.inner.iter().position(|fiber| fiber.id == id)?;
        self.inner.remove(idx)
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
    fn fiber_queue_pop_back_iter_and_remove_work() {
        let mut q = FiberQueue::new();
        let f1 = Fiber::new(WorkerId(0));
        let f2 = Fiber::new(WorkerId(0));
        let f3 = Fiber::new(WorkerId(0));
        let id1 = f1.id;
        let id2 = f2.id;
        let id3 = f3.id;

        q.push(f1);
        q.push(f2);
        q.push(f3);

        let ids: Vec<_> = q.iter().map(|fiber| fiber.id).collect();
        assert_eq!(ids, vec![id1, id2, id3]);

        let removed = q.remove(id2).expect("remove middle fiber");
        assert_eq!(removed.id, id2);
        assert_eq!(q.len(), 2);

        let newest = q.pop_back().expect("pop newest");
        assert_eq!(newest.id, id3);
        let oldest = q.pop().expect("pop oldest");
        assert_eq!(oldest.id, id1);
        assert!(q.remove(id2).is_none());
    }

    #[test]
    fn home_worker_is_preserved() {
        let f = Fiber::new(WorkerId(7));
        assert_eq!(f.home_worker, WorkerId(7));
    }

    #[test]
    fn arc_fiber_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ArcFiber>();
    }

    #[test]
    fn promote_fails_with_legacy_resume_closure() {
        let mut f = Fiber::new(WorkerId(0));
        f.resume = Some(Box::new(|| {}));
        assert!(f.promote().is_err());
    }

    #[test]
    fn promote_demote_round_trips_a_parked_fiber() {
        use crate::runtime::closure::Closure;
        use crate::runtime::compiled_function::CompiledFunction;
        use crate::runtime::frame::Frame;
        use std::cell::RefCell;
        use std::sync::Arc;

        let func = Arc::new(CompiledFunction::new(vec![], 0, 0, None));
        let closure = Rc::new(Closure::new(func, vec![Value::Integer(99)]));
        let frame = Frame::new_with_return_slot(closure, 3, 2);
        let cont = Continuation {
            frames: vec![frame],
            stack: vec![Value::Integer(1), Value::Boolean(true)],
            sp: 5,
            entry_sp: 3,
            entry_frame_index: 0,
            inner_handlers: vec![],
            state_marker: Some(7),
        };
        let mut f = Fiber::new(WorkerId(2));
        f.state = FiberState::Suspended { request_id: 42 };
        f.parked = Some(Rc::new(RefCell::new(cont)));
        f.last_completion_req = Some(42);
        f.mark_parked();
        let id = f.id;

        let arc: ArcFiber = f.promote_ref().expect("promote_ref");
        assert!(f.parked.is_some(), "promote_ref must not consume the fiber");
        let back = arc.demote();

        assert_eq!(back.id, id);
        assert_eq!(back.home_worker, WorkerId(2));
        assert_eq!(back.state, FiberState::Suspended { request_id: 42 });
        assert_eq!(back.last_completion_req, Some(42));
        assert!(back.stealable);
        let cont = back.parked.expect("parked round-trips").borrow().clone();
        assert_eq!(cont.sp, 5);
        assert_eq!(cont.entry_sp, 3);
        assert_eq!(cont.entry_frame_index, 0);
        assert_eq!(cont.state_marker, Some(7));
        assert_eq!(cont.frames.len(), 1);
        assert_eq!(cont.frames[0].base_pointer, 3);
        assert_eq!(cont.frames[0].return_slot, 2);
        assert_eq!(cont.frames[0].closure.free, vec![Value::Integer(99)]);
        assert_eq!(cont.stack, vec![Value::Integer(1), Value::Boolean(true)]);
    }
}
