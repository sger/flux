//! Stacks that follow continuations (proposal 0188 S7).
//!
//! While a handler arm runs, the frames that performed the operation are
//! not on `VM.frames`: the general path moved them into a `Continuation`,
//! and a handler that never resumes dropped them. While a resumed
//! continuation runs, the arm that called `resume` is off the stack too,
//! kept by `execute_resume` until the continuation returns. The debugger
//! notes both, only while it is attached, so the stack it shows is the
//! logical one:
//!
//! ```text
//! #0 <handler Flip.flip> at app.flx:17:25
//!    ⟂ handling Flip.flip — performed at app.flx:6:13
//! #1 decide at app.flx:6:13      (suspended)
//! #2 main at app.flx:16:18
//! ```
//!
//! and, once `resume` runs the continuation, the resumed frames above the
//! arm that resumed them. Stepping measures depth on that logical stack, so
//! stepping over `resume(x)` runs the continuation and stops back in the arm.

use crate::runtime::frame::Frame;
use crate::runtime::value::Value;

use super::super::VM;
use super::{DebugControl, FrameSummary, TailCalls, inspect};

/// Frames a perform took off the stack, shown below the arm handling it.
#[derive(Debug, Clone)]
pub(crate) struct Suspension {
    /// The arm's frame: its index and the serial of its call.
    pub(crate) arm_index: usize,
    pub(crate) arm_serial: u64,
    /// `Effect.op`.
    pub(crate) operation: String,
    /// The frames, outermost first, each at its perform or call.
    pub(crate) frames: Vec<SuspendedFrame>,
    /// False for a handler that never resumes: its frames are gone.
    pub(crate) resumable: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SuspendedFrame {
    pub(crate) summary: FrameSummary,
    pub(crate) artificial: bool,
}

/// The frames a non-tail `resume` took off the stack while the continuation
/// runs: the arm that called it, and any frame between the handle site and
/// the arm. They come back when the continuation returns.
#[derive(Debug, Clone)]
pub(crate) struct ResumeNote {
    /// The handle site's frame index: the resumed frames start above it.
    pub(crate) entry: usize,
    /// The logical depth of the frame that called `resume`: the resumed
    /// frames are deeper.
    pub(crate) top_depth: usize,
    /// Outermost first.
    pub(crate) hidden: Vec<HiddenFrame>,
    /// The value stack from `stack_from`, for the hidden frames' variables.
    pub(crate) stack_from: usize,
    pub(crate) stack: Vec<Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct HiddenFrame {
    pub(crate) index: usize,
    pub(crate) frame: Frame,
    /// The instruction the frame is at: the `resume` call, or the call that
    /// is running.
    pub(crate) ip: usize,
    last_line: Option<(usize, usize)>,
    serial: u64,
    pub(crate) tail_calls: TailCalls,
}

impl DebugControl {
    /// Whether the suspension's arm is still the call it noted.
    pub(crate) fn suspension_live(&self, suspension: &Suspension, frame_index: usize) -> bool {
        suspension.arm_index <= frame_index
            && self.serials.get(suspension.arm_index) == Some(&suspension.arm_serial)
    }

    /// The depth of VM frame `index` on the logical stack: the frames a
    /// perform suspended count below the arm handling it, and a resumed
    /// continuation's frames count above the arm that resumed it.
    pub(crate) fn logical_depth(&self, index: usize, frame_index: usize) -> usize {
        let note = self.resumes.iter().rev().find(|note| note.entry < index);
        let (base, above) = match note {
            Some(note) => (note.top_depth + (index - note.entry), note.entry),
            None => (index, 0),
        };
        let suspended: usize = self
            .suspensions
            .iter()
            .filter(|suspension| {
                (note.is_none() || suspension.arm_index > above)
                    && suspension.arm_index <= index
                    && self.suspension_live(suspension, frame_index)
            })
            .map(|suspension| suspension.frames.len())
            .sum();
        base + suspended
    }
}

impl VM {
    /// `perform` is about to take frames `entry + 1 ..= frame_index` off the
    /// stack for the arm of `operation`: note them, to show below the arm.
    /// Returns the serials of their calls, for the continuation to keep.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_note_suspend(
        &mut self,
        entry: usize,
        operation: String,
        resumable: bool,
    ) -> Vec<u64> {
        let frames = (entry + 1..=self.frame_index)
            .map(|index| SuspendedFrame {
                summary: inspect::frame_summary(self, index),
                artificial: inspect::is_artificial(
                    self.frames[index]
                        .closure
                        .function
                        .debug_info
                        .as_ref()
                        .and_then(|info| info.name.as_deref()),
                ),
            })
            .collect();
        let Some(state) = self.debugger.as_mut() else {
            return Vec::new();
        };
        let control = &mut state.control;
        control.pending_suspension = Some(Suspension {
            arm_index: 0,
            arm_serial: 0,
            operation,
            frames,
            resumable,
        });
        (entry + 1..=self.frame_index)
            .map(|index| control.serials.get(index).copied().unwrap_or(0))
            .collect()
    }

    /// A non-tail `resume` is about to run a continuation above frame
    /// `entry`: keep the frames it will cover, the arm among them.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_note_resume_start(&mut self, entry: usize) {
        let top = self.frame_index;
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        let control = &mut state.control;
        let top_depth = control.logical_depth(top, top);
        let hidden: Vec<HiddenFrame> = (entry + 1..=top)
            .map(|index| HiddenFrame {
                index,
                frame: self.frames[index].clone(),
                ip: if index == top {
                    self.frames[index].ip
                } else {
                    self.call_ips
                        .get(index + 1)
                        .copied()
                        .unwrap_or(self.frames[index].ip)
                },
                last_line: control.last_line.get(index).copied().flatten(),
                serial: control.serials.get(index).copied().unwrap_or(0),
                tail_calls: control.tail_calls.get(index).cloned().unwrap_or_default(),
            })
            .collect();
        let stack_from = hidden
            .first()
            .map_or(self.sp, |frame| frame.frame.base_pointer);
        let stack = self.stack[stack_from.min(self.sp)..self.sp]
            .iter()
            .map(super::super::slot::from_slot_ref)
            .collect();
        control.resumes.push(ResumeNote {
            entry,
            top_depth,
            hidden,
            stack_from,
            stack,
        });
    }

    /// The continuation a non-tail `resume` ran has returned: the frames it
    /// covered are back, with what the debugger knew about them.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_note_resume_end(&mut self) {
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        let control = &mut state.control;
        let Some(note) = control.resumes.pop() else {
            return;
        };
        for frame in note.hidden {
            let index = frame.index;
            if control.last_line.len() <= index {
                control.last_line.resize(index + 1, None);
            }
            control.last_line[index] = frame.last_line;
            if control.serials.len() <= index {
                control.serials.resize(index + 1, 0);
            }
            control.serials[index] = frame.serial;
            if control.tail_calls.len() <= index {
                control
                    .tail_calls
                    .resize_with(index + 1, TailCalls::default);
            }
            control.tail_calls[index] = frame.tail_calls;
        }
    }

    /// `resume` has put a continuation's frames back above `entry`: the
    /// same calls, with their `serials`. Each is at the line it left, which
    /// is not a new place to stop; a step into `resume` stops at once, at
    /// the perform the top frame is in.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_note_resumed(
        &mut self,
        entry: usize,
        serials: &[u64],
    ) -> Result<(), String> {
        let Some(mut state) = self.debugger.take() else {
            return Ok(());
        };
        if serials.len() == self.frame_index.saturating_sub(entry) {
            for (index, serial) in (entry + 1..=self.frame_index).zip(serials) {
                state.control.serials[index] = *serial;
            }
        }
        for index in entry + 1..=self.frame_index {
            let function = &self.frames[index].closure.function;
            let key = std::sync::Arc::as_ptr(function) as usize;
            let ip = self.frames[index].ip.saturating_sub(1);
            let line = function
                .debug_info
                .as_ref()
                .and_then(|info| info.location_at(ip))
                .map(|location| location.span.start.line);
            if let Some(line) = line {
                let last_line = &mut state.control.last_line;
                if last_line.len() <= index {
                    last_line.resize(index + 1, None);
                }
                last_line[index] = Some((key, line));
            }
        }
        let result = if state.control.stepping_in() && self.frame_index > entry {
            let top = self.frame_index;
            let visible = !inspect::is_artificial(
                self.frames[top]
                    .closure
                    .function
                    .debug_info
                    .as_ref()
                    .and_then(|info| info.name.as_deref()),
            ) || state.control.show_internals;
            if visible {
                state.control.top_ip = Some(self.frames[top].ip.saturating_sub(1));
                let result = self.stop(&mut state, super::StopReason::Step);
                state.control.top_ip = None;
                result
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        self.debugger = Some(state);
        result
    }
}
