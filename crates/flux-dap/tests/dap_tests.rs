//! DAP sessions end to end (proposal 0188 S4).
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

#[test]
fn the_native_backend_is_refused_for_now() {
    let scratch = Scratch::new("native");
    let program = scratch.write("calls.flx", CALLS);
    let mut editor = Editor::start(&scratch);
    editor.request("initialize", json!({ "adapterID": "flux" }));
    let args = editor.launch_args(&program, json!({ "backend": "native" }));
    editor.request("launch", args);
    editor.request("disconnect", json!({}));
    snapshot(
        "the_native_backend_is_refused_for_now",
        &editor.transcript.join("\n"),
    );
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
