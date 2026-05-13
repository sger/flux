use crate::runtime::{frame::Frame, handler_frame::HandlerFrame, value::Value};

/// Upper bound on the per-VM free-list of recycled [`Continuation`] shells
/// (see `VM::cont_pool`). Steady-state fiber park/resume cycles one shell back
/// and forth through the pool; the cap stops a pathological burst of abandoned
/// continuations from pinning unbounded capacity.
pub const CONT_POOL_CAP: usize = 32;

/// A captured delimited continuation.
///
/// Created by `OpPerform` when a matching handler is found.
/// Restored when the captured continuation value is called with a resume value.
///
/// The continuation holds a snapshot of:
/// - The call frames that were active between the `handle` entry and the `perform` site.
/// - The value stack slice between the handler boundary and the `perform` site.
/// - All nested handlers that were within that region.
#[derive(Debug, Clone, PartialEq, Default)]
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
        let mut owned: Vec<Continuation> = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Value::Continuation(rc) => owned.push(rc.borrow().clone()),
                other => {
                    return Err(format!(
                        "Continuation::compose expected Continuation piece, got {}",
                        other.type_name()
                    ));
                }
            }
        }
        let mut scratch_pool = Vec::new();
        let composed = Continuation::compose_pieces(
            &mut owned,
            inner_handlers,
            state_marker,
            &mut scratch_pool,
        )?;
        Ok(Value::Continuation(std::rc::Rc::new(
            std::cell::RefCell::new(composed),
        )))
    }

    /// Compose continuation pieces — innermost-first, as accumulated during
    /// yield unwinding — into a single resumable [`Continuation`], **consuming**
    /// `pieces` (drained) and recycling spent piece shells into `pool`.
    ///
    /// Allocation-light path used by the fiber suspend hook and `OpPerform`:
    /// a single-piece capture is returned by value with no copying; a
    /// multi-piece capture splices the pieces' stack slices into one buffer
    /// (drawn from `pool` when available). `pieces[0]` is the innermost
    /// (perform/suspend-site) frame; `pieces[len-1]` is the outermost frame,
    /// just inside the handler/`run_async` boundary.
    pub fn compose_pieces(
        pieces: &mut Vec<Continuation>,
        inner_handlers: Vec<HandlerFrame>,
        state_marker: Option<u32>,
        pool: &mut Vec<Continuation>,
    ) -> Result<Continuation, String> {
        match pieces.len() {
            0 => Err("Continuation::compose_pieces called with no pieces".to_string()),
            1 => {
                let mut c = pieces.pop().expect("len == 1");
                c.inner_handlers = inner_handlers;
                c.state_marker = state_marker;
                Ok(c)
            }
            n => {
                // pieces[0] = innermost, pieces[n-1] = outermost.
                let outermost_entry_sp = pieces[n - 1].entry_sp;
                let outermost_entry_frame_index = pieces[n - 1].entry_frame_index;
                let innermost_sp = pieces[0].sp;
                if innermost_sp < outermost_entry_sp {
                    return Err(
                        "Continuation::compose_pieces found inverted stack span".to_string()
                    );
                }
                let span = innermost_sp - outermost_entry_sp;

                let mut result = pool.pop().unwrap_or_default();
                result.frames.clear();
                result.stack.clear();
                result.stack.resize(span, Value::Uninit);
                result.sp = innermost_sp;
                result.entry_sp = outermost_entry_sp;
                result.entry_frame_index = outermost_entry_frame_index;
                result.inner_handlers = inner_handlers;
                result.state_marker = state_marker;

                // Walk outermost-first so frames land deepest-first in `result.frames`.
                for mut piece in pieces.drain(..).rev() {
                    if piece.entry_sp < outermost_entry_sp {
                        return Err(
                            "Continuation::compose_pieces found piece before outer boundary"
                                .to_string(),
                        );
                    }
                    let offset = piece.entry_sp - outermost_entry_sp;
                    let end = offset + piece.stack.len();
                    if end > span || piece.sp > innermost_sp {
                        return Err(
                            "Continuation::compose_pieces found piece outside stack span"
                                .to_string(),
                        );
                    }
                    result.frames.append(&mut piece.frames);
                    for (idx, value) in piece.stack.drain(..).enumerate() {
                        result.stack[offset + idx] = value;
                    }
                    piece.inner_handlers.clear();
                    if pool.len() < CONT_POOL_CAP {
                        pool.push(piece);
                    }
                }
                Ok(result)
            }
        }
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
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    fn frame(base_pointer: usize, return_slot: usize) -> Frame {
        let func = Arc::new(CompiledFunction::new(vec![], 0, 0, None));
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
        assert_eq!(cont.stack.len(), 12);
        assert_eq!(cont.stack[0], Value::Integer(3));
        assert!(
            cont.stack[1..10]
                .iter()
                .all(|value| matches!(value, Value::Uninit))
        );
        assert_eq!(cont.stack[10], Value::Integer(1));
        assert_eq!(cont.stack[11], Value::Integer(2));
    }

    fn piece(
        base_pointer: usize,
        return_slot: usize,
        entry_sp: usize,
        sp: usize,
        entry_frame_index: usize,
        stack: Vec<Value>,
    ) -> Continuation {
        Continuation {
            frames: vec![frame(base_pointer, return_slot)],
            stack,
            sp,
            entry_sp,
            entry_frame_index,
            inner_handlers: vec![],
            state_marker: None,
        }
    }

    #[test]
    fn compose_pieces_single_piece_moves_through_with_handlers_and_marker() {
        let mut pieces = vec![piece(
            5,
            9,
            5,
            7,
            2,
            vec![Value::Integer(10), Value::Integer(20)],
        )];
        let mut pool = Vec::new();
        let composed = Continuation::compose_pieces(&mut pieces, vec![], Some(7), &mut pool)
            .expect("single-piece compose succeeds");
        assert!(pieces.is_empty());
        assert!(pool.is_empty(), "single-piece path recycles nothing");
        assert_eq!(composed.entry_sp, 5);
        assert_eq!(composed.sp, 7);
        assert_eq!(composed.entry_frame_index, 2);
        assert_eq!(composed.frames.len(), 1);
        assert_eq!(composed.stack, vec![Value::Integer(10), Value::Integer(20)]);
        assert_eq!(composed.state_marker, Some(7));
    }

    #[test]
    fn compose_pieces_multi_splices_stack_and_recycles_shells() {
        // innermost first, matching the order capture_to_boundary pushes.
        let mut pieces = vec![
            piece(
                20,
                29,
                20,
                22,
                1,
                vec![Value::Integer(1), Value::Integer(2)],
            ),
            piece(10, 19, 10, 19, 0, vec![Value::Integer(3)]),
        ];
        let mut pool = Vec::new();
        let composed = Continuation::compose_pieces(&mut pieces, vec![], None, &mut pool)
            .expect("multi-piece compose succeeds");
        assert!(pieces.is_empty());
        assert_eq!(pool.len(), 2, "both spent piece shells recycled");
        assert!(
            pool.iter()
                .all(|c| c.frames.is_empty() && c.stack.is_empty()),
            "recycled shells are emptied"
        );
        assert_eq!(composed.entry_sp, 10);
        assert_eq!(composed.entry_frame_index, 0);
        assert_eq!(composed.sp, 22);
        assert_eq!(composed.frames.len(), 2);
        assert_eq!(composed.stack.len(), 12);
        assert_eq!(composed.stack[0], Value::Integer(3));
        assert!(
            composed.stack[1..10]
                .iter()
                .all(|v| matches!(v, Value::Uninit))
        );
        assert_eq!(composed.stack[10], Value::Integer(1));
        assert_eq!(composed.stack[11], Value::Integer(2));
    }

    #[test]
    fn compose_pieces_reuses_a_pooled_shell_for_the_result() {
        // a fat shell with spare capacity, as if recycled from a prior resume
        let mut fat = Continuation::default();
        fat.stack.reserve(64);
        fat.frames.reserve(8);
        let pooled_stack_cap = fat.stack.capacity();
        let mut pool: Vec<Continuation> = vec![fat];
        let mut pieces = vec![
            piece(4, 9, 4, 6, 1, vec![Value::Integer(7)]),
            piece(0, 3, 0, 3, 0, vec![Value::Integer(8)]),
        ];
        let composed = Continuation::compose_pieces(&mut pieces, vec![], None, &mut pool)
            .expect("compose succeeds");
        assert!(
            composed.stack.capacity() >= pooled_stack_cap,
            "result reused the pooled buffer's capacity"
        );
    }

    #[test]
    fn compose_pieces_rejects_empty() {
        let mut pieces: Vec<Continuation> = Vec::new();
        let mut pool = Vec::new();
        assert!(Continuation::compose_pieces(&mut pieces, vec![], None, &mut pool).is_err());
    }
}
