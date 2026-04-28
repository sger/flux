//! Scheduler-side task and suspension state.
//!
//! This module is intentionally single-threaded for now. It defines the state
//! shape that the Phase 1a worker pool will wrap with OS threads: request ids,
//! parked continuations, worker-local ready queues, and completion delivery.

use std::collections::HashMap;

use crate::runtime::value::Value;

use super::{
    backend::{CancelHandle, Completion, RequestId, RuntimeTarget},
    context::{FiberId, TaskId, WorkerId},
    worker::WorkerState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    DuplicateRequest(RequestId),
    UnknownRequest(RequestId),
    TargetMismatch {
        request_id: RequestId,
        expected: RuntimeTarget,
        actual: RuntimeTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    UnknownWorker(WorkerId),
    UnknownTask(TaskId),
    UnknownFiber(FiberId),
    Wait(WaitError),
}

impl From<WaitError> for SchedulerError {
    fn from(error: WaitError) -> Self {
        Self::Wait(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuspendedContinuation {
    pub request_id: RequestId,
    pub target: RuntimeTarget,
    pub continuation: Value,
    pub cancel_handle: Option<CancelHandle>,
}

impl SuspendedContinuation {
    pub fn new(request_id: RequestId, target: RuntimeTarget, continuation: Value) -> Self {
        Self {
            request_id,
            target,
            continuation,
            cancel_handle: None,
        }
    }

    pub fn with_cancel_handle(mut self, cancel_handle: CancelHandle) -> Self {
        self.cancel_handle = Some(cancel_handle);
        self
    }
}

#[derive(Debug, Default)]
pub struct RequestIdAllocator {
    next: u64,
}

impl RequestIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_id(&mut self) -> RequestId {
        self.next = self.next.wrapping_add(1);
        RequestId(self.next)
    }
}

#[derive(Debug, Default)]
pub struct TaskIdAllocator {
    next: u64,
}

impl TaskIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_id(&mut self) -> TaskId {
        self.next = self.next.wrapping_add(1);
        TaskId(self.next)
    }
}

#[derive(Debug, Default)]
pub struct WaitRegistry {
    waits: HashMap<RequestId, SuspendedContinuation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub worker_count: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { worker_count: 1 }
    }
}

#[derive(Debug)]
pub struct SchedulerState {
    workers: Vec<WorkerState>,
    waits: WaitRegistry,
    task_home: HashMap<TaskId, WorkerId>,
    fiber_home: HashMap<FiberId, WorkerId>,
    request_ids: RequestIdAllocator,
    task_ids: TaskIdAllocator,
    next_worker: usize,
}

impl SchedulerState {
    pub fn new(config: SchedulerConfig) -> Self {
        let worker_count = config.worker_count.max(1);
        let workers = (0..worker_count)
            .map(|idx| WorkerState::new(WorkerId(idx as u32)))
            .collect();
        Self {
            workers,
            waits: WaitRegistry::new(),
            task_home: HashMap::new(),
            fiber_home: HashMap::new(),
            request_ids: RequestIdAllocator::new(),
            task_ids: TaskIdAllocator::new(),
            next_worker: 0,
        }
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn waits(&self) -> &WaitRegistry {
        &self.waits
    }

    pub fn worker(&self, worker_id: WorkerId) -> Option<&WorkerState> {
        self.workers.get(worker_id.0 as usize)
    }

    pub fn worker_mut(&mut self, worker_id: WorkerId) -> Option<&mut WorkerState> {
        self.workers.get_mut(worker_id.0 as usize)
    }

    pub fn allocate_request_id(&mut self) -> RequestId {
        self.request_ids.next_id()
    }

    pub fn spawn_task(&mut self) -> Result<(TaskId, WorkerId), SchedulerError> {
        let task_id = self.task_ids.next_id();
        let worker_id = self.select_worker();
        self.register_task_home(task_id, worker_id)?;
        self.enqueue(worker_id, RuntimeTarget::Task(task_id))?;
        Ok((task_id, worker_id))
    }

    pub fn register_task_home(
        &mut self,
        task_id: TaskId,
        worker_id: WorkerId,
    ) -> Result<(), SchedulerError> {
        self.ensure_worker(worker_id)?;
        self.task_home.insert(task_id, worker_id);
        Ok(())
    }

    pub fn register_fiber_home(
        &mut self,
        fiber_id: FiberId,
        worker_id: WorkerId,
    ) -> Result<(), SchedulerError> {
        self.ensure_worker(worker_id)?;
        self.fiber_home.insert(fiber_id, worker_id);
        Ok(())
    }

    pub fn home_worker_for(&self, target: RuntimeTarget) -> Result<WorkerId, SchedulerError> {
        match target {
            RuntimeTarget::Task(task_id) => self
                .task_home
                .get(&task_id)
                .copied()
                .ok_or(SchedulerError::UnknownTask(task_id)),
            RuntimeTarget::Fiber(fiber_id) => self
                .fiber_home
                .get(&fiber_id)
                .copied()
                .ok_or(SchedulerError::UnknownFiber(fiber_id)),
        }
    }

    pub fn park(&mut self, wait: SuspendedContinuation) -> Result<(), SchedulerError> {
        self.home_worker_for(wait.target)?;
        self.waits.insert(wait)?;
        Ok(())
    }

    pub fn cancel_wait(
        &mut self,
        request_id: RequestId,
    ) -> Result<SuspendedContinuation, SchedulerError> {
        Ok(self.waits.cancel(request_id)?)
    }

    pub fn deliver_completion(
        &mut self,
        completion: &Completion,
    ) -> Result<SuspendedContinuation, SchedulerError> {
        let wait = self.waits.complete(completion)?;
        let worker_id = self.home_worker_for(wait.target)?;
        self.enqueue(worker_id, wait.target)?;
        Ok(wait)
    }

    pub fn pop_ready(
        &mut self,
        worker_id: WorkerId,
    ) -> Result<Option<RuntimeTarget>, SchedulerError> {
        let worker = self
            .worker_mut(worker_id)
            .ok_or(SchedulerError::UnknownWorker(worker_id))?;
        Ok(worker.dequeue())
    }

    fn enqueue(
        &mut self,
        worker_id: WorkerId,
        target: RuntimeTarget,
    ) -> Result<(), SchedulerError> {
        let worker = self.ensure_worker_mut(worker_id)?;
        worker.enqueue(target);
        Ok(())
    }

    fn ensure_worker(&self, worker_id: WorkerId) -> Result<(), SchedulerError> {
        self.worker(worker_id)
            .map(|_| ())
            .ok_or(SchedulerError::UnknownWorker(worker_id))
    }

    fn ensure_worker_mut(
        &mut self,
        worker_id: WorkerId,
    ) -> Result<&mut WorkerState, SchedulerError> {
        self.worker_mut(worker_id)
            .ok_or(SchedulerError::UnknownWorker(worker_id))
    }

    fn select_worker(&mut self) -> WorkerId {
        let selected = self.next_worker % self.workers.len();
        self.next_worker = (self.next_worker + 1) % self.workers.len();
        WorkerId(selected as u32)
    }
}

impl WaitRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.waits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    pub fn contains(&self, request_id: RequestId) -> bool {
        self.waits.contains_key(&request_id)
    }

    pub fn insert(&mut self, wait: SuspendedContinuation) -> Result<(), WaitError> {
        if self.waits.contains_key(&wait.request_id) {
            return Err(WaitError::DuplicateRequest(wait.request_id));
        }
        self.waits.insert(wait.request_id, wait);
        Ok(())
    }

    pub fn cancel(&mut self, request_id: RequestId) -> Result<SuspendedContinuation, WaitError> {
        self.take(request_id)
    }

    pub fn complete(
        &mut self,
        completion: &Completion,
    ) -> Result<SuspendedContinuation, WaitError> {
        let wait = self
            .waits
            .get(&completion.request_id)
            .ok_or(WaitError::UnknownRequest(completion.request_id))?;
        if wait.target != completion.target {
            return Err(WaitError::TargetMismatch {
                request_id: completion.request_id,
                expected: wait.target,
                actual: completion.target,
            });
        }
        self.take(completion.request_id)
    }

    fn take(&mut self, request_id: RequestId) -> Result<SuspendedContinuation, WaitError> {
        self.waits
            .remove(&request_id)
            .ok_or(WaitError::UnknownRequest(request_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::r#async::{
        backend::{CompletionPayload, RuntimeTarget},
        context::TaskId,
    };

    #[test]
    fn request_ids_start_at_one() {
        let mut ids = RequestIdAllocator::new();
        assert_eq!(ids.next_id(), RequestId(1));
        assert_eq!(ids.next_id(), RequestId(2));
    }

    #[test]
    fn task_ids_start_at_one() {
        let mut ids = TaskIdAllocator::new();
        assert_eq!(ids.next_id(), TaskId(1));
        assert_eq!(ids.next_id(), TaskId(2));
    }

    #[test]
    fn registry_parks_and_completes_wait() {
        let request_id = RequestId(10);
        let target = RuntimeTarget::Task(TaskId(1));
        let mut registry = WaitRegistry::new();
        registry
            .insert(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(42),
            ))
            .expect("insert succeeds");

        let completion = Completion::ok(request_id, target, CompletionPayload::Unit);
        let wait = registry.complete(&completion).expect("completion matches");

        assert_eq!(wait.continuation, Value::Integer(42));
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_rejects_duplicate_request_ids() {
        let request_id = RequestId(10);
        let target = RuntimeTarget::Task(TaskId(1));
        let mut registry = WaitRegistry::new();
        registry
            .insert(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(1),
            ))
            .expect("first insert succeeds");

        let err = registry
            .insert(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(2),
            ))
            .expect_err("duplicate rejected");

        assert_eq!(err, WaitError::DuplicateRequest(request_id));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn completion_target_must_match_registered_wait() {
        let request_id = RequestId(10);
        let mut registry = WaitRegistry::new();
        registry
            .insert(SuspendedContinuation::new(
                request_id,
                RuntimeTarget::Task(TaskId(1)),
                Value::Integer(42),
            ))
            .expect("insert succeeds");

        let completion = Completion::ok(
            request_id,
            RuntimeTarget::Task(TaskId(2)),
            CompletionPayload::Unit,
        );
        let err = registry
            .complete(&completion)
            .expect_err("target mismatch rejected");

        assert_eq!(
            err,
            WaitError::TargetMismatch {
                request_id,
                expected: RuntimeTarget::Task(TaskId(1)),
                actual: RuntimeTarget::Task(TaskId(2)),
            }
        );
        assert!(registry.contains(request_id));
    }

    #[test]
    fn scheduler_spawn_round_robins_workers() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });

        let first = scheduler.spawn_task().expect("first task");
        let second = scheduler.spawn_task().expect("second task");
        let third = scheduler.spawn_task().expect("third task");

        assert_eq!(first, (TaskId(1), WorkerId(0)));
        assert_eq!(second, (TaskId(2), WorkerId(1)));
        assert_eq!(third, (TaskId(3), WorkerId(0)));
        assert_eq!(
            scheduler.pop_ready(WorkerId(0)).expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(1)))
        );
        assert_eq!(
            scheduler.pop_ready(WorkerId(1)).expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(2)))
        );
    }

    #[test]
    fn scheduler_delivers_completion_to_home_worker() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("first task");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));
        let (task_id, worker_id) = scheduler.spawn_task().expect("second task");
        assert_eq!((task_id, worker_id), (TaskId(2), WorkerId(1)));
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(TaskId(2));
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(42),
            ))
            .expect("park succeeds");

        let completion = Completion::ok(request_id, target, CompletionPayload::Unit);
        let wait = scheduler
            .deliver_completion(&completion)
            .expect("delivery succeeds");

        assert_eq!(wait.continuation, Value::Integer(42));
        assert_eq!(
            scheduler.pop_ready(WorkerId(1)).expect("worker exists"),
            Some(target)
        );
        assert!(scheduler.waits().is_empty());
    }

    #[test]
    fn scheduler_requires_explicit_task_home() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let err = scheduler
            .park(SuspendedContinuation::new(
                RequestId(11),
                RuntimeTarget::Task(TaskId(99)),
                Value::Integer(42),
            ))
            .expect_err("unregistered task home is rejected");

        assert_eq!(err, SchedulerError::UnknownTask(TaskId(99)));
        assert!(scheduler.waits().is_empty());
    }

    #[test]
    fn scheduler_delivers_fiber_completion_to_registered_home_worker() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Fiber(FiberId(7));
        scheduler
            .register_fiber_home(FiberId(7), WorkerId(1))
            .expect("fiber home registers");
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(99),
            ))
            .expect("park succeeds");

        let completion = Completion::ok(request_id, target, CompletionPayload::Unit);
        let wait = scheduler
            .deliver_completion(&completion)
            .expect("delivery succeeds");

        assert_eq!(wait.continuation, Value::Integer(99));
        assert_eq!(
            scheduler.pop_ready(WorkerId(1)).expect("worker exists"),
            Some(target)
        );
    }

    #[test]
    fn scheduler_cancel_wait_removes_registered_wait() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(7),
            ))
            .expect("park succeeds");

        let wait = scheduler.cancel_wait(request_id).expect("wait cancels");

        assert_eq!(wait.target, target);
        assert_eq!(wait.continuation, Value::Integer(7));
        assert!(scheduler.waits().is_empty());
    }
}
