//! Phase 1a `mio` backend skeleton.
//!
//! This is not the full reactor yet. It owns a `mio::Poll`, a `mio::Waker`,
//! and a completion queue so scheduler code can integrate against the real
//! readiness substrate before TCP, DNS/file pools, and reactor threads are
//! added. Timers live here because `mio` only exposes readiness polling.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use mio::{Events, Poll, Token, Waker};

use crate::runtime::r#async::backend::{
    AsyncBackend, AsyncError, AsyncErrorKind, BackendCompletion, BackendCompletionPayload,
    BackendCompletionSink, BackendCompletionSource, CancelHandle, Completion, RequestId,
    RuntimeTarget, backend_completion_channel,
};

const WAKE_TOKEN: Token = Token(0);

fn io_error(context: &str, error: std::io::Error) -> AsyncError {
    AsyncError::new(AsyncErrorKind::Other, format!("{context}: {error}"))
}

pub struct MioBackend {
    poll: Poll,
    waker: Arc<Waker>,
    events: Events,
    commands: SharedCommandQueue,
    completion_sink: BackendCompletionSink,
    completion_source: BackendCompletionSource,
    timers: BinaryHeap<Reverse<TimerEntry>>,
    cancelled: HashSet<RequestId>,
    stopped: bool,
}

type SharedCommandQueue = Arc<Mutex<VecDeque<MioCommand>>>;

#[derive(Clone)]
pub struct MioBackendHandle {
    commands: SharedCommandQueue,
    waker: Arc<Waker>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MioCommand {
    ScheduleTimer {
        request_id: RequestId,
        target: RuntimeTarget,
        deadline: Instant,
    },
    ScheduleTimerAfter {
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    },
    Cancel(CancelHandle),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MioReactorTick {
    pub commands: usize,
    pub completions: usize,
}

impl MioReactorTick {
    pub fn made_progress(self) -> bool {
        self.commands > 0 || self.completions > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MioReactorRunLimit {
    pub max_ticks: usize,
    pub timeout: Option<Duration>,
}

impl Default for MioReactorRunLimit {
    fn default() -> Self {
        Self {
            max_ticks: 1024,
            timeout: Some(Duration::from_millis(0)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MioReactorRunReport {
    pub ticks: usize,
    pub commands: usize,
    pub completions: usize,
    pub stopped: bool,
    pub idle: bool,
    pub exhausted: bool,
}

pub fn spawn_mio_reactor(
    mut backend: MioBackend,
    limit: MioReactorRunLimit,
) -> thread::JoinHandle<Result<MioReactorRunReport, AsyncError>> {
    thread::spawn(move || backend.run_reactor_until_idle(limit))
}

pub fn spawn_mio_reactor_until_stopped(
    mut backend: MioBackend,
    limit: MioReactorRunLimit,
) -> thread::JoinHandle<Result<MioReactorRunReport, AsyncError>> {
    thread::spawn(move || backend.run_reactor_until_stopped(limit))
}

#[derive(Debug, Clone)]
pub struct MioDriverBackend {
    source: BackendCompletionSource,
    handle: MioBackendHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TimerEntry {
    deadline: Instant,
    request_id: RequestId,
    target: RuntimeTarget,
}

impl std::fmt::Debug for MioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MioBackend")
            .field("pending_completions", &self.pending_completions())
            .field("pending_commands", &self.pending_commands())
            .field("pending_timers", &self.timers.len())
            .field("cancelled_requests", &self.cancelled.len())
            .finish_non_exhaustive()
    }
}

impl MioBackend {
    pub fn new() -> Result<Self, AsyncError> {
        let poll = Poll::new().map_err(|err| io_error("mio poll init failed", err))?;
        let waker = Arc::new(
            Waker::new(poll.registry(), WAKE_TOKEN)
                .map_err(|err| io_error("mio waker init failed", err))?,
        );
        let (completion_sink, completion_source) = backend_completion_channel();
        Ok(Self {
            poll,
            waker,
            events: Events::with_capacity(64),
            commands: Arc::new(Mutex::new(VecDeque::new())),
            completion_sink,
            completion_source,
            timers: BinaryHeap::new(),
            cancelled: HashSet::new(),
            stopped: false,
        })
    }

    pub fn pending_completions(&self) -> usize {
        self.completion_source
            .pending()
            .expect("mio completion queue lock is not poisoned")
    }

    pub fn completion_source(&self) -> BackendCompletionSource {
        self.completion_source.clone()
    }

    pub fn completion_sink(&self) -> BackendCompletionSink {
        self.completion_sink.clone()
    }

    pub fn driver_backend(&self) -> MioDriverBackend {
        MioDriverBackend {
            source: self.completion_source(),
            handle: self.handle(),
        }
    }

    pub fn pending_commands(&self) -> usize {
        self.commands
            .lock()
            .expect("mio command queue lock is not poisoned")
            .len()
    }

    pub fn pending_timers(&self) -> usize {
        self.timers.len()
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn submit_completion(&mut self, completion: Completion) -> Result<(), AsyncError> {
        self.submit_backend_completion(BackendCompletion::try_from_completion(completion)?)
    }

    pub fn submit_backend_completion(
        &mut self,
        completion: BackendCompletion,
    ) -> Result<(), AsyncError> {
        self.completion_sink.submit(completion)?;
        self.waker
            .wake()
            .map_err(|err| io_error("mio wake failed", err))
    }

    pub fn submit_command(&mut self, command: MioCommand) -> Result<(), AsyncError> {
        self.handle().submit_command(command)
    }

    pub fn handle(&self) -> MioBackendHandle {
        MioBackendHandle {
            commands: Arc::clone(&self.commands),
            waker: Arc::clone(&self.waker),
        }
    }

    pub fn schedule_timer_after(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        self.schedule_timer(request_id, target, Instant::now() + duration)
    }

    pub fn schedule_timer(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        deadline: Instant,
    ) -> Result<CancelHandle, AsyncError> {
        self.insert_timer(request_id, target, deadline)?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn schedule_timer_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        deadline: Instant,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::ScheduleTimer {
            request_id,
            target,
            deadline,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn schedule_timer_after_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::ScheduleTimerAfter {
            request_id,
            target,
            duration,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn poll_once(&mut self, timeout: Option<Duration>) -> Result<(), AsyncError> {
        self.poll_reactor_once(timeout)?;
        Ok(())
    }

    pub fn reactor_tick(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<MioReactorTick, AsyncError> {
        let completions_before = self.completion_source.pending()?;
        let commands = self.poll_reactor_once(timeout)?;
        Ok(MioReactorTick {
            commands,
            completions: self
                .completion_source
                .pending()?
                .saturating_sub(completions_before),
        })
    }

    pub fn run_reactor_until_idle(
        &mut self,
        limit: MioReactorRunLimit,
    ) -> Result<MioReactorRunReport, AsyncError> {
        let mut report = MioReactorRunReport::default();
        for _ in 0..limit.max_ticks {
            let tick = self.reactor_tick(limit.timeout)?;
            report.ticks += 1;
            report.commands += tick.commands;
            report.completions += tick.completions;
            if !tick.made_progress() {
                report.idle = true;
                report.stopped = self.stopped;
                return Ok(report);
            }
            if self.stopped {
                report.stopped = true;
                return Ok(report);
            }
        }
        report.exhausted = true;
        report.stopped = self.stopped;
        Ok(report)
    }

    pub fn run_reactor_until_stopped(
        &mut self,
        limit: MioReactorRunLimit,
    ) -> Result<MioReactorRunReport, AsyncError> {
        let mut report = MioReactorRunReport::default();
        for _ in 0..limit.max_ticks {
            let tick = self.reactor_tick(limit.timeout)?;
            report.ticks += 1;
            report.commands += tick.commands;
            report.completions += tick.completions;
            if self.stopped {
                report.stopped = true;
                return Ok(report);
            }
        }
        report.exhausted = true;
        report.stopped = self.stopped;
        Ok(report)
    }

    fn poll_reactor_once(&mut self, timeout: Option<Duration>) -> Result<usize, AsyncError> {
        let mut commands = self.drain_commands()?;
        self.drain_expired_timers()?;
        if self.completion_source.pending()? > 0 {
            return Ok(commands);
        }
        let timeout = self.timeout_with_next_timer(timeout);
        self.poll
            .poll(&mut self.events, timeout)
            .map_err(|err| io_error("mio poll failed", err))?;
        commands += self.drain_commands()?;
        self.drain_expired_timers()?;
        Ok(commands)
    }

    fn insert_timer(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        deadline: Instant,
    ) -> Result<(), AsyncError> {
        self.timers.push(Reverse(TimerEntry {
            deadline,
            request_id,
            target,
        }));
        self.waker
            .wake()
            .map_err(|err| io_error("mio wake failed", err))?;
        Ok(())
    }

    fn drain_commands(&mut self) -> Result<usize, AsyncError> {
        let mut commands = {
            let mut queued = self.commands.lock().map_err(|_| {
                AsyncError::new(AsyncErrorKind::Other, "mio command queue poisoned")
            })?;
            std::mem::take(&mut *queued)
        };
        let mut drained = 0;
        while let Some(command) = commands.pop_front() {
            match command {
                MioCommand::ScheduleTimer {
                    request_id,
                    target,
                    deadline,
                } => {
                    self.insert_timer(request_id, target, deadline)?;
                }
                MioCommand::ScheduleTimerAfter {
                    request_id,
                    target,
                    duration,
                } => {
                    self.insert_timer(request_id, target, Instant::now() + duration)?;
                }
                MioCommand::Cancel(handle) => {
                    self.cancelled.insert(handle.request_id());
                }
                MioCommand::Stop => {
                    self.stopped = true;
                }
            }
            drained += 1;
        }
        Ok(drained)
    }

    fn timeout_with_next_timer(&self, timeout: Option<Duration>) -> Option<Duration> {
        let timer_timeout = self
            .timers
            .peek()
            .map(|Reverse(timer)| timer.deadline.saturating_duration_since(Instant::now()));
        match (timeout, timer_timeout) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (None, Some(right)) => Some(right),
            (Some(left), None) => Some(left),
            (None, None) => None,
        }
    }

    fn drain_expired_timers(&mut self) -> Result<(), AsyncError> {
        let now = Instant::now();
        while let Some(Reverse(timer)) = self.timers.peek().copied() {
            if timer.deadline > now {
                break;
            }
            self.timers.pop();
            if self.cancelled.remove(&timer.request_id) {
                continue;
            }
            self.completion_sink.submit(BackendCompletion::ok(
                timer.request_id,
                timer.target,
                BackendCompletionPayload::Unit,
            ))?;
            self.cancelled.remove(&timer.request_id);
        }
        Ok(())
    }
}

impl std::fmt::Debug for MioBackendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending_commands = self.commands.lock().map(|commands| commands.len()).ok();
        f.debug_struct("MioBackendHandle")
            .field("pending_commands", &pending_commands)
            .finish_non_exhaustive()
    }
}

impl MioBackendHandle {
    pub fn submit_command(&self, command: MioCommand) -> Result<(), AsyncError> {
        self.commands
            .lock()
            .map_err(|_| AsyncError::new(AsyncErrorKind::Other, "mio command queue poisoned"))?
            .push_back(command);
        self.waker
            .wake()
            .map_err(|err| io_error("mio wake failed", err))
    }

    pub fn schedule_timer(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        deadline: Instant,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::ScheduleTimer {
            request_id,
            target,
            deadline,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn schedule_timer_after(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::ScheduleTimerAfter {
            request_id,
            target,
            duration,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn cancel(&self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.submit_command(MioCommand::Cancel(handle))
    }

    pub fn stop(&self) -> Result<(), AsyncError> {
        self.submit_command(MioCommand::Stop)
    }
}

impl AsyncBackend for MioDriverBackend {
    fn poll_completion(&mut self) -> Option<Completion> {
        self.source.poll_completion().ok().flatten()
    }

    fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.handle.cancel(handle)
    }
}

impl AsyncBackend for MioBackend {
    fn poll_completion(&mut self) -> Option<Completion> {
        if self.pending_completions() == 0 {
            let _ = self.poll_once(Some(Duration::from_millis(0)));
        }
        self.completion_source.poll_completion().ok().flatten()
    }

    fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError> {
        self.submit_command(MioCommand::Cancel(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn backend_starts_with_no_completions() {
        let mut backend = MioBackend::new().expect("mio backend initializes");
        assert_eq!(backend.poll_completion(), None);
    }

    #[test]
    fn backend_returns_submitted_completions_fifo() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .submit_completion(Completion::ok(
                RequestId(1),
                RuntimeTarget::Task(TaskId(1)),
                CompletionPayload::Count(10),
            ))
            .expect("wake succeeds");
        backend
            .submit_completion(Completion::ok(
                RequestId(2),
                RuntimeTarget::Task(TaskId(2)),
                CompletionPayload::Count(20),
            ))
            .expect("wake succeeds");

        assert_eq!(backend.pending_completions(), 2);
        assert_eq!(
            backend
                .poll_completion()
                .map(|completion| completion.request_id),
            Some(RequestId(1))
        );
        assert_eq!(
            backend
                .poll_completion()
                .map(|completion| completion.request_id),
            Some(RequestId(2))
        );
        assert_eq!(backend.poll_completion(), None);
    }

    #[test]
    fn timer_completion_is_ready_after_deadline() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .schedule_timer_after(
                RequestId(3),
                RuntimeTarget::Task(TaskId(3)),
                Duration::from_millis(0),
            )
            .expect("timer schedules");

        let completion = backend
            .poll_completion()
            .expect("expired timer completes immediately");
        assert_eq!(completion.request_id, RequestId(3));
        assert_eq!(completion.target, RuntimeTarget::Task(TaskId(3)));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
    }

    #[test]
    fn timer_waits_until_deadline() {
        use crate::runtime::r#async::{
            backend::{RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .schedule_timer_after(
                RequestId(4),
                RuntimeTarget::Task(TaskId(4)),
                Duration::from_secs(60),
            )
            .expect("timer schedules");

        assert_eq!(backend.pending_timers(), 1);
        assert_eq!(backend.poll_completion(), None);
        assert_eq!(backend.pending_timers(), 1);
    }

    #[test]
    fn cancelled_timer_does_not_complete() {
        use crate::runtime::r#async::{
            backend::{RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend
            .schedule_timer_after(
                RequestId(5),
                RuntimeTarget::Task(TaskId(5)),
                Duration::from_millis(0),
            )
            .expect("timer schedules");
        backend.cancel(handle).expect("timer cancels");

        assert_eq!(backend.poll_completion(), None);
        assert_eq!(backend.pending_timers(), 0);
    }

    #[test]
    fn timer_command_completes_through_command_queue() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .schedule_timer_after_command(
                RequestId(7),
                RuntimeTarget::Task(TaskId(7)),
                Duration::from_millis(0),
            )
            .expect("timer command submits");

        assert_eq!(backend.pending_commands(), 1);
        let completion = backend.poll_completion().expect("timer command completes");
        assert_eq!(completion.request_id, RequestId(7));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
        assert_eq!(backend.pending_commands(), 0);
    }

    #[test]
    fn timers_complete_in_deadline_order() {
        use crate::runtime::r#async::{
            backend::{RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let now = Instant::now();
        backend
            .schedule_timer(
                RequestId(21),
                RuntimeTarget::Task(TaskId(21)),
                now.checked_sub(Duration::from_millis(2))
                    .expect("deadline remains in range"),
            )
            .expect("first timer schedules");
        backend
            .schedule_timer(
                RequestId(22),
                RuntimeTarget::Task(TaskId(22)),
                now.checked_sub(Duration::from_millis(1))
                    .expect("deadline remains in range"),
            )
            .expect("second timer schedules");

        backend
            .reactor_tick(Some(Duration::from_millis(0)))
            .expect("reactor tick succeeds");

        assert_eq!(
            backend
                .poll_completion()
                .expect("first timer completes")
                .request_id,
            RequestId(21)
        );
        assert_eq!(
            backend
                .poll_completion()
                .expect("second timer completes")
                .request_id,
            RequestId(22)
        );
    }

    #[test]
    fn reactor_tick_drains_timer_command_without_consuming_completion() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .schedule_timer_after_command(
                RequestId(11),
                RuntimeTarget::Task(TaskId(11)),
                Duration::from_millis(0),
            )
            .expect("timer command submits");

        let tick = backend
            .reactor_tick(Some(Duration::from_millis(0)))
            .expect("reactor tick succeeds");

        assert_eq!(
            tick,
            MioReactorTick {
                commands: 1,
                completions: 1,
            }
        );
        assert_eq!(backend.pending_completions(), 1);
        let completion = backend
            .poll_completion()
            .expect("driver can consume completion later");
        assert_eq!(completion.request_id, RequestId(11));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
    }

    #[test]
    fn cloned_completion_source_observes_reactor_completion() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let source = backend.completion_source();
        backend
            .schedule_timer_after_command(
                RequestId(14),
                RuntimeTarget::Task(TaskId(14)),
                Duration::from_millis(0),
            )
            .expect("timer command submits");

        backend
            .reactor_tick(Some(Duration::from_millis(0)))
            .expect("reactor tick succeeds");

        assert_eq!(source.pending().expect("pending succeeds"), 1);
        let completion = source
            .poll_completion()
            .expect("poll succeeds")
            .expect("completion is available");
        assert_eq!(completion.request_id, RequestId(14));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
        assert_eq!(backend.pending_completions(), 0);
    }

    #[test]
    fn reactor_run_until_idle_reports_progress_then_idle() {
        use crate::runtime::r#async::{
            backend::{RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        backend
            .schedule_timer_after_command(
                RequestId(12),
                RuntimeTarget::Task(TaskId(12)),
                Duration::from_millis(0),
            )
            .expect("timer command submits");

        let report = backend
            .run_reactor_until_idle(MioReactorRunLimit {
                max_ticks: 4,
                timeout: Some(Duration::from_millis(0)),
            })
            .expect("reactor runs");

        assert_eq!(
            report,
            MioReactorRunReport {
                ticks: 2,
                commands: 1,
                completions: 1,
                stopped: false,
                idle: true,
                exhausted: false,
            }
        );
        assert_eq!(backend.pending_completions(), 1);
    }

    #[test]
    fn reactor_until_stopped_wakes_from_handle_stop() {
        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let reactor = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: 4,
                timeout: None,
            },
        );

        handle.stop().expect("stop command submits");
        let report = reactor
            .join()
            .expect("reactor thread joins")
            .expect("reactor stops");

        assert!(report.stopped);
        assert!(!report.exhausted);
        assert_eq!(report.commands, 1);
    }

    #[test]
    fn cloned_handle_submits_timer_from_another_thread() {
        use crate::runtime::r#async::{
            backend::{CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        thread::spawn(move || {
            handle
                .schedule_timer_after(
                    RequestId(8),
                    RuntimeTarget::Task(TaskId(8)),
                    Duration::from_millis(0),
                )
                .expect("timer command submits from thread");
        })
        .join()
        .expect("submission thread joins");

        let completion = backend
            .poll_completion()
            .expect("cross-thread timer completes");
        assert_eq!(completion.request_id, RequestId(8));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
        assert_eq!(backend.pending_commands(), 0);
    }

    #[test]
    fn cloned_handle_cancel_prevents_timer_completion() {
        use crate::runtime::r#async::{
            backend::{RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let target = RuntimeTarget::Task(TaskId(9));
        let cancel = handle
            .schedule_timer_after(RequestId(9), target, Duration::from_millis(0))
            .expect("timer command submits");
        let cancel_handle = handle.clone();
        thread::spawn(move || {
            cancel_handle
                .cancel(cancel)
                .expect("cancel command submits from thread");
        })
        .join()
        .expect("cancel thread joins");

        assert_eq!(backend.poll_completion(), None);
        assert_eq!(backend.pending_commands(), 0);
        assert_eq!(backend.pending_timers(), 0);
    }

    #[test]
    fn cross_thread_timer_delivery_reaches_scheduler_driver() {
        use crate::runtime::{
            r#async::{
                backend::{RequestId, RuntimeTarget},
                context::{TaskId, WorkerId},
                driver::{DriverTick, RuntimeDriver},
                scheduler::{SchedulerConfig, SchedulerState, SuspendedContinuation},
            },
            value::Value,
        };

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));

        let request_id = RequestId(10);
        let target = RuntimeTarget::Task(task_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(10),
            ))
            .expect("wait parks");

        thread::spawn(move || {
            handle
                .schedule_timer_after(request_id, target, Duration::from_millis(0))
                .expect("timer command submits from thread");
        })
        .join()
        .expect("submission thread joins");

        let mut driver = RuntimeDriver::new(scheduler, backend);
        let tick = driver.tick().expect("driver delivers completion");

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
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(target)
        );
    }

    #[test]
    fn reactor_tick_then_driver_tick_delivers_completion() {
        use crate::runtime::{
            r#async::{
                backend::{RequestId, RuntimeTarget},
                context::{TaskId, WorkerId},
                driver::{DriverTick, RuntimeDriver},
                scheduler::{SchedulerConfig, SchedulerState, SuspendedContinuation},
            },
            value::Value,
        };

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));

        let request_id = RequestId(13);
        let target = RuntimeTarget::Task(task_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(13),
            ))
            .expect("wait parks");
        handle
            .schedule_timer_after(request_id, target, Duration::from_millis(0))
            .expect("timer command submits");

        let mut driver = RuntimeDriver::new(scheduler, backend);
        let reactor_tick = driver
            .backend_mut()
            .reactor_tick(Some(Duration::from_millis(0)))
            .expect("reactor tick succeeds");
        assert_eq!(
            reactor_tick,
            MioReactorTick {
                commands: 1,
                completions: 1,
            }
        );

        let driver_tick = driver.tick().expect("driver delivers completion");
        assert_eq!(
            driver_tick,
            DriverTick {
                commands: 0,
                completions: 1,
            }
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(target)
        );
    }

    #[test]
    fn reactor_thread_publishes_completion_to_split_driver_source() {
        use crate::runtime::{
            r#async::{
                backend::{RequestId, RuntimeTarget},
                context::{TaskId, WorkerId},
                driver::{DriverTick, RuntimeDriver},
                scheduler::{SchedulerConfig, SchedulerState, SuspendedContinuation},
            },
            value::Value,
        };

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let source = backend.completion_source();
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));

        let request_id = RequestId(15);
        let target = RuntimeTarget::Task(task_id);
        scheduler
            .park(SuspendedContinuation::new(
                request_id,
                target,
                Value::Integer(15),
            ))
            .expect("wait parks");
        handle
            .schedule_timer_after(request_id, target, Duration::from_millis(0))
            .expect("timer command submits");

        let reactor = spawn_mio_reactor(
            backend,
            MioReactorRunLimit {
                max_ticks: 4,
                timeout: Some(Duration::from_millis(0)),
            },
        );
        let report = reactor
            .join()
            .expect("reactor thread joins")
            .expect("reactor runs");
        assert_eq!(
            report,
            MioReactorRunReport {
                ticks: 2,
                commands: 1,
                completions: 1,
                stopped: false,
                idle: true,
                exhausted: false,
            }
        );

        let mut driver = RuntimeDriver::new(scheduler, source);
        let tick = driver.tick().expect("driver delivers completion");
        assert_eq!(
            tick,
            DriverTick {
                commands: 0,
                completions: 1,
            }
        );
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(WorkerId(0))
                .expect("worker exists"),
            Some(target)
        );
    }

    #[test]
    fn split_driver_backend_routes_cancel_to_mio_reactor_handle() {
        use crate::runtime::{
            r#async::{
                backend::{RequestId, RuntimeTarget},
                context::{TaskId, WorkerId},
                driver::{DriverTick, RuntimeCommand, RuntimeDriver},
                scheduler::{SchedulerConfig, SchedulerState, SuspendedContinuation},
            },
            value::Value,
        };

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let driver_backend = backend.driver_backend();
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));

        let request_id = RequestId(16);
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = handle
            .schedule_timer_after(request_id, target, Duration::from_secs(60))
            .expect("timer command submits");
        scheduler
            .park(
                SuspendedContinuation::new(request_id, target, Value::Integer(16))
                    .with_cancel_handle(cancel_handle),
            )
            .expect("wait parks");

        let mut driver = RuntimeDriver::new(scheduler, driver_backend);
        driver.submit_runtime_command(RuntimeCommand::Cancel(request_id));
        let driver_tick = driver.tick().expect("driver cancels wait");
        assert_eq!(
            driver_tick,
            DriverTick {
                commands: 1,
                completions: 0,
            }
        );
        assert!(driver.scheduler().waits().is_empty());

        handle.stop().expect("stop command submits");
        let reactor = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: 4,
                timeout: Some(Duration::from_millis(0)),
            },
        );
        let report = reactor
            .join()
            .expect("reactor thread joins")
            .expect("reactor runs");

        assert!(report.stopped);
        assert_eq!(report.commands, 3);
        assert_eq!(report.completions, 0);
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
}
