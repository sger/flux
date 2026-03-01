use crate::runtime::{frame::Frame, handler_frame::HandlerFrame, value::Value};

/// A captured one-shot delimited continuation.
///
/// Created by `OpPerform` when a matching handler is found.
/// Restored (exactly once) when the captured continuation value is called
/// with a resume value.
///
/// The continuation holds a snapshot of:
/// - The call frames that were active between the `handle` entry and the `perform` site.
/// - The value stack slice between the handler boundary and the `perform` site.
/// - Any nested handlers that were within that region.
#[derive(Debug, Clone, PartialEq)]
pub struct Continuation {
    /// Cloned frames from `entry_frame_index + 1` up to (and including) the
    /// frame that executed `OpPerform`. These are restored verbatim on resume.
    pub frames: Vec<Frame>,

    /// Cloned value stack from `entry_sp` up to (but not including) the
    /// arguments that were passed to the effect operation.
    pub stack: Vec<Value>,

    /// The absolute `sp` value at capture time (= `entry_sp + stack.len()`).
    pub sp: usize,

    /// The absolute `entry_sp` stored separately so the resume path knows
    /// where to splice the stack back.
    pub entry_sp: usize,

    /// The `frame_index` of the handle boundary frame (the frame that called
    /// the continuation-producing code).
    pub entry_frame_index: usize,

    /// Any `HandlerFrame`s that were nested inside the captured region
    /// (between `entry_handler_stack_len` and `handler_pos`).
    pub inner_handlers: Vec<HandlerFrame>,

    /// One-shot enforcement: set to `true` after the first resume.
    pub used: bool,
}
