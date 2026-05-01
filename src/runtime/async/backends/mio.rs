//! Phase 1a `mio` backend skeleton.
//!
//! This is not the full reactor yet. It owns a `mio::Poll`, a `mio::Waker`,
//! and a completion queue so scheduler code can integrate against the real
//! readiness substrate before TCP, DNS/file pools, and reactor threads are
//! added. Timers live here because `mio` only exposes readiness polling.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
    io::{Read, Write},
    net::{Shutdown, SocketAddr, ToSocketAddrs},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::runtime::r#async::blocking::{
    BlockingRouteTable, BlockingServices, BlockingServicesConfig,
};
use mio::{
    Events, Interest, Poll, Token, Waker,
    net::{TcpListener, TcpStream},
};

use crate::runtime::r#async::backend::{
    AsyncBackend, AsyncError, AsyncErrorKind, BackendCompletion, BackendCompletionPayload,
    BackendCompletionSink, BackendCompletionSource, CancelHandle, Completion, IoHandle, RequestId,
    RuntimeTarget, backend_completion_channel,
};

const WAKE_TOKEN: Token = Token(0);
const FIRST_IO_TOKEN: usize = 1;

fn io_error(context: &str, error: std::io::Error) -> AsyncError {
    AsyncError::new(AsyncErrorKind::Other, format!("{context}: {error}"))
}

fn resolve_tcp_addr(host: &str, port: u16) -> Result<SocketAddr, AsyncError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|err| io_error("mio tcp resolve failed", err))?
        .next()
        .ok_or_else(|| {
            AsyncError::new(
                AsyncErrorKind::InvalidInput,
                format!("no TCP address resolved for {host}:{port}"),
            )
        })
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
    next_token: usize,
    next_io_handle: u64,
    tcp_streams: HashMap<IoHandle, TcpStream>,
    tcp_listeners: HashMap<IoHandle, TcpListener>,
    tcp_waits: HashMap<Token, TcpWait>,
    services: BlockingServices,
    stopped: bool,
    routes: SharedRouteTable,
}

type SharedCommandQueue = Arc<Mutex<VecDeque<MioCommand>>>;
type SharedRouteTable = BlockingRouteTable;

#[derive(Clone)]
pub struct MioBackendHandle {
    commands: SharedCommandQueue,
    waker: Arc<Waker>,
    routes: SharedRouteTable,
    sink: Option<BackendCompletionSink>,
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
    TcpConnect {
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    },
    TcpListen {
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    },
    TcpAccept {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    TcpRead {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    },
    TcpWrite {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    },
    TcpClose {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    TcpLocalAddr {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    TcpRemoteAddr {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    TcpCloseListener {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    TcpListenerLocalAddr {
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    },
    DnsResolve {
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    },
    FileRead {
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    },
    FileWrite {
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Cancel(CancelHandle),
    Stop,
}

impl MioCommand {
    fn request_id(&self) -> Option<RequestId> {
        match self {
            MioCommand::ScheduleTimer { request_id, .. }
            | MioCommand::ScheduleTimerAfter { request_id, .. }
            | MioCommand::TcpConnect { request_id, .. }
            | MioCommand::TcpListen { request_id, .. }
            | MioCommand::TcpAccept { request_id, .. }
            | MioCommand::TcpRead { request_id, .. }
            | MioCommand::TcpWrite { request_id, .. }
            | MioCommand::TcpClose { request_id, .. }
            | MioCommand::TcpLocalAddr { request_id, .. }
            | MioCommand::TcpRemoteAddr { request_id, .. }
            | MioCommand::TcpCloseListener { request_id, .. }
            | MioCommand::TcpListenerLocalAddr { request_id, .. }
            | MioCommand::DnsResolve { request_id, .. }
            | MioCommand::FileRead { request_id, .. }
            | MioCommand::FileWrite { request_id, .. } => Some(*request_id),
            MioCommand::Cancel(handle) => Some(handle.request_id()),
            MioCommand::Stop => None,
        }
    }
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

impl MioDriverBackend {
    /// Build a child driver that shares this driver's reactor but owns a
    /// fresh completion channel. Commands submitted through the child
    /// register routes back to its sink, so completions return to the
    /// child's source rather than the parent's primary sink.
    pub fn child(&self) -> Self {
        let (sink, source) = backend_completion_channel();
        Self {
            source,
            handle: self.handle.with_completion_sink(sink),
        }
    }

    pub fn handle(&self) -> &MioBackendHandle {
        &self.handle
    }

    pub fn source(&self) -> &BackendCompletionSource {
        &self.source
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TimerEntry {
    deadline: Instant,
    request_id: RequestId,
    target: RuntimeTarget,
}

#[derive(Debug)]
struct TcpWait {
    request_id: RequestId,
    target: RuntimeTarget,
    kind: TcpWaitKind,
}

#[derive(Debug)]
enum TcpWaitKind {
    Connect { stream: TcpStream },
    Accept { handle: IoHandle },
    Read { handle: IoHandle, max: usize },
    Write { handle: IoHandle, bytes: Vec<u8> },
}

#[derive(Debug, Clone, Copy)]
enum TcpAddrKind {
    Local,
    Remote,
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
        let routes: SharedRouteTable = Arc::new(Mutex::new(HashMap::new()));
        let services = BlockingServices::new(
            BlockingServicesConfig::default(),
            completion_sink.clone(),
            Arc::clone(&routes),
        );
        Ok(Self {
            poll,
            waker,
            events: Events::with_capacity(64),
            commands: Arc::new(Mutex::new(VecDeque::new())),
            completion_sink,
            completion_source,
            timers: BinaryHeap::new(),
            cancelled: HashSet::new(),
            next_token: FIRST_IO_TOKEN,
            next_io_handle: 1,
            tcp_streams: HashMap::new(),
            tcp_listeners: HashMap::new(),
            tcp_waits: HashMap::new(),
            services,
            stopped: false,
            routes,
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

    pub fn pending_tcp_waits(&self) -> usize {
        self.tcp_waits.len()
    }

    pub fn tcp_handle_count(&self) -> usize {
        self.tcp_streams.len()
    }

    pub fn tcp_listener_count(&self) -> usize {
        self.tcp_listeners.len()
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
        self.submit_routed(completion)?;
        self.waker
            .wake()
            .map_err(|err| io_error("mio wake failed", err))
    }

    fn submit_routed(&self, completion: BackendCompletion) -> Result<(), AsyncError> {
        let routed = self
            .routes
            .lock()
            .map_err(|_| AsyncError::new(AsyncErrorKind::Other, "mio route table poisoned"))?
            .remove(&completion.request_id);
        match routed {
            Some(sink) => sink.submit(completion),
            None => self.completion_sink.submit(completion),
        }
    }

    pub fn submit_command(&mut self, command: MioCommand) -> Result<(), AsyncError> {
        self.handle().submit_command(command)
    }

    pub fn handle(&self) -> MioBackendHandle {
        MioBackendHandle {
            commands: Arc::clone(&self.commands),
            waker: Arc::clone(&self.waker),
            routes: Arc::clone(&self.routes),
            sink: None,
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

    pub fn tcp_connect_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: impl Into<String>,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpConnect {
            request_id,
            target,
            host: host.into(),
            port,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn dns_resolve_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: impl Into<String>,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::DnsResolve {
            request_id,
            target,
            host: host.into(),
            port,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn file_read_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::FileRead {
            request_id,
            target,
            path,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn file_write_command(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::FileWrite {
            request_id,
            target,
            path,
            bytes,
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
        self.drain_tcp_events()?;
        commands += self.drain_commands()?;
        self.drain_expired_timers()?;
        Ok(commands)
    }

    fn allocate_token(&mut self) -> Token {
        let token = Token(self.next_token);
        self.next_token += 1;
        token
    }

    fn allocate_io_handle(&mut self) -> IoHandle {
        let handle = IoHandle(self.next_io_handle);
        self.next_io_handle += 1;
        handle
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
                MioCommand::TcpConnect {
                    request_id,
                    target,
                    host,
                    port,
                } => {
                    self.start_tcp_connect(request_id, target, host, port)?;
                }
                MioCommand::TcpListen {
                    request_id,
                    target,
                    host,
                    port,
                } => {
                    self.finish_tcp_listen(request_id, target, host, port)?;
                }
                MioCommand::TcpAccept {
                    request_id,
                    target,
                    handle,
                } => {
                    self.start_tcp_accept(request_id, target, handle)?;
                }
                MioCommand::TcpRead {
                    request_id,
                    target,
                    handle,
                    max,
                } => {
                    self.start_tcp_read(request_id, target, handle, max)?;
                }
                MioCommand::TcpWrite {
                    request_id,
                    target,
                    handle,
                    bytes,
                } => {
                    self.start_tcp_write(request_id, target, handle, bytes)?;
                }
                MioCommand::TcpClose {
                    request_id,
                    target,
                    handle,
                } => {
                    self.finish_tcp_close(request_id, target, handle)?;
                }
                MioCommand::TcpLocalAddr {
                    request_id,
                    target,
                    handle,
                } => {
                    self.finish_tcp_addr(request_id, target, handle, TcpAddrKind::Local)?;
                }
                MioCommand::TcpRemoteAddr {
                    request_id,
                    target,
                    handle,
                } => {
                    self.finish_tcp_addr(request_id, target, handle, TcpAddrKind::Remote)?;
                }
                MioCommand::TcpCloseListener {
                    request_id,
                    target,
                    handle,
                } => {
                    self.finish_tcp_close_listener(request_id, target, handle)?;
                }
                MioCommand::TcpListenerLocalAddr {
                    request_id,
                    target,
                    handle,
                } => {
                    self.finish_tcp_listener_addr(request_id, target, handle)?;
                }
                MioCommand::DnsResolve {
                    request_id,
                    target,
                    host,
                    port,
                } => {
                    self.services.resolve_dns(request_id, target, host, port)?;
                }
                MioCommand::FileRead {
                    request_id,
                    target,
                    path,
                } => {
                    self.services.read_file(request_id, target, path)?;
                }
                MioCommand::FileWrite {
                    request_id,
                    target,
                    path,
                    bytes,
                } => {
                    self.services.write_file(request_id, target, path, bytes)?;
                }
                MioCommand::Cancel(handle) => {
                    self.cancelled.insert(handle.request_id());
                    self.services.cancel(handle)?;
                }
                MioCommand::Stop => {
                    self.stopped = true;
                }
            }
            drained += 1;
        }
        Ok(drained)
    }

    fn start_tcp_connect(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<(), AsyncError> {
        let addr = resolve_tcp_addr(&host, port)?;
        let mut stream =
            TcpStream::connect(addr).map_err(|err| io_error("mio tcp connect failed", err))?;
        let token = self.allocate_token();
        self.poll
            .registry()
            .register(&mut stream, token, Interest::WRITABLE)
            .map_err(|err| io_error("mio tcp connect register failed", err))?;
        self.tcp_waits.insert(
            token,
            TcpWait {
                request_id,
                target,
                kind: TcpWaitKind::Connect { stream },
            },
        );
        Ok(())
    }

    fn finish_tcp_listen(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<(), AsyncError> {
        let addr = resolve_tcp_addr(&host, port)?;
        let listener =
            TcpListener::bind(addr).map_err(|err| io_error("mio tcp listen failed", err))?;
        let handle = self.allocate_io_handle();
        self.tcp_listeners.insert(handle, listener);
        self.submit_routed(BackendCompletion::ok(
            request_id,
            target,
            BackendCompletionPayload::Handle(handle.0),
        ))?;
        Ok(())
    }

    fn start_tcp_accept(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<(), AsyncError> {
        let Some(listener) = self.tcp_listeners.get_mut(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP listener handle"),
            ))?;
            return Ok(());
        };
        match listener.accept() {
            Ok((stream, _addr)) => {
                let stream_handle = self.allocate_io_handle();
                self.tcp_streams.insert(stream_handle, stream);
                self.submit_routed(BackendCompletion::ok(
                    request_id,
                    target,
                    BackendCompletionPayload::Handle(stream_handle.0),
                ))?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                let token = self.allocate_token();
                let listener = self
                    .tcp_listeners
                    .get_mut(&handle)
                    .expect("listener presence checked above");
                self.poll
                    .registry()
                    .register(listener, token, Interest::READABLE)
                    .map_err(|err| io_error("mio tcp accept register failed", err))?;
                self.tcp_waits.insert(
                    token,
                    TcpWait {
                        request_id,
                        target,
                        kind: TcpWaitKind::Accept { handle },
                    },
                );
            }
            Err(err) => {
                self.submit_routed(BackendCompletion::err(
                    request_id,
                    target,
                    io_error("mio tcp accept failed", err),
                ))?;
            }
        }
        Ok(())
    }

    fn start_tcp_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    ) -> Result<(), AsyncError> {
        if max == 0 {
            self.submit_routed(BackendCompletion::ok(
                request_id,
                target,
                BackendCompletionPayload::Bytes(Vec::new()),
            ))?;
            return Ok(());
        }
        let Some(stream) = self.tcp_streams.get_mut(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP handle"),
            ))?;
            return Ok(());
        };
        let mut buf = vec![0; max];
        match stream.read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                self.submit_routed(BackendCompletion::ok(
                    request_id,
                    target,
                    BackendCompletionPayload::Bytes(buf),
                ))?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                let token = self.allocate_token();
                let stream = self
                    .tcp_streams
                    .get_mut(&handle)
                    .expect("handle presence checked above");
                self.poll
                    .registry()
                    .register(stream, token, Interest::READABLE)
                    .map_err(|err| io_error("mio tcp read register failed", err))?;
                self.tcp_waits.insert(
                    token,
                    TcpWait {
                        request_id,
                        target,
                        kind: TcpWaitKind::Read { handle, max },
                    },
                );
            }
            Err(err) => {
                self.submit_routed(BackendCompletion::err(
                    request_id,
                    target,
                    io_error("mio tcp read failed", err),
                ))?;
            }
        }
        Ok(())
    }

    fn start_tcp_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<(), AsyncError> {
        let Some(stream) = self.tcp_streams.get_mut(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP handle"),
            ))?;
            return Ok(());
        };
        match stream.write(&bytes) {
            Ok(n) => {
                self.submit_routed(BackendCompletion::ok(
                    request_id,
                    target,
                    BackendCompletionPayload::Count(n),
                ))?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                let token = self.allocate_token();
                let stream = self
                    .tcp_streams
                    .get_mut(&handle)
                    .expect("handle presence checked above");
                self.poll
                    .registry()
                    .register(stream, token, Interest::WRITABLE)
                    .map_err(|err| io_error("mio tcp write register failed", err))?;
                self.tcp_waits.insert(
                    token,
                    TcpWait {
                        request_id,
                        target,
                        kind: TcpWaitKind::Write { handle, bytes },
                    },
                );
            }
            Err(err) => {
                self.submit_routed(BackendCompletion::err(
                    request_id,
                    target,
                    io_error("mio tcp write failed", err),
                ))?;
            }
        }
        Ok(())
    }

    fn finish_tcp_close(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<(), AsyncError> {
        let Some(stream) = self.tcp_streams.remove(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP handle"),
            ))?;
            return Ok(());
        };
        let _ = stream.shutdown(Shutdown::Both);
        self.submit_routed(BackendCompletion::ok(
            request_id,
            target,
            BackendCompletionPayload::Unit,
        ))?;
        Ok(())
    }

    fn finish_tcp_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        kind: TcpAddrKind,
    ) -> Result<(), AsyncError> {
        let Some(stream) = self.tcp_streams.get(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP handle"),
            ))?;
            return Ok(());
        };
        let addr = match kind {
            TcpAddrKind::Local => stream
                .local_addr()
                .map_err(|err| io_error("mio tcp local_addr failed", err)),
            TcpAddrKind::Remote => stream
                .peer_addr()
                .map_err(|err| io_error("mio tcp remote_addr failed", err)),
        };
        match addr {
            Ok(addr) => self.submit_routed(BackendCompletion::ok(
                request_id,
                target,
                BackendCompletionPayload::Text(addr.to_string()),
            ))?,
            Err(error) => {
                self.completion_sink
                    .submit(BackendCompletion::err(request_id, target, error))?;
            }
        }
        Ok(())
    }

    fn finish_tcp_close_listener(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<(), AsyncError> {
        if self.tcp_listeners.remove(&handle).is_none() {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP listener handle"),
            ))?;
            return Ok(());
        }
        self.submit_routed(BackendCompletion::ok(
            request_id,
            target,
            BackendCompletionPayload::Unit,
        ))?;
        Ok(())
    }

    fn finish_tcp_listener_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<(), AsyncError> {
        let Some(listener) = self.tcp_listeners.get(&handle) else {
            self.submit_routed(BackendCompletion::err(
                request_id,
                target,
                AsyncError::new(AsyncErrorKind::InvalidInput, "unknown TCP listener handle"),
            ))?;
            return Ok(());
        };
        match listener
            .local_addr()
            .map_err(|err| io_error("mio tcp listener local_addr failed", err))
        {
            Ok(addr) => self.submit_routed(BackendCompletion::ok(
                request_id,
                target,
                BackendCompletionPayload::Text(addr.to_string()),
            ))?,
            Err(error) => {
                self.completion_sink
                    .submit(BackendCompletion::err(request_id, target, error))?;
            }
        }
        Ok(())
    }

    fn drain_tcp_events(&mut self) -> Result<(), AsyncError> {
        let tokens: Vec<Token> = self
            .events
            .iter()
            .filter_map(|event| {
                let token = event.token();
                (token != WAKE_TOKEN).then_some(token)
            })
            .collect();
        for token in tokens {
            let Some(wait) = self.tcp_waits.remove(&token) else {
                continue;
            };
            if self.cancelled.remove(&wait.request_id) {
                self.drop_tcp_wait(wait)?;
                continue;
            }
            self.complete_tcp_wait(token, wait)?;
        }
        Ok(())
    }

    fn drop_tcp_wait(&mut self, wait: TcpWait) -> Result<(), AsyncError> {
        match wait.kind {
            TcpWaitKind::Connect { mut stream } => self
                .poll
                .registry()
                .deregister(&mut stream)
                .map_err(|err| io_error("mio tcp connect deregister failed", err)),
            TcpWaitKind::Accept { handle } => {
                if let Some(listener) = self.tcp_listeners.get_mut(&handle) {
                    let _ = self.poll.registry().deregister(listener);
                }
                Ok(())
            }
            TcpWaitKind::Read { handle, .. } | TcpWaitKind::Write { handle, .. } => {
                if let Some(stream) = self.tcp_streams.get_mut(&handle) {
                    let _ = self.poll.registry().deregister(stream);
                }
                Ok(())
            }
        }
    }

    fn complete_tcp_wait(&mut self, token: Token, wait: TcpWait) -> Result<(), AsyncError> {
        let request_id = wait.request_id;
        match wait.kind {
            TcpWaitKind::Connect { mut stream } => {
                self.poll
                    .registry()
                    .deregister(&mut stream)
                    .map_err(|err| io_error("mio tcp connect deregister failed", err))?;
                match stream
                    .take_error()
                    .map_err(|err| io_error("mio tcp connect status failed", err))?
                {
                    Some(err) => self.submit_routed(BackendCompletion::err(
                        wait.request_id,
                        wait.target,
                        io_error("mio tcp connect failed", err),
                    ))?,
                    None => {
                        let handle = self.allocate_io_handle();
                        self.tcp_streams.insert(handle, stream);
                        self.submit_routed(BackendCompletion::ok(
                            wait.request_id,
                            wait.target,
                            BackendCompletionPayload::Handle(handle.0),
                        ))?;
                    }
                }
            }
            TcpWaitKind::Accept { handle } => {
                if let Some(listener) = self.tcp_listeners.get_mut(&handle) {
                    self.poll
                        .registry()
                        .deregister(listener)
                        .map_err(|err| io_error("mio tcp accept deregister failed", err))?;
                }
                self.start_tcp_accept(wait.request_id, wait.target, handle)?;
            }
            TcpWaitKind::Read { handle, max } => {
                if let Some(stream) = self.tcp_streams.get_mut(&handle) {
                    self.poll
                        .registry()
                        .deregister(stream)
                        .map_err(|err| io_error("mio tcp read deregister failed", err))?;
                }
                self.start_tcp_read(wait.request_id, wait.target, handle, max)?;
            }
            TcpWaitKind::Write { handle, bytes } => {
                if let Some(stream) = self.tcp_streams.get_mut(&handle) {
                    self.poll
                        .registry()
                        .deregister(stream)
                        .map_err(|err| io_error("mio tcp write deregister failed", err))?;
                }
                self.start_tcp_write(wait.request_id, wait.target, handle, bytes)?;
            }
        }
        self.cancelled.remove(&request_id);
        self.tcp_waits.remove(&token);
        Ok(())
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
            self.submit_routed(BackendCompletion::ok(
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
    pub fn with_completion_sink(&self, sink: BackendCompletionSink) -> Self {
        Self {
            commands: Arc::clone(&self.commands),
            waker: Arc::clone(&self.waker),
            routes: Arc::clone(&self.routes),
            sink: Some(sink),
        }
    }

    pub fn submit_command(&self, command: MioCommand) -> Result<(), AsyncError> {
        if let (Some(sink), Some(request_id)) = (self.sink.as_ref(), command.request_id()) {
            self.routes
                .lock()
                .map_err(|_| AsyncError::new(AsyncErrorKind::Other, "mio route table poisoned"))?
                .insert(request_id, sink.clone());
        }
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

    pub fn tcp_connect(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: impl Into<String>,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpConnect {
            request_id,
            target,
            host: host.into(),
            port,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_listen(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: impl Into<String>,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpListen {
            request_id,
            target,
            host: host.into(),
            port,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_accept(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpAccept {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_read(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpRead {
            request_id,
            target,
            handle,
            max,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_write(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpWrite {
            request_id,
            target,
            handle,
            bytes,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_close(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpClose {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_local_addr(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpLocalAddr {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_remote_addr(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpRemoteAddr {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_close_listener(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpCloseListener {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn tcp_listener_local_addr(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::TcpListenerLocalAddr {
            request_id,
            target,
            handle,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn dns_resolve(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: impl Into<String>,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::DnsResolve {
            request_id,
            target,
            host: host.into(),
            port,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn file_read(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::FileRead {
            request_id,
            target,
            path,
        })?;
        Ok(CancelHandle::new(request_id, target))
    }

    pub fn file_write(
        &self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.submit_command(MioCommand::FileWrite {
            request_id,
            target,
            path,
            bytes,
        })?;
        Ok(CancelHandle::new(request_id, target))
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

    fn timer_start(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle
            .schedule_timer_after(request_id, target, duration)
    }

    fn tcp_connect(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_connect(request_id, target, host, port)
    }

    fn tcp_listen(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_listen(request_id, target, host, port)
    }

    fn tcp_accept(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_accept(request_id, target, handle)
    }

    fn tcp_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_read(request_id, target, handle, max)
    }

    fn tcp_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_write(request_id, target, handle, bytes)
    }

    fn tcp_close(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_close(request_id, target, handle)
    }

    fn tcp_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_local_addr(request_id, target, handle)
    }

    fn tcp_remote_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_remote_addr(request_id, target, handle)
    }

    fn tcp_close_listener(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.tcp_close_listener(request_id, target, handle)
    }

    fn tcp_listener_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle
            .tcp_listener_local_addr(request_id, target, handle)
    }

    fn dns_resolve(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.dns_resolve(request_id, target, host, port)
    }

    fn file_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.file_read(request_id, target, path)
    }

    fn file_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.handle.file_write(request_id, target, path, bytes)
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
        self.cancelled.insert(handle.request_id());
        self.services.cancel(handle)
    }

    fn timer_start(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        self.schedule_timer_after(request_id, target, duration)
    }

    fn tcp_connect(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.start_tcp_connect(request_id, target, host, port)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_listen(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_listen(request_id, target, host, port)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_accept(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.start_tcp_accept(request_id, target, handle)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    ) -> Result<CancelHandle, AsyncError> {
        self.start_tcp_read(request_id, target, handle, max)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.start_tcp_write(request_id, target, handle, bytes)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_close(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_close(request_id, target, handle)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_addr(request_id, target, handle, TcpAddrKind::Local)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_remote_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_addr(request_id, target, handle, TcpAddrKind::Remote)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_close_listener(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_close_listener(request_id, target, handle)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn tcp_listener_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        self.finish_tcp_listener_addr(request_id, target, handle)?;
        Ok(CancelHandle::new(request_id, target))
    }

    fn dns_resolve(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        self.services.resolve_dns(request_id, target, host, port)
    }

    fn file_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        self.services.read_file(request_id, target, path)
    }

    fn file_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        self.services.write_file(request_id, target, path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::ErrorKind,
        net::TcpListener,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("flux_mio_{name}_{unique}"))
    }

    fn poll_backend_completion(source: &BackendCompletionSource) -> BackendCompletion {
        for _ in 0..200 {
            if let Some(completion) = source
                .poll_backend_completion()
                .expect("completion source readable")
            {
                return completion;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for routed backend completion")
    }

    fn poll_until_completion(backend: &mut MioBackend) -> Completion {
        for _ in 0..100 {
            if let Some(completion) = backend.poll_completion() {
                return completion;
            }
            backend
                .reactor_tick(Some(Duration::from_millis(10)))
                .expect("reactor tick succeeds");
        }
        panic!("timed out waiting for mio completion")
    }

    fn bind_loopback_or_skip() -> Option<TcpListener> {
        match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => Some(listener),
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                eprintln!("skipping loopback TCP test: {err}");
                None
            }
            Err(err) => panic!("bind loopback listener: {err}"),
        }
    }

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
    fn async_backend_timer_start_schedules_timer() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let handle = AsyncBackend::timer_start(
            &mut backend,
            RequestId(29),
            RuntimeTarget::Task(TaskId(29)),
            Duration::from_millis(0),
        )
        .expect("timer schedules through trait");

        assert_eq!(handle.request_id(), RequestId(29));
        let completion = backend
            .poll_completion()
            .expect("expired timer completes immediately");
        assert_eq!(completion.request_id, RequestId(29));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
    }

    #[test]
    fn tcp_connect_completes_with_opaque_handle() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let Some(listener) = bind_loopback_or_skip() else {
            return;
        };
        let addr = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (_stream, _addr) = listener.accept().expect("accept client");
        });

        let mut backend = MioBackend::new().expect("mio backend initializes");
        AsyncBackend::tcp_connect(
            &mut backend,
            RequestId(30),
            RuntimeTarget::Task(TaskId(30)),
            addr.ip().to_string(),
            addr.port(),
        )
        .expect("connect schedules");

        let completion = poll_until_completion(&mut backend);
        assert_eq!(completion.request_id, RequestId(30));
        assert!(matches!(
            completion.payload,
            Ok(CompletionPayload::Handle(handle)) if handle > 0
        ));
        assert_eq!(backend.tcp_handle_count(), 1);
        server.join().expect("server thread joins");
    }

    #[test]
    fn tcp_write_then_read_roundtrips_loopback_bytes() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, IoHandle, RequestId, RuntimeTarget},
            context::TaskId,
        };
        use std::io::{Read, Write};

        let Some(listener) = bind_loopback_or_skip() else {
            return;
        };
        let addr = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut stream, _addr) = listener.accept().expect("accept client");
            let mut buf = [0; 4];
            stream.read_exact(&mut buf).expect("server reads ping");
            assert_eq!(&buf, b"ping");
            stream.write_all(b"pong").expect("server writes pong");
        });

        let mut backend = MioBackend::new().expect("mio backend initializes");
        AsyncBackend::tcp_connect(
            &mut backend,
            RequestId(31),
            RuntimeTarget::Task(TaskId(31)),
            addr.ip().to_string(),
            addr.port(),
        )
        .expect("connect schedules");
        let connect = poll_until_completion(&mut backend);
        let handle = match connect.payload {
            Ok(CompletionPayload::Handle(handle)) => IoHandle(handle),
            other => panic!("expected TCP handle, got {other:?}"),
        };

        AsyncBackend::tcp_write(
            &mut backend,
            RequestId(32),
            RuntimeTarget::Task(TaskId(32)),
            handle,
            b"ping".to_vec(),
        )
        .expect("write schedules");
        let write = poll_until_completion(&mut backend);
        assert_eq!(write.payload, Ok(CompletionPayload::Count(4)));

        AsyncBackend::tcp_read(
            &mut backend,
            RequestId(33),
            RuntimeTarget::Task(TaskId(33)),
            handle,
            4,
        )
        .expect("read schedules");
        let read = poll_until_completion(&mut backend);
        assert_eq!(read.payload, Ok(CompletionPayload::Bytes(b"pong".to_vec())));

        server.join().expect("server thread joins");
    }

    #[test]
    fn tcp_addr_and_close_complete_for_connected_handle() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, IoHandle, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let Some(listener) = bind_loopback_or_skip() else {
            return;
        };
        let addr = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (_stream, _addr) = listener.accept().expect("accept client");
            thread::sleep(Duration::from_millis(50));
        });

        let mut backend = MioBackend::new().expect("mio backend initializes");
        AsyncBackend::tcp_connect(
            &mut backend,
            RequestId(41),
            RuntimeTarget::Task(TaskId(41)),
            addr.ip().to_string(),
            addr.port(),
        )
        .expect("connect schedules");
        let connect = poll_until_completion(&mut backend);
        let handle = match connect.payload {
            Ok(CompletionPayload::Handle(handle)) => IoHandle(handle),
            other => panic!("expected TCP handle, got {other:?}"),
        };

        AsyncBackend::tcp_local_addr(
            &mut backend,
            RequestId(42),
            RuntimeTarget::Task(TaskId(42)),
            handle,
        )
        .expect("local_addr schedules");
        let local = poll_until_completion(&mut backend);
        assert!(matches!(local.payload, Ok(CompletionPayload::Text(text)) if text.contains(':')));

        AsyncBackend::tcp_remote_addr(
            &mut backend,
            RequestId(43),
            RuntimeTarget::Task(TaskId(43)),
            handle,
        )
        .expect("remote_addr schedules");
        let remote = poll_until_completion(&mut backend);
        assert_eq!(
            remote.payload,
            Ok(CompletionPayload::Text(addr.to_string()))
        );

        AsyncBackend::tcp_close(
            &mut backend,
            RequestId(44),
            RuntimeTarget::Task(TaskId(44)),
            handle,
        )
        .expect("close schedules");
        let close = poll_until_completion(&mut backend);
        assert_eq!(close.payload, Ok(CompletionPayload::Unit));
        assert_eq!(backend.tcp_handle_count(), 0);

        server.join().expect("server thread joins");
    }

    #[test]
    fn tcp_listen_accept_and_close_listener_complete() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, IoHandle, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        let listen_result = AsyncBackend::tcp_listen(
            &mut backend,
            RequestId(45),
            RuntimeTarget::Task(TaskId(45)),
            "127.0.0.1".to_string(),
            0,
        );
        if listen_result
            .as_ref()
            .is_err_and(|err| err.message.contains("Operation not permitted"))
        {
            return;
        }
        listen_result.expect("listen schedules");
        let listen = poll_until_completion(&mut backend);
        let listener_handle = match listen.payload {
            Ok(CompletionPayload::Handle(handle)) => IoHandle(handle),
            other => panic!("expected listener handle, got {other:?}"),
        };
        assert_eq!(backend.tcp_listener_count(), 1);

        AsyncBackend::tcp_listener_local_addr(
            &mut backend,
            RequestId(46),
            RuntimeTarget::Task(TaskId(46)),
            listener_handle,
        )
        .expect("listener addr schedules");
        let listener_addr = poll_until_completion(&mut backend);
        let addr = match listener_addr.payload {
            Ok(CompletionPayload::Text(text)) => text,
            other => panic!("expected listener addr, got {other:?}"),
        };

        let client = thread::spawn(move || {
            std::net::TcpStream::connect(addr).expect("client connects");
        });

        AsyncBackend::tcp_accept(
            &mut backend,
            RequestId(47),
            RuntimeTarget::Task(TaskId(47)),
            listener_handle,
        )
        .expect("accept schedules");
        let accepted = poll_until_completion(&mut backend);
        assert!(matches!(
            accepted.payload,
            Ok(CompletionPayload::Handle(handle)) if handle > 0
        ));
        client.join().expect("client thread joins");

        AsyncBackend::tcp_close_listener(
            &mut backend,
            RequestId(48),
            RuntimeTarget::Task(TaskId(48)),
            listener_handle,
        )
        .expect("listener close schedules");
        let close = poll_until_completion(&mut backend);
        assert_eq!(close.payload, Ok(CompletionPayload::Unit));
        assert_eq!(backend.tcp_listener_count(), 0);
    }

    #[test]
    fn file_service_completes_through_mio_backend() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let path = temp_path("file_service");
        let mut backend = MioBackend::new().expect("mio backend initializes");
        AsyncBackend::file_write(
            &mut backend,
            RequestId(34),
            RuntimeTarget::Task(TaskId(34)),
            path.clone(),
            b"phase1".to_vec(),
        )
        .expect("file write schedules");
        let write = poll_until_completion(&mut backend);
        assert_eq!(write.payload, Ok(CompletionPayload::Count(6)));

        AsyncBackend::file_read(
            &mut backend,
            RequestId(35),
            RuntimeTarget::Task(TaskId(35)),
            path.clone(),
        )
        .expect("file read schedules");
        let read = poll_until_completion(&mut backend);
        assert_eq!(
            read.payload,
            Ok(CompletionPayload::Bytes(b"phase1".to_vec()))
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn dns_service_completes_through_mio_backend() {
        use crate::runtime::r#async::{
            backend::{AsyncBackend, CompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let mut backend = MioBackend::new().expect("mio backend initializes");
        AsyncBackend::dns_resolve(
            &mut backend,
            RequestId(36),
            RuntimeTarget::Task(TaskId(36)),
            "localhost".to_string(),
            80,
        )
        .expect("dns resolve schedules");
        let completion = poll_until_completion(&mut backend);
        match completion.payload {
            Ok(CompletionPayload::AddressList(addresses)) => assert!(!addresses.is_empty()),
            other => panic!("expected address list, got {other:?}"),
        }
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

    #[test]
    fn split_driver_backend_delivers_tcp_listen_completion_to_scheduler() {
        use crate::runtime::{
            r#async::{
                backend::{CompletionPayload, RequestId, RuntimeTarget},
                context::{TaskId, WorkerId},
                driver::RuntimeDriver,
                scheduler::{SchedulerConfig, SchedulerState, SuspendedContinuation},
            },
            value::Value,
        };

        if bind_loopback_or_skip().is_none() {
            return;
        }

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let mut driver_backend = backend.driver_backend();
        let mut scheduler = SchedulerState::new(SchedulerConfig { worker_count: 1 });
        let (task_id, worker_id) = scheduler.spawn_task().expect("task spawns");
        assert_eq!((task_id, worker_id), (TaskId(1), WorkerId(0)));

        let request_id = RequestId(17);
        let target = RuntimeTarget::Task(task_id);
        let cancel_handle = driver_backend
            .tcp_listen(request_id, target, "127.0.0.1".to_string(), 0)
            .expect("tcp listen command submits");
        scheduler
            .park(
                SuspendedContinuation::new(request_id, target, Value::Integer(17))
                    .with_cancel_handle(cancel_handle),
            )
            .expect("wait parks");

        let reactor = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: usize::MAX,
                timeout: Some(Duration::from_millis(10)),
            },
        );
        let mut driver = RuntimeDriver::new(scheduler, driver_backend);
        let mut resumed = None;
        for _ in 0..100 {
            driver.tick().expect("driver ticks");
            if let Some(continuation) = driver.pop_completed_continuation() {
                resumed = Some(continuation);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        handle.stop().expect("stop command submits");
        let report = reactor
            .join()
            .expect("reactor thread joins")
            .expect("reactor runs");
        assert!(report.stopped);

        let resumed = resumed.expect("tcp listen completion resumes continuation");
        assert_eq!(resumed.request_id, request_id);
        assert_eq!(resumed.target, target);
        match resumed.completion.expect("completion payload exists") {
            Ok(CompletionPayload::Handle(handle)) => assert!(handle > 0),
            other => panic!("expected TCP listener handle, got {other:?}"),
        }
        assert_eq!(
            driver
                .scheduler_mut()
                .pop_ready(worker_id)
                .expect("worker exists"),
            Some(target)
        );
    }

    #[test]
    fn child_driver_routes_dns_completion_to_child_source() {
        use crate::runtime::r#async::{
            backend::{BackendCompletion, BackendCompletionPayload, RequestId, RuntimeTarget},
            context::TaskId,
        };

        let backend = MioBackend::new().expect("mio backend initializes");
        let handle = backend.handle();
        let parent_driver = backend.driver_backend();
        let child_driver = parent_driver.child();

        let parent_request = RequestId(901);
        let child_request = RequestId(902);
        let parent_target = RuntimeTarget::Task(TaskId(1));
        let child_target = RuntimeTarget::Task(TaskId(2));

        parent_driver
            .handle()
            .submit_command(MioCommand::DnsResolve {
                request_id: parent_request,
                target: parent_target,
                host: "localhost".to_string(),
                port: 80,
            })
            .expect("parent dns command submits");
        child_driver
            .handle()
            .submit_command(MioCommand::DnsResolve {
                request_id: child_request,
                target: child_target,
                host: "localhost".to_string(),
                port: 80,
            })
            .expect("child dns command submits");

        let reactor = spawn_mio_reactor_until_stopped(
            backend,
            MioReactorRunLimit {
                max_ticks: usize::MAX,
                timeout: Some(Duration::from_millis(10)),
            },
        );

        let parent_completion: BackendCompletion = poll_backend_completion(parent_driver.source());
        let child_completion: BackendCompletion = poll_backend_completion(child_driver.source());

        handle.stop().expect("stop command submits");
        let report = reactor
            .join()
            .expect("reactor thread joins")
            .expect("reactor runs");
        assert!(report.stopped);

        assert_eq!(parent_completion.request_id, parent_request);
        match parent_completion.payload {
            Ok(BackendCompletionPayload::AddressList(addresses)) => {
                assert!(
                    !addresses.is_empty(),
                    "parent dns resolves at least one address"
                );
            }
            other => panic!("expected parent address list, got {other:?}"),
        }

        assert_eq!(child_completion.request_id, child_request);
        match child_completion.payload {
            Ok(BackendCompletionPayload::AddressList(addresses)) => {
                assert!(
                    !addresses.is_empty(),
                    "child dns resolves at least one address"
                );
            }
            other => panic!("expected child address list, got {other:?}"),
        }
    }
}
