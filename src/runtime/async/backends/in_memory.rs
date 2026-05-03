//! Deterministic in-memory backend (proposal 0174 Phase 0e).
//!
//! Exists to drive `Suspend → completion → resume` cycles in unit tests
//! without bringing up `mio`. Submissions queue a completion; nothing fires
//! until the test calls [`InMemoryBackend::tick`], so ordering is fully
//! observable.
//!
//! Cancellation drops any not-yet-ticked completion for the request,
//! mirroring what the `mio` backend will do once it lands: the scheduler
//! decides whether to surface the cancellation to the fiber via the request
//! registry's `Cancelled` state — the backend just stops producing the
//! completion.

use super::super::backend::{AsyncBackend, Completion, CompletionPayload, RequestId};
use std::cell::RefCell;
use std::collections::VecDeque;

/// Phase-0 backend. Single-threaded, deterministic, no I/O. Holds a FIFO of
/// pending completions; `tick()` pops one. `cancel()` removes any matching
/// pending completion. Interior mutability so the trait can take `&self`
/// (matching the `mio` backend's eventual `Arc<Self>` shape).
#[derive(Debug, Default)]
pub struct InMemoryBackend {
    pending: RefCell<VecDeque<Completion>>,
    started: RefCell<bool>,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a completion to be delivered on the next `tick`. Test-only API
    /// — `mio` synthesises completions itself from readiness events.
    pub fn enqueue(&self, request_id: RequestId, payload: CompletionPayload) {
        self.pending.borrow_mut().push_back(Completion {
            request_id,
            payload,
        });
    }

    /// Pop the next pending completion, or `None` if the queue is empty.
    pub fn tick(&self) -> Option<Completion> {
        self.pending.borrow_mut().pop_front()
    }

    /// Number of pending completions. Useful for leak assertions.
    pub fn pending_len(&self) -> usize {
        self.pending.borrow().len()
    }
}

impl AsyncBackend for InMemoryBackend {
    fn start(&self) -> Result<(), String> {
        *self.started.borrow_mut() = true;
        Ok(())
    }

    fn shutdown(&self) -> Result<(), String> {
        *self.started.borrow_mut() = false;
        self.pending.borrow_mut().clear();
        Ok(())
    }

    fn cancel(&self, req: RequestId) {
        self.pending.borrow_mut().retain(|c| c.request_id != req);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_is_idempotent() {
        let b = InMemoryBackend::new();
        assert!(b.start().is_ok());
        assert!(b.start().is_ok());
        assert!(b.shutdown().is_ok());
        assert!(b.shutdown().is_ok());
    }

    #[test]
    fn enqueue_then_tick_returns_in_fifo_order() {
        let b = InMemoryBackend::new();
        b.enqueue(RequestId(1), CompletionPayload::Unit);
        b.enqueue(RequestId(2), CompletionPayload::Bytes(vec![0xab]));
        assert_eq!(b.pending_len(), 2);

        let first = b.tick().unwrap();
        assert_eq!(first.request_id, RequestId(1));
        assert_eq!(first.payload, CompletionPayload::Unit);

        let second = b.tick().unwrap();
        assert_eq!(second.request_id, RequestId(2));
        assert_eq!(second.payload, CompletionPayload::Bytes(vec![0xab]));

        assert!(b.tick().is_none());
    }

    #[test]
    fn cancel_drops_matching_pending_completion() {
        let b = InMemoryBackend::new();
        b.enqueue(RequestId(1), CompletionPayload::Unit);
        b.enqueue(RequestId(2), CompletionPayload::Unit);
        b.enqueue(RequestId(1), CompletionPayload::Bytes(vec![0xff]));

        b.cancel(RequestId(1));
        assert_eq!(b.pending_len(), 1);

        let only = b.tick().unwrap();
        assert_eq!(only.request_id, RequestId(2));
        assert!(b.tick().is_none());
    }

    #[test]
    fn shutdown_clears_pending_completions() {
        let b = InMemoryBackend::new();
        b.enqueue(RequestId(7), CompletionPayload::Unit);
        b.shutdown().unwrap();
        assert_eq!(b.pending_len(), 0);
    }
}
