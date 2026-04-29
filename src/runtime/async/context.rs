//! Scheduler-owned runtime context for async effects.
//!
//! The VM already stores effect state in `YieldState`. This context packages
//! that state with the task/fiber identity that the Phase 1 scheduler will use
//! when a Flux computation suspends and later resumes from a backend
//! completion.

use crate::runtime::{evidence::EvidenceVector, yield_state::YieldState};

use super::backend::{RequestId, RuntimeTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiberId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CancelScopeId(pub u64);

/// Per-running-computation state owned by the scheduler.
///
/// Phase 1a uses one active task per worker thread. Phase 1b reuses this shape
/// by filling `fiber_id` and storing suspended continuations in the scheduler.
#[derive(Debug)]
pub struct RuntimeContext {
    pub task_id: TaskId,
    pub fiber_id: Option<FiberId>,
    pub home_worker: WorkerId,
    pub target: RuntimeTarget,
    pub yield_state: YieldState,
    pub evidence: EvidenceVector,
    pub continuation_request: Option<RequestId>,
    pub cancel_scope: Option<CancelScopeId>,
}

impl RuntimeContext {
    pub fn for_task(task_id: TaskId, home_worker: WorkerId) -> Self {
        Self {
            task_id,
            fiber_id: None,
            home_worker,
            target: RuntimeTarget::Task(task_id),
            yield_state: YieldState::new(),
            evidence: EvidenceVector::new(),
            continuation_request: None,
            cancel_scope: None,
        }
    }

    pub fn for_fiber(task_id: TaskId, fiber_id: FiberId, home_worker: WorkerId) -> Self {
        Self {
            task_id,
            fiber_id: Some(fiber_id),
            home_worker,
            target: RuntimeTarget::Fiber(fiber_id),
            yield_state: YieldState::new(),
            evidence: EvidenceVector::new(),
            continuation_request: None,
            cancel_scope: None,
        }
    }

    pub fn target(&self) -> RuntimeTarget {
        self.target
    }

    pub fn is_yielding(&self) -> bool {
        self.yield_state.is_yielding()
    }

    pub fn with_continuation_request(mut self, request_id: RequestId) -> Self {
        self.continuation_request = Some(request_id);
        self
    }

    pub fn with_cancel_scope(mut self, cancel_scope: CancelScopeId) -> Self {
        self.cancel_scope = Some(cancel_scope);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_context_targets_task() {
        let ctx = RuntimeContext::for_task(TaskId(7), WorkerId(2));
        assert_eq!(ctx.task_id, TaskId(7));
        assert_eq!(ctx.fiber_id, None);
        assert_eq!(ctx.home_worker, WorkerId(2));
        assert_eq!(ctx.target(), RuntimeTarget::Task(TaskId(7)));
        assert!(!ctx.is_yielding());
    }

    #[test]
    fn fiber_context_targets_fiber_but_keeps_parent_task() {
        let ctx = RuntimeContext::for_fiber(TaskId(7), FiberId(11), WorkerId(2));
        assert_eq!(ctx.task_id, TaskId(7));
        assert_eq!(ctx.fiber_id, Some(FiberId(11)));
        assert_eq!(ctx.target(), RuntimeTarget::Fiber(FiberId(11)));
    }

    #[test]
    fn context_records_scheduler_owned_suspend_metadata() {
        let ctx = RuntimeContext::for_task(TaskId(3), WorkerId(0))
            .with_continuation_request(RequestId(99))
            .with_cancel_scope(CancelScopeId(4));

        assert_eq!(ctx.continuation_request, Some(RequestId(99)));
        assert_eq!(ctx.cancel_scope, Some(CancelScopeId(4)));
    }

    #[test]
    fn suspended_contexts_keep_independent_yield_and_evidence_state() {
        use crate::runtime::yield_state::Yielding;
        use crate::syntax::interner::Interner;
        use std::rc::Rc;

        let mut interner = Interner::new();
        let effect_a = interner.intern("A");
        let effect_b = interner.intern("B");
        let arms = Rc::new(Vec::new());

        let mut first = RuntimeContext::for_task(TaskId(1), WorkerId(0))
            .with_continuation_request(RequestId(1));
        first.yield_state.yielding = Yielding::Pending;
        first.yield_state.marker = 10;
        first.evidence = first.evidence.insert(effect_a, 10, Rc::clone(&arms));

        let mut second = RuntimeContext::for_task(TaskId(2), WorkerId(1))
            .with_continuation_request(RequestId(2));
        second.yield_state.yielding = Yielding::Final;
        second.yield_state.marker = 20;
        second.evidence = second.evidence.insert(effect_b, 20, Rc::clone(&arms));

        assert_eq!(first.yield_state.marker, 10);
        assert_eq!(second.yield_state.marker, 20);
        assert!(first.evidence.lookup(effect_a).is_some());
        assert!(first.evidence.lookup(effect_b).is_none());
        assert!(second.evidence.lookup(effect_b).is_some());
        assert!(second.evidence.lookup(effect_a).is_none());
    }
}
