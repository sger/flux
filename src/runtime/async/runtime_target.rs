//! Identifiers for the things a completion can be routed to (proposal 0174
//! Phase 1a-iii).
//!
//! Phase 1a's "work" is a [`Task<a>`](self::TaskId); Phase 1b extends
//! [`RuntimeTarget`] with a `Fiber` variant once structured concurrency
//! lands. The enum is a single point of truth for the scheduler so adding
//! the new variant in 1b is a `match` exhaustiveness check, not a hunt
//! through the codebase.

/// Scheduler-side identifier for a `Task<a>`. Allocated by the task manager
/// when [`Task.spawn`](https://) runs (slice 1a-vi); zero is reserved for
/// "no task" so the runtime can use `TaskId(0)` as a sentinel without
/// risking collision with a real id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(pub u64);

/// Target a backend completion is routed to.
///
/// Phase 1a-iii has the [`Task`](RuntimeTarget::Task) variant only; Phase 1b
/// adds `Fiber(FiberId)`. Routing today simply names a task; later slices
/// use the variant to pick the right scheduler queue (per-fiber wakelist
/// vs. per-task condvar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeTarget {
    Task(TaskId),
}

impl RuntimeTarget {
    /// Convenience: target the given task.
    pub fn task(id: TaskId) -> Self {
        RuntimeTarget::Task(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids_are_distinct_when_constructed_distinctly() {
        assert_ne!(TaskId(1), TaskId(2));
        assert_eq!(TaskId(7), TaskId(7));
    }

    #[test]
    fn runtime_target_round_trips() {
        let t = RuntimeTarget::task(TaskId(42));
        match t {
            RuntimeTarget::Task(id) => assert_eq!(id, TaskId(42)),
        }
    }
}
