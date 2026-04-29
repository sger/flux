//! Single-threaded async runtime driver.
//!
//! This is the handoff point between backend completions and scheduler state.
//! OS threads are deliberately out of scope here; the same completion delivery
//! loop can later run inside a reactor/worker coordinator.

use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use super::{
    backend::{AsyncBackend, AsyncError, Completion, RequestId},
    scheduler::{SchedulerError, SchedulerState, SuspendedContinuation},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DriverTick {
    pub commands: usize,
    pub completions: usize,
}

impl DriverTick {
    pub fn made_progress(self) -> bool {
        self.commands > 0 || self.completions > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverRunLimit {
    pub max_ticks: usize,
}

impl Default for DriverRunLimit {
    fn default() -> Self {
        Self { max_ticks: 1024 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DriverRunReport {
    pub ticks: usize,
    pub commands: usize,
    pub completions: usize,
    pub stopped: bool,
    pub idle: bool,
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeCommand {
    SpawnTask,
    Park(SuspendedContinuation),
    Cancel(RequestId),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeControlCommand {
    SpawnTask,
    Cancel(RequestId),
    Stop,
}

type SharedControlQueue = Arc<Mutex<VecDeque<RuntimeControlCommand>>>;

#[derive(Clone)]
pub struct RuntimeControlHandle {
    commands: SharedControlQueue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    Scheduler(SchedulerError),
    Backend(AsyncError),
    ControlQueuePoisoned,
}

impl From<SchedulerError> for DriverError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl From<AsyncError> for DriverError {
    fn from(error: AsyncError) -> Self {
        Self::Backend(error)
    }
}

#[derive(Debug)]
pub struct RuntimeDriver<B> {
    scheduler: SchedulerState,
    backend: B,
    commands: VecDeque<RuntimeCommand>,
    control_commands: SharedControlQueue,
    completed: VecDeque<SuspendedContinuation>,
    canceled: HashSet<RequestId>,
    stopped: bool,
}

impl<B> RuntimeDriver<B> {
    pub fn new(scheduler: SchedulerState, backend: B) -> Self {
        Self {
            scheduler,
            backend,
            commands: VecDeque::new(),
            control_commands: Arc::new(Mutex::new(VecDeque::new())),
            completed: VecDeque::new(),
            canceled: HashSet::new(),
            stopped: false,
        }
    }

    pub fn scheduler(&self) -> &SchedulerState {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut SchedulerState {
        &mut self.scheduler
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn submit_runtime_command(&mut self, command: RuntimeCommand) {
        self.commands.push_back(command);
    }

    pub fn pending_runtime_commands(&self) -> usize {
        self.commands.len()
    }

    pub fn pending_control_commands(&self) -> Result<usize, DriverError> {
        Ok(self
            .control_commands
            .lock()
            .map_err(|_| DriverError::ControlQueuePoisoned)?
            .len())
    }

    pub fn pending_completed_continuations(&self) -> usize {
        self.completed.len()
    }

    pub fn pop_completed_continuation(&mut self) -> Option<SuspendedContinuation> {
        self.completed.pop_front()
    }

    pub fn deliver_local_completion(&mut self, completion: Completion) -> Result<(), DriverError> {
        let continuation = self.scheduler.deliver_completion(&completion)?;
        self.completed.push_back(continuation);
        Ok(())
    }

    pub fn was_canceled(&self, request_id: RequestId) -> bool {
        self.canceled.contains(&request_id)
    }

    pub fn control_handle(&self) -> RuntimeControlHandle {
        RuntimeControlHandle {
            commands: Arc::clone(&self.control_commands),
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn into_parts(self) -> (SchedulerState, B) {
        (self.scheduler, self.backend)
    }
}

impl<B: AsyncBackend> RuntimeDriver<B> {
    pub fn tick(&mut self) -> Result<DriverTick, DriverError> {
        let mut tick = DriverTick::default();
        tick.commands = self.drain_runtime_commands()?;
        tick.commands += self.drain_control_commands()?;
        while let Some(completion) = self.backend.poll_completion() {
            if self.canceled.remove(&completion.request_id) {
                continue;
            }
            let continuation = self.deliver_completion(&completion)?;
            self.completed.push_back(continuation);
            tick.completions += 1;
        }
        Ok(tick)
    }

    pub fn run_until_idle(
        &mut self,
        limit: DriverRunLimit,
    ) -> Result<DriverRunReport, DriverError> {
        let mut report = DriverRunReport::default();
        for _ in 0..limit.max_ticks {
            let tick = self.tick()?;
            report.ticks += 1;
            report.commands += tick.commands;
            report.completions += tick.completions;
            if self.stopped {
                report.stopped = true;
                return Ok(report);
            }
            if !tick.made_progress() {
                report.idle = true;
                return Ok(report);
            }
        }
        report.exhausted = true;
        report.stopped = self.stopped;
        Ok(report)
    }

    pub fn deliver_completion(
        &mut self,
        completion: &Completion,
    ) -> Result<SuspendedContinuation, DriverError> {
        Ok(self.scheduler.deliver_completion(completion)?)
    }

    pub fn drain_runtime_commands(&mut self) -> Result<usize, DriverError> {
        let mut drained = 0;
        while let Some(command) = self.commands.pop_front() {
            match command {
                RuntimeCommand::SpawnTask => {
                    self.scheduler.spawn_task()?;
                }
                RuntimeCommand::Park(wait) => {
                    self.scheduler.park(wait)?;
                }
                RuntimeCommand::Cancel(request_id) => {
                    let wait = self.scheduler.cancel_wait(request_id)?;
                    self.canceled.insert(request_id);
                    if let Some(cancel_handle) = wait.cancel_handle {
                        self.backend.cancel(cancel_handle)?;
                    }
                }
                RuntimeCommand::Stop => {
                    self.stopped = true;
                }
            }
            drained += 1;
        }
        Ok(drained)
    }

    pub fn drain_control_commands(&mut self) -> Result<usize, DriverError> {
        let mut commands = {
            let mut queued = self
                .control_commands
                .lock()
                .map_err(|_| DriverError::ControlQueuePoisoned)?;
            std::mem::take(&mut *queued)
        };
        let mut drained = 0;
        while let Some(command) = commands.pop_front() {
            match command {
                RuntimeControlCommand::SpawnTask => {
                    self.scheduler.spawn_task()?;
                }
                RuntimeControlCommand::Cancel(request_id) => {
                    let wait = self.scheduler.cancel_wait(request_id)?;
                    self.canceled.insert(request_id);
                    if let Some(cancel_handle) = wait.cancel_handle {
                        self.backend.cancel(cancel_handle)?;
                    }
                }
                RuntimeControlCommand::Stop => {
                    self.stopped = true;
                }
            }
            drained += 1;
        }
        Ok(drained)
    }
}

impl std::fmt::Debug for RuntimeControlHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending_commands = self.commands.lock().map(|commands| commands.len()).ok();
        f.debug_struct("RuntimeControlHandle")
            .field("pending_commands", &pending_commands)
            .finish_non_exhaustive()
    }
}

impl RuntimeControlHandle {
    pub fn submit(&self, command: RuntimeControlCommand) -> Result<(), DriverError> {
        self.commands
            .lock()
            .map_err(|_| DriverError::ControlQueuePoisoned)?
            .push_back(command);
        Ok(())
    }

    pub fn spawn_task(&self) -> Result<(), DriverError> {
        self.submit(RuntimeControlCommand::SpawnTask)
    }

    pub fn cancel(&self, request_id: RequestId) -> Result<(), DriverError> {
        self.submit(RuntimeControlCommand::Cancel(request_id))
    }

    pub fn stop(&self) -> Result<(), DriverError> {
        self.submit(RuntimeControlCommand::Stop)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::thread;

    use super::*;
    use crate::runtime::{
        r#async::{
            backend::{AsyncError, CancelHandle, CompletionPayload, RequestId, RuntimeTarget},
            context::{TaskId, WorkerId},
            scheduler::{SchedulerConfig, SuspendedContinuation},
        },
        value::Value,
    };

    #[derive(Debug, Default)]
    struct FakeBackend {
        completions: VecDeque<Completion>,
        cancelled: Vec<CancelHandle>,
    }

    impl FakeBackend {
        fn push(&mut self, completion: Completion) {
            self.completions.push_back(completion);
        }
    }

    impl AsyncBackend for FakeBackend {
        fn poll_completion(&mut self) -> Option<Completion> {
            self.completions.pop_front()
        }

        fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError> {
            self.cancelled.push(handle);
            Ok(())
        }
    }

    #[test]
    fn driver_delivers_backend_completions_to_scheduler() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 2 });
        let (_task_id, _worker_id) = scheduler.spawn_task().expect("first task");
        let (task_id, home_worker) = scheduler.spawn_task().expect("second task");
        assert_eq!((task_id, home_worker), (TaskId(2), WorkerId(1)));

        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(42),
            ))
            .expect("park succeeds");

        let mut backend = FakeBackend::default();
        backend.push(Completion::ok(request_id, target, CompletionPayload::Unit));

        let mut driver = RuntimeDriver::new(scheduler, backend);
        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(
            tick,
            DriverTick {
                commands: 0,
                completions: 1
            }
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(1))
                .expect("worker exists"),
            Some(target)
        );
        let completed = driver
            .pop_completed_continuation()
            .expect("completed continuation is retained for VM resume");
        assert_eq!(completed.request_id, request_id);
        assert_eq!(completed.continuation, Value::Integer(42));
        assert!(driver.scheduler().waits().is_empty());
    }

    #[test]
    fn driver_stops_on_scheduler_delivery_error() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let target = RuntimeTarget::Task(TaskId(1));
        let mut backend = FakeBackend::default();
        backend.push(Completion::ok(
            RequestId(404),
            target,
            CompletionPayload::Unit,
        ));

        let mut driver = RuntimeDriver::new(scheduler, backend);
        let err = driver.tick().expect_err("unknown wait is rejected");

        assert!(matches!(
            err,
            DriverError::Scheduler(SchedulerError::Wait(_))
        ));
    }

    #[test]
    fn driver_runtime_command_spawns_task() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::SpawnTask);

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(
            tick,
            DriverTick {
                commands: 1,
                completions: 0
            }
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(1)))
        );
    }

    #[test]
    fn driver_runtime_command_parks_wait() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Park(SuspendedContinuation::new(
            request_id,
            target,
            Value::Integer(64),
        )));

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert!(driver.scheduler().waits().contains(request_id));
    }

    #[test]
    fn driver_runtime_command_cancel_removes_wait_and_cancels_backend() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = CancelHandle::new(request_id, target);
        scheduler
            .park(
                SuspendedContinuation::new(request_id, target, Value::Integer(64))
                    .with_cancel_handle(cancel_handle),
            )
            .expect("park succeeds");
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Cancel(request_id));

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert!(driver.scheduler().waits().is_empty());
        assert_eq!(driver.backend().cancelled, vec![cancel_handle]);
        assert!(driver.was_canceled(request_id));
    }

    #[test]
    fn driver_runtime_command_cancel_unknown_wait_errors() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Cancel(RequestId(404)));

        let err = driver.tick().expect_err("unknown wait cancel is rejected");

        assert!(matches!(
            err,
            DriverError::Scheduler(SchedulerError::Wait(_))
        ));
    }

    #[test]
    fn driver_runtime_command_stop_marks_driver_stopped() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Stop);

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert!(driver.is_stopped());
    }

    #[test]
    fn control_handle_spawns_task_from_another_thread() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        let handle = driver.control_handle();

        thread::spawn(move || {
            handle.spawn_task().expect("spawn command submits");
        })
        .join()
        .expect("control thread joins");

        assert_eq!(
            driver.pending_control_commands().expect("queue readable"),
            1
        );
        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(1)))
        );
    }

    #[test]
    fn control_handle_cancel_from_another_thread_cancels_backend() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = CancelHandle::new(request_id, target);
        scheduler
            .park(
                SuspendedContinuation::new(request_id, target, Value::Integer(64))
                    .with_cancel_handle(cancel_handle),
            )
            .expect("park succeeds");
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        let handle = driver.control_handle();

        thread::spawn(move || {
            handle.cancel(request_id).expect("cancel command submits");
        })
        .join()
        .expect("control thread joins");

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert!(driver.scheduler().waits().is_empty());
        assert_eq!(driver.backend().cancelled, vec![cancel_handle]);
    }

    #[test]
    fn driver_ignores_late_completion_after_cancel() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = CancelHandle::new(request_id, target);
        scheduler
            .park(
                SuspendedContinuation::new(request_id, target, Value::Integer(64))
                    .with_cancel_handle(cancel_handle),
            )
            .expect("park succeeds");
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Cancel(request_id));
        driver.tick().expect("cancel succeeds");
        driver
            .backend_mut()
            .push(Completion::ok(request_id, target, CompletionPayload::Unit));

        let tick = driver.tick().expect("late canceled completion is ignored");

        assert_eq!(
            tick,
            DriverTick {
                commands: 0,
                completions: 0,
            }
        );
        assert_eq!(driver.pending_completed_continuations(), 0);
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(target)
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            None
        );
    }

    #[test]
    fn control_handle_stop_from_another_thread_marks_driver_stopped() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        let handle = driver.control_handle();

        thread::spawn(move || {
            handle.stop().expect("stop command submits");
        })
        .join()
        .expect("control thread joins");

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 1);
        assert!(driver.is_stopped());
    }

    #[test]
    fn local_park_drains_before_thread_safe_cancel() {
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, _) = scheduler.spawn_task().expect("task spawns");
        let request_id = scheduler.allocate_request_id();
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = CancelHandle::new(request_id, target);
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Park(
            SuspendedContinuation::new(request_id, target, Value::Integer(64))
                .with_cancel_handle(cancel_handle),
        ));
        let handle = driver.control_handle();
        thread::spawn(move || {
            handle.cancel(request_id).expect("cancel command submits");
        })
        .join()
        .expect("control thread joins");

        let tick = driver.tick().expect("tick succeeds");

        assert_eq!(tick.commands, 2);
        assert!(driver.scheduler().waits().is_empty());
        assert_eq!(driver.backend().cancelled, vec![cancel_handle]);
    }

    #[test]
    fn run_until_idle_accumulates_ticks_until_no_progress() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::SpawnTask);
        driver.submit_runtime_command(RuntimeCommand::SpawnTask);

        let report = driver
            .run_until_idle(DriverRunLimit { max_ticks: 4 })
            .expect("driver runs");

        assert_eq!(
            report,
            DriverRunReport {
                ticks: 2,
                commands: 2,
                completions: 0,
                stopped: false,
                idle: true,
                exhausted: false,
            }
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(1)))
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(RuntimeTarget::Task(TaskId(2)))
        );
    }

    #[test]
    fn run_until_idle_stops_when_stop_command_is_seen() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::Stop);

        let report = driver
            .run_until_idle(DriverRunLimit { max_ticks: 4 })
            .expect("driver runs");

        assert_eq!(
            report,
            DriverRunReport {
                ticks: 1,
                commands: 1,
                completions: 0,
                stopped: true,
                idle: false,
                exhausted: false,
            }
        );
    }

    #[test]
    fn run_until_idle_reports_exhausted_limit() {
        let scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let backend = FakeBackend::default();
        let mut driver = RuntimeDriver::new(scheduler, backend);
        driver.submit_runtime_command(RuntimeCommand::SpawnTask);
        driver.submit_runtime_command(RuntimeCommand::SpawnTask);

        let report = driver
            .run_until_idle(DriverRunLimit { max_ticks: 1 })
            .expect("driver runs");

        assert_eq!(
            report,
            DriverRunReport {
                ticks: 1,
                commands: 2,
                completions: 0,
                stopped: false,
                idle: false,
                exhausted: true,
            }
        );
    }
}
