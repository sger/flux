//! Reading a paused VM: its stack, each frame's variables, and breakpoints.

use std::rc::Rc;
use std::sync::Arc;

use crate::bytecode::debug_info::FunctionDebugInfo;
use crate::runtime::closure::Closure;
use crate::runtime::compiled_function::CompiledFunction;
use crate::runtime::value::Value;

use super::super::{VM, slot};
use super::values::ValueView;
use super::{DebugControl, LineBreakpoint};

/// One frame of the call stack, newest first.
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// The frame's place on the logical stack, counted from the bottom, for
    /// [`Inspector::locals`]. Without effects in flight it is the VM frame
    /// index.
    pub index: usize,
    pub name: String,
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    /// Compiler-made (dictionary methods, wrappers); hidden unless the
    /// debugger shows internals.
    pub artificial: bool,
    /// Calls this frame replaced by tail calls, and the latest of them.
    pub elided: Option<(u64, String)>,
    pub kind: FrameKind,
}

/// What a line of the logical stack is (0188 S7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// A frame running or waiting for a call to return.
    Live,
    /// A frame a perform took off the stack, waiting in a continuation
    /// below the arm handling it. A handler that never resumes has dropped
    /// it: then it is not `resumable`.
    Suspended { resumable: bool },
    /// Not a frame: `handling Effect.op — performed at file:line:col`,
    /// between an arm and the frames it suspended. Its text is the name.
    Label,
}

/// An active effect handler, innermost first.
#[derive(Debug, Clone)]
pub struct HandlerView {
    pub effect: String,
    /// A parameterized handler's state.
    pub state: Option<ValueView>,
    /// The function the `handle` is in.
    pub handled_in: Option<String>,
    /// A default handler the compiler put around `main`.
    pub internal: bool,
}

/// Where the logical stack's frame comes from.
#[derive(Debug, Clone)]
enum FrameSource {
    /// `VM.frames[index]`.
    Live(usize),
    /// Frame `frame` of resume note `note`.
    Hidden {
        note: usize,
        frame: usize,
    },
    /// Frame `frame` (outermost first) of the suspension below the arm at
    /// VM frame `arm`.
    Suspended {
        arm: usize,
        frame: usize,
    },
    /// Frame `frame` (outermost first) of a parked fiber's continuation (S8).
    Parked {
        continuation: Rc<std::cell::RefCell<crate::runtime::continuation::Continuation>>,
        frame: usize,
    },
    Label,
}

/// A frame's function, instruction and local slots, wherever it lives.
struct FrameView {
    closure: Rc<Closure>,
    ip: usize,
    slots: Vec<Value>,
}

/// A named value in a frame.
#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub value: ValueView,
    pub artificial: bool,
}

/// A paused VM, as the observer sees it.
pub struct Inspector<'a> {
    pub(super) vm: &'a VM,
    pub(super) control: &'a mut DebugControl,
}

/// A compiler-made function: dictionaries, wrappers and other `__` names.
/// An instance method (`__tc_…`) is code the person wrote, so it is not.
pub(super) fn is_artificial(name: Option<&str>) -> bool {
    name.is_some_and(|name| {
        // `Flow.Primops.__primop_print`: the name, not its module path. A
        // description after the name (`__default_handler Console.print`) is
        // not part of it.
        let name = name.split(' ').next().unwrap_or(name);
        let name = name.rsplit('.').next().unwrap_or(name);
        name.starts_with("__") && !name.starts_with(INSTANCE_METHOD)
    })
}

const INSTANCE_METHOD: &str = "__tc_";

/// How a function is named on the stack: an instance method as
/// `Class<Type>.method`, everything else as compiled.
pub(crate) fn display_name(name: &str) -> String {
    name.strip_prefix(INSTANCE_METHOD)
        .and_then(demangle_instance_method)
        .unwrap_or_else(|| name.to_string())
}

/// `Class_Type_method`, where a module-qualified class or type is written
/// `m<len>_<module as hex>_<Name>` (see `types::class_env`).
fn demangle_instance_method(mangled: &str) -> Option<String> {
    let (class, rest) = mangled_symbol(mangled)?;
    let (ty, method) = mangled_type(rest)?;
    (!method.is_empty()).then(|| format!("{class}<{ty}>.{method}"))
}

/// One mangled name and the text after its separating `_`.
fn mangled_symbol(text: &str) -> Option<(String, &str)> {
    if let Some(qualified) = module_qualified(text) {
        return Some(qualified);
    }
    let (name, rest) = text.split_once('_')?;
    Some((name.to_string(), rest))
}

/// A type key: a name with any `<…>` arguments, up to its separating `_`.
fn mangled_type(text: &str) -> Option<(String, &str)> {
    if let Some(qualified) = module_qualified(text) {
        return Some(qualified);
    }
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            '_' if depth == 0 => return Some((text[..index].to_string(), &text[index + 1..])),
            _ => {}
        }
    }
    None
}

/// `m<len>_<hex>_<Name>_rest` as `Name` and `rest`: the module is left
/// out, as a stack line has no room for it.
fn module_qualified(text: &str) -> Option<(String, &str)> {
    let digits = text.strip_prefix('m')?;
    let (len, rest) = digits.split_once('_')?;
    let len: usize = len.parse().ok()?;
    let hex = rest.get(..len * 2)?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.get(len * 2..)?.strip_prefix('_')?;
    let (name, rest) = rest.split_once('_')?;
    Some((name.to_string(), rest))
}

/// The name and location of frame `index`, for the tail-call record.
pub(super) fn frame_summary(vm: &VM, index: usize) -> super::FrameSummary {
    let function = &vm.frames[index].closure.function;
    let name = function_name(function.debug_info.as_ref(), index);
    let location = location_at(function, vm.frame_ip(index));
    (name, location)
}

fn function_name(info: Option<&FunctionDebugInfo>, index: usize) -> String {
    // The program's entry code, named as runtime stack traces name it.
    if index == 0 {
        return "<entry>".to_string();
    }
    match info.and_then(|info| info.name.as_deref()) {
        Some(name) => display_name(name),
        None => "<anonymous>".to_string(),
    }
}

fn location_at(function: &CompiledFunction, ip: usize) -> Option<(String, usize, usize)> {
    let info = function.debug_info.as_ref()?;
    let location = info.location_at(ip)?;
    let file = info.file_for(location.file_id)?.to_string();
    Some((
        file,
        location.span.start.line,
        location.span.start.column + 1,
    ))
}

/// Whether `file` (as the bytecode names it) is the file the user wrote
/// (`app.flx`, `src/app.flx` or an absolute path).
fn same_file(file: &str, wanted: &str) -> bool {
    FileMatcher::new(wanted).matches(file)
}

/// Matches the files the bytecode names against one the user wrote. Two
/// absolute paths can name one file through a symlink (`/var` is
/// `/private/var` on macOS), so those are compared resolved.
struct FileMatcher<'a> {
    wanted: &'a std::path::Path,
    resolved: Option<std::path::PathBuf>,
    seen: std::collections::HashMap<String, bool>,
}

impl<'a> FileMatcher<'a> {
    fn new(wanted: &'a str) -> Self {
        let wanted = std::path::Path::new(wanted);
        Self {
            wanted,
            resolved: std::fs::canonicalize(wanted).ok(),
            seen: std::collections::HashMap::new(),
        }
    }

    fn matches(&mut self, file: &str) -> bool {
        if let Some(known) = self.seen.get(file) {
            return *known;
        }
        let path = std::path::Path::new(file);
        let matched = path == self.wanted
            || path.ends_with(self.wanted)
            || self.wanted.ends_with(path)
            || self.resolved.as_ref().is_some_and(|resolved| {
                std::fs::canonicalize(path).ok().as_ref() == Some(resolved)
            });
        self.seen.insert(file.to_string(), matched);
        matched
    }
}

impl<'a> Inspector<'a> {
    pub(super) fn new(vm: &'a VM, control: &'a mut DebugControl) -> Self {
        Self { vm, control }
    }

    /// The call stack, newest first: the logical stack, which shows the
    /// frames a perform suspended below the arm handling it, and the arm a
    /// running `resume` will return to. Compiler-made frames are left out
    /// unless internals are shown.
    pub fn stack(&self) -> Vec<StackFrame> {
        let mut frames: Vec<StackFrame> = self
            .layout()
            .into_iter()
            .map(|(_, frame)| frame)
            .filter(|frame| !frame.artificial || self.control.show_internals)
            .collect();
        frames.reverse();
        frames
    }

    /// The logical stack, oldest first, with where each frame comes from.
    fn layout(&self) -> Vec<(FrameSource, StackFrame)> {
        match self.other_thread() {
            Some(super::threads::OtherThread::Parked {
                continuation,
                below: Some(below),
            }) => {
                let mut out = self.live_layout(0, below.min(self.vm.frame_index));
                out.extend(parked_layout(&continuation));
                for (index, (_, frame)) in out.iter_mut().enumerate() {
                    frame.index = index;
                }
                return out;
            }
            Some(other) => return other_layout(other),
            None => {}
        }
        // A child fiber's stack starts above `run_async`'s caller (S8).
        let top = self.vm.frame_index;
        let first = self.fiber_base().map_or(0, |base| base + 1).min(top);
        self.live_layout(first, top)
    }

    /// The VM's frames `first ..= last`, with what effects took off them.
    fn live_layout(&self, first: usize, last: usize) -> Vec<(FrameSource, StackFrame)> {
        let vm = self.vm;
        let control = &*self.control;
        let top = vm.frame_index;
        let mut out: Vec<(FrameSource, StackFrame)> = Vec::new();
        let push = |out: &mut Vec<(FrameSource, StackFrame)>, source, mut frame: StackFrame| {
            frame.index = out.len();
            out.push((source, frame));
        };
        for index in first..=last {
            if let Some(suspension) = control
                .suspensions
                .iter()
                .rev()
                .find(|s| s.arm_index == index && control.suspension_live(s, top))
            {
                for (k, suspended) in suspension.frames.iter().enumerate() {
                    let (name, location) = &suspended.summary;
                    push(
                        &mut out,
                        FrameSource::Suspended {
                            arm: index,
                            frame: k,
                        },
                        summary_frame(
                            name,
                            location.as_ref(),
                            suspended.artificial,
                            None,
                            FrameKind::Suspended {
                                resumable: suspension.resumable,
                            },
                        ),
                    );
                }
                let performed = suspension
                    .frames
                    .last()
                    .and_then(|frame| frame.summary.1.as_ref())
                    .map(|(file, line, column)| {
                        format!(" — performed at {}:{line}:{column}", short_file(file))
                    })
                    .unwrap_or_default();
                push(
                    &mut out,
                    FrameSource::Label,
                    summary_frame(
                        &format!("handling {}{performed}", suspension.operation),
                        None,
                        false,
                        None,
                        FrameKind::Label,
                    ),
                );
            }
            let function = &vm.frames[index].closure.function;
            let info = function.debug_info.as_ref();
            let mut name = function_name(info, index);
            let mut artificial = is_artificial(info.and_then(|info| info.name.as_deref()));
            let mut location = location_at(function, self.frame_ip(index));
            let mut calls = control.tail_calls.get(index).cloned();
            if location.is_none() && index > 0 {
                // An intrinsic (`panic`, `print`): no source of its own. A
                // tail call to it shows as the call, in the function that
                // made it, as native code does.
                match calls.as_mut().and_then(|calls| {
                    let caller = calls.recent.pop()?;
                    calls.count -= 1;
                    Some(caller)
                }) {
                    Some((caller, at)) => {
                        name = caller;
                        location = at;
                    }
                    None => artificial = true,
                }
            }
            let tail = elided(calls.as_ref());
            push(
                &mut out,
                FrameSource::Live(index),
                summary_frame(&name, location.as_ref(), artificial, tail, FrameKind::Live),
            );
            for (note_index, note) in control.resumes.iter().enumerate() {
                if note.entry != index {
                    continue;
                }
                for (k, hidden) in note.hidden.iter().enumerate() {
                    let function = &hidden.frame.closure.function;
                    let info = function.debug_info.as_ref();
                    let name = function_name(info, hidden.index);
                    let artificial = is_artificial(info.and_then(|info| info.name.as_deref()));
                    let location = location_at(function, hidden.ip);
                    push(
                        &mut out,
                        FrameSource::Hidden {
                            note: note_index,
                            frame: k,
                        },
                        summary_frame(
                            &name,
                            location.as_ref(),
                            artificial,
                            elided(Some(&hidden.tail_calls)),
                            FrameKind::Live,
                        ),
                    );
                }
            }
        }
        out
    }

    /// The instruction VM frame `index` is shown at.
    fn frame_ip(&self, index: usize) -> usize {
        match self.control.top_ip {
            Some(ip) if index == self.vm.frame_index => ip,
            _ => self.vm.frame_ip(index),
        }
    }

    /// The function, instruction and slots of logical frame `index`.
    fn frame_view(&self, index: usize) -> Option<FrameView> {
        let (source, _) = self.layout().into_iter().nth(index)?;
        match source {
            FrameSource::Live(index) => {
                let frame = self.vm.frames.get(index)?;
                let count = frame.closure.function.num_locals;
                let from = frame.base_pointer.min(self.vm.stack.len());
                let to = (frame.base_pointer + count).min(self.vm.stack.len());
                Some(FrameView {
                    closure: frame.closure.clone(),
                    ip: self.frame_ip(index),
                    slots: self.vm.stack[from..to]
                        .iter()
                        .map(slot::from_slot_ref)
                        .collect(),
                })
            }
            FrameSource::Hidden { note, frame } => {
                let note = self.control.resumes.get(note)?;
                let hidden = note.hidden.get(frame)?;
                let count = hidden.frame.closure.function.num_locals;
                let from = hidden.frame.base_pointer.checked_sub(note.stack_from)?;
                let to = (from + count).min(note.stack.len());
                Some(FrameView {
                    closure: hidden.frame.closure.clone(),
                    ip: hidden.ip,
                    slots: note.stack.get(from..to)?.to_vec(),
                })
            }
            FrameSource::Suspended { arm, frame } => {
                // The continuation is the arm's first argument, `resume`,
                // while the arm still holds it.
                let suspension = self
                    .control
                    .suspensions
                    .iter()
                    .rev()
                    .find(|s| s.arm_index == arm)?;
                let arm_frame = self.vm.frames.get(arm)?;
                let resume = self.vm.stack.get(arm_frame.base_pointer)?;
                let Value::Continuation(continuation) = slot::from_slot_ref(resume) else {
                    return None;
                };
                let continuation = continuation.borrow();
                if continuation.frames.len() != suspension.frames.len() {
                    return None;
                }
                let captured = continuation.frames.get(frame)?;
                let count = captured.closure.function.num_locals;
                let from = captured.base_pointer.checked_sub(continuation.entry_sp)?;
                let to = (from + count).min(continuation.stack.len());
                Some(FrameView {
                    closure: captured.closure.clone(),
                    ip: captured.ip.saturating_sub(1),
                    slots: continuation.stack.get(from..to)?.to_vec(),
                })
            }
            FrameSource::Parked {
                continuation,
                frame,
            } => {
                let continuation = continuation.borrow();
                let captured = continuation.frames.get(frame)?;
                let count = captured.closure.function.num_locals;
                let from = captured.base_pointer.checked_sub(continuation.entry_sp)?;
                let to = (from + count).min(continuation.stack.len());
                Some(FrameView {
                    closure: captured.closure.clone(),
                    ip: captured.ip.saturating_sub(1),
                    slots: continuation.stack.get(from..to)?.to_vec(),
                })
            }
            FrameSource::Label => None,
        }
    }

    /// The local variables of frame `index` that hold a value at its current
    /// instruction. A name bound twice shows its innermost binding.
    pub fn locals(&self, index: usize) -> Vec<Variable> {
        let Some(view) = self.frame_view(index) else {
            return Vec::new();
        };
        let Some(info) = view.closure.function.debug_info.as_ref() else {
            return Vec::new();
        };
        let ip = view.ip;
        let mut live: Vec<&crate::bytecode::debug_info::LocalVar> = info
            .locals
            .iter()
            .filter(|local| local.is_live_at(ip))
            .filter(|local| self.control.show_internals || !local.artificial)
            .collect();
        // Innermost binding of each name: the one bound last.
        live.sort_by_key(|local| std::cmp::Reverse(local.start_ip));
        let mut seen = std::collections::HashSet::new();
        live.retain(|local| seen.insert(local.name.clone()));
        live.sort_by_key(|local| local.slot);
        live.into_iter()
            .filter_map(|local| {
                let value = view.slots.get(local.slot as usize)?.clone();
                if matches!(value, Value::Uninit) {
                    return None;
                }
                Some(Variable {
                    name: local.name.clone(),
                    value: ValueView::new(value),
                    artificial: local.artificial,
                })
            })
            .collect()
    }

    /// The values frame `index`'s closure captured.
    pub fn captured(&self, index: usize) -> Vec<Variable> {
        let Some(view) = self.frame_view(index) else {
            return Vec::new();
        };
        let Some(info) = view.closure.function.debug_info.as_ref() else {
            return Vec::new();
        };
        info.free_names
            .iter()
            .zip(view.closure.free.iter())
            .filter(|(name, _)| self.control.show_internals || !name.starts_with("__"))
            .map(|(name, value)| Variable {
                name: name.clone(),
                value: ValueView::new(value.clone()),
                artificial: name.starts_with("__"),
            })
            .collect()
    }

    /// The value of `name` in frame `index`: a local, else a captured value.
    pub fn lookup(&mut self, index: usize, name: &str) -> Option<ValueView> {
        let show = self.control.show_internals;
        self.control.show_internals = true;
        let found = self
            .locals(index)
            .into_iter()
            .chain(self.captured(index))
            .find(|variable| variable.name == name)
            .map(|variable| variable.value);
        self.control.show_internals = show;
        found
    }

    /// The active effect handlers, innermost first. The default handlers
    /// the compiler puts around `main` are left out unless internals are
    /// shown.
    pub fn handlers(&self) -> Vec<HandlerView> {
        // A parked fiber keeps no handlers of its own on the VM (S8).
        if self.control.thread.is_some() {
            return Vec::new();
        }
        self.vm
            .handler_stack
            .iter()
            .rev()
            .map(|handler| {
                let arm_name = handler
                    .arms
                    .first()
                    .and_then(|arm| arm.closure.function.debug_info.as_ref())
                    .and_then(|info| info.name.clone())
                    .unwrap_or_default();
                let internal = arm_name.starts_with("__default_handler");
                let effect = handled_effect(&arm_name).unwrap_or_else(|| "<effect>".to_string());
                let handled_in = (handler.entry_frame_index <= self.vm.frame_index)
                    .then(|| frame_summary(self.vm, handler.entry_frame_index).0);
                HandlerView {
                    effect,
                    state: handler.state.clone().map(ValueView::new),
                    handled_in,
                    internal,
                }
            })
            .filter(|handler| !handler.internal || self.control.show_internals)
            .collect()
    }

    /// Sets a breakpoint at the first line at or after `line` in `file` that
    /// has code. Returns the line it was set on.
    pub fn set_breakpoint(&mut self, file: &str, line: usize) -> Result<usize, String> {
        let mut best: Option<(String, usize)> = None;
        let mut matcher = FileMatcher::new(file);
        for function in self.vm.all_functions() {
            let Some(info) = function.debug_info.as_ref() else {
                continue;
            };
            for site in &info.sites {
                let Some(location) = &site.location else {
                    continue;
                };
                let Some(site_file) = info.file_for(location.file_id) else {
                    continue;
                };
                let site_line = location.span.start.line;
                if site_line < line || !matcher.matches(site_file) {
                    continue;
                }
                if best
                    .as_ref()
                    .is_none_or(|(_, best_line)| site_line < *best_line)
                {
                    best = Some((site_file.to_string(), site_line));
                }
            }
        }
        let Some((file, line)) = best else {
            return Err(format!("no code at or after line {line} of {file}"));
        };
        self.control
            .breakpoints
            .insert(LineBreakpoint { file, line });
        Ok(line)
    }

    /// Clears the breakpoint on `line` of `file`. Returns whether there was
    /// one.
    pub fn clear_breakpoint(&mut self, file: &str, line: usize) -> bool {
        let before = self.control.breakpoints.len();
        self.control
            .breakpoints
            .retain(|bp| !(bp.line == line && same_file(&bp.file, file)));
        self.control.breakpoints.len() != before
    }

    /// The runtime error the program stopped at, and its message.
    pub fn exception(&self) -> Option<(super::ExceptionKind, String)> {
        self.control
            .exception
            .as_ref()
            .map(|(kind, message, _, _)| (*kind, message.clone()))
    }

    /// The runtime errors the debugger stops at.
    pub fn caught(&self) -> Vec<super::ExceptionKind> {
        super::ExceptionKind::ALL
            .into_iter()
            .filter(|kind| self.control.catch.contains(kind))
            .collect()
    }

    /// Stops at these runtime errors, and no others.
    pub fn set_caught(&mut self, kinds: &[super::ExceptionKind]) {
        self.control.catch = kinds.iter().copied().collect();
    }

    /// Sites the program has run so far: where it is in its run.
    pub fn position(&self) -> u64 {
        self.control.position
    }

    /// Why the debugger cannot go back now, if it cannot.
    pub fn reverse_unavailable(&self) -> Option<String> {
        super::reverse_unavailable()
    }

    /// How many checkpoints the session holds.
    pub fn checkpoint_count(&self) -> usize {
        self.control.checkpoints.len()
    }

    /// Shows or hides compiler-made frames and variables.
    pub fn set_show_internals(&mut self, show: bool) {
        self.control.show_internals = show;
    }

    pub fn show_internals(&self) -> bool {
        self.control.show_internals
    }
}

/// The stack of a thread other than the stopped one, oldest first (S8).
/// A parked fiber's frames, oldest first. The primop it parked in (`sleep`,
/// `recv`) has no source of its own: the frame that called it is where the
/// fiber waits.
fn parked_layout(
    continuation: &Rc<std::cell::RefCell<crate::runtime::continuation::Continuation>>,
) -> Vec<(FrameSource, StackFrame)> {
    let frames: Vec<(super::FrameSummary, bool)> = continuation
        .borrow()
        .frames
        .iter()
        .map(|frame| {
            let function = &frame.closure.function;
            let info = function.debug_info.as_ref();
            let location = location_at(function, frame.ip.saturating_sub(1));
            let artificial =
                location.is_none() || is_artificial(info.and_then(|info| info.name.as_deref()));
            ((function_name(info, 1), location), artificial)
        })
        .collect();
    frames
        .into_iter()
        .enumerate()
        .map(|(k, ((name, location), artificial))| {
            (
                FrameSource::Parked {
                    continuation: continuation.clone(),
                    frame: k,
                },
                summary_frame(
                    &name,
                    location.as_ref(),
                    artificial,
                    None,
                    FrameKind::Suspended { resumable: true },
                ),
            )
        })
        .collect()
}

fn other_layout(other: super::threads::OtherThread) -> Vec<(FrameSource, StackFrame)> {
    use super::threads::OtherThread;
    let mut out: Vec<(FrameSource, StackFrame)> = Vec::new();
    match other {
        OtherThread::Parked { continuation, .. } => {
            out = parked_layout(&continuation);
            for (index, (_, frame)) in out.iter_mut().enumerate() {
                frame.index = index;
            }
        }
        OtherThread::NotStarted(body) => {
            let function = match &body {
                Value::Closure(closure) => Some(closure.function.clone()),
                Value::Function(function) => Some(function.clone()),
                _ => None,
            };
            if let Some(function) = function {
                let info = function.debug_info.as_ref();
                let name = function_name(info, 1);
                let location = location_at(&function, 0);
                out.push((
                    FrameSource::Label,
                    summary_frame(
                        &name,
                        location.as_ref(),
                        false,
                        None,
                        FrameKind::Suspended { resumable: true },
                    ),
                ));
            }
        }
        OtherThread::Task(task) => out.push((
            FrameSource::Label,
            summary_frame(
                &format!(
                    "task {task} runs on a thread of its own, which the debugger does not pause"
                ),
                None,
                false,
                None,
                FrameKind::Label,
            ),
        )),
    }
    out
}

/// A stack line from a name and a location.
fn summary_frame(
    name: &str,
    location: Option<&(String, usize, usize)>,
    artificial: bool,
    elided: Option<(u64, String)>,
    kind: FrameKind,
) -> StackFrame {
    let (file, line, column) = location
        .map(|(file, line, column)| (Some(file.clone()), *line, *column))
        .unwrap_or((None, 0, 0));
    StackFrame {
        index: 0,
        name: name.to_string(),
        file,
        line,
        column,
        artificial,
        elided,
        kind,
    }
}

/// The calls a frame replaced by tail calls, and the latest of them.
fn elided(calls: Option<&super::TailCalls>) -> Option<(u64, String)> {
    let calls = calls.filter(|calls| calls.count > 0)?;
    let last = calls
        .recent
        .last()
        .map(|(name, location)| match location {
            Some((file, line, column)) => {
                format!("{name} at {}:{line}:{column}", short_file(file))
            }
            None => name.clone(),
        })
        .unwrap_or_default();
    Some((calls.count, last))
}

/// The effect an arm named `<handler Effect.op>` (or `__default_handler
/// Effect.op`) handles.
fn handled_effect(arm_name: &str) -> Option<String> {
    let operation = arm_name
        .strip_prefix("<handler ")
        .and_then(|rest| rest.strip_suffix('>'))
        .or_else(|| arm_name.strip_prefix("__default_handler "))?;
    let (effect, _op) = operation.rsplit_once('.')?;
    Some(effect.to_string())
}

/// The file name for a stack line: the last component.
pub(super) fn short_file(file: &str) -> String {
    std::path::Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string())
}

impl VM {
    /// Every compiled function the program can run: the entry code and each
    /// function in the constant pool.
    pub(super) fn all_functions(&self) -> Vec<Arc<CompiledFunction>> {
        let mut functions = vec![self.frames[0].closure.function.clone()];
        for constant in &self.constants {
            if let Value::Function(function) = slot::from_slot_ref(constant) {
                functions.push(function);
            }
        }
        functions
    }

    /// The instruction frame `index` is at: the current one for the newest
    /// frame, and the call that is running for the frames below it.
    pub(crate) fn frame_ip(&self, index: usize) -> usize {
        if index == self.frame_index {
            return self.frames[index].ip;
        }
        self.call_ips
            .get(index + 1)
            .copied()
            .unwrap_or_else(|| self.frames[index].ip.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::display_name;

    #[test]
    fn instance_methods_read_as_class_type_method() {
        assert_eq!(display_name("__tc_Show_Int_show"), "Show<Int>.show");
        assert_eq!(display_name("__tc_Eq_List<a>_neq"), "Eq<List<a>>.neq");
        assert_eq!(
            display_name("__tc_Show_Int_to_string"),
            "Show<Int>.to_string"
        );
        assert_eq!(
            display_name("__tc_m8_466C6F772E4E756D_Num_Float_sub"),
            "Num<Float>.sub"
        );
        assert_eq!(display_name("__dict_Show_Int"), "__dict_Show_Int");
        assert_eq!(display_name("main"), "main");
    }

    #[test]
    fn compiler_made_names_are_artificial() {
        use super::is_artificial;
        assert!(is_artificial(Some("__dict_Show_Int")));
        assert!(is_artificial(Some("Flow.Primops.__primop_print")));
        assert!(is_artificial(Some("__default_handler Console.print")));
        assert!(!is_artificial(Some("__tc_Show_Int_show")));
        assert!(!is_artificial(Some("<handler Console.print>")));
        assert!(!is_artificial(Some("main")));
    }

    #[test]
    fn breakpoint_files_match_by_name_or_through_a_symlink() {
        use super::same_file;
        assert!(same_file("/work/app/src/main.flx", "main.flx"));
        assert!(same_file("/work/app/src/main.flx", "src/main.flx"));
        assert!(same_file("src/main.flx", "/work/app/src/main.flx"));
        assert!(!same_file("/work/app/src/main.flx", "other.flx"));

        // An editor and the compiler can name one file by two absolute
        // paths, as `/var` and `/private/var` on macOS.
        #[cfg(unix)]
        {
            let dir = std::env::temp_dir().join(format!("flux-same-file-{}", std::process::id()));
            let real = dir.join("real");
            std::fs::create_dir_all(&real).unwrap();
            std::fs::write(real.join("app.flx"), "fn main() {}\n").unwrap();
            let link = dir.join("link");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let compiled = real.join("app.flx").display().to_string();
            let edited = link.join("app.flx").display().to_string();
            assert!(same_file(&compiled, &edited));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
