use std::rc::Rc;

use crate::{
    runtime::{evidence::EvidenceVector, handler_arm::HandlerArm},
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
}
