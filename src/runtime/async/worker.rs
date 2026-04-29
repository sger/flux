//! Worker state for the Phase 1a scheduler.
//!
//! This is deliberately not a thread pool yet. It models the state a worker
//! owns once Phase 1a starts OS threads: a stable worker id and a local ready
//! queue of runtime targets. The thread helper below is intentionally limited
//! to `RuntimeControlCommand`, so worker threads cannot move Flux heap values
//! or continuations across OS threads.

use std::collections::VecDeque;
use std::thread;

use super::{
    backend::RuntimeTarget,
    context::{FiberId, RuntimeContext, TaskId, WorkerId},
    driver::{DriverError, RuntimeControlCommand, RuntimeControlHandle},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStatus {
    Idle,
    Running,
    Parked,
    Stopped,
}

#[derive(Debug)]
pub struct WorkerState {
    id: WorkerId,
    status: WorkerStatus,
    ready: VecDeque<RuntimeTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerThreadPlan {
    worker_id: WorkerId,
    commands: VecDeque<RuntimeControlCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerThreadReport {
    pub worker_id: WorkerId,
    pub commands: usize,
}

impl WorkerState {
    pub fn new(id: WorkerId) -> Self {
        Self {
            id,
            status: WorkerStatus::Idle,
            ready: VecDeque::new(),
        }
    }

    pub fn id(&self) -> WorkerId {
        self.id
    }

    pub fn status(&self) -> WorkerStatus {
        self.status
    }

    pub fn set_status(&mut self, status: WorkerStatus) {
        self.status = status;
    }

    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    pub fn is_ready_empty(&self) -> bool {
        self.ready.is_empty()
    }

    pub fn enqueue(&mut self, target: RuntimeTarget) {
        self.ready.push_back(target);
    }

    pub fn dequeue(&mut self) -> Option<RuntimeTarget> {
        self.ready.pop_front()
    }

    pub fn new_task_context(&self, task_id: TaskId) -> RuntimeContext {
        RuntimeContext::for_task(task_id, self.id)
    }

    pub fn new_fiber_context(&self, task_id: TaskId, fiber_id: FiberId) -> RuntimeContext {
        RuntimeContext::for_fiber(task_id, fiber_id, self.id)
    }
}

impl WorkerThreadPlan {
    pub fn new(worker_id: WorkerId) -> Self {
        Self {
            worker_id,
            commands: VecDeque::new(),
        }
    }

    pub fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    pub fn push(&mut self, command: RuntimeControlCommand) {
        self.commands.push_back(command);
    }

    pub fn spawn_task(&mut self) {
        self.push(RuntimeControlCommand::SpawnTask);
    }

    pub fn cancel(&mut self, request_id: super::backend::RequestId) {
        self.push(RuntimeControlCommand::Cancel(request_id));
    }

    pub fn stop(&mut self) {
        self.push(RuntimeControlCommand::Stop);
    }

    pub fn command_len(&self) -> usize {
        self.commands.len()
    }

    fn run(mut self, handle: RuntimeControlHandle) -> Result<WorkerThreadReport, DriverError> {
        let mut submitted = 0;
        while let Some(command) = self.commands.pop_front() {
            handle.submit(command)?;
            submitted += 1;
        }
        Ok(WorkerThreadReport {
            worker_id: self.worker_id,
            commands: submitted,
        })
    }
}

pub fn spawn_control_worker(
    plan: WorkerThreadPlan,
    handle: RuntimeControlHandle,
) -> thread::JoinHandle<Result<WorkerThreadReport, DriverError>> {
    thread::spawn(move || plan.run(handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::r#async::{
        backend::{AsyncBackend, AsyncError, CancelHandle, Completion},
        driver::{RuntimeControlCommand, RuntimeDriver},
        scheduler::{SchedulerConfig, SchedulerState},
    };

    #[derive(Debug, Default)]
    struct NoopBackend {
        cancelled: Vec<CancelHandle>,
    }

    impl AsyncBackend for NoopBackend {
        fn poll_completion(&mut self) -> Option<Completion> {
            None
        }

        fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError> {
            self.cancelled.push(handle);
            Ok(())
        }
    }

    #[test]
    fn worker_ready_queue_is_fifo() {
        let mut worker = WorkerState::new(WorkerId(0));
        worker.enqueue(RuntimeTarget::Task(TaskId(1)));
        worker.enqueue(RuntimeTarget::Task(TaskId(2)));

        assert_eq!(worker.ready_len(), 2);
        assert_eq!(worker.dequeue(), Some(RuntimeTarget::Task(TaskId(1))));
        assert_eq!(worker.dequeue(), Some(RuntimeTarget::Task(TaskId(2))));
        assert_eq!(worker.dequeue(), None);
    }

    #[test]
    fn worker_builds_task_context_on_its_id() {
        let worker = WorkerState::new(WorkerId(3));
        let ctx = worker.new_task_context(TaskId(9));

        assert_eq!(ctx.task_id, TaskId(9));
        assert_eq!(ctx.home_worker, WorkerId(3));
        assert_eq!(ctx.target(), RuntimeTarget::Task(TaskId(9)));
    }

    #[test]
    fn worker_builds_fiber_context_on_its_id() {
        let worker = WorkerState::new(WorkerId(3));
        let ctx = worker.new_fiber_context(TaskId(9), FiberId(12));

        assert_eq!(ctx.task_id, TaskId(9));
        assert_eq!(ctx.fiber_id, Some(FiberId(12)));
        assert_eq!(ctx.home_worker, WorkerId(3));
        assert_eq!(ctx.target(), RuntimeTarget::Fiber(FiberId(12)));
    }

    #[test]
    fn worker_thread_plan_records_safe_control_commands() {
        let mut plan = WorkerThreadPlan::new(WorkerId(2));
        plan.spawn_task();
        plan.stop();

        assert_eq!(plan.worker_id(), WorkerId(2));
        assert_eq!(plan.command_len(), 2);
    }

    #[test]
    fn control_worker_submits_commands_to_runtime_driver() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = NoopBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        let handle = driver.control_handle();
        let mut plan = WorkerThreadPlan::new(WorkerId(0));
        plan.push(RuntimeControlCommand::SpawnTask);
        plan.push(RuntimeControlCommand::Stop);

        let report = spawn_control_worker(plan, handle)
            .join()
            .expect("worker thread joins")
            .expect("commands submit");

        assert_eq!(
            report,
            WorkerThreadReport {
                worker_id: WorkerId(0),
                commands: 2,
            }
        );
        assert_eq!(
            driver.pending_control_commands().expect("queue readable"),
            2
        );
        let tick = driver.tick().expect("driver tick succeeds");
        assert_eq!(tick.commands, 2);
        assert!(driver.is_stopped());
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(1)))
        );
    }
}
