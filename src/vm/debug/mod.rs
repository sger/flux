//! The VM debugger engine (proposal 0188 S2, S3).
//!
//! A `-g` build has an `OpBreakpoint` at every stop site: function entries,
//! statements, match arms, `|>` stages and effect operations. Without a
//! debugger attached the opcode does nothing. With one, each site checks
//! the breakpoints and the step mode, and a stop hands the paused VM to the
//! [`VmObserver`] through an [`Inspector`], which reads the stack, locals and
//! values and sets breakpoints. The observer's answer says how to go on.
//!
//! Going back (S3): every site run is counted, and the count is the
//! program's *position*. Checkpoints of the VM are taken as it runs. To go
//! back, the debugger restores a checkpoint and runs forward again without
//! stopping, effects answered from the session's recording, to the position
//! it wants. Finding that position takes a first silent run over the stretch
//! before the stop, noting the last site that matches (the last breakpoint
//! hit, the last line of this call, ...).
//!
//! The observer is the front end: `flux debug`'s command reader now, the
//! VS Code adapter (S4) later.

mod checkpoint;
mod effects;
mod inspect;
mod values;

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub use inspect::{FrameKind, HandlerView, Inspector, StackFrame, Variable};

pub use values::ValueView;

use crate::runtime::replay;

use super::VM;
use checkpoint::{Checkpoint, Checkpoints};

/// Why the program stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    Step,
    /// Going back found nothing earlier: this is the first stop.
    StartOfRecording,
    /// The front end asked the running program to stop.
    Pause,
    /// A runtime error the debugger catches, where it happened, before the
    /// program ends with it. [`Inspector::exception`] has the message.
    Exception(ExceptionKind),
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Breakpoint => "breakpoint",
            StopReason::Step => "step",
            StopReason::StartOfRecording => "start of the recording",
            StopReason::Pause => "pause",
            StopReason::Exception(_) => "exception",
        }
    }
}

/// The runtime errors the debugger can stop at (0188 S7). Async and fiber
/// failures come with fibers, in S8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExceptionKind {
    Panic,
    DivisionByZero,
    UnhandledEffect,
    /// Any other runtime error: a failed `unwrap`, a type error, ...
    RuntimeError,
}

impl ExceptionKind {
    pub const ALL: [ExceptionKind; 4] = [
        ExceptionKind::Panic,
        ExceptionKind::DivisionByZero,
        ExceptionKind::UnhandledEffect,
        ExceptionKind::RuntimeError,
    ];

    /// The name commands and the debug protocol use.
    pub fn id(self) -> &'static str {
        match self {
            ExceptionKind::Panic => "panic",
            ExceptionKind::DivisionByZero => "division_by_zero",
            ExceptionKind::UnhandledEffect => "unhandled_effect",
            ExceptionKind::RuntimeError => "runtime_error",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "panic" => Some(ExceptionKind::Panic),
            "division" | "division_by_zero" => Some(ExceptionKind::DivisionByZero),
            "unhandled" | "unhandled_effect" => Some(ExceptionKind::UnhandledEffect),
            "errors" | "runtime_error" => Some(ExceptionKind::RuntimeError),
            _ => None,
        }
    }

    /// What the stop says it is.
    pub fn label(self) -> &'static str {
        match self {
            ExceptionKind::Panic => "panic",
            ExceptionKind::DivisionByZero => "division by zero",
            ExceptionKind::UnhandledEffect => "unhandled effect",
            ExceptionKind::RuntimeError => "runtime error",
        }
    }

    /// The kinds a session catches until told otherwise.
    pub fn caught_by_default(self) -> bool {
        !matches!(self, ExceptionKind::RuntimeError)
    }

    /// The kind of a VM error and its message, without the diagnostic's
    /// decoration.
    pub fn classify(error: &str) -> (ExceptionKind, String) {
        let plain = crate::vm::trace::strip_ansi(error);
        let plain = plain.trim();
        let lower = plain.to_lowercase();
        if let Some(message) = plain.strip_prefix("panic: ") {
            return (ExceptionKind::Panic, message.to_string());
        }
        if lower.contains("e1008") || lower.contains("division by zero") {
            return (
                ExceptionKind::DivisionByZero,
                "division by zero".to_string(),
            );
        }
        if lower.starts_with("unhandled effect") || lower.starts_with("unhandled operation") {
            return (ExceptionKind::UnhandledEffect, plain.to_string());
        }
        let first = plain
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("runtime error");
        (ExceptionKind::RuntimeError, first.to_string())
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
    /// Go back to the previous place a step would stop.
    ReverseStep,
    /// Go back to the previous line of this call, or of its callers.
    ReverseNext,
    /// Go back to where this call was made.
    ReverseFinish,
    /// Go back to the previous breakpoint hit.
    ReverseContinue,
    /// Go to a position, earlier or later.
    GoTo(u64),
    /// Go to the last stop before effect `n` of the recording happened.
    ToEvent(u64),
    /// End the program now.
    Quit,
}

impl Resume {
    pub fn goes_back(self) -> bool {
        matches!(
            self,
            Resume::ReverseStep
                | Resume::ReverseNext
                | Resume::ReverseFinish
                | Resume::ReverseContinue
                | Resume::GoTo(_)
                | Resume::ToEvent(_)
        )
    }
}

/// The front end of a debugging session.
pub trait VmObserver {
    /// Called once before the program runs, to set breakpoints.
    fn on_start(&mut self, inspector: &mut Inspector<'_>) -> Resume;
    /// Called at each stop.
    fn on_stop(&mut self, reason: StopReason, inspector: &mut Inspector<'_>) -> Resume;
    /// Called when the program has finished.
    fn on_exit(&mut self) {}
    /// Called at a stop site after [`request_interrupt`], while the program
    /// runs forward: the front end has something to say, such as new
    /// breakpoints. Returns true to stop here.
    fn on_interrupt(&mut self, _inspector: &mut Inspector<'_>) -> bool {
        false
    }
}

static INTERRUPT: AtomicBool = AtomicBool::new(false);

/// Asks the running program to call [`VmObserver::on_interrupt`] at its next
/// stop site. Safe to call from any thread.
pub fn request_interrupt() {
    INTERRUPT.store(true, Ordering::Release);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StepMode {
    Continue,
    StepIn,
    /// Stop at a site whose logical depth is at most `depth`, or in the
    /// call `serial` (a resumed frame is the same call, deeper).
    StepOver {
        depth: usize,
        serial: u64,
    },
    /// Stop at a site whose frame index is below `depth`.
    StepOut {
        depth: usize,
    },
    /// Run silently over positions before `search.end`, noting matches.
    Scan(Search),
    /// Run silently to `target` and stop there.
    RunTo {
        target: u64,
        reason: StopReason,
        /// The site expected there: (function, site index).
        expect: Option<(usize, usize)>,
    },
    /// Run forward until effect `event` has happened, then go to it.
    ToEvent {
        event: u64,
    },
}

/// What going back is looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Goal {
    /// Any place a step would stop.
    Step,
    /// A line of the call with this serial, or of its callers.
    Next,
    /// A line of a caller.
    Finish,
    /// A breakpoint hit.
    Breakpoint,
}

/// A search backwards from `end`, one stretch between checkpoints at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Search {
    goal: Goal,
    /// Positions before this are scanned.
    end: u64,
    /// Where the current stretch starts: the checkpoint the scan ran from.
    from: u64,
    /// The serial of the frame the search started in, and its callers'.
    serial: u64,
    callers: Vec<u64>,
    /// The latest match so far: position and (function, site).
    found: Option<(u64, (usize, usize))>,
    /// The first place a step would stop, should nothing match.
    first_stop: Option<(u64, (usize, usize))>,
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
    /// Sites run so far.
    pub(crate) position: u64,
    /// Per frame index, a number naming the call running there, so going
    /// back can tell this call's lines from a sibling call's.
    serials: Vec<u64>,
    next_serial: u64,
    checkpoints: Checkpoints,
    /// The checkpoint position to restore once the run has unwound to the
    /// top-level loop.
    rewind: Option<u64>,
    /// Frames performs took off the stack, per arm (S7).
    pub(crate) suspensions: Vec<effects::Suspension>,
    /// A perform's frames, until its arm's frame is pushed.
    pending_suspension: Option<effects::Suspension>,
    /// Frames non-tail resumes took off the stack, outermost first.
    pub(crate) resumes: Vec<effects::ResumeNote>,
    /// The instruction the top frame is shown at, when that is not its `ip`:
    /// a stop right after `resume`, at the perform.
    pub(crate) top_ip: Option<usize>,
    /// The runtime errors to stop at.
    pub(crate) catch: HashSet<ExceptionKind>,
    /// The error stopped at, while the program is stopped there and after:
    /// it unwinds through nested dispatch loops, each of which sees it.
    pub(crate) exception: Option<(ExceptionKind, String, String)>,
}

impl DebugControl {
    fn stepping_in(&self) -> bool {
        matches!(self.mode, StepMode::StepIn)
    }
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

/// The error a rewind unwinds the VM with, out of any nested dispatch
/// loops, to [`VM::run`], which restores the checkpoint. The VM checks its
/// debugger for a pending rewind, not this text.
const REWIND: &str = "the debugger is going back";

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
                position: 0,
                serials: vec![0],
                next_serial: 1,
                checkpoints: Checkpoints::new(),
                rewind: None,
                suspensions: Vec::new(),
                pending_suspension: None,
                resumes: Vec::new(),
                top_ip: None,
                catch: ExceptionKind::ALL
                    .into_iter()
                    .filter(|kind| kind.caught_by_default())
                    .collect(),
                exception: None,
            },
            observer,
        }
    }
}

// ── Whether going back is possible ──────────────────────────────────────

static ASYNC_STARTED: AtomicBool = AtomicBool::new(false);
static REVERSE_OFF: Mutex<Option<String>> = Mutex::new(None);

/// Going back is off for the rest of the run, for `reason`.
pub(crate) fn turn_off_reverse(reason: String) {
    let mut off = REVERSE_OFF
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    off.get_or_insert(reason);
}

/// The program is entering `run_async`. The scheduler's state lives outside
/// the VM, where checkpoints do not reach yet (0188 S8), so going back is
/// off from here on, and a debug session's own recording stops.
pub(crate) fn note_async_start() {
    if ASYNC_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    replay::stop_implicit();
    turn_off_reverse(
        "the program has run async code, which going back does not cover yet (0188 S8)".to_string(),
    );
}

/// Why going back is not possible now, if it is not.
pub(crate) fn reverse_unavailable() -> Option<String> {
    if let Some(reason) = REVERSE_OFF
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Some(reason);
    }
    if replay::is_active() && replay::mark().is_none() {
        return Some("this session's recording does not keep its events".to_string());
    }
    None
}

/// Where a stop site is, and what a stop there would be.
struct SiteHere {
    /// The VM frame index.
    depth: usize,
    /// The depth on the logical stack, which counts the frames a perform or
    /// a resume took off the VM's stack.
    logical: usize,
    identity: (usize, usize),
    /// The first site of its line in this frame since another line ran.
    fresh: bool,
    on_breakpoint: bool,
    /// Not compiler-made, or internals are shown.
    visible: bool,
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
        // Going back needs a stop to go back from.
        let resume = if resume.goes_back() {
            Resume::StepIn
        } else {
            resume
        };
        self.apply_forward(&mut state.control, resume);
        self.debugger = Some(state);
    }

    /// Tells the observer the program has finished.
    pub fn detach_debugger(&mut self) {
        if let Some(mut state) = self.debugger.take() {
            state.observer.on_exit();
        }
    }

    /// Runs the program under the debugger: the first checkpoint, before
    /// anything ran, then the dispatch loop, restarted from a checkpoint
    /// each time the debugger goes back.
    pub(super) fn run_debugged(&mut self) -> Result<(), String> {
        if let Some(mut state) = self.debugger.take() {
            if reverse_unavailable().is_none() {
                let checkpoint = Checkpoint::take(self, &state.control);
                state.control.checkpoints.push(checkpoint);
            }
            self.debugger = Some(state);
        }
        loop {
            let result = self.run_inner();
            let Some(mut state) = self.debugger.take() else {
                return result;
            };
            // A search that ran to the end of the program ends there.
            if result.is_ok()
                && state.control.rewind.is_none()
                && matches!(state.control.mode, StepMode::Scan(_))
            {
                self.end_scan_stretch(&mut state.control);
            }
            let rewind = state.control.rewind.take();
            let restored = rewind.and_then(|position| {
                // Restoring writes the control; the checkpoints stay as they are.
                let checkpoints =
                    std::mem::replace(&mut state.control.checkpoints, Checkpoints::empty());
                let found = checkpoints.at(position).map(|checkpoint| {
                    checkpoint.restore(self, &mut state.control);
                });
                state.control.checkpoints = checkpoints;
                found
            });
            self.debugger = Some(state);
            match (rewind, restored) {
                (Some(_), Some(())) => continue,
                (Some(position), None) => {
                    return Err(format!(
                        "debugger: no checkpoint at position {position} to go back to"
                    ));
                }
                (None, _) => return result,
            }
        }
    }

    /// `OpBreakpoint`: counts the site, takes a checkpoint when one is due,
    /// and decides whether site `site` of the current function stops; if so
    /// it hands the VM to the observer. An `Err` unwinds the run for the
    /// debugger to go back.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_site(&mut self, site: usize) -> Result<(), String> {
        let Some(mut state) = self.debugger.take() else {
            return Ok(());
        };
        let result = self.debug_site_with(&mut state, site);
        self.debugger = Some(state);
        result
    }

    /// An instruction failed with `error`. When the debugger catches its
    /// kind, stop where it happened, with the stack as it was; then the
    /// error goes on as before and ends the program. Returns the error to
    /// unwind with: `error`, or the debugger's own to go back.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_on_error(&mut self, error: String) -> String {
        if error == REWIND || error == "__fiber_error__" {
            return error;
        }
        let Some(mut state) = self.debugger.take() else {
            return error;
        };
        let seen = state
            .control
            .exception
            .as_ref()
            .is_some_and(|(_, _, raw)| *raw == error);
        let (kind, message) = ExceptionKind::classify(&error);
        let result =
            if seen || state.control.rewind.is_some() || !state.control.catch.contains(&kind) {
                Ok(())
            } else if matches!(
                state.control.mode,
                StepMode::Scan(_) | StepMode::RunTo { .. }
            ) {
                // Going back passes the error's place without stopping: the
                // stop before it is where the search ends.
                Ok(())
            } else {
                state.control.exception = Some((kind, message, error.clone()));
                self.stop(&mut state, StopReason::Exception(kind))
            };
        self.debugger = Some(state);
        match result {
            Err(rewind) => rewind,
            Ok(()) => error,
        }
    }

    fn debug_site_with(&mut self, state: &mut DebugState, site: usize) -> Result<(), String> {
        let control = &mut state.control;
        if self.nested_runs == 0
            && control.checkpoints.due(control.position)
            && control.checkpoints.len() > 0
            && reverse_unavailable().is_none()
        {
            let checkpoint = Checkpoint::take(self, control);
            control.checkpoints.push(checkpoint);
        }
        control.position += 1;
        replay::set_position(control.position);
        let Some(here) = self.site_here(control, site) else {
            return Ok(());
        };
        let position = control.position;

        // The front end has something to say. Not while going back: the
        // run is passing places it already stopped at.
        if INTERRUPT.load(Ordering::Relaxed)
            && !matches!(control.mode, StepMode::Scan(_) | StepMode::RunTo { .. })
        {
            INTERRUPT.store(false, Ordering::Relaxed);
            let pause = {
                let mut inspector = Inspector::new(self, &mut state.control);
                state.observer.on_interrupt(&mut inspector)
            };
            if pause {
                return self.stop(state, StopReason::Pause);
            }
        }
        let control = &mut state.control;

        let reason = match &mut control.mode {
            StepMode::Continue => {
                (here.fresh && here.on_breakpoint).then_some(StopReason::Breakpoint)
            }
            StepMode::StepIn => forward_stop(&here, true),
            StepMode::StepOver {
                depth: from,
                serial,
            } => {
                let same_call = control.serials.get(here.depth) == Some(serial);
                forward_stop(&here, here.logical <= *from || same_call)
            }
            StepMode::StepOut { depth: from } => forward_stop(&here, here.logical < *from),
            StepMode::Scan(search) => {
                if position >= search.end {
                    self.end_scan_stretch(control);
                    return Err(REWIND.to_string());
                }
                let serial = control.serials.get(here.depth).copied().unwrap_or(0);
                if search.first_stop.is_none() && here.fresh && here.visible {
                    search.first_stop = Some((position, here.identity));
                }
                if search.matches(&here, serial) {
                    search.found = Some((position, here.identity));
                }
                None
            }
            StepMode::RunTo {
                target,
                reason,
                expect,
            } => {
                if position < *target {
                    None
                } else if expect.is_some_and(|expect| expect != here.identity) {
                    return Err(format!(
                        "debugger: running the recording again reached a different place at \
                         position {position}; the run is not deterministic here"
                    ));
                } else {
                    Some(*reason)
                }
            }
            StepMode::ToEvent { event } => {
                let event = *event;
                match replay::event_position(event as usize) {
                    Some(at) => {
                        self.start_search(control, Goal::Step, at + 1);
                        return Err(REWIND.to_string());
                    }
                    None => None,
                }
            }
        };
        let Some(reason) = reason else {
            return Ok(());
        };
        self.stop(state, reason)
    }

    /// Hands the paused VM to the observer and acts on its answer.
    fn stop(&mut self, state: &mut DebugState, reason: StopReason) -> Result<(), String> {
        let resume = {
            let mut inspector = Inspector::new(self, &mut state.control);
            state.observer.on_stop(reason, &mut inspector)
        };
        let control = &mut state.control;
        match resume {
            Resume::Quit => {
                control.mode = StepMode::Continue;
                state.observer.on_exit();
                use std::io::Write;
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            _ if resume.goes_back() && reverse_unavailable().is_some() => {
                // The front end checks first; should it not, stay here.
                self.stop(state, reason)
            }
            Resume::ReverseStep => self.go_back(control, Goal::Step),
            Resume::ReverseNext => self.go_back(control, Goal::Next),
            Resume::ReverseFinish => self.go_back(control, Goal::Finish),
            Resume::ReverseContinue => self.go_back(control, Goal::Breakpoint),
            Resume::GoTo(target) => {
                let here = control.position;
                if target > here {
                    control.mode = StepMode::RunTo {
                        target,
                        reason: StopReason::Step,
                        expect: None,
                    };
                    Ok(())
                } else {
                    let target = target.max(1);
                    self.rewind_to_run(control, target, StopReason::Step, None);
                    Err(REWIND.to_string())
                }
            }
            Resume::ToEvent(event) => match replay::event_position(event as usize) {
                Some(at) => {
                    self.start_search(control, Goal::Step, at + 1);
                    Err(REWIND.to_string())
                }
                None => {
                    control.mode = StepMode::ToEvent { event };
                    Ok(())
                }
            },
            forward => {
                self.apply_forward(control, forward);
                Ok(())
            }
        }
    }

    fn apply_forward(&self, control: &mut DebugControl, resume: Resume) {
        let depth = control.logical_depth(self.frame_index, self.frame_index);
        let serial = control.serials.get(self.frame_index).copied().unwrap_or(0);
        control.mode = match resume {
            Resume::StepIn => StepMode::StepIn,
            Resume::StepOver => StepMode::StepOver { depth, serial },
            Resume::StepOut => StepMode::StepOut { depth },
            _ => StepMode::Continue,
        };
    }

    /// Starts going back from the current stop.
    fn go_back(&self, control: &mut DebugControl, goal: Goal) -> Result<(), String> {
        self.start_search(control, goal, control.position);
        Err(REWIND.to_string())
    }

    /// Starts a search for the latest position before `end` that meets
    /// `goal`, from the newest checkpoint before it.
    fn start_search(&self, control: &mut DebugControl, goal: Goal, end: u64) {
        let depth = self.frame_index;
        let serial = control.serials.get(depth).copied().unwrap_or(0);
        let callers = control.serials[..depth.min(control.serials.len())].to_vec();
        let from = control
            .checkpoints
            .before(end)
            .map_or(0, |checkpoint| checkpoint.position);
        control.mode = StepMode::Scan(Search {
            goal,
            end,
            from,
            serial,
            callers,
            found: None,
            first_stop: None,
        });
        control.rewind = Some(from);
    }

    /// A scanned stretch has ended: go to what it found, or scan the stretch
    /// before it, or, at the start, stop at the first stop.
    fn end_scan_stretch(&self, control: &mut DebugControl) {
        let StepMode::Scan(search) = &mut control.mode else {
            return;
        };
        if let Some((target, identity)) = search.found {
            let reason = if search.goal == Goal::Breakpoint {
                StopReason::Breakpoint
            } else {
                StopReason::Step
            };
            self.rewind_to_run(control, target, reason, Some(identity));
            return;
        }
        if search.from == 0 {
            match search.first_stop {
                Some((target, identity)) => {
                    self.rewind_to_run(
                        control,
                        target,
                        StopReason::StartOfRecording,
                        Some(identity),
                    );
                }
                None => {
                    // Nothing stops before the search's end: stop there.
                    let end = search.end;
                    self.rewind_to_run(control, end, StopReason::StartOfRecording, None);
                }
            }
            return;
        }
        // Scan the stretch before this one, up to and including the site
        // this stretch's checkpoint stood at.
        let end = search.from + 1;
        let from = control
            .checkpoints
            .before(search.from)
            .map_or(0, |checkpoint| checkpoint.position);
        search.end = end;
        search.from = from;
        search.first_stop = None;
        control.rewind = Some(from);
    }

    /// Restores the newest checkpoint before `target` and runs silently to
    /// it.
    fn rewind_to_run(
        &self,
        control: &mut DebugControl,
        target: u64,
        reason: StopReason,
        expect: Option<(usize, usize)>,
    ) {
        let from = control
            .checkpoints
            .before(target)
            .map_or(0, |checkpoint| checkpoint.position);
        control.mode = StepMode::RunTo {
            target,
            reason,
            expect,
        };
        control.rewind = Some(from);
    }

    /// Where site `site` is and what a stop there would be. Notes the line
    /// as run in this frame.
    fn site_here(&self, control: &mut DebugControl, site: usize) -> Option<SiteHere> {
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
        let fresh = control.last_line[depth].replace(here) != Some(here);
        let on_breakpoint = control.breakpoints.contains(&LineBreakpoint {
            file: file.to_string(),
            line,
        });
        let artificial = inspect::is_artificial(info.name.as_deref());
        Some(SiteHere {
            depth,
            logical: control.logical_depth(depth, depth),
            identity: (function_key, site),
            fresh,
            on_breakpoint,
            visible: !artificial || control.show_internals,
        })
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
    /// at that index made, and number the call.
    #[cold]
    #[inline(never)]
    pub(super) fn debug_note_push(&mut self) {
        let index = self.frame_index;
        let Some(state) = self.debugger.as_mut() else {
            return;
        };
        let control = &mut state.control;
        if let Some(last) = control.last_line.get_mut(index) {
            *last = None;
        }
        if let Some(entry) = control.tail_calls.get_mut(index) {
            *entry = TailCalls::default();
        }
        if control.serials.len() <= index {
            control.serials.resize(index + 1, 0);
        }
        control.serials[index] = control.next_serial;
        control.next_serial += 1;
        // The arm of a perform that took frames off the stack.
        if let Some(mut suspension) = control.pending_suspension.take() {
            if control.resumes.is_empty() {
                let serials = &control.serials;
                control.suspensions.retain(|old| {
                    old.arm_index < index && serials.get(old.arm_index) == Some(&old.arm_serial)
                });
            }
            suspension.arm_index = index;
            suspension.arm_serial = control.serials[index];
            control.suspensions.push(suspension);
        }
    }
}

/// A forward step's stop: a fresh line on a breakpoint, or a fresh visible
/// line the step mode accepts.
fn forward_stop(here: &SiteHere, mode_accepts: bool) -> Option<StopReason> {
    if !here.fresh {
        None
    } else if here.on_breakpoint {
        Some(StopReason::Breakpoint)
    } else if here.visible && mode_accepts {
        Some(StopReason::Step)
    } else {
        None
    }
}

impl Search {
    fn matches(&self, here: &SiteHere, serial: u64) -> bool {
        if !here.fresh {
            return false;
        }
        match self.goal {
            Goal::Step => here.visible,
            Goal::Next => here.visible && (serial == self.serial || self.callers.contains(&serial)),
            Goal::Finish => here.visible && self.callers.contains(&serial),
            Goal::Breakpoint => here.on_breakpoint,
        }
    }
}
