//! `flux debug`: a command-line front end for the VM debugger (proposal
//! 0188 S2), until the VS Code adapter (S4) drives the same engine.
//!
//! Commands come one per line from a script (`--script`) or from stdin.
//! Everything the debugger says goes to stderr, so the program's own stdout
//! stays as it would be in a normal run.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::driver::flags::DebuggerRequest;
use crate::vm::debug::{Inspector, Resume, StopReason, ValueView, VmObserver};

const HELP: &str = "\
commands:
  break <file>:<line> | break <line>   set a breakpoint (b)
  delete <file>:<line> | delete <line> clear a breakpoint
  continue                             run to the next breakpoint (c)
  step                                 stop at the next site, entering calls (s)
  next                                 stop at the next site in this function (n)
  finish                               run until this function returns (f)
  bt                                   show the call stack
  locals                               show this frame's variables
  print <name>[.<n>...]                show a value, or one of its parts (p)
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

/// Attaches the command-line debugger to `vm` when this run is under
/// `flux debug`.
pub(crate) fn attach(vm: &mut crate::vm::VM, program_path: &str) {
    let Some(request) = REQUEST.get() else {
        return;
    };
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
                "q" | "quit" => return Resume::Quit,
                "b" | "break" => self.set_breakpoint(inspector, rest),
                "delete" => self.clear_breakpoint(inspector, rest),
                "bt" | "backtrace" | "locals" | "p" | "print" if !paused => say(
                    "the program has not started yet; `continue` to run to a breakpoint, \
                     or `step` to stop at its first line",
                ),
                "bt" | "backtrace" => backtrace(inspector),
                "locals" => locals(inspector),
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
                say(&format!(
                    "stopped: {} at {place} in {}",
                    reason.as_str(),
                    frame.name
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
    for (number, frame) in inspector.stack().iter().enumerate() {
        match &frame.file {
            Some(file) => say(&format!(
                "#{number} {} at {}:{}:{}",
                frame.name,
                short(file),
                frame.line,
                frame.column
            )),
            None => say(&format!("#{number} {}", frame.name)),
        }
        if let Some((count, last)) = &frame.elided {
            let calls = if *count == 1 { "call" } else { "calls" };
            say(&format!(
                "   … {count} {calls} elided by tail calls (last: {last})"
            ));
        }
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
