//! VM-side mirror of the C runtime's yield state (Proposal 0162 Phase 3).
//!
//! `YieldState` tracks in-flight effect performs while the stack unwinds from
//! the `perform` site back to the matching `handle`. Every function return
//! checks `yielding`: when set, the function appends its "rest of the
//! computation" closure to `conts` and returns the sentinel. The prompt loop
//! at the handle frame matches markers, composes `conts`, and invokes the
//! operation clause with the composed continuation.
//!
//! This mirrors the globals in `runtime/c/effects.c`
//! (`flux_yield_yielding`, `flux_yield_marker`, `flux_yield_clause`,
//! `flux_yield_op_arg`, `flux_yield_conts`) as a single struct owned by the
//! VM, rather than global state. Single-threaded — matches the rest of the
//! VM's state model.

use crate::runtime::handler_arm::HandlerArm;
use crate::runtime::value::Value;
use std::rc::Rc;

/// The unwind mode requested by a `perform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Yielding {
    /// No yield in flight; normal execution.
    #[default]
    None,
    /// A perform is unwinding toward its handler. Function returns should
    /// append their continuation to `conts` and propagate the sentinel.
    Pending,
    /// A final yield (the handler clause returned without resuming). The
    /// handle frame should unwind *past* itself without composing — used for
    /// non-tail-resumptive / discard shapes.
    Final,
}

/// VM-side yield state. One instance owned by the VM.
#[derive(Debug, Default)]
pub struct YieldState {
    /// Current yield mode.
    pub yielding: Yielding,
    /// Marker of the handler this perform targets. Mirrors `flux_yield_marker`.
    pub marker: u32,
    /// The operation clause the prompt loop should invoke once continuations
    /// are composed. Mirrors `flux_yield_clause`.
    pub clause: Option<Rc<HandlerArm>>,
    /// Argument passed to the perform, to be handed to the clause alongside
    /// the composed resume continuation. Mirrors `flux_yield_op_arg`.
    pub op_arg: Option<Value>,
    /// Accumulated continuation closures, innermost first. Mirrors
    /// `flux_yield_conts`. The C runtime caps at 8 and compresses on overflow;
    /// the VM uses a growable `Vec` since it isn't constrained by C's
    /// fixed-array representation.
    pub conts: Vec<Value>,
    /// Monotonic counter feeding `evidence::fresh_marker`. Mirrors the C
    /// runtime's `marker_counter` file-static.
    pub marker_counter: u32,
}

impl YieldState {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a perform is unwinding.
    pub fn is_yielding(&self) -> bool {
        !matches!(self.yielding, Yielding::None)
    }

    /// Reset to idle after the prompt loop has consumed the yield.
    /// Mirrors the clearing block at the end of `flux_yield_prompt`.
    pub fn clear(&mut self) {
        self.yielding = Yielding::None;
        self.marker = 0;
        self.clause = None;
        self.op_arg = None;
        self.conts.clear();
    }

    /// Append a continuation during unwind. Mirrors `flux_yield_extend`.
    pub fn extend(&mut self, cont: Value) {
        self.conts.push(cont);
    }

    /// Allocate a fresh marker, bumping the internal counter.
    /// Mirrors `flux_fresh_marker`.
    pub fn fresh_marker(&mut self) -> u32 {
        crate::runtime::evidence::fresh_marker(&mut self.marker_counter)
    }
}
