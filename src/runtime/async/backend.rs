//! `AsyncBackend` trait + completion types (proposal 0174 Phase 0e).
//!
//! Phase 0 only ships the trait shape and a deterministic in-memory backend
//! ([`super::backends::in_memory`]) good enough to exercise the
//! `Suspend → completion → resume` round trip at the [`super::context`] level.
//! Phase 1a layers the real `mio` reactor, TCP state machines, timer heap,
//! and blocking pools on top of the same trait without rewriting it — that's
//! the whole point of pinning the surface now.
//!
//! Load-bearing rule (proposal 0174 §"Async backend"): the backend never
//! owns, inspects, drops, or resumes ordinary Flux heap values. It deals in
//! request IDs and copied payloads; the fiber's home worker reconstructs
//! Flux values when the completion is delivered.

/// Scheduler-issued identifier for an in-flight async request.
///
/// Allocated by the request registry, handed to the backend on submit, and
/// quoted on the completion record so the scheduler can route the resume to
/// the right fiber/task. Zero is reserved as a sentinel for "no request".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Payload delivered when a request completes.
///
/// Phase 0 keeps this small on purpose. Phase 1a extends it with the variants
/// the `mio` backend needs (`TcpHandle`, `AddressList`, …); Phase 0 only needs
/// enough variants to drive the EffectContext-level tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionPayload {
    /// Generic success carrying no value (timer fired, write flushed).
    Unit,
    /// Backend-owned byte buffer copied out for the home worker.
    Bytes(Vec<u8>),
    /// Backend-side error (DNS failed, connect refused, cancelled).
    Error(String),
}

/// A delivered completion, ready for the scheduler to route to its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub request_id: RequestId,
    pub payload: CompletionPayload,
}

/// Backend surface. Phase 0 only wires the lifecycle + cancel hooks; Phase 1a
/// adds the I/O submission methods (`tcp_connect`, `timer_start`, …).
pub trait AsyncBackend {
    /// Start the backend (spawn reactor thread, open `mio::Poll`, …). The
    /// in-memory test backend is a no-op.
    fn start(&self) -> Result<(), String>;

    /// Stop the backend and release its resources. Idempotent.
    fn shutdown(&self) -> Result<(), String>;

    /// Cancel an in-flight request. The backend MUST drop any not-yet-delivered
    /// completion for `req`; the scheduler will deliver a synthetic
    /// `CompletionPayload::Error("cancelled")` if the fiber needs to observe
    /// the cancellation, or drop the request entirely if the continuation has
    /// been abandoned.
    fn cancel(&self, req: RequestId);
}
