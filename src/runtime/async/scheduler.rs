//! Scheduler-side suspension registry.
//!
//! Phase 0 defines the shape that later worker threads and backends will use:
//! a Flux continuation is parked under a backend `RequestId`, then removed and
//! resumed when a completion record arrives. This module is intentionally
//! single-threaded for now; Phase 1a will wrap it in the scheduler's worker
//! ownership model.

use std::collections::HashMap;

use crate::runtime::value::Value;

use super::backend::{CancelHandle, Completion, RequestId, RuntimeTarget};

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
pub struct WaitRegistry {
    waits: HashMap<RequestId, SuspendedContinuation>,
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
}
