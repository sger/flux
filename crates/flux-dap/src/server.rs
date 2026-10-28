//! A DAP session: requests from the editor, answered by the engine.
//!
//! Everything the session hears arrives as an [`Input`] on one channel: the
//! editor's messages, the engine's replies and events, the program's output
//! and its exit. Requests that need the engine wait for its reply; whatever
//! else arrives meanwhile is set aside, except output, which is passed on at
//! once.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

use flux::debug_protocol::{
    EngineMessage, Event, ExceptionInfo, Frame, FrameKind, Reply, Request, ResumeKind, Scope,
    StopKind, Variable, describe_refs,
};
use serde_json::{Value, json};

use crate::engine::{LaunchSpec, VmEngine};
use crate::native::{self, ConsoleCommand, NativeEngine};

/// Something the session hears.
#[derive(Debug)]
pub enum Input {
    /// A message from the editor.
    Client(Value),
    /// A reply or event from the engine.
    Engine(EngineMessage),
    /// Output from the program, or the compiler before it.
    Output { category: &'static str, text: String },
    /// The program has ended, with this exit code.
    Exited(Option<i32>),
    /// A message from `lldb-dap`, in a native session.
    Native(Value),
    /// `lldb-dap` has ended.
    NativeEnded,
}

/// The program's thread outside `run_async`, and its root fiber inside.
/// Each other fiber is a thread of its own (0188 S8).
const THREAD: i64 = 1;
/// Frame ids at or above this are tail-call labels, not frames.
const LABEL_FRAMES: i64 = 1_000_000;
/// Frame ids at or above this are frames of a thread other than the one
/// that stopped: `OTHER_FRAMES + n` is entry `n` of the session's table.
const OTHER_FRAMES: i64 = 3_000_000;
/// In a native session, frame ids at or above this are lines the session
/// adds to lldb-dap's stack: `⟂ handling …` labels and the frames an effect
/// handler holds (0188 S7). They have no variables. lldb-dap's own frame ids
/// are `(thread index << 19) + frame`, past two million from the fourth
/// thread on, so the session's start far above them and still well inside
/// what a JSON number holds exactly (2^53).
const SPLICED_FRAMES: i64 = 1 << 48;
/// In a native session, thread ids at or above this are fibers that wait:
/// `FIBER_THREADS + n` is thread `n` of `flux threads` (0188 S8). A running
/// fiber is the OS thread lldb-dap knows it by. On macOS that id counts the
/// threads made since boot, millions after a day or two, so fibers start
/// above anything it reaches.
const FIBER_THREADS: i64 = 1 << 49;

/// A `variablesReference`: a frame's scope, or a value in it.
#[derive(Debug, Clone)]
struct VarRef {
    /// The thread the frame is in, when not the one that stopped.
    thread: Option<u64>,
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
    /// The editor's `initialize` arguments, which `lldb-dap` gets too.
    client_init: Value,
    /// In a native session, the `lldb-dap` the session passes messages to.
    native: Option<NativeEngine>,
    /// Requests passed to `lldb-dap`, by its sequence number: the editor's
    /// sequence number and the command.
    pending: HashMap<i64, (Value, String)>,
    /// Requests `lldb-dap` made of the editor: the session's sequence
    /// number for each, and `lldb-dap`'s.
    reverse: HashMap<i64, Value>,
    show_internals: bool,
    source_paths: native::SourcePaths,
    /// The runtime error the program is stopped at, for `exceptionInfo`.
    exception: Option<ExceptionInfo>,
    /// The thread the program last stopped in, while it is stopped.
    stopped: Option<u64>,
    /// Frames of other threads handed out since the last stop.
    other_frames: Vec<(u64, usize)>,
    /// In a native session, the lldb thread the program last stopped in.
    native_stopped: Option<i64>,
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
            client_init: json!({}),
            native: None,
            pending: HashMap::new(),
            reverse: HashMap::new(),
            show_internals: false,
            source_paths: native::SourcePaths::default(),
            exception: None,
            stopped: None,
            other_frames: Vec::new(),
            native_stopped: None,
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
        // Dropping it ends lldb-dap and removes the build.
        self.native = None;
    }

    fn handle(&mut self, input: Input) {
        match input {
            Input::Client(message) if self.native.is_some() => self.on_editor_message(message),
            Input::Client(message) => {
                if message["type"] == "request" {
                    self.request(&message);
                }
            }
            Input::Native(message) => self.on_lldb_dap_message(message),
            Input::NativeEnded => {
                if !self.exited {
                    self.exited = true;
                    self.event("terminated", json!({}));
                }
                self.native = None;
                self.done = true;
            }
            Input::Engine(EngineMessage::Event {
                event: Event::Stopped { reason, position, description, exception, thread },
            }) => {
                self.refs.clear();
                self.other_frames.clear();
                self.stopped = Some(thread);
                let reason = match reason {
                    StopKind::Entry => "entry",
                    StopKind::Breakpoint => "breakpoint",
                    StopKind::Step => "step",
                    StopKind::Pause => "pause",
                    StopKind::Exception => "exception",
                };
                let description = match (&exception, description) {
                    (Some(exception), _) => {
                        format!("Paused on {}", exception_label(&exception.kind))
                    }
                    (None, Some(text)) => format!("position {position}: {text}"),
                    (None, None) => format!("position {position}"),
                };
                let mut body = json!({
                    "reason": reason,
                    "threadId": thread,
                    "allThreadsStopped": true,
                    "description": description,
                });
                if let Some(exception) = &exception {
                    body["text"] = json!(exception.message);
                }
                self.exception = exception;
                self.event("stopped", body);
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
        if command == "launch" && args["backend"] == "native" {
            if let Err(text) = self.launch_native(message) {
                self.respond(message, Err(text));
            }
            return;
        }
        let result = match command.as_str() {
            "initialize" => {
                self.client_init = args.clone();
                Ok(capabilities())
            }
            "launch" => self.launch(args),
            "setBreakpoints" => self.set_breakpoints(args),
            "setExceptionBreakpoints" => {
                let filters = strings(&args["filters"]);
                let count = filters.len();
                self.call(Request::SetExceptionFilters { filters })
                    .map(|_| json!({ "breakpoints": vec![json!({ "verified": true }); count] }))
            }
            "exceptionInfo" => self.exception_info(),
            "setFunctionBreakpoints" => Ok(json!({ "breakpoints": [] })),
            "configurationDone" => {
                let stop_on_entry = self.stop_on_entry;
                self.call(Request::Start { stop_on_entry }).map(|_| json!({}))
            }
            "threads" => self.threads(),
            "stackTrace" => self.stack_trace(args),
            "scopes" => self.scopes(args),
            "variables" => self.variables(args),
            "evaluate" => self.evaluate(args),
            "continue" => {
                self.resume(ResumeKind::Continue).map(|_| json!({ "allThreadsContinued": true }))
            }
            "next" => self.resume(ResumeKind::StepOver),
            "stepIn" => self.resume(ResumeKind::StepIn),
            "stepOut" => self.resume(ResumeKind::StepOut),
            "stepBack" => self.resume(ResumeKind::ReverseNext),
            "reverseContinue" => self.resume(ResumeKind::ReverseContinue),
            "pause" => self.call(Request::Pause).map(|_| json!({})),
            // The Timeline view (0188 S9b).
            "fluxEvents" => self.tape_events(),
            "fluxGoToEvent" => match args["index"].as_u64() {
                Some(number) => self.resume(ResumeKind::Event(number)),
                None => Err("fluxGoToEvent needs an `index`".to_string()),
            },
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
        self.respond(message, result);
        if command == "launch" && self.engine.is_some() {
            self.event("initialized", json!({}));
        }
    }

    /// Answers the editor's request `message`.
    fn respond(&mut self, message: &Value, result: Result<Value, String>) {
        let command = message["command"].clone();
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
        self.other_frames.clear();
        self.stopped = None;
        Ok(json!({}))
    }

    /// The program's threads: its fibers and tasks while it is stopped, and
    /// one thread while it runs.
    fn threads(&mut self) -> Result<Value, String> {
        let main = || json!({ "threads": [{ "id": THREAD, "name": "main" }] });
        if self.stopped.is_none() {
            return Ok(main());
        }
        match self.call(Request::Threads) {
            Ok(Reply::Threads { threads }) => Ok(json!({
                "threads": threads
                    .into_iter()
                    .map(|thread| {
                        let name = if thread.state == "stopped" {
                            thread.name
                        } else {
                            format!("{} — {}", thread.name, thread.state)
                        };
                        json!({ "id": thread.id, "name": name })
                    })
                    .collect::<Vec<_>>(),
            })),
            _ => Ok(main()),
        }
    }

    /// Thread `id` as the engine is asked for it: none for the one that
    /// stopped.
    fn other_thread(&self, id: Option<i64>) -> Option<u64> {
        let id = id.filter(|id| *id > 0)? as u64;
        (Some(id) != self.stopped).then_some(id)
    }

    /// The frame a frame id names: its thread when not the one stopped,
    /// and its index on that thread's stack.
    fn frame_ref(&self, id: i64) -> Option<(Option<u64>, usize)> {
        if (1..LABEL_FRAMES).contains(&id) {
            return Some((None, id as usize - 1));
        }
        let (thread, index) = *self.other_frames.get(id.checked_sub(OTHER_FRAMES)? as usize)?;
        Some((Some(thread), index))
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        let backend = args["backend"].as_str().unwrap_or("vm");
        if backend != "vm" {
            return Err(format!(
                "there is no `{backend}` backend; use \"backend\": \"vm\" or \"native\""
            ));
        }
        let spec = launch_spec(args)?;
        self.stop_on_entry = args["stopOnEntry"].as_bool().unwrap_or(false);
        let Some(engine) = VmEngine::launch(&spec, self.sender.clone())? else {
            return Err("the program ended before the debugger could start; see the Debug Console"
                .to_string());
        };
        self.engine = Some(engine);
        self.await_ready()?;
        if args["showInternals"].as_bool().unwrap_or(false) {
            self.call(Request::SetInternals { show: true })?;
        }
        Ok(json!({}))
    }

    // ── Native sessions: passing messages to lldb-dap ─────────────────

    /// Builds the program with `-g`, starts `lldb-dap` and launches the
    /// program under it. `lldb-dap` answers the editor's `launch` itself.
    fn launch_native(&mut self, message: &Value) -> Result<(), String> {
        if cfg!(windows) {
            return Err(
                "native debugging on Windows comes with proposal 0188 S11; use \"backend\": \"vm\""
                    .to_string(),
            );
        }
        let args = &message["arguments"];
        let spec = launch_spec(args)?;
        let lldb_dap = native::find_lldb_dap(args["lldbDap"].as_str())?;
        let build = native::build(&spec, |category, text| self.output(category, text))?;
        let mut engine =
            NativeEngine::start(&lldb_dap, build.workdir.clone(), self.sender.clone())?;
        let mut init = self.client_init.clone();
        if !init.is_object() {
            init = json!({});
        }
        init["adapterID"] = json!("lldb-dap");
        // DAP's defaults, which newer lldb-dap wants spelled out.
        for (key, default) in [
            ("pathFormat", json!("path")),
            ("linesStartAt1", json!(true)),
            ("columnsStartAt1", json!(true)),
        ] {
            if init.get(key).is_none() {
                init[key] = default;
            }
        }
        let init_seq = engine.request("initialize", init);
        self.native = Some(engine);
        self.show_internals = args["showInternals"].as_bool().unwrap_or(false);

        // lldb-dap's capabilities replace the VM's: no Step Back.
        let reply = self.await_lldb_dap(init_seq)?;
        let mut capabilities = reply["body"].clone();
        if !capabilities.is_object() {
            capabilities = json!({});
        }
        capabilities["supportsStepBack"] = json!(false);
        // Flux's runtime errors, not C++'s and Objective-C's.
        capabilities["exceptionBreakpointFilters"] = exception_filters();
        capabilities["supportsExceptionInfoRequest"] = json!(true);
        self.event("capabilities", json!({ "capabilities": capabilities }));

        let arguments = native::launch_arguments(&spec, &build, args);
        let engine = self.native.as_mut().expect("just started");
        let seq = engine.request("launch", arguments);
        self.pending.insert(seq, (message["seq"].clone(), "launch".to_string()));
        Ok(())
    }

    /// Waits for `lldb-dap`'s answer to its request `seq`.
    fn await_lldb_dap(&mut self, seq: i64) -> Result<Value, String> {
        loop {
            let Ok(input) = self.inputs.recv() else {
                return Err("lldb-dap has gone".to_string());
            };
            match input {
                Input::Native(message)
                    if message["type"] == "response" && message["request_seq"] == seq =>
                {
                    if message["success"] == false {
                        let reason = message["body"]["error"]["format"]
                            .as_str()
                            .or_else(|| message["message"].as_str())
                            .unwrap_or("no reason given");
                        return Err(format!("lldb-dap refused: {reason}"));
                    }
                    return Ok(message);
                }
                Input::Output { category, text } => self.output(category, &text),
                Input::NativeEnded => {
                    self.native = None;
                    return Err("lldb-dap ended before the program started".to_string());
                }
                other => self.deferred.push_back(other),
            }
        }
    }

    /// A message from the editor, in a native session.
    fn on_editor_message(&mut self, message: Value) {
        match message["type"].as_str() {
            Some("request") => {}
            Some("response") => {
                // An answer to a request lldb-dap made of the editor.
                let Some(seq) = message["request_seq"].as_i64() else {
                    return;
                };
                if let Some(theirs) = self.reverse.remove(&seq) {
                    let mut message = message;
                    message["request_seq"] = theirs;
                    if let Some(engine) = self.native.as_mut() {
                        engine.send(message);
                    }
                }
                return;
            }
            _ => return,
        }
        let command = message["command"].as_str().unwrap_or_default().to_string();
        let mut message = message;
        // A fiber that waits is no thread of lldb-dap's: run requests go
        // to the thread the program stopped in.
        if is_fiber_thread(&message)
            && command != "stackTrace"
            && let Some(stopped) = self.native_stopped
        {
            message["arguments"]["threadId"] = json!(stopped);
        }
        match command.as_str() {
            "threads" => {
                // The fibers of `run_async`, when the program is in it.
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({ "expression": "`flux threads --json", "context": "repl" }),
                    );
                    self.pending.insert(seq, (message, "@threads".to_string()));
                }
                return;
            }
            "stackTrace" if is_fiber_thread(&message) => {
                let arguments = &message["arguments"];
                let number = arguments["threadId"].as_i64().unwrap_or(0) - FIBER_THREADS;
                let held = json!({
                    "editor": {
                        "seq": message["seq"],
                        "start": arguments["startFrame"].as_u64().unwrap_or(0),
                        "levels": arguments["levels"].as_u64().unwrap_or(0),
                    },
                    "response": {
                        "type": "response",
                        "command": "stackTrace",
                        "success": true,
                        "body": { "stackFrames": [] },
                    },
                });
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({
                            "expression": format!("`flux bt --json --thread {number}"),
                            "context": "repl",
                        }),
                    );
                    self.pending.insert(seq, (held, "@flux-bt".to_string()));
                }
                return;
            }
            "continue" => {
                // As on the VM, continuing runs to the next breakpoint, not to
                // the end of a step a breakpoint interrupted.
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({ "expression": "`flux forget-steps", "context": "repl" }),
                    );
                    self.pending.insert(seq, (Value::Null, String::new()));
                }
            }
            "stepBack" | "reverseContinue" => {
                self.respond(
                    &message,
                    Err("going back needs the VM backend: native programs do not record their \
                         run"
                    .to_string()),
                );
                return;
            }
            // `flux_lldb.py` steps as `flux debug` does: once per line, and
            // across a perform to where its frame goes on.
            "next" | "stepIn" | "stepOut" => {
                let step = match command.as_str() {
                    "next" => "next",
                    "stepIn" => "step",
                    _ => "finish",
                };
                self.respond(&message, Ok(json!({})));
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({ "expression": format!("`flux {step}"), "context": "repl" }),
                    );
                    self.pending.insert(seq, (Value::Null, "@step".to_string()));
                }
                return;
            }
            "setExceptionBreakpoints" => {
                let filters = strings(&message["arguments"]["filters"]);
                let kinds = if filters.is_empty() { "none".to_string() } else { filters.join(" ") };
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({ "expression": format!("`flux catch {kinds}"), "context": "repl" }),
                    );
                    self.pending.insert(seq, (Value::Null, String::new()));
                }
                let verified = vec![json!({ "verified": true }); filters.len()];
                self.respond(&message, Ok(json!({ "breakpoints": verified })));
                return;
            }
            "exceptionInfo" => {
                let answer = self.exception_info();
                self.respond(&message, answer);
                return;
            }
            // The Timeline view (0188 S9b): the tape's events, and going to one.
            "fluxEvents" => {
                if let Some(engine) = self.native.as_mut() {
                    let seq = engine.request(
                        "evaluate",
                        json!({ "expression": "`flux events --json", "context": "repl" }),
                    );
                    self.pending.insert(seq, (message, "@events".to_string()));
                }
                return;
            }
            "fluxGoToEvent" => {
                match message["arguments"]["index"].as_u64() {
                    Some(number) => {
                        let lldb = native::go_to_event(number);
                        self.run_to_stop(message, &lldb, "@goto-event");
                    }
                    None => self.respond(&message, Err("fluxGoToEvent needs an `index`".into())),
                }
                return;
            }
            "scopes" if is_spliced_frame(&message) => {
                self.respond(&message, Ok(json!({ "scopes": [] })));
                return;
            }
            "evaluate" if is_spliced_frame(&message) => {
                if let Some(arguments) = message["arguments"].as_object_mut() {
                    arguments.remove("frameId");
                }
            }
            "evaluate" if message["arguments"]["context"] == "repl" => {
                let expression = message["arguments"]["expression"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if let Some(command) = expression.strip_prefix('.') {
                    match native::console_command(command) {
                        ConsoleCommand::Lldb(lldb) => {
                            message["arguments"]["expression"] = json!(format!("`{lldb}"));
                        }
                        ConsoleCommand::Run(lldb) => {
                            self.run_to_stop(message, &lldb, "@run");
                            return;
                        }
                        ConsoleCommand::Answer(answer) => {
                            let answer = answer
                                .map(|text| json!({ "result": text, "variablesReference": 0 }));
                            self.respond(&message, answer);
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
        let mut editor_seq = message["seq"].clone();
        let mut command = command;
        if command == "stackTrace" && !self.show_internals {
            // The whole stack, for the frames the session adds to it; the
            // editor's page of it is cut once they are in.
            let arguments = &mut message["arguments"];
            editor_seq = json!({
                "seq": editor_seq,
                "start": arguments["startFrame"].as_u64().unwrap_or(0),
                "levels": arguments["levels"].as_u64().unwrap_or(0),
            });
            arguments["startFrame"] = json!(0);
            arguments["levels"] = json!(0);
            command = "@stackTrace".to_string();
        }
        self.source_paths.to_lldb(&mut message["arguments"]);
        let Some(engine) = self.native.as_mut() else {
            return;
        };
        let seq = engine.send(message);
        self.pending.insert(seq, (editor_seq, command));
    }

    /// A message from lldb-dap: renumbered for the editor, with the stack
    /// made Flux's.
    fn on_lldb_dap_message(&mut self, mut message: Value) {
        self.source_paths.to_editor(&mut message);
        match message["type"].as_str() {
            Some("response") => {
                let Some(seq) = message["request_seq"].as_i64() else {
                    return;
                };
                let Some((editor_seq, command)) = self.pending.remove(&seq) else {
                    return;
                };
                if command.is_empty() {
                    // The adapter's own request: no one to tell.
                    return;
                }
                match command.as_str() {
                    "@stackTrace" => return self.splice_stack(editor_seq, message),
                    "@flux-bt" => return self.send_spliced_stack(editor_seq, &message),
                    "@stopped" => return self.send_stop(editor_seq, &message),
                    "@threads" => return self.send_threads(editor_seq, &message),
                    "@step" => return self.after_step(&message),
                    "@events" => {
                        let text = message["body"]["result"].as_str().unwrap_or_default();
                        let events = native::tape_events(text);
                        return self.respond(&editor_seq, Ok(events));
                    }
                    "@run" | "@goto-event" => {
                        return self.after_run(&editor_seq, &message, command == "@run");
                    }
                    _ => {}
                }
                if message["success"] == true {
                    let body = message["body"].take();
                    message["body"] = match command.as_str() {
                        "stackTrace" => native::flux_stack(body, self.show_internals),
                        "scopes" => native::flux_scopes(body, self.show_internals),
                        "variables" => native::flux_variables(body),
                        "evaluate" => native::flux_evaluation(body),
                        _ => body,
                    };
                }
                message["request_seq"] = editor_seq;
                message["seq"] = json!(self.next_seq());
                (self.send)(message);
                if command == "disconnect" {
                    self.done = true;
                }
            }
            Some("event") => {
                if message["event"] == "terminated" {
                    self.exited = true;
                }
                if message["event"] == "stopped" {
                    // Held until `flux where` says whether it is a runtime
                    // error the session catches.
                    if let Some(engine) = self.native.as_mut() {
                        let seq = engine.request(
                            "evaluate",
                            json!({ "expression": "`flux where", "context": "repl" }),
                        );
                        self.pending.insert(seq, (message, "@stopped".to_string()));
                        return;
                    }
                }
                message["seq"] = json!(self.next_seq());
                (self.send)(message);
            }
            Some("request") => {
                let seq = self.next_seq();
                self.reverse.insert(seq, message["seq"].clone());
                message["seq"] = json!(seq);
                (self.send)(message);
            }
            _ => {}
        }
    }

    /// lldb-dap's whole stack has come: ask `flux_lldb.py` for the frames it
    /// does not see.
    fn splice_stack(&mut self, editor: Value, mut response: Value) {
        if response["success"] != true {
            response["request_seq"] = editor["seq"].clone();
            response["seq"] = json!(self.next_seq());
            (self.send)(response);
            return;
        }
        let Some(engine) = self.native.as_mut() else {
            return;
        };
        let mut arguments = json!({ "expression": "`flux bt --json", "context": "repl" });
        if let Some(frame) = response["body"]["stackFrames"][0]["id"].as_i64() {
            arguments["frameId"] = json!(frame);
        }
        let seq = engine.request("evaluate", arguments);
        let held = json!({ "editor": editor, "response": response });
        self.pending.insert(seq, (held, "@flux-bt".to_string()));
    }

    /// The stack for the editor: lldb-dap's Flux frames, with the frames an
    /// effect handler holds and their labels between them.
    fn send_spliced_stack(&mut self, held: Value, evaluated: &Value) {
        let editor = &held["editor"];
        let mut response = held["response"].clone();
        let body = response["body"].take();
        let text = evaluated["body"]["result"].as_str().unwrap_or_default();
        let lines: Option<Vec<Value>> = text
            .lines()
            .find(|line| line.trim_start().starts_with('['))
            .and_then(|line| serde_json::from_str(line.trim()).ok());
        let mut frames = match lines {
            Some(lines) => native::spliced_stack(&body, &lines, SPLICED_FRAMES),
            None => native::flux_stack(body, false)["stackFrames"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        };
        let total = frames.len();
        let start = editor["start"].as_u64().unwrap_or(0) as usize;
        let levels = editor["levels"].as_u64().unwrap_or(0) as usize;
        frames = frames
            .into_iter()
            .skip(start)
            .take(if levels == 0 { usize::MAX } else { levels })
            .collect();
        response["body"] = json!({ "stackFrames": frames, "totalFrames": total });
        self.source_paths.to_editor(&mut response);
        response["request_seq"] = editor["seq"].clone();
        response["seq"] = json!(self.next_seq());
        (self.send)(response);
    }

    /// The editor's `threads`: the fibers `flux threads` lists, or, outside
    /// `run_async`, lldb-dap's own threads.
    fn send_threads(&mut self, request: Value, evaluated: &Value) {
        let text = evaluated["body"]["result"].as_str().unwrap_or_default();
        match native::fiber_threads(text, FIBER_THREADS) {
            Some(threads) => self.respond(&request, Ok(json!({ "threads": threads }))),
            None => {
                let editor_seq = request["seq"].clone();
                let Some(engine) = self.native.as_mut() else {
                    return;
                };
                let seq = engine.send(request);
                self.pending.insert(seq, (editor_seq, "threads".to_string()));
            }
        }
    }

    /// A held `stopped` event, now that `flux where` has said why.
    fn send_stop(&mut self, mut event: Value, evaluated: &Value) {
        self.native_stopped = event["body"]["threadId"].as_i64();
        let text = evaluated["body"]["result"].as_str().unwrap_or_default();
        self.exception = native::stop_exception(text);
        if let Some(exception) = &self.exception {
            event["body"]["reason"] = json!("exception");
            event["body"]["description"] =
                json!(format!("Paused on {}", exception_label(&exception.kind)));
            event["body"]["text"] = json!(exception.message);
        }
        event["seq"] = json!(self.next_seq());
        (self.send)(event);
    }

    /// Runs `lldb`, an lldb command that runs the program until it stops,
    /// for the editor's `request`, which [`Self::after_run`] answers.
    fn run_to_stop(&mut self, request: Value, lldb: &str, kind: &str) {
        if let Some(engine) = self.native.as_mut() {
            let seq = engine.request(
                "evaluate",
                json!({ "expression": format!("`{lldb}"), "context": "repl" }),
            );
            self.pending.insert(seq, (request, kind.to_string()));
        }
    }

    /// A command [`Self::run_to_stop`] ran has finished: answer the
    /// editor's request with what it said, and report the stop.
    fn after_run(&mut self, request: &Value, evaluated: &Value, console: bool) {
        let text = evaluated["body"]["result"].as_str().unwrap_or_default();
        let said = text
            .lines()
            .filter(|line| !line.trim().starts_with("thread "))
            .collect::<Vec<_>>()
            .join("\n");
        if native::stopped_thread(text).is_none() && !said.contains("ended") {
            // It refused: going back, an event not on the tape, no tape.
            self.respond(request, Err(said));
            return;
        }
        let body =
            if console { json!({ "result": said, "variablesReference": 0 }) } else { json!({}) };
        self.respond(request, Ok(body));
        self.after_step(evaluated);
    }

    /// A `flux next`, `step` or `finish` has run: lldb ran it in the
    /// command's own synchronous mode, so lldb-dap saw no stop to report.
    /// Report it.
    fn after_step(&mut self, evaluated: &Value) {
        let text = evaluated["body"]["result"].as_str().unwrap_or_default();
        let Some(thread) = native::stopped_thread(text) else {
            // The program ended: lldb-dap says so itself.
            return;
        };
        let event = json!({
            "type": "event",
            "event": "stopped",
            "body": { "reason": "step", "threadId": thread, "allThreadsStopped": true },
        });
        self.on_lldb_dap_message(event);
    }

    /// Waits for the engine to say the program is compiled.
    fn await_ready(&mut self) -> Result<(), String> {
        loop {
            let Ok(input) = self.inputs.recv() else {
                return Err("the debugger has gone".to_string());
            };
            match input {
                Input::Engine(EngineMessage::Event { event: Event::Ready }) => return Ok(()),
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
        let Reply::Breakpoints { lines: set } =
            self.call(Request::SetBreakpoints { file, lines: lines.clone() })?
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

    fn frames(&mut self, thread: Option<u64>) -> Result<Vec<Frame>, String> {
        match self.call(Request::Stack { thread })? {
            Reply::Stack { frames } => Ok(frames
                .into_iter()
                .filter(|frame| !(frame.file.is_none() && frame.name == "<entry>"))
                .collect()),
            _ => Err("unexpected reply to stackTrace".to_string()),
        }
    }

    fn stack_trace(&mut self, args: &Value) -> Result<Value, String> {
        let thread = self.other_thread(args["threadId"].as_i64());
        let mut frames = Vec::new();
        for frame in self.frames(thread)? {
            if frame.kind == FrameKind::Label {
                frames.push(json!({
                    "id": LABEL_FRAMES + frame.index as i64,
                    "name": format!("⟂ {}", frame.name),
                    "line": 0,
                    "column": 0,
                    "presentationHint": "label",
                }));
                continue;
            }
            let name = match frame.kind {
                FrameKind::Suspended => format!("{} (suspended)", frame.name),
                FrameKind::Dropped => format!("{} (suspended, not resumable)", frame.name),
                _ => frame.name,
            };
            let id = match thread {
                None => frame.index as i64 + 1,
                Some(thread) => {
                    self.other_frames.push((thread, frame.index));
                    OTHER_FRAMES + self.other_frames.len() as i64 - 1
                }
            };
            let mut shown = json!({
                "id": id,
                "name": name,
                "line": frame.line,
                "column": frame.column,
            });
            if let Some(file) = &frame.file {
                shown["source"] = source(file);
            }
            if frame.artificial || frame.kind != FrameKind::Live {
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
        let Some((thread, frame)) = self.frame_ref(id) else {
            return Ok(json!({ "scopes": [] }));
        };
        let mut scopes = vec![json!({
            "name": "Locals",
            "presentationHint": "locals",
            "variablesReference": self.reference(thread, frame, Scope::Locals, Vec::new()),
            "expensive": false,
        })];
        let captured = match self.call(Request::Variables {
            frame,
            scope: Scope::Captured,
            path: Vec::new(),
            thread,
        })? {
            Reply::Variables { variables } => variables,
            _ => Vec::new(),
        };
        if !captured.is_empty() {
            scopes.push(json!({
                "name": "Captured",
                "variablesReference": self.reference(thread, frame, Scope::Captured, Vec::new()),
                "expensive": false,
            }));
        }
        if thread.is_none() && !self.handlers()?.is_empty() {
            scopes.push(json!({
                "name": "Handlers",
                "variablesReference": self.reference(thread, frame, Scope::Handlers, Vec::new()),
                "expensive": false,
            }));
        }
        Ok(json!({ "scopes": scopes }))
    }

    /// The active effect handlers, innermost first.
    fn handlers(&mut self) -> Result<Vec<Variable>, String> {
        match self.call(Request::Variables {
            frame: 0,
            scope: Scope::Handlers,
            path: Vec::new(),
            thread: None,
        })? {
            Reply::Variables { variables } => Ok(variables),
            _ => Err("unexpected reply to handlers".to_string()),
        }
    }

    fn exception_info(&mut self) -> Result<Value, String> {
        let exception = self
            .exception
            .as_ref()
            .ok_or_else(|| "the program is not stopped at an exception".to_string())?;
        Ok(json!({
            "exceptionId": exception.kind,
            "description": exception.message,
            "breakMode": "always",
            "details": {
                "message": exception.message,
                "typeName": exception_label(&exception.kind),
            },
        }))
    }

    fn variables(&mut self, args: &Value) -> Result<Value, String> {
        let reference = args["variablesReference"].as_u64().unwrap_or(0) as usize;
        let Some(var_ref) =
            reference.checked_sub(1).and_then(|index| self.refs.get(index)).cloned()
        else {
            return Ok(json!({ "variables": [] }));
        };
        let Reply::Variables { variables } = self.call(Request::Variables {
            frame: var_ref.frame,
            scope: var_ref.scope,
            path: var_ref.path.clone(),
            thread: var_ref.thread,
        })?
        else {
            return Err("unexpected reply to variables".to_string());
        };
        let shown: Vec<Value> = variables
            .into_iter()
            .map(|variable| {
                let mut path = var_ref.path.clone();
                path.push(variable.name.clone());
                self.variable_json(var_ref.thread, var_ref.frame, var_ref.scope, path, variable)
            })
            .collect();
        Ok(json!({ "variables": shown }))
    }

    fn variable_json(
        &mut self,
        thread: Option<u64>,
        frame: usize,
        scope: Scope,
        path: Vec<String>,
        variable: Variable,
    ) -> Value {
        let reference =
            if variable.has_children { self.reference(thread, frame, scope, path) } else { 0 };
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

    fn reference(
        &mut self,
        thread: Option<u64>,
        frame: usize,
        scope: Scope,
        path: Vec<String>,
    ) -> usize {
        self.refs.push(VarRef { thread, frame, scope, path });
        self.refs.len()
    }

    /// The frame an expression is read in: the one asked for, else the top
    /// of the stopped thread.
    fn frame_for(&mut self, args: &Value) -> Result<(Option<u64>, usize), String> {
        if let Some(found) = args["frameId"].as_i64().and_then(|id| self.frame_ref(id)) {
            return Ok(found);
        }
        self.frames(None)?
            .first()
            .map(|frame| (None, frame.index))
            .ok_or_else(|| "there is no frame to read".to_string())
    }

    fn evaluate(&mut self, args: &Value) -> Result<Value, String> {
        let expression = args["expression"].as_str().unwrap_or_default().trim();
        if args["context"] == "repl"
            && let Some(command) = expression.strip_prefix('.')
        {
            return self
                .console_command(command, args)
                .map(|text| json!({ "result": text, "variablesReference": 0 }));
        }
        let (thread, frame) = self.frame_for(args)?;
        let Reply::Value { variable } =
            self.call(Request::Evaluate { frame, thread, expression: expression.to_string() })?
        else {
            return Err("unexpected reply to evaluate".to_string());
        };
        let path = expression.split('.').map(|part| part.trim().to_string()).collect();
        let shown = self.variable_json(thread, frame, Scope::Any, path, variable);
        Ok(json!({
            "result": shown["value"],
            "type": shown["type"],
            "variablesReference": shown["variablesReference"],
        }))
    }

    /// The Debug Console's own commands, which reach what DAP has no request
    /// for.
    fn console_command(&mut self, command: &str, args: &Value) -> Result<String, String> {
        let (name, rest) = command
            .split_once(char::is_whitespace)
            .map(|(name, rest)| (name, rest.trim()))
            .unwrap_or((command, ""));
        let number =
            |rest: &str| rest.parse::<u64>().map_err(|_| format!("usage: .{name} <number>"));
        match name {
            "help" => Ok(CONSOLE_HELP.to_string()),
            // In the frame the console reads (0188 S10b).
            "refs" => {
                if rest.is_empty() {
                    return Err("usage: .refs <name>[.<n>...]".to_string());
                }
                let (thread, frame) = self.frame_for(args)?;
                match self.call(Request::Refs { frame, expression: rest.to_string(), thread })? {
                    Reply::Refs { count, type_name } => {
                        Ok(describe_refs(rest, count, false, &type_name))
                    }
                    _ => Err("unexpected reply to .refs".to_string()),
                }
            }
            "handlers" => {
                let handlers = self.handlers()?;
                if handlers.is_empty() {
                    return Ok("no handlers".to_string());
                }
                Ok(handlers
                    .into_iter()
                    .enumerate()
                    .map(|(number, handler)| match handler.type_name.as_str() {
                        "handler" if handler.value.is_empty() => {
                            format!("#{number} {}", handler.name)
                        }
                        "handler" => format!("#{number} {} — {}", handler.name, handler.value),
                        _ => format!("#{number} {} state = {}", handler.name, handler.value),
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "threads" => match self.call(Request::Threads)? {
                Reply::Threads { threads } => Ok(threads
                    .into_iter()
                    .map(|thread| format!("{} {} [{}]", thread.id, thread.name, thread.state))
                    .collect::<Vec<_>>()
                    .join("\n")),
                _ => Err("unexpected reply to .threads".to_string()),
            },
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
            "goto" => self.resume(ResumeKind::Goto(number(rest)?)).map(|_| String::new()),
            "event" => self.resume(ResumeKind::Event(number(rest)?)).map(|_| String::new()),
            "rs" => self.resume(ResumeKind::ReverseStep).map(|_| String::new()),
            "rn" => self.resume(ResumeKind::ReverseNext).map(|_| String::new()),
            "rf" => self.resume(ResumeKind::ReverseFinish).map(|_| String::new()),
            "rc" => self.resume(ResumeKind::ReverseContinue).map(|_| String::new()),
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

    /// The tape's events for the Timeline view: each with where it happened
    /// and whether the run, after going back, has reached it again.
    fn tape_events(&mut self) -> Result<Value, String> {
        let Reply::Events { events } = self.call(Request::Events)? else {
            return Err("unexpected reply to fluxEvents".to_string());
        };
        let current = match self.call(Request::Where) {
            Ok(Reply::Position { position }) => Some(position),
            _ => None,
        };
        let events: Vec<Value> = events
            .into_iter()
            .map(|event| {
                let reached =
                    matches!((event.position, current), (Some(at), Some(now)) if at <= now);
                json!({
                    "index": event.index,
                    "text": event.text,
                    "position": event.position,
                    "reached": reached,
                })
            })
            .collect();
        let next = events.iter().position(|event| event["reached"] != true);
        let mut events = events;
        if let Some(next) = next {
            events[next]["next"] = json!(true);
        }
        Ok(json!({ "events": events, "position": current }))
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
  .refs <name>     how many references a value has: a unique one can be reused
  .handlers        the active effect handlers, innermost first
  .threads         the fibers and tasks, and what each waits for
  .internals on|off  show or hide compiler-made frames and variables";

/// How to run the program, from the editor's launch arguments.
fn launch_spec(args: &Value) -> Result<LaunchSpec, String> {
    let program = args["program"]
        .as_str()
        .filter(|program| !program.is_empty())
        .ok_or_else(|| "the launch configuration needs a `program`".to_string())?;
    let program = PathBuf::from(program);
    let flux_command = match &args["fluxCommand"] {
        Value::Array(parts) => {
            parts.iter().filter_map(|part| part.as_str().map(str::to_string)).collect()
        }
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
    Ok(spec)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The DAP `source` of `file`, without the `\\?\` of a canonical Windows
/// path: the editor would not take that for the file it has open.
fn source(file: &str) -> Value {
    let file = flux::shared::paths::without_verbatim_prefix(file);
    let file = file.as_ref();
    let name = Path::new(file)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    json!({ "name": name, "path": file })
}

/// The exception breakpoint filters, which the engine's exception kinds
/// name.
const EXCEPTION_FILTERS: [(&str, &str, bool); 5] = [
    ("panic", "Panics", true),
    ("division_by_zero", "Division by zero", true),
    ("unhandled_effect", "Unhandled effects", true),
    ("fiber_failure", "Fiber failures", true),
    ("runtime_error", "Other runtime errors", false),
];

fn exception_filters() -> Value {
    EXCEPTION_FILTERS
        .iter()
        .map(|(filter, label, default)| {
            json!({ "filter": filter, "label": label, "default": default })
        })
        .collect()
}

/// What a stop at an exception of this kind says it is.
fn exception_label(kind: &str) -> &str {
    match kind {
        "panic" => "panic",
        "division_by_zero" => "division by zero",
        "unhandled_effect" => "unhandled effect",
        "fiber_failure" => "fiber failure",
        "runtime_error" => "runtime error",
        other => other,
    }
}

/// Whether a native request names a fiber that waits, not a thread of
/// lldb-dap's.
fn is_fiber_thread(request: &Value) -> bool {
    request["arguments"]["threadId"].as_i64() >= Some(FIBER_THREADS)
}

/// Whether a native request names a frame the session added to lldb-dap's
/// stack.
fn is_spliced_frame(request: &Value) -> bool {
    request["arguments"]["frameId"].as_i64() >= Some(SPLICED_FRAMES)
}

fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "exceptionBreakpointFilters": exception_filters(),
        "supportsExceptionInfoRequest": true,
        "supportsStepBack": true,
        "supportsEvaluateForHovers": true,
        "supportsTerminateRequest": true,
        "supportsLoadedSourcesRequest": false,
        "supportsSetVariable": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On macOS a thread id counts every thread since boot: this one is from
    /// a Mac up for 40 hours, where the program's own thread was taken for a
    /// waiting fiber and its stack came back empty.
    #[test]
    fn a_thread_lldb_dap_names_is_not_a_fiber() {
        let request = json!({ "arguments": { "threadId": 7_175_886 } });
        assert!(!is_fiber_thread(&request));
    }

    /// lldb-dap numbers a frame `(thread index << 19) + frame`, so the frames
    /// of the fourth thread are past two million.
    #[test]
    fn a_frame_lldb_dap_names_is_not_one_the_session_added() {
        let request = json!({ "arguments": { "frameId": (4 << 19) + 2 } });
        assert!(!is_spliced_frame(&request));
    }
}
