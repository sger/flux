use crate::runtime::{frame::Frame, handler_frame::HandlerFrame, value::Value};

/// A captured delimited continuation.
///
/// Created by `OpPerform` when a matching handler is found.
/// Restored when the captured continuation value is called with a resume value.
///
/// The continuation holds a snapshot of:
/// - The call frames that were active between the `handle` entry and the `perform` site.
/// - The value stack slice between the handler boundary and the `perform` site.
/// - All nested handlers that were within that region.
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

    /// All `HandlerFrame`s that were nested inside the captured region
    /// (between `entry_handler_stack_len` and `handler_pos`).
    pub inner_handlers: Vec<HandlerFrame>,

    /// Handler marker whose state should be replaced when this continuation is
    /// resumed with `resume(value, next_state)`.
    pub state_marker: Option<u32>,
}

impl Continuation {
    /// Compose continuation pieces captured during yield unwinding into a
    /// single resumable continuation.
    ///
    /// `pieces` are expected innermost-first, matching `YieldState.conts`.
    /// The composed result stores frames and stack outermost-first so
    /// `execute_resume` can restore them in one shot.
    pub fn compose(
        pieces: &[Value],
        inner_handlers: Vec<HandlerFrame>,
        state_marker: Option<u32>,
    ) -> Result<Value, String> {
        if pieces.is_empty() {
            return Ok(Value::None);
        }

        let mut composed_frames = Vec::new();
        let mut composed_stack = Vec::new();
        let mut outermost: Option<Continuation> = None;
        let mut innermost_sp = 0usize;

        for piece in pieces.iter().rev() {
            let cont = match piece {
                Value::Continuation(rc) => rc.borrow().clone(),
                other => {
                    return Err(format!(
                        "Continuation::compose expected Continuation piece, got {}",
                        other.type_name()
                    ));
                }
            };
            if outermost.is_none() {
                outermost = Some(cont.clone());
            }
            innermost_sp = cont.sp;
            composed_frames.extend(cont.frames.clone());
            composed_stack.extend(cont.stack.clone());
        }

        let outermost = outermost.expect("pieces.is_empty handled above");
        Ok(Value::Continuation(std::rc::Rc::new(
            std::cell::RefCell::new(Continuation {
                frames: composed_frames,
                stack: composed_stack,
                sp: innermost_sp,
                entry_sp: outermost.entry_sp,
                entry_frame_index: outermost.entry_frame_index,
                inner_handlers,
                state_marker,
            }),
        )))
    }
}

/// Safety net for non-linear control flow (Perceus Section 2.7.1).
///
/// Explicitly clear captured values when the continuation is dropped. Without
/// this, Rc-wrapped values in the captured stack would leak — their refcounts
/// would never reach zero because the continuation holds extra strong
/// references.
impl Drop for Continuation {
    fn drop(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.inner_handlers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{closure::Closure, compiled_function::CompiledFunction};
    use std::{cell::RefCell, rc::Rc};

    fn frame(base_pointer: usize, return_slot: usize) -> Frame {
        let func = Rc::new(CompiledFunction::new(vec![], 0, 0, None));
        let closure = Rc::new(Closure::new(func, vec![]));
        Frame::new_with_return_slot(closure, base_pointer, return_slot)
    }

    #[test]
    fn compose_preserves_outermost_boundary_and_innermost_resume_slot() {
        let inner = Value::Continuation(Rc::new(RefCell::new(Continuation {
            frames: vec![frame(20, 29)],
            stack: vec![Value::Integer(1), Value::Integer(2)],
            sp: 22,
            entry_sp: 20,
            entry_frame_index: 1,
            inner_handlers: vec![],
            state_marker: None,
        })));
        let outer = Value::Continuation(Rc::new(RefCell::new(Continuation {
            frames: vec![frame(10, 19)],
            stack: vec![Value::Integer(3)],
            sp: 19,
            entry_sp: 10,
            entry_frame_index: 0,
            inner_handlers: vec![],
            state_marker: None,
        })));

        let composed =
            Continuation::compose(&[inner, outer], vec![], None).expect("compose succeeds");
        let Value::Continuation(rc) = composed else {
            panic!("compose should produce a continuation");
        };
        let cont = rc.borrow();
        assert_eq!(cont.entry_sp, 10);
        assert_eq!(cont.entry_frame_index, 0);
        assert_eq!(cont.sp, 22);
        assert_eq!(cont.frames.len(), 2);
        assert_eq!(cont.stack.len(), 3);
    }
}
