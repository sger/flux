//! The VM debugger engine (proposal 0188 S2).
//!
//! A `-g` build has an `OpBreakpoint` at every stop site: function entries,
//! statements, match arms, `|>` stages and effect operations. Without a
//! debugger attached the opcode does nothing. With one, each site checks
//! the breakpoints and the step mode, and a stop hands the paused VM to the
//! [`VmObserver`] through an [`Inspector`], which reads the stack, locals and
//! values and sets breakpoints. The observer's answer says how to go on.
//!
//! The observer is the front end: `flux debug`'s command reader now, the
//! VS Code adapter (S4) later.

mod inspect;
mod values;

use std::collections::HashSet;

pub use inspect::{Inspector, StackFrame, Variable};
pub use values::ValueView;

use super::VM;

/// Why the program stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    Step,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Breakpoint => "breakpoint",
            StopReason::Step => "step",
        }
    }
}

/// How to go on after a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Run to the next breakpoint.
    Continue,
    /// Stop at the next site anywhere.
    StepIn,
    /// Stop at the next site in this frame or one it returns to.
    StepOver,
    /// Stop at the next site after this frame returns.
    StepOut,
    /// End the program now.
    Quit,
}

/// The front end of a debugging session.
pub trait VmObserver {
    /// Called once before the program runs, to set breakpoints.
    fn on_start(&mut self, inspector: &mut Inspector<'_>) -> Resume;
    /// Called at each stop.
    fn on_stop(&mut self, reason: StopReason, inspector: &mut Inspector<'_>) -> Resume;
    /// Called when the program has finished.
    fn on_exit(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepMode {
    Continue,
    StepIn,
    /// Stop at a site whose frame index is at most `depth`.
    StepOver {
        depth: usize,
    },
    /// Stop at a site whose frame index is below `depth`.
    StepOut {
        depth: usize,
    },
}

/// A resolved line breakpoint: the file as the bytecode names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LineBreakpoint {
    pub(crate) file: String,
    pub(crate) line: usize,
}

/// What the debugger keeps between stops, apart from the observer.
pub(crate) struct DebugControl {
    pub(crate) breakpoints: HashSet<LineBreakpoint>,
    pub(crate) show_internals: bool,
    mode: StepMode,
    /// Per frame index, the last line that ran there, as (function, line):
    /// a line stops once, at its first site, until another line runs in that
    /// frame or the frame starts a new call.
    last_line: Vec<Option<(usize, usize)>>,
    /// Per frame index: tail calls made in place of returning, and the most
    /// recent callers they replaced.
    pub(crate) tail_calls: Vec<TailCalls>,
}

/// Calls a frame no longer shows because each was replaced by a tail call.
#[derive(Debug, Clone, Default)]
pub(crate) struct TailCalls {
    pub(crate) count: u64,
    /// The latest replaced callers, newest last, as `(name, location)`.
    pub(crate) recent: Vec<FrameSummary>,
}

const RECENT_TAIL_CALLERS: usize = 4;

/// A frame's function name and its (file, line, column).
pub(crate) type FrameSummary = (String, Option<(String, usize, usize)>);

/// A debugger attached to a VM.
pub struct DebugState {
    pub(crate) control: DebugControl,
    observer: Box<dyn VmObserver>,
}

impl DebugState {
    pub fn new(observer: Box<dyn VmObserver>) -> Self {
        Self {
            control: DebugControl {
                breakpoints: HashSet::new(),
                show_internals: false,
                mode: StepMode::Continue,
                last_line: Vec::new(),
                tail_calls: Vec::new(),
            },
            observer,
        }
    }
}

impl VM {
    /// Attaches a debugger and lets it set breakpoints before the program
    /// runs. The program must be built with `-g` for it to stop anywhere.
    pub fn attach_debugger(&mut self, observer: Box<dyn VmObserver>) {
        let mut state = Box::new(DebugState::new(observer));
        let resume = {
            let mut inspector = Inspector::new(self, &mut state.control);
            state.observer.on_start(&mut inspector)
        };
        self.apply_resume(&mut state.control, resume);
        self.debugger = Some(state);
    }

    /// Tells the observer the program has finished.
    pub fn detach_debugger(&mut self) {
        if let Some(mut state) = self.debugger.take() {
            state.observer.on_exit();
        }
    }

    /// `OpBreakpoint`: decides whether site `site` of the current function
    /// stops, and if so hands the VM to the observer.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_site(&mut self, site: usize) {
        let Some(mut state) = self.debugger.take() else {
            return;
        };
        if let Some(reason) = self.should_stop(&mut state.control, site) {
            let resume = {
                let mut inspector = Inspector::new(self, &mut state.control);
                state.observer.on_stop(reason, &mut inspector)
            };
            self.apply_resume(&mut state.control, resume);
            if resume == Resume::Quit {
                state.observer.on_exit();
                use std::io::Write;
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
        }
        self.debugger = Some(state);
    }

    fn should_stop(&self, control: &mut DebugControl, site: usize) -> Option<StopReason> {
        let depth = self.frame_index;
        let function = &self.frames[depth].closure.function;
        let info = function.debug_info.as_ref()?;
        let location = info.sites.get(site)?.location.as_ref()?;
        let file = info.file_for(location.file_id)?;
        let line = location.span.start.line;
        let function_key = std::sync::Arc::as_ptr(function) as usize;

        // One stop per line: the statement, arm and stage sites of a line
        // are one place to the person debugging.
        if control.last_line.len() <= depth {
            control.last_line.resize(depth + 1, None);
        }
        let here = (function_key, line);
        if control.last_line[depth].replace(here) == Some(here) {
            return None;
        }
        let on_breakpoint = control.breakpoints.contains(&LineBreakpoint {
            file: file.to_string(),
            line,
        });
        let artificial = inspect::is_artificial(info.name.as_deref());
        let stepping = !artificial || control.show_internals;
        let step_stop = stepping
            && match control.mode {
                StepMode::Continue => false,
                StepMode::StepIn => true,
                StepMode::StepOver { depth: from } => depth <= from,
                StepMode::StepOut { depth: from } => depth < from,
            };
        if !on_breakpoint && !step_stop {
            return None;
        }
        Some(if on_breakpoint {
            StopReason::Breakpoint
        } else {
            StopReason::Step
        })
    }

    fn apply_resume(&self, control: &mut DebugControl, resume: Resume) {
        let depth = self.frame_index;
        control.mode = match resume {
            Resume::Continue | Resume::Quit => StepMode::Continue,
            Resume::StepIn => StepMode::StepIn,
            Resume::StepOver => StepMode::StepOver { depth },
            Resume::StepOut => StepMode::StepOut { depth },
        };
    }

    /// A tail call is about to replace the current frame's function: count
    /// it, while a debugger is attached, so the stack can say so.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_note_tail_call(&mut self) {
        let index = self.frame_index;
        let caller = inspect::frame_summary(self, index);
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        let table = &mut state.control.tail_calls;
        if table.len() <= index {
            table.resize_with(index + 1, TailCalls::default);
        }
        // The function starts again: its first line is a new place to stop.
        if let Some(last) = state.control.last_line.get_mut(index) {
            *last = None;
        }
        let table = &mut state.control.tail_calls;
        let entry = &mut table[index];
        entry.count += 1;
        entry.recent.push(caller);
        if entry.recent.len() > RECENT_TAIL_CALLERS {
            entry.recent.remove(0);
        }
    }

    /// A new frame starts at `index`: forget the tail calls an earlier frame
    /// at that index made.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_note_push(&mut self) {
        let index = self.frame_index;
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        if let Some(last) = state.control.last_line.get_mut(index) {
            *last = None;
        }
        if let Some(entry) = state.control.tail_calls.get_mut(index) {
            *entry = TailCalls::default();
        }
    }
}
