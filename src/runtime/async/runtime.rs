//! VM-facing async runtime facade.
//!
//! This layer keeps VM values and continuations on the VM/runtime thread. It
//! owns the scheduler driver and exposes operations a VM integration can call:
//! spawn a task, park a continuation, start backend waits, poll completions,
//! cancel waits, and take resumed continuations.

use std::{path::PathBuf, time::Duration};

use super::{
    backend::{
        AsyncBackend, AsyncError, CancelHandle, Completion, IoHandle, RequestId, RuntimeTarget,
    },
    context::{CancelScopeId, FiberId, TaskId, WorkerId},
    driver::{
        DriverError, DriverRunLimit, DriverRunReport, DriverTick, RuntimeCommand, RuntimeDriver,
    },
    scheduler::{SchedulerConfig, SchedulerError, SchedulerState, SuspendedContinuation},
};
use crate::runtime::value::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncRuntimeError {
    Driver(DriverError),
    Scheduler(SchedulerError),
}

impl From<DriverError> for AsyncRuntimeError {
    fn from(error: DriverError) -> Self {
        Self::Driver(error)
    }
}

impl From<SchedulerError> for AsyncRuntimeError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

#[derive(Debug)]
pub struct AsyncRuntime<B> {
    driver: RuntimeDriver<B>,
}

impl<B> AsyncRuntime<B> {
    pub fn new(config: SchedulerConfig, backend: B) -> Self {
        Self {
            driver: RuntimeDriver::new(SchedulerState::new(config), backend),
        }
    }

    pub fn from_driver(driver: RuntimeDriver<B>) -> Self {
        Self { driver }
    }

    pub fn driver(&self) -> &RuntimeDriver<B> {
        &self.driver
    }

    pub fn driver_mut(&mut self) -> &mut RuntimeDriver<B> {
        &mut self.driver
    }

    pub fn scheduler(&self) -> &SchedulerState {
        self.driver.scheduler()
    }

    pub fn scheduler_mut(&mut self) -> &mut SchedulerState {
        self.driver.scheduler_mut()
    }

    pub fn backend(&self) -> &B {
        self.driver.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.driver.backend_mut()
    }

    pub fn spawn_task(&mut self) -> Result<(TaskId, WorkerId), AsyncRuntimeError> {
        Ok(self.scheduler_mut().spawn_task()?)
    }

    pub fn register_fiber_home(
        &mut self,
        fiber_id: FiberId,
        worker_id: WorkerId,
    ) -> Result<(), AsyncRuntimeError> {
        Ok(self
            .scheduler_mut()
            .register_fiber_home(fiber_id, worker_id)?)
    }

    pub fn open_cancel_scope(
        &mut self,
        parent: Option<CancelScopeId>,
    ) -> Result<CancelScopeId, AsyncRuntimeError> {
        Ok(self.scheduler_mut().open_cancel_scope(parent)?)
    }

    pub fn spawn_fiber(
        &mut self,
        task_id: TaskId,
        parent: Option<FiberId>,
        cancel_scope: CancelScopeId,
    ) -> Result<(FiberId, WorkerId), AsyncRuntimeError> {
        Ok(self
            .scheduler_mut()
            .spawn_fiber(task_id, parent, cancel_scope)?)
    }

    pub fn cancel_scope_tree(
        &mut self,
        scope_id: CancelScopeId,
    ) -> Result<Vec<FiberId>, AsyncRuntimeError> {
        Ok(self.scheduler_mut().cancel_scope_tree(scope_id)?)
    }

    pub fn park_task(
        &mut self,
        task_id: TaskId,
        continuation: Value,
        cancel_handle: Option<CancelHandle>,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.park(RuntimeTarget::Task(task_id), continuation, cancel_handle)
    }

    pub fn park_fiber(
        &mut self,
        fiber_id: FiberId,
        continuation: Value,
        cancel_handle: Option<CancelHandle>,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.park(RuntimeTarget::Fiber(fiber_id), continuation, cancel_handle)
    }

    pub fn pop_ready(
        &mut self,
        worker_id: WorkerId,
    ) -> Result<Option<RuntimeTarget>, AsyncRuntimeError> {
        Ok(self.scheduler_mut().pop_ready(worker_id)?)
    }

    pub fn pop_resumed_continuation(&mut self) -> Option<SuspendedContinuation> {
        self.driver.pop_completed_continuation()
    }

    pub fn deliver_local_completion(
        &mut self,
        completion: Completion,
    ) -> Result<(), AsyncRuntimeError> {
        Ok(self.driver.deliver_local_completion(completion)?)
    }

    pub fn pending_resumed_continuations(&self) -> usize {
        self.driver.pending_completed_continuations()
    }

    fn park(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        cancel_handle: Option<CancelHandle>,
    ) -> Result<RequestId, AsyncRuntimeError> {
        let request_id = self.scheduler_mut().allocate_request_id();
        let wait = match cancel_handle {
            Some(cancel_handle) => SuspendedContinuation::new(request_id, target, continuation)
                .with_cancel_handle(cancel_handle),
            None => SuspendedContinuation::new(request_id, target, continuation),
        };
        self.scheduler_mut().park(wait)?;
        Ok(request_id)
    }
}

impl<B: AsyncBackend> AsyncRuntime<B> {
    pub fn poll(&mut self) -> Result<DriverTick, AsyncRuntimeError> {
        Ok(self.driver.tick()?)
    }

    pub fn run_until_idle(
        &mut self,
        limit: DriverRunLimit,
    ) -> Result<DriverRunReport, AsyncRuntimeError> {
        Ok(self.driver.run_until_idle(limit)?)
    }

    pub fn request_cancel(&mut self, request_id: RequestId) {
        self.driver
            .submit_runtime_command(RuntimeCommand::Cancel(request_id));
    }

    pub fn request_stop(&mut self) {
        self.driver.submit_runtime_command(RuntimeCommand::Stop);
    }

    pub fn start_timer(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        duration: Duration,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.timer_start(request_id, target, duration)
        })
    }

    pub fn start_tcp_connect(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        host: String,
        port: u16,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_connect(request_id, target, host, port)
        })
    }

    pub fn start_tcp_listen(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        host: String,
        port: u16,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_listen(request_id, target, host, port)
        })
    }

    pub fn start_tcp_accept(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_accept(request_id, target, handle)
        })
    }

    pub fn start_tcp_read(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
        max: usize,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_read(request_id, target, handle, max)
        })
    }

    pub fn start_tcp_write(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_write(request_id, target, handle, bytes)
        })
    }

    pub fn start_tcp_close(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_close(request_id, target, handle)
        })
    }

    pub fn start_tcp_local_addr(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_local_addr(request_id, target, handle)
        })
    }

    pub fn start_tcp_remote_addr(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_remote_addr(request_id, target, handle)
        })
    }

    pub fn start_tcp_close_listener(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_close_listener(request_id, target, handle)
        })
    }

    pub fn start_tcp_listener_local_addr(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        handle: IoHandle,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.tcp_listener_local_addr(request_id, target, handle)
        })
    }

    pub fn start_dns_resolve(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        host: String,
        port: u16,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.dns_resolve(request_id, target, host, port)
        })
    }

    pub fn start_file_read(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        path: PathBuf,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.file_read(request_id, target, path)
        })
    }

    pub fn start_file_write(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<RequestId, AsyncRuntimeError> {
        self.start_backend_wait(target, continuation, |backend, request_id, target| {
            backend.file_write(request_id, target, path, bytes)
        })
    }

    fn start_backend_wait<F>(
        &mut self,
        target: RuntimeTarget,
        continuation: Value,
        start: F,
    ) -> Result<RequestId, AsyncRuntimeError>
    where
        F: FnOnce(&mut B, RequestId, RuntimeTarget) -> Result<CancelHandle, AsyncError>,
    {
        let request_id = self.scheduler_mut().allocate_request_id();
        let cancel_handle = start(self.backend_mut(), request_id, target)?;
        let wait = SuspendedContinuation::new(request_id, target, continuation)
            .with_cancel_handle(cancel_handle);
        self.scheduler_mut().park(wait)?;
        Ok(request_id)
    }
}

impl From<AsyncError> for AsyncRuntimeError {
    fn from(error: AsyncError) -> Self {
        Self::Driver(DriverError::Backend(error))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::runtime::{
        r#async::backend::{Completion, CompletionPayload},
        value::Value,
    };

    #[derive(Debug, Default)]
    struct FakeBackend {
        completions: VecDeque<Completion>,
        cancelled: Vec<CancelHandle>,
        started_timers: Vec<(RequestId, RuntimeTarget, Duration)>,
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

        fn timer_start(
            &mut self,
            request_id: RequestId,
            target: RuntimeTarget,
            duration: Duration,
        ) -> Result<CancelHandle, AsyncError> {
            self.started_timers.push((request_id, target, duration));
            Ok(CancelHandle::new(request_id, target))
        }
    }

    #[test]
    fn runtime_parks_and_resumes_task_continuation() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, worker_id) = runtime.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );

        let request_id = runtime
            .park_task(task_id, Value::Integer(99), None)
            .expect("task parks");
        runtime.backend_mut().push(Completion::ok(
            request_id,
            RuntimeTarget::Task(task_id),
            CompletionPayload::Unit,
        ));

        let tick = runtime.poll().expect("runtime polls");
        assert_eq!(tick.completions, 1);
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        let resumed = runtime
            .pop_resumed_continuation()
            .expect("continuation is retained");
        assert_eq!(resumed.request_id, request_id);
        assert_eq!(resumed.continuation, Value::Integer(99));
    }

    #[test]
    fn runtime_delivers_local_value_completion() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, worker_id) = runtime.spawn_task().expect("task spawns");
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        let request_id = runtime
            .park_task(task_id, Value::Integer(99), None)
            .expect("task parks");

        runtime
            .deliver_local_completion(Completion::ok(
                request_id,
                RuntimeTarget::Task(task_id),
                CompletionPayload::Value(Value::Integer(42)),
            ))
            .expect("local completion delivers");

        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        let resumed = runtime
            .pop_resumed_continuation()
            .expect("continuation is retained");
        assert_eq!(resumed.request_id, request_id);
        assert_eq!(resumed.continuation, Value::Integer(99));
        assert_eq!(
            resumed.completion,
            Some(Ok(CompletionPayload::Value(Value::Integer(42))))
        );
    }

    #[test]
    fn runtime_cancel_routes_to_backend_when_wait_has_cancel_handle() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, _) = runtime.spawn_task().expect("task spawns");
        let cancel_handle = CancelHandle::new(RequestId(500), RuntimeTarget::Task(task_id));
        let request_id = runtime
            .park_task(task_id, Value::Integer(7), Some(cancel_handle))
            .expect("task parks");

        runtime.request_cancel(request_id);
        let tick = runtime.poll().expect("runtime polls");

        assert_eq!(tick.commands, 1);
        assert!(runtime.scheduler().waits().is_empty());
        assert_eq!(runtime.backend().cancelled, vec![cancel_handle]);
    }

    #[test]
    fn runtime_start_timer_parks_continuation_with_backend_cancel_handle() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, _worker_id) = runtime.spawn_task().expect("task spawns");
        let target = RuntimeTarget::Task(task_id);

        let request_id = runtime
            .start_timer(target, Value::Integer(77), Duration::from_millis(5))
            .expect("timer wait starts");

        assert_eq!(
            runtime.backend().started_timers,
            vec![(request_id, target, Duration::from_millis(5))]
        );
        assert_eq!(runtime.scheduler().waits().len(), 1);

        runtime
            .backend_mut()
            .push(Completion::ok(request_id, target, CompletionPayload::Unit));
        runtime.poll().expect("runtime polls");
        let resumed = runtime
            .pop_resumed_continuation()
            .expect("continuation resumes");
        assert_eq!(resumed.request_id, request_id);
        assert_eq!(resumed.continuation, Value::Integer(77));
    }

    #[test]
    fn runtime_spawns_fiber_inside_cancel_scope() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, worker_id) = runtime.spawn_task().expect("task spawns");
        let scope = runtime.open_cancel_scope(None).expect("scope opens");

        let (fiber_id, fiber_worker) = runtime
            .spawn_fiber(task_id, None, scope)
            .expect("fiber spawns");

        assert_eq!(fiber_worker, worker_id);
        let fiber = runtime
            .scheduler()
            .fiber(fiber_id)
            .expect("fiber record exists");
        assert_eq!(fiber.task_id, task_id);
        assert_eq!(fiber.cancel_scope, scope);
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Fiber(fiber_id))
        );
    }

    #[test]
    fn runtime_cancel_scope_consumes_late_fiber_completion_without_requeue() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, worker_id) = runtime.spawn_task().expect("task spawns");
        let scope = runtime.open_cancel_scope(None).expect("scope opens");
        let (fiber_id, _) = runtime
            .spawn_fiber(task_id, None, scope)
            .expect("fiber spawns");
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Fiber(fiber_id))
        );
        let target = RuntimeTarget::Fiber(fiber_id);
        let request_id = runtime
            .start_timer(target, Value::Integer(13), Duration::from_millis(1))
            .expect("fiber timer starts");

        let canceled = runtime
            .cancel_scope_tree(scope)
            .expect("scope cancellation succeeds");
        assert_eq!(canceled, vec![fiber_id]);
        runtime
            .backend_mut()
            .push(Completion::ok(request_id, target, CompletionPayload::Unit));
        let tick = runtime.poll().expect("runtime polls");

        assert_eq!(tick.completions, 1);
        assert_eq!(runtime.pop_ready(worker_id).expect("worker exists"), None);
        let resumed = runtime
            .pop_resumed_continuation()
            .expect("late completion is still retained for cleanup");
        assert_eq!(resumed.request_id, request_id);
        assert_eq!(resumed.target, target);
    }

    #[test]
    fn runtime_keeps_two_suspended_continuations_independent() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 2 }, FakeBackend::default());
        let (first_task, first_worker) = runtime.spawn_task().expect("first task spawns");
        let (second_task, second_worker) = runtime.spawn_task().expect("second task spawns");
        assert_eq!((first_worker, second_worker), (WorkerId(0), WorkerId(1)));
        assert_eq!(
            runtime.pop_ready(first_worker).expect("worker exists"),
            Some(RuntimeTarget::Task(first_task))
        );
        assert_eq!(
            runtime.pop_ready(second_worker).expect("worker exists"),
            Some(RuntimeTarget::Task(second_task))
        );

        let first_request = runtime
            .park_task(first_task, Value::Integer(1), None)
            .expect("first task parks");
        let second_request = runtime
            .park_task(second_task, Value::Integer(2), None)
            .expect("second task parks");
        runtime.backend_mut().push(Completion::ok(
            second_request,
            RuntimeTarget::Task(second_task),
            CompletionPayload::Unit,
        ));
        runtime.backend_mut().push(Completion::ok(
            first_request,
            RuntimeTarget::Task(first_task),
            CompletionPayload::Unit,
        ));

        let tick = runtime.poll().expect("runtime polls");
        assert_eq!(tick.completions, 2);
        let resumed_second = runtime
            .pop_resumed_continuation()
            .expect("second continuation resumed first");
        let resumed_first = runtime
            .pop_resumed_continuation()
            .expect("first continuation resumed second");

        assert_eq!(resumed_second.request_id, second_request);
        assert_eq!(resumed_second.continuation, Value::Integer(2));
        assert_eq!(resumed_first.request_id, first_request);
        assert_eq!(resumed_first.continuation, Value::Integer(1));
        assert_eq!(
            runtime.pop_ready(second_worker).expect("worker exists"),
            Some(RuntimeTarget::Task(second_task))
        );
        assert_eq!(
            runtime.pop_ready(first_worker).expect("worker exists"),
            Some(RuntimeTarget::Task(first_task))
        );
    }

    #[test]
    fn runtime_cancel_before_completion_cleans_up_abandoned_continuation() {
        let mut runtime =
            AsyncRuntime::new(SchedulerConfig { worker_count: 1 }, FakeBackend::default());
        let (task_id, worker_id) = runtime.spawn_task().expect("task spawns");
        assert_eq!(
            runtime.pop_ready(worker_id).expect("worker exists"),
            Some(RuntimeTarget::Task(task_id))
        );
        let cancel_handle = CancelHandle::new(RequestId(900), RuntimeTarget::Task(task_id));
        let request_id = runtime
            .park_task(task_id, Value::Integer(9), Some(cancel_handle))
            .expect("task parks");

        runtime.request_cancel(request_id);
        runtime.poll().expect("cancel polls");
        runtime.backend_mut().push(Completion::ok(
            request_id,
            RuntimeTarget::Task(task_id),
            CompletionPayload::Unit,
        ));
        let tick = runtime.poll().expect("late completion polls");

        assert_eq!(tick.completions, 0);
        assert_eq!(runtime.pending_resumed_continuations(), 0);
        assert!(runtime.scheduler().waits().is_empty());
        assert_eq!(runtime.backend().cancelled, vec![cancel_handle]);
        assert_eq!(runtime.pop_ready(worker_id).expect("worker exists"), None);
    }
}
