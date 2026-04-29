//! Scheduler-side task and suspension state.
//!
//! This module is intentionally single-threaded for now. It defines the state
//! shape that the Phase 1a worker pool will wrap with OS threads: request ids,
//! parked continuations, worker-local ready queues, and completion delivery.

use std::collections::{HashMap, HashSet};

use crate::runtime::value::Value;

use super::{
    backend::{AsyncError, CancelHandle, Completion, CompletionPayload, RequestId, RuntimeTarget},
    context::{CancelScopeId, FiberId, TaskId, WorkerId},
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
    UnknownCancelScope(CancelScopeId),
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
    pub completion: Option<Result<CompletionPayload, AsyncError>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberStatus {
    Ready,
    Parked,
    Canceled,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiberRecord {
    pub id: FiberId,
    pub task_id: TaskId,
    pub parent: Option<FiberId>,
    pub cancel_scope: CancelScopeId,
    pub home_worker: WorkerId,
    pub status: FiberStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelScopeRecord {
    pub id: CancelScopeId,
    pub parent: Option<CancelScopeId>,
    pub canceled: bool,
    fibers: HashSet<FiberId>,
}

impl SuspendedContinuation {
    pub fn new(request_id: RequestId, target: RuntimeTarget, continuation: Value) -> Self {
        Self {
            request_id,
            target,
            continuation,
            cancel_handle: None,
            completion: None,
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
pub struct FiberIdAllocator {
    next: u64,
}

impl FiberIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_id(&mut self) -> FiberId {
        self.next = self.next.wrapping_add(1);
        FiberId(self.next)
    }
}

#[derive(Debug, Default)]
pub struct CancelScopeIdAllocator {
    next: u64,
}

impl CancelScopeIdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_id(&mut self) -> CancelScopeId {
        self.next = self.next.wrapping_add(1);
        CancelScopeId(self.next)
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
    fibers: HashMap<FiberId, FiberRecord>,
    cancel_scopes: HashMap<CancelScopeId, CancelScopeRecord>,
    request_ids: RequestIdAllocator,
    task_ids: TaskIdAllocator,
    fiber_ids: FiberIdAllocator,
    cancel_scope_ids: CancelScopeIdAllocator,
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
            fibers: HashMap::new(),
            cancel_scopes: HashMap::new(),
            request_ids: RequestIdAllocator::new(),
            task_ids: TaskIdAllocator::new(),
            fiber_ids: FiberIdAllocator::new(),
            cancel_scope_ids: CancelScopeIdAllocator::new(),
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

    pub fn open_cancel_scope(
        &mut self,
        parent: Option<CancelScopeId>,
    ) -> Result<CancelScopeId, SchedulerError> {
        if let Some(parent) = parent {
            self.ensure_cancel_scope(parent)?;
        }
        let id = self.cancel_scope_ids.next_id();
        self.cancel_scopes.insert(
            id,
            CancelScopeRecord {
                id,
                parent,
                canceled: false,
                fibers: HashSet::new(),
            },
        );
        Ok(id)
    }

    pub fn spawn_fiber(
        &mut self,
        task_id: TaskId,
        parent: Option<FiberId>,
        cancel_scope: CancelScopeId,
    ) -> Result<(FiberId, WorkerId), SchedulerError> {
        self.ensure_cancel_scope(cancel_scope)?;
        let home_worker = match parent {
            Some(parent) => {
                let parent_record = self
                    .fibers
                    .get(&parent)
                    .ok_or(SchedulerError::UnknownFiber(parent))?;
                if parent_record.task_id != task_id {
                    return Err(SchedulerError::UnknownTask(task_id));
                }
                parent_record.home_worker
            }
            None => *self
                .task_home
                .get(&task_id)
                .ok_or(SchedulerError::UnknownTask(task_id))?,
        };
        let fiber_id = self.fiber_ids.next_id();
        self.register_fiber_home(fiber_id, home_worker)?;
        let status = if self.is_scope_canceled(cancel_scope)? {
            FiberStatus::Canceled
        } else {
            FiberStatus::Ready
        };
        self.fibers.insert(
            fiber_id,
            FiberRecord {
                id: fiber_id,
                task_id,
                parent,
                cancel_scope,
                home_worker,
                status,
            },
        );
        self.cancel_scopes
            .get_mut(&cancel_scope)
            .expect("scope was checked above")
            .fibers
            .insert(fiber_id);
        if status == FiberStatus::Ready {
            self.enqueue(home_worker, RuntimeTarget::Fiber(fiber_id))?;
        }
        Ok((fiber_id, home_worker))
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

    pub fn fiber(&self, fiber_id: FiberId) -> Option<&FiberRecord> {
        self.fibers.get(&fiber_id)
    }

    pub fn cancel_scope(&self, scope_id: CancelScopeId) -> Option<&CancelScopeRecord> {
        self.cancel_scopes.get(&scope_id)
    }

    pub fn is_scope_canceled(&self, scope_id: CancelScopeId) -> Result<bool, SchedulerError> {
        let mut current = Some(scope_id);
        while let Some(scope) = current {
            let record = self
                .cancel_scopes
                .get(&scope)
                .ok_or(SchedulerError::UnknownCancelScope(scope))?;
            if record.canceled {
                return Ok(true);
            }
            current = record.parent;
        }
        Ok(false)
    }

    pub fn cancel_scope_tree(
        &mut self,
        scope_id: CancelScopeId,
    ) -> Result<Vec<FiberId>, SchedulerError> {
        self.ensure_cancel_scope(scope_id)?;
        let descendants = self.descendant_scopes(scope_id);
        let mut canceled_fibers = Vec::new();
        for scope in descendants {
            if let Some(record) = self.cancel_scopes.get_mut(&scope) {
                record.canceled = true;
                for fiber in record.fibers.iter().copied() {
                    if let Some(fiber_record) = self.fibers.get_mut(&fiber)
                        && fiber_record.status != FiberStatus::Completed
                    {
                        fiber_record.status = FiberStatus::Canceled;
                        canceled_fibers.push(fiber);
                    }
                }
            }
        }
        Ok(canceled_fibers)
    }

    pub fn park(&mut self, wait: SuspendedContinuation) -> Result<(), SchedulerError> {
        self.home_worker_for(wait.target)?;
        if let RuntimeTarget::Fiber(fiber_id) = wait.target
            && let Some(fiber) = self.fibers.get_mut(&fiber_id)
            && fiber.status != FiberStatus::Canceled
        {
            fiber.status = FiberStatus::Parked;
        }
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
        let mut wait = self.waits.complete(completion)?;
        wait.completion = Some(completion.payload.clone());
        let worker_id = self.home_worker_for(wait.target)?;
        if let RuntimeTarget::Fiber(fiber_id) = wait.target
            && let Some(fiber) = self.fibers.get_mut(&fiber_id)
        {
            if fiber.status == FiberStatus::Canceled {
                return Ok(wait);
            }
            fiber.status = FiberStatus::Ready;
        }
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

    fn ensure_cancel_scope(&self, scope_id: CancelScopeId) -> Result<(), SchedulerError> {
        self.cancel_scopes
            .get(&scope_id)
            .map(|_| ())
            .ok_or(SchedulerError::UnknownCancelScope(scope_id))
    }

    fn descendant_scopes(&self, scope_id: CancelScopeId) -> Vec<CancelScopeId> {
        let mut out = vec![scope_id];
        let mut idx = 0;
        while idx < out.len() {
            let current = out[idx];
            for (candidate, record) in &self.cancel_scopes {
                if record.parent == Some(current) && !out.contains(candidate) {
                    out.push(*candidate);
                }
            }
            idx += 1;
        }
        out
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
    fn fiber_ids_start_at_one() {
        let mut ids = FiberIdAllocator::new();
        assert_eq!(ids.next_id(), FiberId(1));
        assert_eq!(ids.next_id(), FiberId(2));
    }

    #[test]
    fn cancel_scope_ids_start_at_one() {
        let mut ids = CancelScopeIdAllocator::new();
        assert_eq!(ids.next_id(), CancelScopeId(1));
        assert_eq!(ids.next_id(), CancelScopeId(2));
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
    fn scheduler_spawns_fiber_on_parent_task_worker() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let (first_task, first_worker) = scheduler.spawn_task().expect("first task");
        let (second_task, second_worker) = scheduler.spawn_task().expect("second task");
        assert_eq!((first_task, first_worker), (TaskId(1), WorkerId(0)));
        assert_eq!((second_task, second_worker), (TaskId(2), WorkerId(1)));
        let scope = scheduler.open_cancel_scope(None).expect("scope opens");

        let (fiber_id, worker_id) = scheduler
            .spawn_fiber(second_task, None, scope)
            .expect("fiber spawns");

        assert_eq!((fiber_id, worker_id), (FiberId(1), WorkerId(1)));
        let record = scheduler.fiber(fiber_id).expect("fiber record exists");
        assert_eq!(record.task_id, second_task);
        assert_eq!(record.parent, None);
        assert_eq!(record.cancel_scope, scope);
        assert_eq!(record.status, FiberStatus::Ready);
        assert_eq!(
            scheduler.pop_ready(WorkerId(1)).expect("worker exists"),
            Some(RuntimeTarget::Task(second_task))
        );
        assert_eq!(
            scheduler.pop_ready(WorkerId(1)).expect("worker exists"),
            Some(RuntimeTarget::Fiber(fiber_id))
        );
    }

    #[test]
    fn child_fiber_stays_on_parent_fiber_worker() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        let scope = scheduler.open_cancel_scope(None).expect("scope opens");
        let (parent, parent_worker) = scheduler
            .spawn_fiber(task_id, None, scope)
            .expect("parent fiber spawns");

        let (child, child_worker) = scheduler
            .spawn_fiber(task_id, Some(parent), scope)
            .expect("child fiber spawns");

        assert_eq!(worker_id, WorkerId(0));
        assert_eq!(parent_worker, worker_id);
        assert_eq!(child_worker, worker_id);
        assert_eq!(
            scheduler.fiber(child).expect("child exists").parent,
            Some(parent)
        );
    }

    #[test]
    fn cancel_scope_tree_marks_child_scopes_and_fibers_cancelled() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let parent_scope = scheduler
            .open_cancel_scope(None)
            .expect("parent scope opens");
        let child_scope = scheduler
            .open_cancel_scope(Some(parent_scope))
            .expect("child scope opens");
        let (parent_fiber, _) = scheduler
            .spawn_fiber(task_id, None, parent_scope)
            .expect("parent fiber spawns");
        let (child_fiber, _) = scheduler
            .spawn_fiber(task_id, Some(parent_fiber), child_scope)
            .expect("child fiber spawns");

        let canceled = scheduler
            .cancel_scope_tree(parent_scope)
            .expect("scope cancels");

        assert_eq!(canceled.len(), 2);
        assert!(canceled.contains(&parent_fiber));
        assert!(canceled.contains(&child_fiber));
        assert!(
            scheduler
                .cancel_scope(parent_scope)
                .expect("parent scope exists")
                .canceled
        );
        assert!(
            scheduler
                .cancel_scope(child_scope)
                .expect("child scope exists")
                .canceled
        );
        assert_eq!(
            scheduler.fiber(parent_fiber).expect("parent exists").status,
            FiberStatus::Canceled
        );
        assert_eq!(
            scheduler.fiber(child_fiber).expect("child exists").status,
            FiberStatus::Canceled
        );
    }

    #[test]
    fn cancelled_fiber_completion_is_not_requeued() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let scope = scheduler.open_cancel_scope(None).expect("scope opens");
        let (fiber_id, worker_id) = scheduler
            .spawn_fiber(task_id, None, scope)
            .expect("fiber spawns");
        assert_eq!(
            scheduler.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        assert_eq!(
            scheduler.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Fiber(fiber_id))
        );
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Fiber(fiber_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(99),
            ))
            .expect("fiber parks");
        scheduler
            .cancel_scope_tree(scope)
            .expect("scope cancellation succeeds");

        let completion = Completion::ok(request_id, target, CompletionPayload::Unit);
        let wait = scheduler
            .deliver_completion(&completion)
            .expect("late completion is consumed");

        assert_eq!(wait.continuation, Value::Integer(99));
        assert_eq!(scheduler.pop_ready(worker_id).expect("worker exists"), None);
        assert!(scheduler.waits().is_empty());
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
