//! Scheduler-owned runtime context for async effects.
//!
//! The VM already stores effect state in `YieldState`. This context packages
//! that state with the task/fiber identity that the Phase 1 scheduler will use
//! when a Flux computation suspends and later resumes from a backend
//! completion.

use crate::runtime::{evidence::EvidenceVector, yield_state::YieldState};

use super::backend::RuntimeTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FiberId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(pub u32);

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
        }
    }

    pub fn target(&self) -> RuntimeTarget {
        self.target
    }

    pub fn is_yielding(&self) -> bool {
        self.yield_state.is_yielding()
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
}
