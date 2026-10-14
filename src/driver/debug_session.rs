//! `flux debug`: a command-line front end for the VM debugger (proposal
//! 0188 S2), until the VS Code adapter (S4) drives the same engine.
//!
//! Commands come one per line from a script (`--script`) or from stdin.
//! Everything the debugger says goes to stderr, so the program's own stdout
//! stays as it would be in a normal run.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::driver::flags::DebuggerRequest;
use crate::runtime::replay;
use crate::vm::debug::{
    ExceptionKind, FrameKind, Inspector, Resume, StopReason, ValueView, VmObserver,
};

const HELP: &str = "\
commands:
  break <file>:<line> | break <line>   set a breakpoint (b)
  delete <file>:<line> | delete <line> clear a breakpoint
  continue                             run to the next breakpoint (c)
  step                                 stop at the next site, entering calls (s)
  next                                 stop at the next site in this function (n)
  finish                               run until this function returns (f)
  reverse-step                         go back to the previous place a step stops (rs)
  reverse-next                         go back to the previous line of this call (rn)
  reverse-finish                       go back to where this function was called (rf)
  reverse-continue                     go back to the previous breakpoint hit (rc)
  where                                show the position: how many stop sites have run
  goto <position>                      go to a position, earlier or later
  events                               list the effects the program has performed
  event <n>                            go to the last stop before effect <n>
  bt                                   show the call stack
  locals                               show this frame's variables
  print <name>[.<n>...]                show a value, or one of its parts (p)
  handlers                             show the active effect handlers, innermost first
  threads                              list the fibers and tasks, and what each waits for
  thread <n>                           look at thread <n>: bt, locals and print read it
  catch [panic|division|unhandled|fibers|errors|all|none]...
                                       choose the runtime errors to stop at; alone, list them
  internals on|off                     show or hide compiler-made frames and variables
  quit                                 stop the program (q)";

/// The debugger this run asked for, set from the flags before compiling.
static REQUEST: std::sync::OnceLock<DebuggerRequest> = std::sync::OnceLock::new();

/// Remembers that this run is under `flux debug`.
pub(crate) fn configure(flags: &crate::driver::flags::DriverFlags) {
    if let Some(request) = &flags.runtime.debugger {
        let _ = REQUEST.set(request.clone());
    }
}

/// Attaches the debugger to `vm` when this run is under `flux debug`: the
/// command reader, or a debug adapter's engine with `--engine`.
pub(crate) fn attach(vm: &mut crate::vm::VM, program_path: &str) {
    let Some(request) = REQUEST.get() else {
        return;
    };
    // Keep the run's effects, so going back answers them from the recording
    // instead of performing them again (0188 S3).
    if replay::is_active() {
        replay::keep_events();
    } else {
        replay::start_memory_record();
    }
    if let Some(address) = &request.engine {
        match super::debug_engine::EngineObserver::connect(address) {
            Ok(observer) => vm.attach_debugger(Box::new(observer)),
            Err(message) => {
                eprintln!("{message}");
                std::process::exit(1);
            }
        }
        return;
    }
    match CommandObserver::new(request.script.as_deref(), Path::new(program_path)) {
        Ok(observer) => vm.attach_debugger(Box::new(observer)),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

/// How many parts of a compound value `print` lists.
const PRINT_CHILDREN: usize = 16;

pub(crate) struct CommandObserver {
    commands: Box<dyn Iterator<Item = String>>,
    /// Echo each command in the transcript: scripts are not typed, so the
    /// transcript would otherwise not show what was asked.
    echo: bool,
    interactive: bool,
    program_path: PathBuf,
}

impl CommandObserver {
    pub(crate) fn new(script: Option<&Path>, program_path: &Path) -> Result<Self, String> {
        let (commands, echo, interactive): (Box<dyn Iterator<Item = String>>, bool, bool) =
            match script {
                Some(path) => {
                    let text = std::fs::read_to_string(path).map_err(|err| {
                        format!(
                            "Error: cannot read debugger script {}: {err}",
                            path.display()
                        )
                    })?;
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    (Box::new(lines.into_iter()), true, false)
                }
                None => {
                    // A terminal is read line by line as the person types; a
                    // pipe is read like a script, echoing each command.
                    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
                    (Box::new(StdinLines), !interactive, interactive)
                }
            };
        Ok(Self {
            commands,
            echo,
            interactive,
            program_path: program_path.to_path_buf(),
        })
    }

    fn next_command(&mut self) -> Option<String> {
        loop {
            if self.interactive {
                eprint!("(flux) ");
                let _ = std::io::stderr().flush();
            }
            let line = self.commands.next()?;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if self.echo {
                say(&format!("> {line}"));
            }
            return Some(line.to_string());
        }
    }

    /// Runs commands until one resumes the program. Out of commands, the
    /// program runs to its end.
    fn command_loop(&mut self, inspector: &mut Inspector<'_>, paused: bool) -> Resume {
        while let Some(line) = self.next_command() {
            let (command, rest) = line
                .split_once(char::is_whitespace)
                .map(|(command, rest)| (command, rest.trim()))
                .unwrap_or((line.as_str(), ""));
            match command {
                "c" | "continue" => return Resume::Continue,
                "s" | "step" if paused => return Resume::StepIn,
                "n" | "next" if paused => return Resume::StepOver,
                "f" | "finish" if paused => return Resume::StepOut,
                "s" | "step" | "n" | "next" | "f" | "finish" => {
                    // Before the program starts, the first step stops at its
                    // first site.
                    return Resume::StepIn;
                }
                "rs" | "reverse-step" | "rn" | "reverse-next" | "rf" | "reverse-finish" | "rc"
                | "reverse-continue" | "where" | "goto" | "event"
                    if !paused =>
                {
                    say(NOT_STARTED)
                }
                "rs" | "reverse-step" | "rn" | "reverse-next" | "rf" | "reverse-finish" | "rc"
                | "reverse-continue" | "goto" | "event" => {
                    if let Some(resume) = self.go_back(inspector, command, rest) {
                        return resume;
                    }
                }
                "where" => say(&format!("position {}", inspector.position())),
                "events" => events(),
                "checkpoints" => say(&format!("{} checkpoints", inspector.checkpoint_count())),
                "q" | "quit" => return Resume::Quit,
                "b" | "break" => self.set_breakpoint(inspector, rest),
                "delete" => self.clear_breakpoint(inspector, rest),
                "bt" | "backtrace" | "locals" | "p" | "print" | "handlers" | "threads"
                | "thread"
                    if !paused =>
                {
                    say(NOT_STARTED)
                }
                "threads" => threads(inspector),
                "thread" => select_thread(inspector, rest),
                "bt" | "backtrace" => backtrace(inspector),
                "locals" => locals(inspector),
                "handlers" => handlers(inspector),
                "catch" => catch(inspector, rest),
                "p" | "print" => print(inspector, rest),
                "internals" => match rest {
                    "on" => inspector.set_show_internals(true),
                    "off" => inspector.set_show_internals(false),
                    _ => say("usage: internals on|off"),
                },
                "help" | "h" => say(HELP),
                other => say(&format!("unknown command `{other}`; try `help`")),
            }
        }
        Resume::Continue
    }

    /// The answer to a command that goes back or jumps, if it can be done.
    fn go_back(&self, inspector: &Inspector<'_>, command: &str, rest: &str) -> Option<Resume> {
        if let Some(reason) = inspector.reverse_unavailable() {
            say(&format!("cannot go back: {reason}"));
            return None;
        }
        Some(match command {
            "rs" | "reverse-step" => Resume::ReverseStep,
            "rn" | "reverse-next" => Resume::ReverseNext,
            "rf" | "reverse-finish" => Resume::ReverseFinish,
            "rc" | "reverse-continue" => Resume::ReverseContinue,
            "goto" => match rest.parse::<u64>() {
                Ok(position) if position > 0 => Resume::GoTo(position),
                _ => {
                    say("usage: goto <position>, a number from 1; `where` shows the current one");
                    return None;
                }
            },
            _ => {
                let count = replay::event_list().len() as u64;
                match rest.parse::<u64>() {
                    Ok(event) if event < count => Resume::ToEvent(event),
                    Ok(event) => {
                        say(&format!("no event #{event}; `events` lists them"));
                        return None;
                    }
                    Err(_) => {
                        say("usage: event <n>");
                        return None;
                    }
                }
            }
        })
    }

    /// `file:line`, or a bare line in the program's own file.
    fn breakpoint_target(&self, spec: &str) -> Option<(String, usize)> {
        match spec.rsplit_once(':') {
            Some((file, line)) => Some((file.to_string(), line.trim().parse().ok()?)),
            None => Some((
                self.program_path.display().to_string(),
                spec.trim().parse().ok()?,
            )),
        }
    }

    fn set_breakpoint(&self, inspector: &mut Inspector<'_>, spec: &str) {
        let Some((file, line)) = self.breakpoint_target(spec) else {
            say("usage: break <file>:<line>");
            return;
        };
        match inspector.set_breakpoint(&file, line) {
            Ok(set) if set == line => say(&format!("breakpoint at {}:{line}", short(&file))),
            Ok(set) => say(&format!(
                "no code on line {line}; breakpoint at {}:{set}",
                short(&file)
            )),
            Err(_) => say(&format!(
                "no code at or after line {line} of {}",
                short(&file)
            )),
        }
    }

    fn clear_breakpoint(&self, inspector: &mut Inspector<'_>, spec: &str) {
        let Some((file, line)) = self.breakpoint_target(spec) else {
            say("usage: delete <file>:<line>");
            return;
        };
        if inspector.clear_breakpoint(&file, line) {
            say(&format!("deleted breakpoint at {}:{line}", short(&file)));
        } else {
            say(&format!("no breakpoint at {}:{line}", short(&file)));
        }
    }
}

impl VmObserver for CommandObserver {
    fn on_start(&mut self, inspector: &mut Inspector<'_>) -> Resume {
        if self.interactive {
            say(
                "The program has not started. Set breakpoints with `break <line>`, then \
                 `continue` to run to them, or `step` to stop at its first line. `help` lists \
                 the commands.",
            );
        }
        self.command_loop(inspector, false)
    }

    fn on_stop(&mut self, reason: StopReason, inspector: &mut Inspector<'_>) -> Resume {
        match inspector.stack().first() {
            Some(frame) => {
                let place = match &frame.file {
                    Some(file) => format!("{}:{}:{}", short(file), frame.line, frame.column),
                    None => "an unknown place".to_string(),
                };
                let why = match (reason, inspector.exception()) {
                    (StopReason::Exception(_), Some((kind, message)))
                        if message == kind.label() =>
                    {
                        format!("exception ({message})")
                    }
                    (StopReason::Exception(_), Some((kind, message))) => {
                        format!("exception ({}: {message})", kind.label())
                    }
                    _ => reason.as_str().to_string(),
                };
                let thread = match inspector.stopped_thread() {
                    1 => String::new(),
                    thread => format!("thread {thread}, "),
                };
                say(&format!(
                    "stopped: {why} at {place} in {} ({thread}position {})",
                    frame.name,
                    inspector.position()
                ));
                if let Some(text) = frame
                    .file
                    .as_deref()
                    .and_then(|file| source_line(file, frame.line))
                {
                    say(&format!("  {:>4} | {text}", frame.line));
                }
            }
            None => say(&format!("stopped: {}", reason.as_str())),
        }
        self.command_loop(inspector, true)
    }

    fn on_exit(&mut self) {
        say("program finished");
    }
}

const NOT_STARTED: &str = "the program has not started yet; `continue` to run to a breakpoint, \
                           or `step` to stop at its first line";

/// Lists the effects on the tape, with where each happened.
fn events() {
    let events = replay::event_list();
    if events.is_empty() {
        say("no effects yet");
        return;
    }
    for (seq, text, position) in events {
        match position {
            Some(position) => say(&format!("#{seq} {text} (position {position})")),
            None => say(&format!("#{seq} {text} (not reached)")),
        }
    }
}

/// Reads one line from stdin each time it is asked.
struct StdinLines;

impl Iterator for StdinLines {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }
}

fn say(text: &str) {
    let _ = std::io::stdout().flush();
    eprintln!("{text}");
}

fn short(file: &str) -> String {
    Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string())
}

fn source_line(file: &str, line: usize) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines()
        .nth(line.checked_sub(1)?)
        .map(|line| line.trim_end().to_string())
}

fn backtrace(inspector: &Inspector<'_>) {
    let mut number = 0;
    for frame in inspector.stack() {
        let note = match frame.kind {
            FrameKind::Label => {
                say(&format!("   ⟂ {}", frame.name));
                continue;
            }
            FrameKind::Suspended { resumable: true } => "   (suspended)",
            FrameKind::Suspended { resumable: false } => "   (suspended, not resumable)",
            FrameKind::Live => "",
        };
        match &frame.file {
            Some(file) => say(&format!(
                "#{number} {} at {}:{}:{}{note}",
                frame.name,
                short(file),
                frame.line,
                frame.column
            )),
            None => say(&format!("#{number} {}{note}", frame.name)),
        }
        number += 1;
        if let Some((count, last)) = &frame.elided {
            let calls = if *count == 1 { "call" } else { "calls" };
            say(&format!(
                "   … {count} {calls} elided by tail calls (last: {last})"
            ));
        }
    }
}

/// `catch`: lists the runtime errors the debugger stops at, or sets them.
fn catch(inspector: &mut Inspector<'_>, rest: &str) {
    if !rest.is_empty() {
        let mut kinds = Vec::new();
        for word in rest.split_whitespace() {
            match word {
                "all" => kinds.extend(ExceptionKind::ALL),
                "none" => {}
                other => match ExceptionKind::from_id(other) {
                    Some(kind) => kinds.push(kind),
                    None => {
                        say(&format!(
                            "unknown error kind `{other}`; use panic, division, unhandled, \
                             fibers, errors, all or none"
                        ));
                        return;
                    }
                },
            }
        }
        inspector.set_caught(&kinds);
    }
    let caught = inspector.caught();
    if caught.is_empty() {
        say("stopping at no runtime errors");
    } else {
        let names: Vec<&str> = caught.iter().map(|kind| kind.label()).collect();
        say(&format!("stopping at: {}", names.join(", ")));
    }
}

/// `threads`: each fiber and task, what it is doing, and where.
fn threads(inspector: &mut Inspector<'_>) {
    let looking_at = inspector.selected_thread();
    for thread in inspector.threads() {
        let mark = if thread.id == looking_at { "*" } else { " " };
        let place = inspector
            .select_thread(thread.id)
            .ok()
            .and_then(|()| inspector.stack().into_iter().next())
            .and_then(|frame| {
                let file = frame.file?;
                Some(format!(
                    " at {}:{}:{}",
                    short(&file),
                    frame.line,
                    frame.column
                ))
            })
            .unwrap_or_default();
        say(&format!(
            "{mark} {} {} [{}]{place}",
            thread.id,
            thread.name,
            thread.state.describe()
        ));
    }
    let _ = inspector.select_thread(looking_at);
}

/// `thread <n>`: look at thread `n` until the next stop.
fn select_thread(inspector: &mut Inspector<'_>, rest: &str) {
    let Ok(id) = rest.trim().parse::<u64>() else {
        say("usage: thread <n>; `threads` lists them");
        return;
    };
    match inspector.select_thread(id) {
        Ok(()) => {
            let name = inspector
                .threads()
                .into_iter()
                .find(|thread| thread.id == id)
                .map(|thread| format!("{} [{}]", thread.name, thread.state.describe()))
                .unwrap_or_default();
            say(&format!("thread {id}: {name}"));
        }
        Err(message) => say(&message),
    }
}

fn handlers(inspector: &Inspector<'_>) {
    let handlers = inspector.handlers();
    if handlers.is_empty() {
        say("no handlers");
        return;
    }
    for (number, handler) in handlers.iter().enumerate() {
        let state = handler
            .state
            .as_ref()
            .map(|state| format!(" state = {}", state.render()))
            .unwrap_or_default();
        let site = handler
            .handled_in
            .as_ref()
            .map(|site| format!(" — handled in {site}"))
            .unwrap_or_default();
        say(&format!("#{number} {}{state}{site}", handler.effect));
    }
}

fn locals(inspector: &Inspector<'_>) {
    let Some(frame) = inspector.stack().into_iter().next() else {
        return;
    };
    let locals = inspector.locals(frame.index);
    let captured = inspector.captured(frame.index);
    if locals.is_empty() && captured.is_empty() {
        say("no variables");
        return;
    }
    for variable in locals {
        say(&format!("{} = {}", variable.name, variable.value.render()));
    }
    for variable in captured {
        say(&format!(
            "{} = {} (captured)",
            variable.name,
            variable.value.render()
        ));
    }
}

fn print(inspector: &mut Inspector<'_>, path: &str) {
    let mut parts = path.split('.');
    let Some(name) = parts.next().filter(|name| !name.is_empty()) else {
        say("usage: print <name>[.<n>...]");
        return;
    };
    let Some(frame) = inspector.stack().into_iter().next() else {
        return;
    };
    let Some(mut value) = inspector.lookup(frame.index, name) else {
        say(&format!("no variable `{name}` here"));
        return;
    };
    for part in parts {
        match value.child(part) {
            Some(child) => value = child,
            None => {
                say(&format!("`{path}` has no part `{part}`"));
                return;
            }
        }
    }
    show_value(path, &value);
}

fn show_value(path: &str, value: &ValueView) {
    say(&format!("{path} = {}", value.render()));
    for (label, child) in value.children(PRINT_CHILDREN) {
        say(&format!("  .{label} = {}", child.render()));
    }
}
