//! Scheduler-owned registry of in-flight async requests (proposal 0174
//! Phase 0e).
//!
//! Maps [`RequestId`] to the per-request bookkeeping the scheduler needs to
//! route a completion: which context to resume, whether the request was
//! cancelled, whether its continuation was abandoned (the surrounding
//! scope dropped before the completion arrived).
//!
//! Phase 0 is single-threaded; Phase 1a wraps this in a `Mutex` once the
//! worker pool exists. The shape is pinned now so Phase 1a's diff is purely
//! about adding the lock, not redesigning the registry.

use super::backend::{Completion, CompletionPayload, RequestId};
use std::collections::HashMap;

/// State of a registered request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    /// Backend is working on it; completion still pending.
    InFlight,
    /// Cancelled by the scheduler. The backend will drop its pending
    /// completion; the scheduler delivers a synthetic `Error("cancelled")`
    /// if the continuation is still live, or nothing if it was abandoned.
    Cancelled,
    /// The owning fiber/scope dropped before completion. The completion,
    /// when it arrives, is discarded — the registry entry is also discarded
    /// the moment the abandonment is recorded so cleanup is prompt.
    Abandoned,
}

#[derive(Debug)]
struct Entry {
    state: RequestState,
}

/// In-memory request registry. Hands out [`RequestId`]s, tracks per-request
/// state, and routes [`Completion`]s back to live entries (dropping
/// completions for cancelled/abandoned requests).
#[derive(Debug, Default)]
pub struct RequestRegistry {
    next_id: u64,
    entries: HashMap<RequestId, Entry>,
}

impl RequestRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh `RequestId` and record it as `InFlight`. `RequestId(0)`
    /// is reserved as the "no request" sentinel and is never handed out.
    pub fn register(&mut self) -> RequestId {
        self.next_id += 1;
        let id = RequestId(self.next_id);
        self.entries.insert(
            id,
            Entry {
                state: RequestState::InFlight,
            },
        );
        id
    }

    /// Look up the state of a request. Returns `None` if the entry has been
    /// removed (completed, abandoned, or never registered).
    pub fn state(&self, req: RequestId) -> Option<RequestState> {
        self.entries.get(&req).map(|e| e.state)
    }

    /// Mark a request cancelled. The pending completion is dropped on arrival.
    /// Idempotent: cancelling a cancelled or abandoned request is a no-op.
    pub fn cancel(&mut self, req: RequestId) {
        if let Some(entry) = self.entries.get_mut(&req)
            && entry.state == RequestState::InFlight
        {
            entry.state = RequestState::Cancelled;
        }
    }

    /// Mark a request's continuation abandoned and remove the entry. Used
    /// when the surrounding fiber/scope drops before the backend completes.
    /// The backend's eventual completion will not match any live entry and
    /// will be dropped by [`Self::deliver`].
    pub fn abandon(&mut self, req: RequestId) {
        self.entries.remove(&req);
    }

    /// Deliver a completion. Returns the completion to be routed to the
    /// fiber's home worker, or `None` if the request was cancelled or
    /// abandoned (in which case the payload is silently dropped — its bytes
    /// were backend-owned, so no Flux RC bookkeeping is needed).
    ///
    /// On a cancelled request, the original payload is rewritten to the
    /// canonical `Error("cancelled")` form so consumers don't have to peer
    /// at the registry to decide what the fiber should observe.
    pub fn deliver(&mut self, completion: Completion) -> Option<Completion> {
        let entry = self.entries.remove(&completion.request_id)?;
        match entry.state {
            RequestState::InFlight => Some(completion),
            RequestState::Cancelled => Some(Completion {
                request_id: completion.request_id,
                payload: CompletionPayload::Error("cancelled".into()),
            }),
            RequestState::Abandoned => None,
        }
    }

    /// Number of live (not-yet-delivered, not-abandoned) entries. Useful for
    /// leak assertions in tests.
    pub fn live_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_yields_independent_request_ids() {
        let mut reg = RequestRegistry::new();
        let a = reg.register();
        let b = reg.register();
        assert_ne!(a, b);
        assert_ne!(a.0, 0, "RequestId(0) is reserved as a sentinel");
        assert_eq!(reg.state(a), Some(RequestState::InFlight));
        assert_eq!(reg.state(b), Some(RequestState::InFlight));
    }

    #[test]
    fn deliver_routes_in_flight_completion_unchanged() {
        let mut reg = RequestRegistry::new();
        let req = reg.register();
        let delivered = reg
            .deliver(Completion {
                request_id: req,
                payload: CompletionPayload::Bytes(vec![1, 2, 3]),
            })
            .expect("in-flight completion should route");
        assert_eq!(delivered.payload, CompletionPayload::Bytes(vec![1, 2, 3]));
        assert_eq!(reg.state(req), None, "entry consumed on delivery");
        assert_eq!(reg.live_count(), 0);
    }

    #[test]
    fn cancel_rewrites_payload_to_cancelled_error() {
        let mut reg = RequestRegistry::new();
        let req = reg.register();
        reg.cancel(req);
        assert_eq!(reg.state(req), Some(RequestState::Cancelled));

        let delivered = reg
            .deliver(Completion {
                request_id: req,
                payload: CompletionPayload::Bytes(vec![9, 9, 9]),
            })
            .expect("cancelled-but-live continuation still needs notification");
        assert_eq!(
            delivered.payload,
            CompletionPayload::Error("cancelled".into())
        );
        assert_eq!(reg.live_count(), 0);
    }

    #[test]
    fn abandon_drops_completion_silently() {
        let mut reg = RequestRegistry::new();
        let req = reg.register();
        reg.abandon(req);
        assert_eq!(reg.state(req), None);
        assert_eq!(reg.live_count(), 0);

        let delivered = reg.deliver(Completion {
            request_id: req,
            payload: CompletionPayload::Bytes(vec![7]),
        });
        assert!(
            delivered.is_none(),
            "abandoned continuation must drop the completion silently"
        );
    }

    #[test]
    fn cancel_is_idempotent_and_only_promotes_in_flight() {
        let mut reg = RequestRegistry::new();
        let req = reg.register();
        reg.cancel(req);
        reg.cancel(req);
        assert_eq!(reg.state(req), Some(RequestState::Cancelled));

        // Cancelling an already-removed (delivered) request is a no-op.
        let delivered = reg.deliver(Completion {
            request_id: req,
            payload: CompletionPayload::Unit,
        });
        assert!(delivered.is_some());
        reg.cancel(req); // no entry — no panic, no insertion
        assert_eq!(reg.state(req), None);
    }
}
