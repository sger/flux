//! Completion-oriented async backend contracts.
//!
//! Phase 1a will add the production `mio` implementation behind this shape.
//! The scheduler consumes completion records, not raw readiness callbacks, so
//! backend code never receives ordinary Flux heap values.

use crate::runtime::value::Value;

use super::context::{FiberId, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeTarget {
    Task(TaskId),
    Fiber(FiberId),
}

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
}
