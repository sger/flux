//! `flux debug --engine <address>`: the VM debugger served to a debug
//! adapter (`flux-dap`, proposal 0188 S4) over a socket.
//!
//! The adapter listens; this process connects, says when the program is
//! compiled, and then answers requests (see [`crate::debug_protocol`]). The
//! program's own output goes to stdout and stderr as usual, which the
//! adapter reads separately.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::debug_protocol::{
    EngineMessage, Event, ExceptionInfo, Frame, FrameKind, Reply, Request, RequestMessage,
    ResumeKind, Scope, StopKind, TapeEvent, Thread, Variable,
};
use crate::runtime::replay;
use crate::vm::debug::{
    self, ExceptionKind, Inspector, Resume, StopReason, ValueView, VmObserver, request_interrupt,
};

/// The most parts of one value listed at once.
const CHILDREN: usize = 1000;

pub(crate) struct EngineObserver {
    out: TcpStream,
    /// Requests from the adapter; `None` when it has gone.
    requests: Receiver<Option<RequestMessage>>,
    /// Per file, the lines its breakpoints were set at.
    breakpoints: HashMap<String, Vec<usize>>,
    /// The next stop is the one `stop_on_entry` asked for.
    entry: bool,
}

/// What a request asks the paused or running program to do.
enum Handled {
    Stay,
    Resume(Resume),
    Pause,
}

impl EngineObserver {
    pub(crate) fn connect(address: &str) -> Result<Self, String> {
        let out = TcpStream::connect(address)
            .map_err(|err| format!("Error: cannot reach the debug adapter at {address}: {err}"))?;
        let input = out
            .try_clone()
            .map_err(|err| format!("Error: cannot read from the debug adapter: {err}"))?;
        let (sender, requests) = channel();
        // Requests can come while the program runs: a reader thread queues
        // them and asks the VM to look at its next stop site.
        std::thread::spawn(move || {
            for line in BufReader::new(input).lines() {
                let Ok(line) = line else {
                    break;
                };
                match serde_json::from_str::<RequestMessage>(&line) {
                    Ok(message) => {
                        if sender.send(Some(message)).is_err() {
                            return;
                        }
                        request_interrupt();
                    }
                    Err(err) => eprintln!("flux debug: unreadable request from the adapter: {err}"),
                }
            }
            let _ = sender.send(None);
            request_interrupt();
        });
        Ok(Self {
            out,
            requests,
            breakpoints: HashMap::new(),
            entry: false,
        })
    }

    fn send(&mut self, message: &EngineMessage) {
        let line = serde_json::to_string(message).expect("engine messages serialize");
        if writeln!(self.out, "{line}")
            .and_then(|_| self.out.flush())
            .is_err()
        {
            // The adapter has gone: nobody is debugging this run any more.
            leave();
        }
    }

    fn reply(&mut self, id: u64, result: Result<Reply, String>) {
        let message = match result {
            Ok(reply) => EngineMessage::Reply { id, reply },
            Err(message) => EngineMessage::Error { id, message },
        };
        self.send(&message);
    }

    /// Serves requests until one resumes the program.
    fn serve(&mut self, inspector: &mut Inspector<'_>, started: bool) -> Resume {
        loop {
            let Ok(Some(message)) = self.requests.recv() else {
                leave();
            };
            match self.handle(inspector, message, started, true) {
                Handled::Resume(resume) => return resume,
                Handled::Stay | Handled::Pause => {}
            }
        }
    }

    fn handle(
        &mut self,
        inspector: &mut Inspector<'_>,
        message: RequestMessage,
        started: bool,
        paused: bool,
    ) -> Handled {
        let id = message.id;
        let running = || Err("the program is running; pause it first".to_string());
        match message.request {
            Request::SetBreakpoints { file, lines } => {
                for old in self.breakpoints.remove(&file).unwrap_or_default() {
                    inspector.clear_breakpoint(&file, old);
                }
                let set: Vec<Option<usize>> = lines
                    .iter()
                    .map(|line| inspector.set_breakpoint(&file, *line).ok())
                    .collect();
                self.breakpoints
                    .insert(file, set.iter().flatten().copied().collect());
                self.reply(id, Ok(Reply::Breakpoints { lines: set }));
            }
            Request::Start { stop_on_entry } => {
                if started {
                    self.reply(id, Err("the program has already started".to_string()));
                } else {
                    self.reply(id, Ok(Reply::Ok));
                    self.entry = stop_on_entry;
                    return Handled::Resume(if stop_on_entry {
                        Resume::StepIn
                    } else {
                        Resume::Continue
                    });
                }
            }
            Request::Resume { how } => {
                if !paused {
                    self.reply(id, running());
                    return Handled::Stay;
                }
                match resume_for(how, inspector) {
                    Ok(resume) => {
                        self.reply(id, Ok(Reply::Ok));
                        return Handled::Resume(resume);
                    }
                    Err(message) => self.reply(id, Err(message)),
                }
            }
            Request::Pause => {
                self.reply(id, Ok(Reply::Ok));
                if !paused {
                    return Handled::Pause;
                }
            }
            Request::Stack { .. } | Request::Threads if !paused => self.reply(id, running()),
            Request::Threads => {
                let threads = inspector
                    .threads()
                    .into_iter()
                    .map(|thread| Thread {
                        id: thread.id,
                        name: thread.name,
                        state: thread.state.describe(),
                    })
                    .collect();
                self.reply(id, Ok(Reply::Threads { threads }));
            }
            Request::Stack { thread } => {
                if let Err(message) = look_at(inspector, thread) {
                    self.reply(id, Err(message));
                    return Handled::Stay;
                }
                let frames = inspector
                    .stack()
                    .into_iter()
                    .map(|frame| Frame {
                        index: frame.index,
                        name: frame.name,
                        file: frame.file,
                        line: frame.line,
                        column: frame.column,
                        artificial: frame.artificial,
                        elided: frame.elided,
                        kind: match frame.kind {
                            debug::FrameKind::Live => FrameKind::Live,
                            debug::FrameKind::Suspended { resumable: true } => FrameKind::Suspended,
                            debug::FrameKind::Suspended { resumable: false } => FrameKind::Dropped,
                            debug::FrameKind::Label => FrameKind::Label,
                        },
                    })
                    .collect();
                self.reply(id, Ok(Reply::Stack { frames }));
            }
            Request::Variables { .. } | Request::Evaluate { .. } if !paused => {
                self.reply(id, running())
            }
            Request::Variables {
                frame,
                scope,
                path,
                thread,
            } => {
                let result = look_at(inspector, thread)
                    .and_then(|()| variables(inspector, frame, scope, &path))
                    .map(|variables| Reply::Variables { variables });
                self.reply(id, result);
            }
            Request::Evaluate {
                frame,
                expression,
                thread,
            } => {
                let result = look_at(inspector, thread)
                    .and_then(|()| evaluate(inspector, frame, &expression))
                    .map(|variable| Reply::Value { variable });
                self.reply(id, result);
            }
            Request::SetInternals { show } => {
                inspector.set_show_internals(show);
                self.reply(id, Ok(Reply::Ok));
            }
            Request::SetExceptionFilters { filters } => {
                let kinds: Result<Vec<ExceptionKind>, String> = filters
                    .iter()
                    .map(|filter| {
                        ExceptionKind::from_id(filter)
                            .ok_or_else(|| format!("no exception filter `{filter}`"))
                    })
                    .collect();
                match kinds {
                    Ok(kinds) => {
                        inspector.set_caught(&kinds);
                        self.reply(id, Ok(Reply::Ok));
                    }
                    Err(message) => self.reply(id, Err(message)),
                }
            }
            Request::Events => {
                let events = replay::event_list()
                    .into_iter()
                    .map(|(index, text, position)| TapeEvent {
                        index,
                        text,
                        position,
                    })
                    .collect();
                self.reply(id, Ok(Reply::Events { events }));
            }
            Request::Where => {
                let position = inspector.position();
                self.reply(id, Ok(Reply::Position { position }));
            }
            Request::Quit => {
                self.reply(id, Ok(Reply::Ok));
                return Handled::Resume(Resume::Quit);
            }
        }
        Handled::Stay
    }
}

impl VmObserver for EngineObserver {
    fn on_start(&mut self, inspector: &mut Inspector<'_>) -> Resume {
        self.send(&EngineMessage::Event {
            event: Event::Ready,
        });
        self.serve(inspector, false)
    }

    fn on_stop(&mut self, reason: StopReason, inspector: &mut Inspector<'_>) -> Resume {
        let mut exception = None;
        let (reason, description) = match reason {
            StopReason::Exception(_) => {
                self.entry = false;
                exception = inspector.exception().map(|(kind, message)| ExceptionInfo {
                    kind: kind.id().to_string(),
                    message,
                });
                (StopKind::Exception, None)
            }
            _ if std::mem::take(&mut self.entry) => (StopKind::Entry, None),
            StopReason::Breakpoint => (StopKind::Breakpoint, None),
            StopReason::Step => (StopKind::Step, None),
            StopReason::Pause => (StopKind::Pause, None),
            StopReason::StartOfRecording => (
                StopKind::Step,
                Some("the start of the recording".to_string()),
            ),
        };
        let position = inspector.position();
        let thread = inspector.stopped_thread();
        self.send(&EngineMessage::Event {
            event: Event::Stopped {
                reason,
                position,
                description,
                exception,
                thread,
            },
        });
        self.serve(inspector, true)
    }

    fn on_interrupt(&mut self, inspector: &mut Inspector<'_>) -> bool {
        let mut pause = false;
        loop {
            match self.requests.try_recv() {
                Ok(Some(message)) => match self.handle(inspector, message, true, false) {
                    Handled::Pause => pause = true,
                    Handled::Resume(Resume::Quit) => leave(),
                    Handled::Resume(_) | Handled::Stay => {}
                },
                Ok(None) | Err(TryRecvError::Disconnected) => leave(),
                Err(TryRecvError::Empty) => return pause,
            }
        }
    }
}

/// Ends the run: the adapter asked to, or has gone.
fn leave() -> ! {
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}

/// Reads `thread` (the stopped one when none) until told otherwise.
fn look_at(inspector: &mut Inspector<'_>, thread: Option<u64>) -> Result<(), String> {
    let thread = thread.unwrap_or_else(|| inspector.stopped_thread());
    inspector.select_thread(thread)
}

fn resume_for(how: ResumeKind, inspector: &Inspector<'_>) -> Result<Resume, String> {
    let resume = match how {
        ResumeKind::Continue => Resume::Continue,
        ResumeKind::StepIn => Resume::StepIn,
        ResumeKind::StepOver => Resume::StepOver,
        ResumeKind::StepOut => Resume::StepOut,
        ResumeKind::ReverseStep => Resume::ReverseStep,
        ResumeKind::ReverseNext => Resume::ReverseNext,
        ResumeKind::ReverseFinish => Resume::ReverseFinish,
        ResumeKind::ReverseContinue => Resume::ReverseContinue,
        ResumeKind::Goto(0) => return Err("positions start at 1".to_string()),
        ResumeKind::Goto(position) => Resume::GoTo(position),
        ResumeKind::Event(event) => {
            if event >= replay::event_list().len() as u64 {
                return Err(format!("no effect #{event} on the recording"));
            }
            Resume::ToEvent(event)
        }
    };
    if resume.goes_back()
        && let Some(reason) = inspector.reverse_unavailable()
    {
        return Err(format!("cannot go back: {reason}"));
    }
    Ok(resume)
}

fn variable(name: String, value: &ValueView, artificial: bool) -> Variable {
    Variable {
        name,
        value: value.render(),
        type_name: value.type_name().to_string(),
        has_children: value.has_children(),
        artificial,
    }
}

/// A frame's variables in `scope`, or the parts of the value `path` names.
fn variables(
    inspector: &mut Inspector<'_>,
    frame: usize,
    scope: Scope,
    path: &[String],
) -> Result<Vec<Variable>, String> {
    if scope == Scope::Handlers {
        return handler_variables(inspector, path);
    }
    let in_scope = match scope {
        Scope::Locals => inspector.locals(frame),
        Scope::Captured => inspector.captured(frame),
        Scope::Any | Scope::Handlers => Vec::new(),
    };
    let Some((name, parts)) = path.split_first() else {
        return Ok(in_scope
            .into_iter()
            .map(|var| variable(var.name, &var.value, var.artificial))
            .collect());
    };
    let found = match scope {
        Scope::Any => inspector.lookup(frame, name),
        _ => in_scope
            .into_iter()
            .find(|var| &var.name == name)
            .map(|var| var.value),
    }
    .ok_or_else(|| format!("no variable `{name}` here"))?;
    let value = descend(found, name, parts)?;
    Ok(value
        .children(CHILDREN)
        .into_iter()
        .map(|(label, child)| variable(label, &child, false))
        .collect())
}

/// The active handlers, as variables named by their effect, or the parts
/// of one's state.
fn handler_variables(inspector: &Inspector<'_>, path: &[String]) -> Result<Vec<Variable>, String> {
    let handlers = inspector.handlers();
    let Some((name, parts)) = path.split_first() else {
        return Ok(handlers
            .into_iter()
            .map(|handler| {
                let site = handler
                    .handled_in
                    .map(|name| format!("handled in {name}"))
                    .unwrap_or_default();
                match &handler.state {
                    Some(state) => {
                        let mut var = variable(handler.effect, state, handler.internal);
                        if !site.is_empty() {
                            var.value = format!("{} ({site})", var.value);
                        }
                        var
                    }
                    None => Variable {
                        name: handler.effect,
                        value: site,
                        type_name: "handler".to_string(),
                        has_children: false,
                        artificial: handler.internal,
                    },
                }
            })
            .collect());
    };
    let state = handlers
        .into_iter()
        .find(|handler| &handler.effect == name)
        .and_then(|handler| handler.state)
        .ok_or_else(|| format!("no handler state `{name}` here"))?;
    let value = descend(state, name, parts)?;
    Ok(value
        .children(CHILDREN)
        .into_iter()
        .map(|(label, child)| variable(label, &child, false))
        .collect())
}

/// `name.0.1`: a variable of the frame, then parts of it.
fn evaluate(
    inspector: &mut Inspector<'_>,
    frame: usize,
    expression: &str,
) -> Result<Variable, String> {
    let expression = expression.trim();
    let mut parts = expression.split('.').map(str::trim);
    let name = parts
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "write a variable, or a part of one such as `pair.0`".to_string())?;
    let value = inspector
        .lookup(frame, name)
        .ok_or_else(|| format!("no variable `{name}` here"))?;
    let parts: Vec<String> = parts.map(str::to_string).collect();
    let value = descend(value, name, &parts)?;
    Ok(variable(expression.to_string(), &value, false))
}

fn descend(mut value: ValueView, name: &str, parts: &[String]) -> Result<ValueView, String> {
    for part in parts {
        value = value
            .child(part)
            .ok_or_else(|| format!("`{name}` has no part `{part}`"))?;
    }
    Ok(value)
}
