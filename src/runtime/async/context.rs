//! Scheduler-owned effect/fiber context (proposal 0174 Phase 0).
//!
//! Aggregates the state that today lives as process-global variables in
//! `runtime/c/effects.c` (yield payload, evidence vector, marker counter,
//! resume short-circuit detector) and as fields on `Vm` in `src/vm/mod.rs`
//! (`yield_state`, `evv`). Phase 0b — this slice — only defines the type
//! and proves two contexts can carry independent state. Phase 0c migrates
//! the VM to read/write through it; Phase 0d migrates the native C runtime;
//! Phase 1a/1b populate the scheduler-side fields (`continuation`,
//! `cancel_scope`, `home_worker`).
//!
//! ## Naming
//!
//! Not `RuntimeContext` — that name is taken by an existing higher-order-call
//! invocation trait in [`crate::runtime`]. `EffectContext` matches the
//! Phase 0 scope ("concurrency-ready effect runtime") and is what the
//! proposal text means by "the active context".

use crate::runtime::{evidence::EvidenceVector, yield_state::YieldState};

/// Identifier for the worker thread a context's fiber/task is anchored to.
///
/// Phase 1a populates this when a task is dispatched; Phase 0 carries
/// `WorkerId(0)` as a placeholder so the field exists for downstream phases
/// without introducing an `Option` they would have to unwrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WorkerId(pub u32);

/// Scheduler-issued handle for a parked continuation.
///
/// Phase 1a/1b populate this when a fiber suspends so the backend's completion
/// callback can route the wakeup back to the right context. Phase 0 always
/// carries `None` (no scheduler exists yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContinuationToken(pub u64);

/// Cancellation scope state owned by the scheduler.
///
/// Phase 1b populates this from structured-concurrency primitives (`scope`,
/// `timeout`, `race`). Phase 0 always reports `Open`. Promoting an
/// already-`Cancelled` scope back to `Open` is a programmer error — once
/// cancellation is observable to a fiber the cancellation must run to
/// completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CancelScope {
    #[default]
    Open,
    Cancelled,
}

/// Scheduler-owned effect/fiber context.
///
/// One instance per running fiber (Phase 1b) or task (Phase 1a). The fields
/// are public for now so Phase 0c can wire the VM through this type with
/// minimal ceremony; we can tighten the surface to accessor methods once the
/// migration is settled.
#[derive(Debug, Default)]
pub struct EffectContext {
    /// In-flight perform / yield bookkeeping. Today this lives on `Vm`;
    /// Phase 0c moves it here.
    pub yield_state: YieldState,
    /// Active evidence vector. Today this lives on `Vm`; Phase 0c moves it
    /// here.
    pub evidence: EvidenceVector,
    /// Scheduler-issued token for this context's parked continuation. `None`
    /// while running. Phase 1a populates on suspend; the backend's completion
    /// hands it back so the scheduler can route the resume.
    pub continuation: Option<ContinuationToken>,
    /// Cancellation state for this context's enclosing scope.
    pub cancel_scope: CancelScope,
    /// Home worker for this context. Phase 1a/1b honour Eio's
    /// no-fiber-migration model: the same worker that started the fiber
    /// resumes it.
    pub home_worker: WorkerId,
}

impl EffectContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a perform is unwinding toward its handler.
    pub fn is_yielding(&self) -> bool {
        self.yield_state.is_yielding()
    }

    /// True once the enclosing scope has been cancelled. Phase 1b wires this
    /// into structured-concurrency primitives; Phase 0 only exercises the
    /// state transition in tests.
    pub fn is_cancelled(&self) -> bool {
        matches!(self.cancel_scope, CancelScope::Cancelled)
    }

    /// Mark the enclosing scope cancelled. Idempotent: cancelling a context
    /// already in `Cancelled` is a no-op. Phase 0 has no scheduler to notify;
    /// Phase 1b adds the registered-wait propagation.
    pub fn cancel(&mut self) {
        self.cancel_scope = CancelScope::Cancelled;
    }

    /// Allocate a fresh marker, bumping the context-local counter. Two
    /// contexts produce independent sequences. Mirrors `flux_fresh_marker`.
    pub fn fresh_marker(&mut self) -> u32 {
        self.yield_state.fresh_marker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::yield_state::Yielding;

    #[test]
    fn default_context_is_idle_and_open() {
        let cx = EffectContext::new();
        assert!(!cx.is_yielding());
        assert!(!cx.is_cancelled());
        assert_eq!(cx.continuation, None);
        assert_eq!(cx.home_worker, WorkerId(0));
    }

    #[test]
    fn is_yielding_reflects_yield_state() {
        let mut cx = EffectContext::new();
        cx.yield_state.yielding = Yielding::Pending;
        assert!(cx.is_yielding());
        cx.yield_state.yielding = Yielding::Final;
        assert!(cx.is_yielding());
        cx.yield_state.clear();
        assert!(!cx.is_yielding());
    }

    #[test]
    fn cancel_is_idempotent_and_observable() {
        let mut cx = EffectContext::new();
        assert!(!cx.is_cancelled());
        cx.cancel();
        assert!(cx.is_cancelled());
        cx.cancel();
        assert!(cx.is_cancelled());
    }

    #[test]
    fn fresh_marker_increments_within_a_context() {
        let mut cx = EffectContext::new();
        let m1 = cx.fresh_marker();
        let m2 = cx.fresh_marker();
        let m3 = cx.fresh_marker();
        assert_ne!(m1, m2);
        assert_ne!(m2, m3);
        assert_ne!(m1, m3);
    }

    /// Load-bearing test for Phase 0: two contexts must not share state.
    /// This is what Phase 0c/0d will rely on once the VM and native runtime
    /// route through `EffectContext` instead of process-global variables.
    #[test]
    fn two_contexts_have_independent_state() {
        let mut a = EffectContext::new();
        let mut b = EffectContext::new();

        // Independent marker streams.
        let a1 = a.fresh_marker();
        let a2 = a.fresh_marker();
        let b1 = b.fresh_marker();
        assert_eq!(a1, b1, "independent counters start at the same value");
        assert_ne!(a1, a2);

        // Independent yield state.
        a.yield_state.yielding = Yielding::Pending;
        a.yield_state.marker = 42;
        assert!(a.is_yielding());
        assert!(!b.is_yielding(), "yield in context A must not leak to B");
        assert_eq!(b.yield_state.marker, 0);

        // Independent cancellation.
        a.cancel();
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "cancellation in A must not leak to B");

        // Independent continuation tokens.
        a.continuation = Some(ContinuationToken(7));
        assert_eq!(a.continuation, Some(ContinuationToken(7)));
        assert_eq!(b.continuation, None);
    }

    #[test]
    fn worker_id_is_hashable_and_copy() {
        // Forward-compat: Phase 1a stores fibers in a `HashMap<WorkerId, ...>`,
        // and the dispatch loop copies `WorkerId` around freely. Lock both
        // properties in now so a Phase 1a refactor can't quietly break them.
        let _: std::collections::HashSet<WorkerId> = [WorkerId(0), WorkerId(1), WorkerId(0)]
            .into_iter()
            .collect();
        let id = WorkerId(3);
        let copy = id;
        assert_eq!(id, copy);
    }
}
