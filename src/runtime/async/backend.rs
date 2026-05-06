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

/// Backend-issued identifier for an open TCP connection (proposal 0174
/// Phase 1a-vii). Returned in `CompletionPayload::TcpHandle` from a
/// successful `tcp_connect`; later passed back to `tcp_read`, `tcp_write`,
/// and `tcp_close`. Opaque from the scheduler's perspective — only the
/// backend that issued it can resolve it to a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IoHandle(pub u64);

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
    /// Successful TCP connect — the handle is owned by the backend until
    /// `tcp_close` is called against it.
    TcpHandle(IoHandle),
}

/// A delivered completion, ready for the scheduler to route to its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub request_id: RequestId,
    pub payload: CompletionPayload,
}

/// Backend surface. Phase 0 wired lifecycle + cancel; Phase 1a-ii adds
/// the timer-submission and completion-pull hooks. Later 1a slices add
/// `tcp_connect`/`tcp_read`/`tcp_write` against the same surface.
pub trait AsyncBackend {
    /// Start the backend (spawn reactor thread, open `mio::Poll`, …).
    fn start(&self) -> Result<(), String>;

    /// Stop the backend and release its resources. Idempotent.
    fn shutdown(&self) -> Result<(), String>;

    /// Cancel an in-flight request. The backend MUST drop any not-yet-delivered
    /// completion for `req`; the scheduler will deliver a synthetic
    /// `CompletionPayload::Error("cancelled")` if the fiber needs to observe
    /// the cancellation, or drop the request entirely if the continuation has
    /// been abandoned.
    fn cancel(&self, req: RequestId);

    /// Schedule a one-shot timer for `req` to fire `ms` milliseconds from now.
    /// On expiry the backend produces a [`Completion`] with payload
    /// [`CompletionPayload::Unit`] retrievable through [`Self::next_completion`].
    fn timer_start(&self, req: RequestId, ms: u64);

    /// Pop the next ready completion, or `None` if none is available right
    /// now. Phase 1a-iii will add a blocking variant once the worker pool
    /// exists; today the scheduler/test driver polls.
    fn next_completion(&self) -> Option<Completion>;

    /// Open a TCP connection to `addr`. On success the completion carries
    /// `CompletionPayload::TcpHandle(handle)`; on failure
    /// `CompletionPayload::Error(message)`.
    ///
    /// Default impl panics — only the `mio` backend implements this today.
    /// The in-memory test backend leaves it unimplemented because there is
    /// no deterministic way to fake a real socket round-trip.
    fn tcp_connect(&self, _req: RequestId, _addr: std::net::SocketAddr) {
        panic!("AsyncBackend::tcp_connect not implemented for this backend");
    }

    /// Issue a read of up to `max` bytes against `handle`. Completion
    /// carries `Bytes(payload)` (possibly empty for EOF) or `Error`.
    fn tcp_read(&self, _req: RequestId, _handle: IoHandle, _max: usize) {
        panic!("AsyncBackend::tcp_read not implemented for this backend");
    }

    /// Issue a write of `bytes` against `handle`. Completion is `Unit` once
    /// every byte has been accepted by the OS, or `Error` on failure.
    /// Phase 1a-vii's backend retries internally on `WouldBlock`.
    fn tcp_write(&self, _req: RequestId, _handle: IoHandle, _bytes: Vec<u8>) {
        panic!("AsyncBackend::tcp_write not implemented for this backend");
    }

    /// Drop a TCP handle. In-flight reads/writes against the handle do not
    /// produce completions — the caller must cancel them first if it cares.
    fn tcp_close(&self, _handle: IoHandle) {
        panic!("AsyncBackend::tcp_close not implemented for this backend");
    }
}
