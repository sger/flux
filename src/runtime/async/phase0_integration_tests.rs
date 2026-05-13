//! Phase 0e integration tests for the `Suspend → completion → resume`
//! contract (proposal 0174 §"Phase 0 deliverables").
//!
//! Each test wires together the four Phase-0 pieces — [`EffectContext`],
//! [`AsyncBackend`], [`RequestRegistry`], and a hand-driven scheduler tick —
//! and exercises one of the proposal's mandated invariants:
//!
//! - Two concurrently-suspended effects can coexist without overwriting each
//!   other's yield payload, evidence vector, or resume state.
//! - Cancellation before completion delivers a synthetic `cancelled` error to
//!   the still-live continuation rather than the original payload.
//! - Cleanup on abandoned continuation: the registry drops the entry, the
//!   backend's pending completion is discarded, and no leaks remain.
//!
//! The "scheduler tick" here is intentionally trivial — a free function that
//! pulls one completion off the backend, hands it to the registry, and (when
//! the request was live) hands the resulting payload to the home context.
//! Phase 1a's tick will lock the registry and route via worker queues; the
//! Phase 0 version exists to verify the data model alone is sound.

use super::backend::{AsyncBackend, Completion, CompletionPayload, RequestId};
use super::backends::in_memory::InMemoryBackend;
use super::context::{CancelScope, ContinuationToken, EffectContext};
use super::request_registry::{RequestRegistry, RequestState};

/// Result of delivering one completion to a context.
#[derive(Debug, PartialEq, Eq)]
enum ResumeOutcome {
    /// Completion was routed; `payload` is what the fiber would observe.
    Resumed(CompletionPayload),
    /// Registry had already discarded the entry (abandoned continuation).
    Discarded,
}

/// Single tick of the (Phase-0) scheduler: pull from backend, route through
/// registry, deliver to the named context. Returns `None` if the backend is
/// idle.
fn tick(
    backend: &dyn AsyncBackend,
    backend_handle: &InMemoryBackend,
    registry: &mut RequestRegistry,
    cx: &mut EffectContext,
    expected_token: ContinuationToken,
) -> Option<ResumeOutcome> {
    let _ = backend; // kept to lock the trait shape into the tick signature
    let raw = backend_handle.next_completion()?;
    match registry.deliver(raw) {
        Some(Completion { payload, .. }) => {
            assert_eq!(
                cx.continuation,
                Some(expected_token),
                "tick must only be called for the context the completion targets",
            );
            cx.continuation = None;
            if matches!(payload, CompletionPayload::Error(ref e) if e == "cancelled") {
                cx.cancel();
            }
            Some(ResumeOutcome::Resumed(payload))
        }
        None => Some(ResumeOutcome::Discarded),
    }
}

/// Helper: register a request and wire the context up to expect its
/// completion. Mirrors what `perform Suspend` will do in Phase 1b.
fn suspend(
    registry: &mut RequestRegistry,
    cx: &mut EffectContext,
) -> (RequestId, ContinuationToken) {
    let req = registry.register();
    let token = ContinuationToken(req.0);
    cx.continuation = Some(token);
    (req, token)
}

#[test]
fn two_contexts_suspend_and_resume_independently() {
    // Two fibers, each performing a Suspend with a different payload. The
    // backend delivers them in reverse order. Neither delivery may corrupt
    // the other context's yield/evidence/cancellation state.
    let backend = InMemoryBackend::new();
    backend.start().unwrap();
    let mut registry = RequestRegistry::new();

    let mut cx_a = EffectContext::new();
    let mut cx_b = EffectContext::new();
    // Mark each context with a distinguishable marker so we can prove
    // post-resume state didn't bleed across.
    let a_marker = cx_a.fresh_marker();
    let b_marker = cx_b.fresh_marker();

    let (req_a, tok_a) = suspend(&mut registry, &mut cx_a);
    let (req_b, tok_b) = suspend(&mut registry, &mut cx_b);
    assert_ne!(req_a, req_b);
    assert_ne!(tok_a, tok_b);

    // Backend delivers B first, then A — out of registration order.
    backend.enqueue(req_b, CompletionPayload::Bytes(vec![0xbb]));
    backend.enqueue(req_a, CompletionPayload::Bytes(vec![0xaa]));

    let out_b = tick(&backend, &backend, &mut registry, &mut cx_b, tok_b);
    assert_eq!(
        out_b,
        Some(ResumeOutcome::Resumed(CompletionPayload::Bytes(vec![0xbb])))
    );
    // A is still parked.
    assert_eq!(cx_a.continuation, Some(tok_a));
    assert_eq!(cx_b.continuation, None);

    let out_a = tick(&backend, &backend, &mut registry, &mut cx_a, tok_a);
    assert_eq!(
        out_a,
        Some(ResumeOutcome::Resumed(CompletionPayload::Bytes(vec![0xaa])))
    );

    // Independent state survived the round trip.
    assert!(!cx_a.is_yielding());
    assert!(!cx_b.is_yielding());
    assert!(!cx_a.is_cancelled());
    assert!(!cx_b.is_cancelled());
    assert_ne!(
        a_marker,
        cx_a.fresh_marker(),
        "marker stream advances locally"
    );
    assert_ne!(
        b_marker,
        cx_b.fresh_marker(),
        "marker stream advances locally"
    );

    assert_eq!(registry.live_count(), 0);
    assert_eq!(backend.pending_len(), 0);
}

#[test]
fn cancellation_before_completion_delivers_cancelled_error() {
    // The fiber suspends, the surrounding scope cancels the request, and the
    // completion that the backend produced must arrive at the context as
    // `Error("cancelled")` — not the original payload — and must mark the
    // context's cancel scope.
    let backend = InMemoryBackend::new();
    backend.start().unwrap();
    let mut registry = RequestRegistry::new();
    let mut cx = EffectContext::new();

    let (req, tok) = suspend(&mut registry, &mut cx);
    backend.enqueue(req, CompletionPayload::Bytes(vec![0xde, 0xad]));

    // Scheduler-side cancel: registry flips state, backend drops the pending
    // completion. We still hand-deliver something so the still-live
    // continuation observes the cancellation.
    registry.cancel(req);
    backend.cancel(req);
    assert_eq!(registry.state(req), Some(RequestState::Cancelled));
    assert_eq!(backend.pending_len(), 0);

    // Without a backend completion to drive the tick, the scheduler
    // synthesises a cancelled completion itself. (Phase 1a's scheduler will
    // do this from a `cancel_pending` queue; Phase 0 just inlines it.)
    let synthetic = Completion {
        request_id: req,
        payload: CompletionPayload::Unit,
    };
    let routed = registry.deliver(synthetic).expect("live entry");
    assert_eq!(routed.payload, CompletionPayload::Error("cancelled".into()));

    // Apply the routed completion to the context.
    assert_eq!(cx.continuation, Some(tok));
    cx.continuation = None;
    cx.cancel();
    assert!(matches!(cx.cancel_scope, CancelScope::Cancelled));
    assert!(cx.is_cancelled());

    assert_eq!(registry.live_count(), 0);
}

#[test]
fn abandoned_continuation_is_cleaned_up() {
    // The owning scope drops while the request is still in flight. The
    // registry forgets the entry, the backend drops its pending completion,
    // and the (eventual) tick observes nothing to deliver.
    let backend = InMemoryBackend::new();
    backend.start().unwrap();
    let mut registry = RequestRegistry::new();
    let mut cx = EffectContext::new();

    let (req, _tok) = suspend(&mut registry, &mut cx);
    backend.enqueue(req, CompletionPayload::Bytes(vec![1, 2, 3]));
    assert_eq!(registry.live_count(), 1);
    assert_eq!(backend.pending_len(), 1);

    // Scope drops: scheduler walks its outstanding requests and abandons
    // each. The continuation token on the context is cleared too — Phase 1b
    // will do this from `scope`'s drop path; Phase 0 just inlines it.
    registry.abandon(req);
    backend.cancel(req);
    cx.continuation = None;

    assert_eq!(registry.state(req), None);
    assert_eq!(registry.live_count(), 0);
    assert_eq!(backend.pending_len(), 0);

    // Even if a stray completion shows up after abandonment (race with the
    // backend that already had it queued), the registry drops it silently.
    let stray = Completion {
        request_id: req,
        payload: CompletionPayload::Bytes(vec![1, 2, 3]),
    };
    assert!(registry.deliver(stray).is_none());
}

#[test]
fn shutdown_drops_pending_and_does_not_route_completions() {
    // After `shutdown` the backend must not produce completions; the
    // registry's live entries are the scheduler's responsibility (Phase 1a
    // walks them and synthesises cancelled errors). Phase 0 just verifies
    // shutdown drains the backend cleanly so the test process exits.
    let backend = InMemoryBackend::new();
    backend.start().unwrap();
    let mut registry = RequestRegistry::new();
    let mut cx = EffectContext::new();

    let (req, _tok) = suspend(&mut registry, &mut cx);
    backend.enqueue(req, CompletionPayload::Unit);

    backend.shutdown().unwrap();
    assert_eq!(backend.pending_len(), 0);
    assert!(backend.next_completion().is_none());

    // The registry entry is still live — Phase 1a will drain on shutdown.
    assert_eq!(registry.live_count(), 1);
    registry.abandon(req);
    assert_eq!(registry.live_count(), 0);
}
