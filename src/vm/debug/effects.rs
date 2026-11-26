//! Stacks that follow continuations (proposal 0188 S7).
//!
//! While a handler arm runs, the frames that performed the operation are
//! not on `VM.frames`: the general path moved them into a `Continuation`,
//! and a handler that never resumes dropped them. The debugger notes them,
//! only while it is attached, so the stack it shows is the logical one:
//!
//! ```text
//! #0 <handler Flip.flip> at app.flx:17:25
//!    ⟂ handling Flip.flip — performed at app.flx:6:13
//! #1 decide at app.flx:6:13      (suspended)
//! #2 main at app.flx:16:18
//! ```
//!
//! and, once `resume` runs the continuation, the resumed frames above the
//! frame that called it, where `resume_on_top` puts them (KI-237), and no
//! longer below the arm. Stepping measures depth on that logical stack, where
//! the suspended frames still count, so stepping over `resume(x)` runs the
//! continuation and stops back in the arm.

use super::super::VM;
use super::{DebugControl, FrameSummary, inspect};

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
    /// The serials of the frames' calls, which the continuation keeps.
    pub(crate) serials: Vec<u64>,
    /// While a `resume` runs the continuation above this VM frame, the
    /// frames are live there and not shown below the arm.
    pub(crate) covered_at: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct SuspendedFrame {
    pub(crate) summary: FrameSummary,
    pub(crate) artificial: bool,
}

impl DebugControl {
    /// Whether the suspension's arm is still the call it noted.
    pub(crate) fn suspension_live(&self, suspension: &Suspension, frame_index: usize) -> bool {
        suspension.arm_index <= frame_index
            && self.serials.get(suspension.arm_index) == Some(&suspension.arm_serial)
    }

    /// The depth of VM frame `index` on the logical stack: the frames a
    /// perform suspended count below the arm handling it. They still count
    /// while a `resume` runs them above (`covered_at`), so the resumed
    /// frames are deeper than the frame that called it.
    pub(crate) fn logical_depth(&self, index: usize, frame_index: usize) -> usize {
        let suspended: usize = self
            .suspensions
            .iter()
            .filter(|suspension| {
                suspension.arm_index <= index && self.suspension_live(suspension, frame_index)
            })
            .map(|suspension| suspension.frames.len())
            .sum();
        index + suspended
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
        let serials: Vec<u64> = (entry + 1..=self.frame_index)
            .map(|index| control.serials.get(index).copied().unwrap_or(0))
            .collect();
        control.pending_suspension = Some(Suspension {
            arm_index: 0,
            arm_serial: 0,
            operation,
            frames,
            resumable,
            serials: serials.clone(),
            covered_at: None,
        });
        serials
    }

    /// A fiber parks: the serials of the calls `entry + 1 ..= frame_index`
    /// that its continuation takes, to give back when it resumes (S8).
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_call_serials(&self, entry: usize) -> Vec<u64> {
        let Some(state) = self.debugger.as_ref() else {
            return Vec::new();
        };
        (entry + 1..=self.frame_index)
            .map(|index| state.control.serials.get(index).copied().unwrap_or(0))
            .collect()
    }

    /// `resume_on_top` runs the continuation whose calls had `serials`
    /// above VM frame `base`: its suspension's frames are live there now.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_cover_suspension(&mut self, base: usize, serials: &[u64]) {
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        if serials.is_empty() {
            return;
        }
        if let Some(suspension) =
            state.control.suspensions.iter_mut().rev().find(|s| s.serials == serials)
        {
            suspension.covered_at = Some(base);
        }
    }

    /// The continuations resumed above VM frame `base` have returned: their
    /// suspensions show below their arms again.
    #[cold]
    #[inline(never)]
    pub(in crate::vm) fn debug_uncover_suspensions(&mut self, base: usize) {
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        for suspension in &mut state.control.suspensions {
            if suspension.covered_at == Some(base) {
                suspension.covered_at = None;
            }
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
        // A parked fiber put back stops at its next line, not where it
        // parked (S8).
        let fiber = std::mem::take(&mut state.control.resuming_fiber);
        let result = if !fiber && state.control.stepping_in() && self.frame_index > entry {
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
