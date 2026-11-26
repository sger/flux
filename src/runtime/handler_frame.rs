use std::rc::Rc;

use crate::{
    runtime::{evidence::EvidenceVector, handler_arm::HandlerArm, value::Value},
    syntax::Identifier,
};

/// An active handler pushed onto the VM's `handler_stack` by `OpHandle`.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerFrame {
    /// The effect this handler covers.
    pub effect: Identifier,
    pub arms: Rc<Vec<HandlerArm>>,
    /// Fresh marker identifying this handler instance in the evidence vector.
    pub marker: u32,
    /// Evidence vector to restore when this handler unwinds.
    pub saved_evv: EvidenceVector,
    /// Current parameter value for parameterized handlers.
    pub state: Option<Value>,
    /// `VM.frame_index` when `OpHandle` executed.
    pub entry_frame_index: usize,
    /// `VM.sp` when `OpHandle` executed.
    pub entry_sp: usize,
    /// `VM.handler_stack.len()` when `OpHandle` executed.
    pub entry_handler_stack_len: usize,
    /// When `true`, the handler is tail-resumptive: `OpPerformDirect` skips
    /// continuation capture and calls the arm closure directly.
    pub is_direct: bool,
    /// When `true`, the handler never resumes — `OpPerform` skips continuation
    /// capture entirely, unwinds the stack to handler entry, and calls the arm
    /// directly. (Perceus Section 2.7.1: non-linear control flow safety.)
    pub is_discard: bool,
    /// A copy of a handler that a `resume` put back above the frame that
    /// called it (KI-237). No `OpEndHandle` runs for it: it comes off when
    /// a frame returns into `entry_frame_index`, taking `saved_evv` back.
    pub resume_boundary: bool,
    /// The handle's clauses are tail-resumptive, as the native backend
    /// decides it. @KI-233: any other clause runs under `saved_evv`, the
    /// handle's own evidence, so a perform in it reaches the handlers
    /// outside the handle, its own effect's included.
    pub tail_resumptive: bool,
}
