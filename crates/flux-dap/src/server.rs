//! A DAP session: requests from the editor, answered by the engine.
//!
//! Everything the session hears arrives as an [`Input`] on one channel: the
//! editor's messages, the engine's replies and events, the program's output
//! and its exit. Requests that need the engine wait for its reply; whatever
//! else arrives meanwhile is set aside, except output, which is passed on at
//! once.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use flux::debug_protocol::{
    EngineMessage, Event, Frame, Reply, Request, ResumeKind, Scope, StopKind, Variable,
};
use serde_json::{Value, json};

use crate::engine::{LaunchSpec, VmEngine};

/// Something the session hears.
#[derive(Debug)]
pub enum Input {
    /// A message from the editor.
    Client(Value),
    /// A reply or event from the engine.
    Engine(EngineMessage),
    /// Output from the program, or the compiler before it.
    Output {
        category: &'static str,
        text: String,
    },
    /// The program has ended, with this exit code.
    Exited(Option<i32>),
}

/// The only thread: fibers become threads in 0188 S8.
const THREAD: i64 = 1;
/// Frame ids at or above this are tail-call labels, not frames.
const LABEL_FRAMES: i64 = 1_000_000;

/// A `variablesReference`: a frame's scope, or a value in it.
#[derive(Debug, Clone)]
struct VarRef {
    frame: usize,
    scope: Scope,
    path: Vec<String>,
}

pub struct Session<W: FnMut(Value)> {
    inputs: Receiver<Input>,
    sender: Sender<Input>,
    send: W,
    seq: i64,
    engine: Option<VmEngine>,
    /// Inputs set aside while a request waited for the engine.
    deferred: VecDeque<Input>,
    /// Variable references handed out since the last stop.
    refs: Vec<VarRef>,
    stop_on_entry: bool,
    exited: bool,
    done: bool,
}

impl<W: FnMut(Value)> Session<W> {
    /// A session reading `inputs`; `sender` feeds the same channel, for the
    /// threads that watch the program. `send` writes to the editor.
    pub fn new(inputs: Receiver<Input>, sender: Sender<Input>, send: W) -> Self {
        Self {
            inputs,
            sender,
            send,
            seq: 0,
            engine: None,
            deferred: VecDeque::new(),
            refs: Vec::new(),
            stop_on_entry: false,
            exited: false,
            done: false,
        }
    }

    /// Runs until the editor disconnects or goes away.
    pub fn run(mut self) {
        while !self.done {
            let input = match self.deferred.pop_front() {
                Some(input) => input,
                None => match self.inputs.recv() {
                    Ok(input) => input,
                    Err(_) => break,
                },
            };
            self.handle(input);
        }
        if let Some(engine) = &self.engine {
            engine.kill();
        }
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Client(message) => {
                if message["type"] == "request" {
                    self.request(&message);
                }
            }
            Input::Engine(EngineMessage::Event {
                event:
                    Event::Stopped {
                        reason,
                        position,
                        description,
                    },
            }) => {
                self.refs.clear();
                let reason = match reason {
                    StopKind::Entry => "entry",
                    StopKind::Breakpoint => "breakpoint",
                    StopKind::Step => "step",
                    StopKind::Pause => "pause",
                };
                let description = match description {
                    Some(text) => format!("position {position}: {text}"),
                    None => format!("position {position}"),
                };
                self.event(
                    "stopped",
                    json!({
                        "reason": reason,
                        "threadId": THREAD,
                        "allThreadsStopped": true,
                        "description": description,
                    }),
                );
            }
            Input::Engine(_) => {}
            Input::Output { category, text } => self.output(category, &text),
            Input::Exited(code) => {
                if !self.exited {
                    self.exited = true;
                    self.engine = None;
                    self.event("exited", json!({ "exitCode": code.unwrap_or(1) }));
                    self.event("terminated", json!({}));
                }
            }
        }
    }

    fn request(&mut self, message: &Value) {
        let command = message["command"].as_str().unwrap_or_default().to_string();
        let args = &message["arguments"];
        let result = match command.as_str() {
            "initialize" => Ok(capabilities()),
            "launch" => self.launch(args),
            "setBreakpoints" => self.set_breakpoints(args),
            "setExceptionBreakpoints" | "setFunctionBreakpoints" => {
                Ok(json!({ "breakpoints": [] }))
            }
            "configurationDone" => {
                let stop_on_entry = self.stop_on_entry;
                self.call(Request::Start { stop_on_entry })
                    .map(|_| json!({}))
            }
            "threads" => Ok(json!({ "threads": [{ "id": THREAD, "name": "main" }] })),
            "stackTrace" => self.stack_trace(args),
            "scopes" => self.scopes(args),
            "variables" => self.variables(args),
            "evaluate" => self.evaluate(args),
            "continue" => self
                .resume(ResumeKind::Continue)
                .map(|_| json!({ "allThreadsContinued": true })),
            "next" => self.resume(ResumeKind::StepOver),
            "stepIn" => self.resume(ResumeKind::StepIn),
            "stepOut" => self.resume(ResumeKind::StepOut),
            "stepBack" => self.resume(ResumeKind::ReverseNext),
            "reverseContinue" => self.resume(ResumeKind::ReverseContinue),
            "pause" => self.call(Request::Pause).map(|_| json!({})),
            "terminate" => {
                if self.engine.is_some() {
                    let _ = self.call(Request::Quit);
                }
                Ok(json!({}))
            }
            "disconnect" => {
                if let Some(engine) = self.engine.as_mut() {
                    engine.send(Request::Quit);
                }
                self.done = true;
                Ok(json!({}))
            }
            other => Err(format!("flux-dap does not support `{other}`")),
        };
        let seq = self.next_seq();
        let response = match result {
            Ok(body) => json!({
                "seq": seq,
                "type": "response",
                "request_seq": message["seq"],
                "success": true,
                "command": command,
                "body": body,
            }),
            Err(text) => json!({
                "seq": seq,
                "type": "response",
                "request_seq": message["seq"],
                "success": false,
                "command": command,
                "message": text,
            }),
        };
        (self.send)(response);
        if command == "launch" && self.engine.is_some() {
            self.event("initialized", json!({}));
        }
    }

    // ── Talking to the engine ──────────────────────────────────────────

    /// Sends `request` and waits for the engine's answer.
    fn call(&mut self, request: Request) -> Result<Reply, String> {
        let Some(engine) = self.engine.as_mut() else {
            return Err("the program is not running".to_string());
        };
        let id = engine.send(request);
        loop {
            let Ok(input) = self.inputs.recv() else {
                return Err("the debugger has gone".to_string());
            };
            match input {
                Input::Engine(EngineMessage::Reply { id: got, reply }) if got == id => {
                    return Ok(reply);
                }
                Input::Engine(EngineMessage::Error { id: got, message }) if got == id => {
                    return Err(message);
                }
                Input::Output { category, text } => self.output(category, &text),
                Input::Exited(code) => {
                    self.deferred.push_back(Input::Exited(code));
                    return Err("the program has ended".to_string());
                }
                other => self.deferred.push_back(other),
            }
        }
    }

    fn resume(&mut self, how: ResumeKind) -> Result<Value, String> {
        self.call(Request::Resume { how })?;
        self.refs.clear();
        Ok(json!({}))
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        let backend = args["backend"].as_str().unwrap_or("vm");
        if backend != "vm" {
            return Err(format!(
                "the `{backend}` backend cannot be debugged yet: native debugging comes in \
                 proposal 0188 S6; use \"backend\": \"vm\""
            ));
        }
        let program = args["program"]
            .as_str()
            .filter(|program| !program.is_empty())
            .ok_or_else(|| "the launch configuration needs a `program`".to_string())?;
        let program = PathBuf::from(program);
        let flux_command = match &args["fluxCommand"] {
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.as_str().map(str::to_string))
                .collect(),
            Value::String(command) => command.split_whitespace().map(str::to_string).collect(),
            _ => vec!["flux".to_string()],
        };
        let spec = LaunchSpec {
            flux_command,
            cwd: args["cwd"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| program.parent().map(Path::to_path_buf)),
            program,
            flux_args: strings(&args["fluxArgs"]),
            args: strings(&args["args"]),
            record: args["record"].as_str().map(PathBuf::from),
            replay: args["replay"].as_str().map(PathBuf::from),
        };
        self.stop_on_entry = args["stopOnEntry"].as_bool().unwrap_or(false);
        let Some(engine) = VmEngine::launch(&spec, self.sender.clone())? else {
            return Err(
                "the program ended before the debugger could start; see the Debug Console"
                    .to_string(),
            );
        };
        self.engine = Some(engine);
        self.await_ready()?;
        if args["showInternals"].as_bool().unwrap_or(false) {
            self.call(Request::SetInternals { show: true })?;
        }
        Ok(json!({}))
    }

    /// Waits for the engine to say the program is compiled.
    fn await_ready(&mut self) -> Result<(), String> {
        loop {
            let Ok(input) = self.inputs.recv() else {
                return Err("the debugger has gone".to_string());
            };
            match input {
                Input::Engine(EngineMessage::Event {
                    event: Event::Ready,
                }) => return Ok(()),
                Input::Output { category, text } => self.output(category, &text),
                Input::Exited(code) => {
                    self.engine = None;
                    self.deferred.push_back(Input::Exited(code));
                    return Err(
                        "the program ended before the debugger could start; see the Debug \
                         Console"
                            .to_string(),
                    );
                }
                other => self.deferred.push_back(other),
            }
        }
    }

    // ── Requests ───────────────────────────────────────────────────────

    fn set_breakpoints(&mut self, args: &Value) -> Result<Value, String> {
        let file = args["source"]["path"]
            .as_str()
            .ok_or_else(|| "setBreakpoints needs a source path".to_string())?
            .to_string();
        let lines: Vec<usize> = args["breakpoints"]
            .as_array()
            .map(|breakpoints| {
                breakpoints
                    .iter()
                    .filter_map(|bp| bp["line"].as_u64().map(|line| line as usize))
                    .collect()
            })
            .unwrap_or_default();
        let Reply::Breakpoints { lines: set } = self.call(Request::SetBreakpoints {
            file,
            lines: lines.clone(),
        })?
        else {
            return Err("unexpected reply to setBreakpoints".to_string());
        };
        let breakpoints: Vec<Value> = lines
            .iter()
            .zip(set)
            .map(|(asked, set)| match set {
                Some(line) => json!({ "verified": true, "line": line }),
                None => json!({
                    "verified": false,
                    "line": asked,
                    "message": "no code at or after this line",
                }),
            })
            .collect();
        Ok(json!({ "breakpoints": breakpoints }))
    }

    fn frames(&mut self) -> Result<Vec<Frame>, String> {
        match self.call(Request::Stack)? {
            Reply::Stack { frames } => Ok(frames
                .into_iter()
                .filter(|frame| !(frame.file.is_none() && frame.name == "<entry>"))
                .collect()),
            _ => Err("unexpected reply to stackTrace".to_string()),
        }
    }

    fn stack_trace(&mut self, args: &Value) -> Result<Value, String> {
        let mut frames = Vec::new();
        for frame in self.frames()? {
            let mut shown = json!({
                "id": frame.index as i64 + 1,
                "name": frame.name,
                "line": frame.line,
                "column": frame.column,
            });
            if let Some(file) = &frame.file {
                shown["source"] = source(file);
            }
            if frame.artificial {
                shown["presentationHint"] = json!("subtle");
            }
            frames.push(shown);
            if let Some((count, last)) = frame.elided {
                let calls = if count == 1 { "call" } else { "calls" };
                frames.push(json!({
                    "id": LABEL_FRAMES + frame.index as i64,
                    "name": format!("… {count} {calls} elided by tail calls (last: {last})"),
                    "line": 0,
                    "column": 0,
                    "presentationHint": "label",
                }));
            }
        }
        let total = frames.len();
        let start = args["startFrame"].as_u64().unwrap_or(0) as usize;
        let levels = args["levels"].as_u64().unwrap_or(0) as usize;
        let shown: Vec<Value> = frames
            .into_iter()
            .skip(start)
            .take(if levels == 0 { usize::MAX } else { levels })
            .collect();
        Ok(json!({ "stackFrames": shown, "totalFrames": total }))
    }

    fn scopes(&mut self, args: &Value) -> Result<Value, String> {
        let id = args["frameId"].as_i64().unwrap_or(0);
        if !(1..LABEL_FRAMES).contains(&id) {
            return Ok(json!({ "scopes": [] }));
        }
        let frame = id as usize - 1;
        let mut scopes = vec![json!({
            "name": "Locals",
            "presentationHint": "locals",
            "variablesReference": self.reference(frame, Scope::Locals, Vec::new()),
            "expensive": false,
        })];
        let captured = match self.call(Request::Variables {
            frame,
            scope: Scope::Captured,
            path: Vec::new(),
        })? {
            Reply::Variables { variables } => variables,
            _ => Vec::new(),
        };
        if !captured.is_empty() {
            scopes.push(json!({
                "name": "Captured",
                "variablesReference": self.reference(frame, Scope::Captured, Vec::new()),
                "expensive": false,
            }));
        }
        Ok(json!({ "scopes": scopes }))
    }

    fn variables(&mut self, args: &Value) -> Result<Value, String> {
        let reference = args["variablesReference"].as_u64().unwrap_or(0) as usize;
        let Some(var_ref) = reference
            .checked_sub(1)
            .and_then(|index| self.refs.get(index))
            .cloned()
        else {
            return Ok(json!({ "variables": [] }));
        };
        let Reply::Variables { variables } = self.call(Request::Variables {
            frame: var_ref.frame,
            scope: var_ref.scope,
            path: var_ref.path.clone(),
        })?
        else {
            return Err("unexpected reply to variables".to_string());
        };
        let shown: Vec<Value> = variables
            .into_iter()
            .map(|variable| {
                let mut path = var_ref.path.clone();
                path.push(variable.name.clone());
                self.variable_json(var_ref.frame, var_ref.scope, path, variable)
            })
            .collect();
        Ok(json!({ "variables": shown }))
    }

    fn variable_json(
        &mut self,
        frame: usize,
        scope: Scope,
        path: Vec<String>,
        variable: Variable,
    ) -> Value {
        let reference = if variable.has_children {
            self.reference(frame, scope, path)
        } else {
            0
        };
        let mut shown = json!({
            "name": variable.name,
            "value": variable.value,
            "type": variable.type_name,
            "variablesReference": reference,
        });
        if variable.artificial {
            shown["presentationHint"] = json!({ "visibility": "internal" });
        }
        shown
    }

    fn reference(&mut self, frame: usize, scope: Scope, path: Vec<String>) -> usize {
        self.refs.push(VarRef { frame, scope, path });
        self.refs.len()
    }

    /// The frame an expression is read in: the one asked for, else the top.
    fn frame_for(&mut self, args: &Value) -> Result<usize, String> {
        match args["frameId"].as_i64() {
            Some(id) if (1..LABEL_FRAMES).contains(&id) => Ok(id as usize - 1),
            _ => self
                .frames()?
                .first()
                .map(|frame| frame.index)
                .ok_or_else(|| "there is no frame to read".to_string()),
        }
    }

    fn evaluate(&mut self, args: &Value) -> Result<Value, String> {
        let expression = args["expression"].as_str().unwrap_or_default().trim();
        if args["context"] == "repl"
            && let Some(command) = expression.strip_prefix('.')
        {
            return self
                .console_command(command)
                .map(|text| json!({ "result": text, "variablesReference": 0 }));
        }
        let frame = self.frame_for(args)?;
        let Reply::Value { variable } = self.call(Request::Evaluate {
            frame,
            expression: expression.to_string(),
        })?
        else {
            return Err("unexpected reply to evaluate".to_string());
        };
        let path = expression
            .split('.')
            .map(|part| part.trim().to_string())
            .collect();
        let shown = self.variable_json(frame, Scope::Any, path, variable);
        Ok(json!({
            "result": shown["value"],
            "type": shown["type"],
            "variablesReference": shown["variablesReference"],
        }))
    }

    /// The Debug Console's own commands, which reach what DAP has no request
    /// for.
    fn console_command(&mut self, command: &str) -> Result<String, String> {
        let (name, rest) = command
            .split_once(char::is_whitespace)
            .map(|(name, rest)| (name, rest.trim()))
            .unwrap_or((command, ""));
        let number = |rest: &str| {
            rest.parse::<u64>()
                .map_err(|_| format!("usage: .{name} <number>"))
        };
        match name {
            "help" => Ok(CONSOLE_HELP.to_string()),
            "where" => match self.call(Request::Where)? {
                Reply::Position { position } => Ok(format!("position {position}")),
                _ => Err("unexpected reply to .where".to_string()),
            },
            "events" => match self.call(Request::Events)? {
                Reply::Events { events } if events.is_empty() => Ok("no effects yet".to_string()),
                Reply::Events { events } => Ok(events
                    .into_iter()
                    .map(|event| match event.position {
                        Some(position) => {
                            format!("#{} {} (position {position})", event.index, event.text)
                        }
                        None => format!("#{} {} (not reached)", event.index, event.text),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")),
                _ => Err("unexpected reply to .events".to_string()),
            },
            "goto" => self
                .resume(ResumeKind::Goto(number(rest)?))
                .map(|_| String::new()),
            "event" => self
                .resume(ResumeKind::Event(number(rest)?))
                .map(|_| String::new()),
            "rs" => self.resume(ResumeKind::ReverseStep).map(|_| String::new()),
            "rn" => self.resume(ResumeKind::ReverseNext).map(|_| String::new()),
            "rf" => self
                .resume(ResumeKind::ReverseFinish)
                .map(|_| String::new()),
            "rc" => self
                .resume(ResumeKind::ReverseContinue)
                .map(|_| String::new()),
            "internals" => {
                let show = match rest {
                    "on" => true,
                    "off" => false,
                    _ => return Err("usage: .internals on|off".to_string()),
                };
                self.call(Request::SetInternals { show })?;
                Ok(format!("internals {rest}"))
            }
            other => Err(format!("unknown command `.{other}`; try `.help`")),
        }
    }

    // ── Sending ────────────────────────────────────────────────────────

    fn next_seq(&mut self) -> i64 {
        self.seq += 1;
        self.seq
    }

    fn event(&mut self, event: &str, body: Value) {
        let seq = self.next_seq();
        (self.send)(json!({ "seq": seq, "type": "event", "event": event, "body": body }));
    }

    fn output(&mut self, category: &str, text: &str) {
        self.event("output", json!({ "category": category, "output": text }));
    }
}

const CONSOLE_HELP: &str = "\
A variable or a part of one (`pair.0`) shows its value. Commands:
  .where           the position: how many stop sites have run
  .goto <n>        go to a position, earlier or later
  .events          the effects the program has performed
  .event <n>       go to the last stop before effect <n>
  .rs .rn .rf .rc  reverse step, next, finish, continue
  .internals on|off  show or hide compiler-made frames and variables";

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn source(file: &str) -> Value {
    let name = Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    json!({ "name": name, "path": file })
}

fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsStepBack": true,
        "supportsEvaluateForHovers": true,
        "supportsTerminateRequest": true,
        "supportsLoadedSourcesRequest": false,
        "supportsSetVariable": false,
    })
}
