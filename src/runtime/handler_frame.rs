use crate::{runtime::handler_arm::HandlerArm, syntax::Identifier};

/// An active handler pushed onto the VM's `handler_stack` by `OpHandle`.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerFrame {
    /// The effect this handler covers.
    pub effect: Identifier,
    pub arms: Vec<HandlerArm>,
    /// `VM.frame_index` when `OpHandle` executed.
    pub entry_frame_index: usize,
    /// `VM.sp` when `OpHandle` executed.
    pub entry_sp: usize,
    /// `VM.handler_stack.len()` when `OpHandle` executed.
    pub entry_handler_stack_len: usize,
}
