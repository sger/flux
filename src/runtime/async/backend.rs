//! Completion-oriented async backend contracts.
//!
//! Phase 1a will add the production `mio` implementation behind this shape.
//! The scheduler consumes completion records, not raw readiness callbacks, so
//! backend code never receives ordinary Flux heap values.

use std::{
    collections::VecDeque,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::runtime::value::Value;

use super::context::{FiberId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeTarget {
    Task(TaskId),
    Fiber(FiberId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IoHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncErrorKind {
    Cancelled,
    Closed,
    ConnectionRefused,
    InvalidInput,
    TimedOut,
    WouldBlock,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncError {
    pub kind: AsyncErrorKind,
    pub message: String,
}

impl AsyncError {
    pub fn new(kind: AsyncErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionPayload {
    Unit,
    Value(Value),
    Bytes(Vec<u8>),
    Count(usize),
    Handle(u64),
    Text(String),
    AddressList(Vec<SocketAddr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendCompletionPayload {
    Unit,
    Bytes(Vec<u8>),
    Count(usize),
    Handle(u64),
    Text(String),
    AddressList(Vec<SocketAddr>),
}

impl From<BackendCompletionPayload> for CompletionPayload {
    fn from(payload: BackendCompletionPayload) -> Self {
        match payload {
            BackendCompletionPayload::Unit => Self::Unit,
            BackendCompletionPayload::Bytes(bytes) => Self::Bytes(bytes),
            BackendCompletionPayload::Count(count) => Self::Count(count),
            BackendCompletionPayload::Handle(handle) => Self::Handle(handle),
            BackendCompletionPayload::Text(text) => Self::Text(text),
            BackendCompletionPayload::AddressList(addresses) => Self::AddressList(addresses),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub request_id: RequestId,
    pub target: RuntimeTarget,
    pub payload: Result<CompletionPayload, AsyncError>,
}

impl Completion {
    pub fn ok(request_id: RequestId, target: RuntimeTarget, payload: CompletionPayload) -> Self {
        Self {
            request_id,
            target,
            payload: Ok(payload),
        }
    }

    pub fn err(request_id: RequestId, target: RuntimeTarget, error: AsyncError) -> Self {
        Self {
            request_id,
            target,
            payload: Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCompletion {
    pub request_id: RequestId,
    pub target: RuntimeTarget,
    pub payload: Result<BackendCompletionPayload, AsyncError>,
}

impl BackendCompletion {
    pub fn ok(
        request_id: RequestId,
        target: RuntimeTarget,
        payload: BackendCompletionPayload,
    ) -> Self {
        Self {
            request_id,
            target,
            payload: Ok(payload),
        }
    }

    pub fn err(request_id: RequestId, target: RuntimeTarget, error: AsyncError) -> Self {
        Self {
            request_id,
            target,
            payload: Err(error),
        }
    }

    pub fn into_completion(self) -> Completion {
        Completion {
            request_id: self.request_id,
            target: self.target,
            payload: self.payload.map(CompletionPayload::from),
        }
    }

    pub fn try_from_completion(completion: Completion) -> Result<Self, AsyncError> {
        let payload = match completion.payload {
            Ok(CompletionPayload::Unit) => Ok(BackendCompletionPayload::Unit),
            Ok(CompletionPayload::Bytes(bytes)) => Ok(BackendCompletionPayload::Bytes(bytes)),
            Ok(CompletionPayload::Count(count)) => Ok(BackendCompletionPayload::Count(count)),
            Ok(CompletionPayload::Handle(handle)) => Ok(BackendCompletionPayload::Handle(handle)),
            Ok(CompletionPayload::Text(text)) => Ok(BackendCompletionPayload::Text(text)),
            Ok(CompletionPayload::AddressList(addresses)) => {
                Ok(BackendCompletionPayload::AddressList(addresses))
            }
            Ok(CompletionPayload::Value(_)) => {
                return Err(AsyncError::new(
                    AsyncErrorKind::InvalidInput,
                    "backend completions cannot carry Flux values",
                ));
            }
            Err(error) => Err(error),
        };
        Ok(Self {
            request_id: completion.request_id,
            target: completion.target,
            payload,
        })
    }
}

#[derive(Clone)]
pub struct BackendCompletionSink {
    queue: SharedBackendCompletionQueue,
}

#[derive(Clone)]
pub struct BackendCompletionSource {
    queue: SharedBackendCompletionQueue,
}

type SharedBackendCompletionQueue = Arc<Mutex<VecDeque<BackendCompletion>>>;

pub fn backend_completion_channel() -> (BackendCompletionSink, BackendCompletionSource) {
    let queue = Arc::new(Mutex::new(VecDeque::new()));
    (
        BackendCompletionSink {
            queue: Arc::clone(&queue),
        },
        BackendCompletionSource { queue },
    )
}

impl std::fmt::Debug for BackendCompletionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending = self.queue.lock().map(|queue| queue.len()).ok();
        f.debug_struct("BackendCompletionSink")
            .field("pending", &pending)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for BackendCompletionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending = self.queue.lock().map(|queue| queue.len()).ok();
        f.debug_struct("BackendCompletionSource")
            .field("pending", &pending)
            .finish_non_exhaustive()
    }
}

impl BackendCompletionSink {
    pub fn submit(&self, completion: BackendCompletion) -> Result<(), AsyncError> {
        self.queue
            .lock()
            .map_err(|_| {
                AsyncError::new(AsyncErrorKind::Other, "backend completion queue poisoned")
            })?
            .push_back(completion);
        Ok(())
    }
}

impl BackendCompletionSource {
    pub fn pending(&self) -> Result<usize, AsyncError> {
        Ok(self
            .queue
            .lock()
            .map_err(|_| {
                AsyncError::new(AsyncErrorKind::Other, "backend completion queue poisoned")
            })?
            .len())
    }

    pub fn poll_backend_completion(&self) -> Result<Option<BackendCompletion>, AsyncError> {
        Ok(self
            .queue
            .lock()
            .map_err(|_| {
                AsyncError::new(AsyncErrorKind::Other, "backend completion queue poisoned")
            })?
            .pop_front())
    }

    pub fn poll_completion(&self) -> Result<Option<Completion>, AsyncError> {
        Ok(self
            .poll_backend_completion()?
            .map(BackendCompletion::into_completion))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelHandle {
    request_id: RequestId,
    target: RuntimeTarget,
}

impl CancelHandle {
    pub fn new(request_id: RequestId, target: RuntimeTarget) -> Self {
        Self { request_id, target }
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub fn target(&self) -> RuntimeTarget {
        self.target
    }
}

/// Backend interface consumed by the scheduler.
///
/// This is intentionally small for Phase 0. Concrete operations such as TCP,
/// timers, DNS, and file I/O will be added with the Phase 1a `mio` backend.
pub trait AsyncBackend {
    fn poll_completion(&mut self) -> Option<Completion>;

    fn cancel(&mut self, handle: CancelHandle) -> Result<(), AsyncError>;

    fn timer_start(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        duration: Duration,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, duration);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support timers",
        ))
    }

    fn tcp_connect(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, host, port);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP connect",
        ))
    }

    fn tcp_listen(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, host, port);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP listen",
        ))
    }

    fn tcp_accept(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP accept",
        ))
    }

    fn tcp_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        max: usize,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle, max);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP read",
        ))
    }

    fn tcp_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle, bytes);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP write",
        ))
    }

    fn tcp_close(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP close",
        ))
    }

    fn tcp_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP local_addr",
        ))
    }

    fn tcp_remote_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP remote_addr",
        ))
    }

    fn tcp_close_listener(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP listener close",
        ))
    }

    fn tcp_listener_local_addr(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        handle: IoHandle,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, handle);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support TCP listener local_addr",
        ))
    }

    fn dns_resolve(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        host: String,
        port: u16,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, host, port);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support DNS resolution",
        ))
    }

    fn file_read(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, path);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support file reads",
        ))
    }

    fn file_write(
        &mut self,
        request_id: RequestId,
        target: RuntimeTarget,
        path: PathBuf,
        bytes: Vec<u8>,
    ) -> Result<CancelHandle, AsyncError> {
        let _ = (request_id, target, path, bytes);
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend does not support file writes",
        ))
    }
}

impl AsyncBackend for BackendCompletionSource {
    fn poll_completion(&mut self) -> Option<Completion> {
        self.poll_backend_completion()
            .ok()
            .flatten()
            .map(BackendCompletion::into_completion)
    }

    fn cancel(&mut self, _handle: CancelHandle) -> Result<(), AsyncError> {
        Err(AsyncError::new(
            AsyncErrorKind::InvalidInput,
            "backend completion source cannot cancel reactor requests",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_handle_keeps_request_and_target() {
        let handle = CancelHandle::new(RequestId(3), RuntimeTarget::Task(TaskId(9)));
        assert_eq!(handle.request_id(), RequestId(3));
        assert_eq!(handle.target(), RuntimeTarget::Task(TaskId(9)));
    }

    #[test]
    fn completion_records_target_and_payload() {
        let completion = Completion::ok(
            RequestId(4),
            RuntimeTarget::Fiber(super::FiberId(12)),
            CompletionPayload::Count(64),
        );
        assert_eq!(completion.request_id, RequestId(4));
        assert_eq!(completion.target, RuntimeTarget::Fiber(super::FiberId(12)));
        assert_eq!(completion.payload, Ok(CompletionPayload::Count(64)));
    }

    #[test]
    fn backend_completion_converts_to_scheduler_completion() {
        let completion = BackendCompletion::ok(
            RequestId(5),
            RuntimeTarget::Task(TaskId(1)),
            BackendCompletionPayload::Count(10),
        )
        .into_completion();

        assert_eq!(completion.request_id, RequestId(5));
        assert_eq!(completion.target, RuntimeTarget::Task(TaskId(1)));
        assert_eq!(completion.payload, Ok(CompletionPayload::Count(10)));
    }

    #[test]
    fn backend_completion_rejects_flux_value_payload() {
        let err = BackendCompletion::try_from_completion(Completion::ok(
            RequestId(6),
            RuntimeTarget::Task(TaskId(1)),
            CompletionPayload::Value(Value::Integer(1)),
        ))
        .expect_err("Flux value payload is not backend safe");

        assert_eq!(err.kind, AsyncErrorKind::InvalidInput);
    }

    #[test]
    fn backend_completion_is_send_safe() {
        fn assert_send<T: Send>() {}
        assert_send::<BackendCompletion>();
    }

    #[test]
    fn backend_completion_channel_preserves_fifo_order() {
        let (sink, source) = backend_completion_channel();
        sink.submit(BackendCompletion::ok(
            RequestId(7),
            RuntimeTarget::Task(TaskId(1)),
            BackendCompletionPayload::Count(1),
        ))
        .expect("first submit succeeds");
        sink.submit(BackendCompletion::ok(
            RequestId(8),
            RuntimeTarget::Task(TaskId(1)),
            BackendCompletionPayload::Count(2),
        ))
        .expect("second submit succeeds");

        assert_eq!(source.pending().expect("pending succeeds"), 2);
        assert_eq!(
            source
                .poll_completion()
                .expect("poll succeeds")
                .map(|completion| completion.request_id),
            Some(RequestId(7))
        );
        assert_eq!(
            source
                .poll_completion()
                .expect("poll succeeds")
                .map(|completion| completion.request_id),
            Some(RequestId(8))
        );
        assert_eq!(source.poll_completion().expect("poll succeeds"), None);
    }

    #[test]
    fn backend_completion_channel_handles_are_send_safe() {
        fn assert_send<T: Send>() {}
        assert_send::<BackendCompletionSink>();
        assert_send::<BackendCompletionSource>();
    }

    #[test]
    fn backend_completion_source_can_drive_scheduler_backend_polling() {
        let (sink, mut source) = backend_completion_channel();
        sink.submit(BackendCompletion::ok(
            RequestId(9),
            RuntimeTarget::Task(TaskId(1)),
            BackendCompletionPayload::Unit,
        ))
        .expect("submit succeeds");

        let completion = AsyncBackend::poll_completion(&mut source).expect("completion exists");
        assert_eq!(completion.request_id, RequestId(9));
        assert_eq!(completion.payload, Ok(CompletionPayload::Unit));
        assert_eq!(AsyncBackend::poll_completion(&mut source), None);
    }

    #[test]
    fn backend_completion_source_cannot_cancel_reactor_requests() {
        let (_sink, mut source) = backend_completion_channel();
        let err = source
            .cancel(CancelHandle::new(
                RequestId(10),
                RuntimeTarget::Task(TaskId(1)),
            ))
            .expect_err("source does not own reactor cancellation");

        assert_eq!(err.kind, AsyncErrorKind::InvalidInput);
    }

    #[test]
    fn backend_default_timer_reports_unsupported() {
        let (_sink, mut source) = backend_completion_channel();

        let err = source
            .timer_start(
                RequestId(11),
                RuntimeTarget::Task(TaskId(1)),
                Duration::from_millis(1),
            )
            .expect_err("completion source is not a timer backend");

        assert_eq!(err.kind, AsyncErrorKind::InvalidInput);
        assert!(err.message.contains("timers"));
    }

    #[test]
    fn backend_default_tcp_reports_unsupported() {
        let (_sink, mut source) = backend_completion_channel();

        let err = source
            .tcp_connect(
                RequestId(12),
                RuntimeTarget::Task(TaskId(1)),
                "localhost".to_string(),
                80,
            )
            .expect_err("completion source is not a TCP backend");

        assert_eq!(err.kind, AsyncErrorKind::InvalidInput);
        assert!(err.message.contains("TCP connect"));
    }
}
