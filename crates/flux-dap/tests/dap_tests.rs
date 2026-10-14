//! DAP sessions end to end (proposal 0188 S4, and S6 for native ones).
//!
//! Each test runs the adapter in-process and plays the editor: it sends DAP
//! requests and waits for the responses and events an editor would. The
//! program runs in a real `flux debug --engine` child. The transcript of
//! requests, responses and awaited events, with sequence numbers and paths
//! taken out, is snapshotted, followed by what the program printed.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use flux_dap::{Input, Session};
use serde_json::{Value, json};

/// The `flux` binary the adapter starts: the one built next to these tests
/// (`cargo test --all` builds it), built here when it is missing.
fn flux_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(bin) = std::env::var_os("FLUX_BIN") {
            return PathBuf::from(bin);
        }
        // tests run from <target>/<profile>/deps/
        let exe = std::env::current_exe().expect("test executable");
        let profile_dir = exe.parent().and_then(Path::parent).expect("target dir");
        let bin = profile_dir.join(format!("flux{}", std::env::consts::EXE_SUFFIX));
        if !bin.exists() {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let target_dir = profile_dir.parent().expect("target dir");
            let status = Command::new(env!("CARGO"))
                .current_dir(&root)
                .args(["build", "--bin", "flux", "--target-dir"])
                .arg(target_dir)
                .status()
                .expect("run cargo build");
            assert!(status.success(), "cargo build --bin flux failed");
        }
        bin
    })
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("flux-dap-tests-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The editor's side of a session.
struct Editor {
    to_adapter: Sender<Input>,
    from_adapter: Receiver<Value>,
    seq: i64,
    /// Events received but not yet awaited.
    pending: VecDeque<Value>,
    transcript: Vec<String>,
    output: String,
    dir: String,
}

const WAIT: Duration = Duration::from_secs(60);

impl Editor {
    fn start(scratch: &Scratch) -> Self {
        let (to_adapter, inputs) = channel();
        let (out, from_adapter) = channel();
        let sender = to_adapter.clone();
        std::thread::spawn(move || {
            Session::new(inputs, sender, move |message| {
                let _ = out.send(message);
            })
            .run();
        });
        Self {
            to_adapter,
            from_adapter,
            seq: 0,
            pending: VecDeque::new(),
            transcript: Vec::new(),
            output: String::new(),
            dir: scratch.dir.display().to_string(),
        }
    }

    /// The launch arguments for `program`, plus `extra`.
    fn launch_args(&self, program: &Path, extra: Value) -> Value {
        let mut args = json!({
            "program": program,
            "fluxCommand": [flux_bin()],
            "fluxArgs": ["--no-cache"],
        });
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                args.insert(key.clone(), value.clone());
            }
        }
        args
    }

    fn receive(&mut self) -> Value {
        let message = self
            .from_adapter
            .recv_timeout(WAIT)
            .expect("the adapter answers in time");
        if message["type"] == "event" && message["event"] == "output" {
            let text = message["body"]["output"].as_str().unwrap_or_default();
            self.output.push_str(text);
        }
        message
    }

    /// Sends a request and waits for its response.
    fn request(&mut self, command: &str, arguments: Value) -> Value {
        self.seq += 1;
        let seq = self.seq;
        let mut message = json!({ "seq": seq, "type": "request", "command": command });
        if !arguments.is_null() {
            message["arguments"] = arguments.clone();
        }
        self.record(&format!("-> {command}"), &arguments);
        self.to_adapter.send(Input::Client(message)).unwrap();
        loop {
            let message = self.receive();
            if message["type"] == "response" && message["request_seq"] == seq {
                self.record("<-", &message);
                return message;
            }
            if message["type"] == "event" && message["event"] != "output" {
                self.pending.push_back(message);
            }
        }
    }

    /// Sends a request without waiting: its response comes with
    /// [`Editor::response`].
    fn send(&mut self, command: &str, arguments: Value) -> i64 {
        self.seq += 1;
        let seq = self.seq;
        let message =
            json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments });
        self.record(&format!("-> {command}"), &arguments);
        self.to_adapter.send(Input::Client(message)).unwrap();
        seq
    }

    /// Waits for the response to request `seq`, keeping events for later.
    fn response(&mut self, seq: i64) -> Value {
        if let Some(index) = self
            .pending
            .iter()
            .position(|m| m["type"] == "response" && m["request_seq"] == seq)
        {
            return self.pending.remove(index).unwrap();
        }
        loop {
            let message = self.receive();
            if message["type"] == "response" && message["request_seq"] == seq {
                self.record("<-", &message);
                return message;
            }
            if message["type"] != "event" || message["event"] != "output" {
                self.pending.push_back(message);
            }
        }
    }

    /// Waits for the next `event` event.
    fn event(&mut self, event: &str) -> Value {
        if let Some(index) = self.pending.iter().position(|m| m["event"] == event) {
            let message = self.pending.remove(index).unwrap();
            self.record("<- event", &message);
            return message;
        }
        loop {
            let message = self.receive();
            if message["type"] == "event" && message["event"] == event {
                self.record("<- event", &message);
                return message;
            }
            if message["type"] == "event" && message["event"] != "output" {
                self.pending.push_back(message);
            }
        }
    }

    fn record(&mut self, label: &str, message: &Value) {
        let mut message = message.clone();
        if let Some(object) = message.as_object_mut() {
            object.remove("seq");
            object.remove("request_seq");
            object.remove("type");
        }
        let mut text = serde_json::to_string(&message).unwrap();
        // The temporary directory can be reached through a symlink
        // (`/var` is `/private/var` on macOS): take out both spellings.
        if let Ok(resolved) = std::fs::canonicalize(&self.dir) {
            text = text.replace(&resolved.display().to_string(), "<dir>");
        }
        text = text.replace(&self.dir, "<dir>");
        text = text.replace(&flux_bin().display().to_string(), "<flux>");
        if std::env::var_os("FLUX_DAP_TEST_TRACE").is_some() {
            eprintln!("{label} {text}");
        }
        self.transcript.push(format!("{label} {text}"));
    }

    /// The common start: initialize, launch, breakpoints, go.
    fn launch(&mut self, program: &Path, extra: Value, breakpoints: &[usize]) {
        self.request("initialize", json!({ "adapterID": "flux" }));
        let args = self.launch_args(program, extra);
        let launched = self.request("launch", args);
        assert_eq!(launched["success"], true, "{launched}");
        self.event("initialized");
        if !breakpoints.is_empty() {
            let lines: Vec<Value> = breakpoints
                .iter()
                .map(|line| json!({ "line": line }))
                .collect();
            self.request(
                "setBreakpoints",
                json!({ "source": { "path": program }, "breakpoints": lines }),
            );
        }
        self.request("configurationDone", Value::Null);
    }

    /// Runs to the end and returns the transcript and the program's output.
    fn finish(mut self) -> String {
        self.event("exited");
        self.event("terminated");
        // Output can come after the exit's events in the stream order the
        // test sees; it has all been read by the time `exited` is sent.
        while let Ok(message) = self.from_adapter.recv_timeout(Duration::from_millis(50)) {
            if message["event"] == "output" {
                let text = message["body"]["output"].as_str().unwrap_or_default();
                self.output.push_str(text);
            }
        }
        format!(
            "{}\n--- program output ---\n{}",
            self.transcript.join("\n"),
            self.output
        )
    }
}

fn snapshot(name: &str, transcript: &str) {
    insta::with_settings!({
        snapshot_path => "snapshots",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(name, transcript);
    });
}

const CALLS: &str = "\
fn square(x) {
    let y = x * x
    y
}

fn sum_squares(a, b) {
    let sa = square(a)
    let pair = (sa, [a, b])
    let sb = square(b)
    sa + sb
}

fn main() {
    println(\"start\")
    let total = sum_squares(3, 4)
    println(total)
}
";

#[test]
fn breakpoints_stack_variables_and_watches() {
    let scratch = Scratch::new("variables");
    let program = scratch.write("calls.flx", CALLS);
    let mut editor = Editor::start(&scratch);
    // Line 5 has no code, so its breakpoint moves down to `sum_squares`;
    // 99 is past the end.
    editor.launch(&program, json!({}), &[5, 9, 99]);
    editor.event("stopped");
    editor.request("threads", Value::Null);
    editor.request("stackTrace", json!({ "threadId": 1 }));
    editor.request("continue", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("scopes", json!({ "frameId": 3 }));
    editor.request("variables", json!({ "variablesReference": 1 }));
    // `pair` has parts: a tuple holding a list.
    editor.request("variables", json!({ "variablesReference": 2 }));
    editor.request("variables", json!({ "variablesReference": 3 }));
    editor.request(
        "evaluate",
        json!({ "expression": "pair.1", "frameId": 3, "context": "watch" }),
    );
    editor.request(
        "evaluate",
        json!({ "expression": "sa", "frameId": 3, "context": "hover" }),
    );
    editor.request(
        "evaluate",
        json!({ "expression": "nothing", "frameId": 3, "context": "watch" }),
    );
    editor.request("continue", json!({ "threadId": 1 }));
    snapshot("breakpoints_stack_variables_and_watches", &editor.finish());
}

#[test]
fn stepping_forward_and_back() {
    let scratch = Scratch::new("stepping");
    let program = scratch.write("calls.flx", CALLS);
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[7]);
    editor.event("stopped");
    editor.request("stepIn", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    editor.request("stepOut", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("next", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    // Step Back, then Reverse Continue to the breakpoint.
    editor.request("stepBack", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    editor.request("reverseContinue", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    // The Debug Console's commands.
    let console = |expression: &str| json!({ "expression": expression, "context": "repl" });
    editor.request("evaluate", console(".where"));
    editor.request("evaluate", console(".events"));
    editor.request("evaluate", console(".rf"));
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    editor.request("evaluate", console(".goto 7"));
    editor.event("stopped");
    editor.request("evaluate", console(".where"));
    editor.request("evaluate", console(".nonsense"));
    editor.request("continue", json!({ "threadId": 1 }));
    editor.event("stopped");
    editor.request("terminate", json!({}));
    snapshot("stepping_forward_and_back", &editor.finish());
}

#[test]
fn tail_calls_show_as_a_label_frame() {
    let scratch = Scratch::new("tail");
    let program = scratch.write(
        "tail.flx",
        "fn count(n, acc) {\n    if n == 0 { acc } else { count(n - 1, acc + 1) }\n}\n\nfn main() {\n    print(count(3, 0))\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[2]);
    for _ in 0..3 {
        editor.event("stopped");
        editor.request("continue", json!({ "threadId": 1 }));
    }
    editor.event("stopped");
    editor.request("stackTrace", json!({ "threadId": 1 }));
    editor.request("continue", json!({ "threadId": 1 }));
    snapshot("tail_calls_show_as_a_label_frame", &editor.finish());
}

#[test]
fn stop_on_entry_pause_and_terminate() {
    let scratch = Scratch::new("pause");
    let program = scratch.write(
        "spin.flx",
        "fn spin(n, acc) {\n    if n == 0 { acc } else { spin(n - 1, acc + 1) }\n}\n\nfn main() {\n    print(spin(100000000, 0))\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({ "stopOnEntry": true }), &[]);
    editor.event("stopped");
    editor.request("continue", json!({ "threadId": 1 }));
    std::thread::sleep(Duration::from_millis(300));
    editor.request("pause", json!({ "threadId": 1 }));
    let stopped = editor.event("stopped");
    assert_eq!(stopped["body"]["reason"], "pause");
    let stack = editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "spin");
    editor.request("terminate", json!({}));
    let transcript = editor.finish();
    // Where the pause lands depends on timing; the shape does not.
    let shape: Vec<String> = transcript
        .lines()
        .map(|line| {
            if line.contains("\"position ") {
                let start = line.find("\"position ").unwrap();
                let end = line[start + 1..].find('"').unwrap() + start + 2;
                format!("{}\"position N\"{}", &line[..start], &line[end..])
            } else if line.contains("\"stackFrames\"") {
                "<- stack: spin (line and column vary)".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    snapshot("stop_on_entry_pause_and_terminate", &shape.join("\n"));
}

#[test]
fn a_program_that_does_not_compile() {
    let scratch = Scratch::new("broken");
    let program = scratch.write("broken.flx", "fn main() {\n    let x =\n}\n");
    let mut editor = Editor::start(&scratch);
    editor.request("initialize", json!({ "adapterID": "flux" }));
    let args = editor.launch_args(&program, json!({}));
    let launched = editor.request("launch", args);
    assert_eq!(launched["success"], false);
    let transcript = editor.finish();
    assert!(transcript.contains("broken.flx"), "{transcript}");
    let (session, _) = transcript.split_once("\n--- program output ---").unwrap();
    snapshot("a_program_that_does_not_compile", session);
}

const FLIP: &str = "\
effect Flip {
    flip : () -> Bool
}

fn decide() -> Int with Flip {
    let b = perform Flip.flip()
    if b { 1 } else { 2 }
}

fn main() with IO {
    let result = decide() handle Flip(0) {
        flip(resume, n) -> resume(true, n + 1) + resume(false, n + 1)
    }
    print(to_string(result))
}
";

#[test]
fn a_handler_shows_the_frames_it_suspended_and_its_state() {
    let scratch = Scratch::new("effects");
    let program = scratch.write("flip.flx", FLIP);
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[12]);
    editor.event("stopped");
    let stack = editor.request("stackTrace", json!({ "threadId": 1 }));
    let top = stack["body"]["stackFrames"][0]["id"].clone();
    let scopes = editor.request("scopes", json!({ "frameId": top }));
    let handlers = scopes["body"]["scopes"]
        .as_array()
        .and_then(|scopes| scopes.iter().find(|scope| scope["name"] == "Handlers"))
        .map(|scope| scope["variablesReference"].clone())
        .expect("a Handlers scope");
    editor.request("variables", json!({ "variablesReference": handlers }));
    // A suspended frame's variables, read from the continuation.
    let suspended = stack["body"]["stackFrames"]
        .as_array()
        .and_then(|frames| {
            frames.iter().find(|frame| {
                frame["name"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("suspended")
            })
        })
        .map(|frame| frame["id"].clone())
        .expect("a suspended frame");
    editor.request("scopes", json!({ "frameId": suspended }));
    editor.request(
        "evaluate",
        json!({ "expression": ".handlers", "context": "repl" }),
    );
    editor.request("continue", json!({ "threadId": 1 }));
    snapshot(
        "a_handler_shows_the_frames_it_suspended_and_its_state",
        &editor.finish(),
    );
}

#[test]
fn a_panic_stops_as_an_exception() {
    let scratch = Scratch::new("panic");
    let program = scratch.write(
        "boom.flx",
        "fn check(n: Int) -> Int {\n    if n > 2 {\n        panic(\"too big\")\n    } else {\n        n\n    }\n}\n\nfn main() with IO {\n    let n = check(3)\n    print(to_string(n))\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    let initialized = editor.request("initialize", json!({ "adapterID": "flux" }));
    let filters: Vec<Value> = initialized["body"]["exceptionBreakpointFilters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(filters.len(), 5, "{initialized}");
    assert_eq!(initialized["body"]["supportsExceptionInfoRequest"], true);
    let args = editor.launch_args(&program, json!({}));
    editor.request("launch", args);
    editor.event("initialized");
    editor.request("setExceptionBreakpoints", json!({ "filters": ["panic"] }));
    editor.request("configurationDone", Value::Null);
    let stopped = editor.event("stopped");
    assert_eq!(stopped["body"]["reason"], "exception");
    editor.request("exceptionInfo", json!({ "threadId": 1 }));
    editor.request("stackTrace", json!({ "threadId": 1 }));
    editor.request("continue", json!({ "threadId": 1 }));
    let transcript = editor.finish();
    let (session, _) = transcript.split_once("\n--- program output ---").unwrap();
    snapshot("a_panic_stops_as_an_exception", session);
}

const BOTH: &str = "\
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(20)
    let m = 3 * 2
    m + 1
}

fn fast() -> Int with Async {
    let k = 4 + 1
    k * 10
}

fn body() -> String with Async {
    let label = \"pair\"
    let pair = both(slow, fast)
    label + to_string(pair.0) + \",\" + to_string(pair.1)
}

fn main() with IO {
    print(run_async(body))
}
";

#[test]
fn fibers_are_threads_with_their_own_stacks() {
    let scratch = Scratch::new("fibers");
    let program = scratch.write("both.flx", BOTH);
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[11]);
    let stopped = editor.event("stopped");
    assert_eq!(stopped["body"]["threadId"], 3, "{stopped}");
    editor.request("threads", Value::Null);
    editor.request("stackTrace", json!({ "threadId": 3 }));
    // A parked fiber's frames, and its variables, from its continuation.
    let parked = editor.request("stackTrace", json!({ "threadId": 1, "levels": 1 }));
    let body = parked["body"]["stackFrames"][0]["id"].clone();
    let scopes = editor.request("scopes", json!({ "frameId": body }));
    let locals = scopes["body"]["scopes"][0]["variablesReference"].clone();
    editor.request("variables", json!({ "variablesReference": locals }));
    editor.request("stackTrace", json!({ "threadId": 2 }));
    editor.request(
        "evaluate",
        json!({ "expression": ".threads", "context": "repl" }),
    );
    editor.request("continue", json!({ "threadId": 3 }));
    snapshot("fibers_are_threads_with_their_own_stacks", &editor.finish());
}

#[test]
fn pause_while_every_fiber_waits() {
    let scratch = Scratch::new("idle");
    let program = scratch.write(
        "idle.flx",
        "import Flow.Async exposing (..)\n\nfn nap() -> Int with Async {\n    sleep(1500)\n    1\n}\n\nfn main() with IO {\n    print(run_async(nap))\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[]);
    std::thread::sleep(Duration::from_millis(500));
    editor.request("pause", json!({ "threadId": 1 }));
    let stopped = editor.event("stopped");
    assert_eq!(stopped["body"]["reason"], "pause");
    let threads = editor.request("threads", Value::Null);
    assert_eq!(
        threads["body"]["threads"][0]["name"], "main — waiting sleep 1500 ms",
        "{threads}"
    );
    let stack = editor.request("stackTrace", json!({ "threadId": 1 }));
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "nap (suspended)");
    editor.request("continue", json!({ "threadId": 1 }));
    let transcript = editor.finish();
    assert!(transcript.ends_with("1\n"), "{transcript}");
}

/// The adapter's own `lldb-dap`, or `None` (with a note) on a machine
/// without one, or with no native backend in `flux`.
fn native_debugging() -> bool {
    if cfg!(windows) {
        return false;
    }
    if flux_dap::native::find_lldb_dap(None).is_err() {
        eprintln!("note: lldb-dap not found; skipping (set FLUX_LLDB_DAP)");
        return false;
    }
    true
}

/// Launches `program` natively and runs to the first stop at
/// `breakpoints`. `None` when this `flux` has no native backend.
fn launch_native(editor: &mut Editor, program: &Path, breakpoints: &[usize]) -> Option<Value> {
    let init = editor.request("initialize", json!({ "adapterID": "flux" }));
    assert_eq!(init["success"], true, "{init}");
    let args = editor.launch_args(program, json!({ "backend": "native" }));
    let launch = editor.send("launch", args);
    // lldb-dap may answer `launch` only after `configurationDone`: wait for
    // whichever comes first, its `initialized` or a failed launch.
    loop {
        let message = editor.receive();
        if message["type"] == "response"
            && message["request_seq"] == launch
            && message["success"] == true
        {
            // lldb-dap answered before its `initialized`, as older ones do.
            editor.pending.push_back(message);
            continue;
        }
        if message["type"] == "response" && message["request_seq"] == launch {
            assert!(
                editor.output.contains("require") && editor.output.contains("llvm"),
                "the native build failed:\n{message}\n{}",
                editor.output
            );
            eprintln!("note: this flux has no native backend; skipping");
            return None;
        }
        if message["type"] == "event" && message["event"] == "initialized" {
            break;
        }
        if message["type"] == "event" && message["event"] != "output" {
            editor.pending.push_back(message);
        }
    }
    let lines: Vec<Value> = breakpoints
        .iter()
        .map(|line| json!({ "line": line }))
        .collect();
    let set = editor.request(
        "setBreakpoints",
        json!({ "source": { "path": program }, "breakpoints": lines }),
    );
    assert_eq!(set["success"], true, "{set}");
    editor.request("configurationDone", json!({}));
    let launched = editor.response(launch);
    assert_eq!(launched["success"], true, "{launched}");
    Some(editor.event("stopped"))
}

fn frame_names(editor: &mut Editor, thread: &Value) -> Vec<(String, Value)> {
    let stack = editor.request("stackTrace", json!({ "threadId": thread }));
    stack["body"]["stackFrames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| {
            (
                frame["name"].as_str().unwrap_or_default().to_string(),
                frame.clone(),
            )
        })
        .collect()
}

fn values(editor: &mut Editor, reference: &Value) -> Vec<(String, String)> {
    let variables = editor.request("variables", json!({ "variablesReference": reference }));
    variables["body"]["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variable| {
            (
                variable["name"].as_str().unwrap_or_default().to_string(),
                variable["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[test]
fn native_breakpoints_stack_and_variables() {
    if !native_debugging() {
        return;
    }
    let scratch = Scratch::new("native");
    let program = scratch.write("calls.flx", CALLS);
    let mut editor = Editor::start(&scratch);
    let Some(stopped) = launch_native(&mut editor, &program, &[9]) else {
        return;
    };
    assert_eq!(stopped["body"]["reason"], "breakpoint", "{stopped}");
    // No Step Back: a native program does not record its run.
    let capabilities = editor
        .pending
        .iter()
        .find(|message| message["event"] == "capabilities")
        .cloned()
        .expect("a capabilities event");
    assert_eq!(
        capabilities["body"]["capabilities"]["supportsStepBack"], false,
        "{capabilities}"
    );
    let thread = stopped["body"]["threadId"].clone();

    // Only Flux frames, by their Flux names.
    let frames = frame_names(&mut editor, &thread);
    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["sum_squares", "main"], "{frames:?}");
    assert_eq!(frames[0].1["line"], 9);
    assert!(
        frames[0].1["source"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("calls.flx")),
        "{frames:?}"
    );

    // The frame's variables, as Flux values, without those not bound yet.
    let scopes = editor.request("scopes", json!({ "frameId": frames[0].1["id"] }));
    let scopes = scopes["body"]["scopes"].as_array().unwrap().clone();
    assert_eq!(scopes.len(), 1, "{scopes:?}");
    let locals = values(&mut editor, &scopes[0]["variablesReference"]);
    let shown = |name: &str| {
        locals
            .iter()
            .find(|(shown, _)| shown == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(shown("a").as_deref(), Some("3"), "{locals:?}");
    assert_eq!(shown("sa").as_deref(), Some("9"), "{locals:?}");
    // Native code frees `pair` after its last use, which was its binding.
    assert!(
        matches!(shown("pair").as_deref(), Some("(9, [3, 4])" | "<dropped>")),
        "{locals:?}"
    );
    assert_eq!(shown("sb"), None, "`sb` is not bound yet: {locals:?}");

    // The Debug Console's commands reach flux_lldb.py.
    let console = editor.request(
        "evaluate",
        json!({ "expression": ".locals", "context": "repl", "frameId": frames[0].1["id"] }),
    );
    let text = console["body"]["result"].as_str().unwrap_or_default();
    assert!(text.contains("sa = 9"), "{console}");
    let back = editor.request("stepBack", json!({ "threadId": thread }));
    assert_eq!(back["success"], false, "{back}");
    let reverse = editor.request(
        "evaluate",
        json!({ "expression": ".rs", "context": "repl" }),
    );
    assert_eq!(reverse["success"], false, "{reverse}");

    // Stepping moves through Flux lines.
    editor.request("next", json!({ "threadId": thread }));
    editor.event("stopped");
    let frames = frame_names(&mut editor, &thread);
    assert_eq!(frames[0].0, "sum_squares", "{frames:?}");
    assert_eq!(frames[0].1["line"], 10, "{frames:?}");

    editor.request("continue", json!({ "threadId": thread }));
    let transcript = editor.finish();
    assert!(transcript.contains("start"), "{transcript}");
    assert!(transcript.contains("25"), "{transcript}");
}

const FLIP_TWICE: &str = "\
effect Flip {
    flip : () -> Bool
}

fn decide() -> Int with Flip {
    let b = perform Flip.flip()
    if b { 1 } else { 2 }
}

fn twice() -> Int with Flip {
    let d = decide()
    d * 10
}

fn main() with IO {
    let result = twice() handle Flip {
        flip(resume) -> resume(true) + resume(false)
    }
    print(to_string(result))
}
";

#[test]
fn native_handler_arms_show_the_frames_they_suspended() {
    if !native_debugging() {
        return;
    }
    let scratch = Scratch::new("native-effects");
    let program = scratch.write("flip.flx", FLIP_TWICE);
    let mut editor = Editor::start(&scratch);
    let Some(stopped) = launch_native(&mut editor, &program, &[17]) else {
        return;
    };
    let thread = stopped["body"]["threadId"].clone();
    let capabilities = editor
        .pending
        .iter()
        .find(|message| message["event"] == "capabilities")
        .cloned()
        .expect("a capabilities event");
    let filters = &capabilities["body"]["capabilities"]["exceptionBreakpointFilters"];
    assert_eq!(filters[0]["filter"], "panic", "{capabilities}");

    let frames = frame_names(&mut editor, &thread);
    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "<handler Flip.flip>",
            "⟂ handling Flip.flip — performed at flip.flx:6:13",
            "decide (suspended)",
            "twice (suspended)",
            "main",
        ],
        "{frames:?}"
    );
    assert_eq!(frames[2].1["line"], 6, "{frames:?}");
    // A suspended frame has no variables natively.
    let scopes = editor.request("scopes", json!({ "frameId": frames[2].1["id"] }));
    assert_eq!(scopes["body"]["scopes"], json!([]), "{scopes}");

    // Into `resume`: the resumed frames, above the arm.
    editor.request("stepIn", json!({ "threadId": thread }));
    editor.event("stopped");
    let frames = frame_names(&mut editor, &thread);
    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["decide", "twice", "<handler Flip.flip>", "main"],
        "{frames:?}"
    );
    let handlers = editor.request(
        "evaluate",
        json!({ "expression": ".handlers", "context": "repl" }),
    );
    let text = handlers["body"]["result"].as_str().unwrap_or_default();
    assert!(text.contains("#0 Flip"), "{handlers}");

    editor.request("continue", json!({ "threadId": thread }));
    let transcript = editor.finish();
    assert!(transcript.contains("30"), "{transcript}");
}

#[test]
fn a_native_panic_stops_as_an_exception() {
    if !native_debugging() {
        return;
    }
    let scratch = Scratch::new("native-panic");
    let program = scratch.write(
        "boom.flx",
        "fn check(n: Int) -> Int {\n    if n > 2 {\n        panic(\"too big\")\n    } else {\n        n\n    }\n}\n\nfn main() with IO {\n    let n = check(3)\n    print(to_string(n))\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    let init = editor.request("initialize", json!({ "adapterID": "flux" }));
    assert_eq!(init["success"], true, "{init}");
    let args = editor.launch_args(&program, json!({ "backend": "native" }));
    let launch = editor.send("launch", args);
    loop {
        let message = editor.receive();
        if message["type"] == "response" && message["request_seq"] == launch {
            if message["success"] != true {
                eprintln!("note: this flux has no native backend; skipping");
                return;
            }
            editor.pending.push_back(message);
            continue;
        }
        if message["type"] == "event" && message["event"] == "initialized" {
            break;
        }
        if message["type"] == "event" && message["event"] != "output" {
            editor.pending.push_back(message);
        }
    }
    let set = editor.request("setExceptionBreakpoints", json!({ "filters": ["panic"] }));
    assert_eq!(set["success"], true, "{set}");
    editor.request("configurationDone", json!({}));
    let launched = editor.response(launch);
    assert_eq!(launched["success"], true, "{launched}");
    let stopped = editor.event("stopped");
    assert_eq!(stopped["body"]["reason"], "exception", "{stopped}");
    assert_eq!(stopped["body"]["text"], "too big", "{stopped}");
    let thread = stopped["body"]["threadId"].clone();
    let info = editor.request("exceptionInfo", json!({ "threadId": thread }));
    assert_eq!(info["body"]["exceptionId"], "panic", "{info}");
    let frames = frame_names(&mut editor, &thread);
    let names: Vec<&str> = frames.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["check", "main"], "{frames:?}");
    assert_eq!(frames[0].1["line"], 3, "{frames:?}");
    editor.request("continue", json!({ "threadId": thread }));
    let transcript = editor.finish();
    assert!(transcript.contains("\"exitCode\":1"), "{transcript}");
}

#[test]
fn a_native_program_that_does_not_build() {
    if !native_debugging() {
        return;
    }
    let scratch = Scratch::new("native-broken");
    let program = scratch.write("broken.flx", "fn main() {\n    print(nope)\n}\n");
    let mut editor = Editor::start(&scratch);
    editor.request("initialize", json!({ "adapterID": "flux" }));
    let args = editor.launch_args(&program, json!({ "backend": "native" }));
    let launched = editor.request("launch", args);
    assert_eq!(launched["success"], false, "{launched}");
    assert!(
        launched["message"]
            .as_str()
            .is_some_and(|text| text.contains("did not build")),
        "{launched}"
    );
    assert!(editor.output.contains("nope"), "{}", editor.output);
    editor.request("disconnect", json!({}));
}

#[test]
fn debugging_a_replay() {
    let scratch = Scratch::new("replay");
    let program = scratch.write(
        "env.flx",
        "import Flow.Env as Env\n\nfn main() with IO {\n    let name = Env.var_or(\"FLUX_DAP_FIXTURE\", \"nobody\")\n    print(\"hello \" + name)\n}\n",
    );
    let tape = scratch.dir.join("run.jsonl");
    let recorded = Command::new(flux_bin())
        .current_dir(&scratch.dir)
        .arg(&program)
        .arg("--no-cache")
        .arg("--record")
        .arg(&tape)
        .env("FLUX_DAP_FIXTURE", "recorded")
        .output()
        .unwrap();
    assert!(recorded.status.success(), "{recorded:?}");
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({ "replay": tape }), &[5]);
    editor.event("stopped");
    editor.request("scopes", json!({ "frameId": 2 }));
    editor.request("variables", json!({ "variablesReference": 1 }));
    editor.request("continue", json!({ "threadId": 1 }));
    snapshot("debugging_a_replay", &editor.finish());
}

#[test]
fn going_back_through_async_code_is_refused() {
    let scratch = Scratch::new("async");
    let program = scratch.write(
        "nap.flx",
        "import Flow.Async exposing (..)\n\nfn nap() -> Int with Async {\n    sleep(1)\n    7\n}\n\nfn main() with IO {\n    let after = run_async(nap)\n    print(after)\n}\n",
    );
    let mut editor = Editor::start(&scratch);
    editor.launch(&program, json!({}), &[10]);
    editor.event("stopped");
    editor.request("stepBack", json!({ "threadId": 1 }));
    editor.request("continue", json!({ "threadId": 1 }));
    snapshot("going_back_through_async_code_is_refused", &editor.finish());
}
