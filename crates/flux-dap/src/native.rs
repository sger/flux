//! Native debugging (proposal 0188 S6): the program built with `-g` and run
//! under `lldb-dap`, which this adapter sits in front of.
//!
//! `flux-dap` builds the program with `--native -g --emit-binary`, writes
//! `flux_lldb.py` next to it, and starts `lldb-dap`. From then on the
//! session passes DAP messages between the editor and `lldb-dap`, renumbering
//! them, and changes only what makes the views Flux's: the stack shows Flux
//! frames, values print through `flux_lldb.py`, and the Debug Console's
//! `.`-commands reach the script's `flux` command.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::engine::{LaunchSpec, is_progress_line};
use crate::server::Input;
use crate::transport::{read_message, write_message};

/// A running `lldb-dap`, and the files made for it.
pub struct NativeEngine {
    child: Child,
    stdin: ChildStdin,
    next_seq: i64,
    /// Where the binary and the script are; removed at the end.
    workdir: PathBuf,
}

/// The binary built for a native session, and the directory it is in.
pub struct NativeBuild {
    pub binary: PathBuf,
    pub script: PathBuf,
    pub workdir: PathBuf,
}

/// Builds `spec.program` for the debugger: `--native -g`, into a directory
/// of its own. The build's messages go to `output` as they come. `Err`
/// carries what to tell the editor; the build's own errors have then gone
/// to `output`.
pub fn build(
    spec: &LaunchSpec,
    mut output: impl FnMut(&'static str, &str),
) -> Result<NativeBuild, String> {
    let workdir = std::env::temp_dir().join(format!(
        "flux-dap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&workdir)
        .map_err(|err| format!("cannot make a directory for the build: {err}"))?;
    let stem = spec
        .program
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "program".to_string());
    let binary = workdir.join(&stem);

    let (program, base) = spec
        .flux_command
        .split_first()
        .ok_or_else(|| "`fluxCommand` is empty".to_string())?;
    let mut command = Command::new(program);
    command
        .args(base)
        .arg(&spec.program)
        .args(["--native", "-g", "--emit-binary", "-o"])
        .arg(&binary)
        .args(&spec.flux_args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    output(
        "console",
        &format!(
            "Building {} for native debugging…\n",
            spec.program.display()
        ),
    );
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot start `{program}`: {err}"))?;
    let stderr = child.stderr.take().expect("piped stderr");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
        if !is_progress_line(&line) {
            output("stderr", &line);
        }
        line.clear();
    }
    let status = child
        .wait()
        .map_err(|err| format!("the build did not finish: {err}"))?;
    if !status.success() || !binary.exists() {
        let _ = std::fs::remove_dir_all(&workdir);
        return Err(
            "the program did not build for native debugging; see the Debug Console".to_string(),
        );
    }
    let script = workdir.join("flux_lldb.py");
    std::fs::write(&script, flux::debuginfo::LLDB_SCRIPT)
        .map_err(|err| format!("cannot write flux_lldb.py: {err}"))?;
    Ok(NativeBuild {
        binary,
        script,
        workdir,
    })
}

/// The `lldb-dap` to run: the launch configuration's `lldbDap`, else
/// `FLUX_LLDB_DAP`, else one on PATH, else (on macOS) Xcode's.
pub fn find_lldb_dap(configured: Option<&str>) -> Result<Vec<String>, String> {
    if let Some(path) = configured.filter(|path| !path.is_empty()) {
        return Ok(vec![path.to_string()]);
    }
    if let Some(path) = std::env::var_os("FLUX_LLDB_DAP") {
        return Ok(vec![path.to_string_lossy().into_owned()]);
    }
    let names = [
        "lldb-dap",
        "lldb-dap-22",
        "lldb-dap-21",
        "lldb-dap-20",
        "lldb-dap-19",
        "lldb-dap-18",
    ];
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(vec![candidate.to_string_lossy().into_owned()]);
                }
            }
        }
    }
    if cfg!(target_os = "macos") {
        let found = Command::new("xcrun")
            .args(["--find", "lldb-dap"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|path| !path.is_empty());
        if let Some(path) = found {
            return Ok(vec![path]);
        }
    }
    Err(
        "cannot find lldb-dap, which native debugging runs on: install LLVM's lldb \
         (or Xcode on macOS), or set `lldbDap` in the launch configuration or \
         `flux.lldbDapPath` in the settings"
            .to_string(),
    )
}

impl NativeEngine {
    /// Starts `lldb-dap`; what it says arrives as [`Input::Native`], and its
    /// end as [`Input::NativeEnded`].
    pub fn start(
        command: &[String],
        workdir: PathBuf,
        inputs: Sender<Input>,
    ) -> Result<Self, String> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| "the lldb-dap command is empty".to_string())?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("cannot start `{program}`: {err}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let messages = inputs.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                if messages.send(Input::Native(message)).is_err() {
                    return;
                }
            }
            let _ = messages.send(Input::NativeEnded);
        });
        // lldb-dap's own complaints, for the Debug Console.
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else {
                    break;
                };
                let sent = inputs.send(Input::Output {
                    category: "console",
                    text: format!("lldb-dap: {line}\n"),
                });
                if sent.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            next_seq: 0,
            workdir,
        })
    }

    /// Sends `message` with a sequence number of `lldb-dap`'s, and returns it.
    pub fn send(&mut self, mut message: Value) -> i64 {
        self.next_seq += 1;
        message["seq"] = json!(self.next_seq);
        // An lldb-dap that has gone is reported by its end, not here.
        let _ = write_message(&mut self.stdin, &message).and_then(|()| self.stdin.flush());
        self.next_seq
    }

    /// Sends a request of the adapter's own.
    pub fn request(&mut self, command: &str, arguments: Value) -> i64 {
        self.send(json!({ "type": "request", "command": command, "arguments": arguments }))
    }

    /// Ends `lldb-dap`, and so the program it runs.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for NativeEngine {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

/// The arguments of `lldb-dap`'s `launch` for `build`, from the editor's.
pub fn launch_arguments(spec: &LaunchSpec, build: &NativeBuild, editor: &Value) -> Value {
    let mut env = Vec::new();
    if let Some(tape) = &spec.record {
        env.push(format!(
            "FLUX_TAPE_RECORD={}",
            absolute(spec, tape).display()
        ));
    }
    if let Some(tape) = &spec.replay {
        env.push(format!(
            "FLUX_TAPE_REPLAY={}",
            absolute(spec, tape).display()
        ));
    }
    let mut arguments = json!({
        "program": build.binary,
        "args": spec.args,
        "stopOnEntry": editor["stopOnEntry"].as_bool().unwrap_or(false),
        "initCommands": [
            // The dSYM carries the script too; this session imports it.
            "settings set target.load-script-from-symbol-file false",
            format!("command script import \"{}\"", build.script.display()),
        ],
    });
    if let Some(cwd) = &spec.cwd {
        arguments["cwd"] = json!(cwd);
    }
    if !env.is_empty() {
        arguments["env"] = json!(env);
    }
    arguments
}

fn absolute(spec: &LaunchSpec, path: &Path) -> PathBuf {
    match &spec.cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

/// `lldb-dap`'s stack, as a Flux stack: frames in `.flx` source, unless
/// the session shows compiler internals, in which case the rest stay,
/// de-emphasised.
pub fn flux_stack(mut body: Value, show_internals: bool) -> Value {
    let Some(frames) = body["stackFrames"].as_array_mut() else {
        return body;
    };
    let is_flux = |frame: &Value| {
        frame["source"]["path"]
            .as_str()
            .or_else(|| frame["source"]["name"].as_str())
            .is_some_and(|path| path.ends_with(".flx"))
            && frame["line"].as_i64().unwrap_or(0) > 0
    };
    if show_internals {
        for frame in frames.iter_mut() {
            if !is_flux(frame) {
                frame["presentationHint"] = json!("subtle");
            }
        }
    } else {
        frames.retain(is_flux);
        let shown = frames.len();
        if let Some(total) = body.get_mut("totalFrames") {
            *total = json!(shown);
        }
    }
    body
}

/// `lldb-dap`'s scopes, as a Flux frame has them: its variables, without
/// lldb's globals and registers unless the session shows internals.
pub fn flux_scopes(mut body: Value, show_internals: bool) -> Value {
    if !show_internals && let Some(scopes) = body["scopes"].as_array_mut() {
        scopes.retain(|scope| {
            scope["presentationHint"] == "locals"
                || scope["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("Local"))
        });
    }
    body
}

/// Variables as Flux shows them: without those not bound yet, which
/// `flux_lldb.py` marks `<unbound>`, so a frame shows what is in scope by
/// now, as on the VM; and with only the Flux value, where lldb-dap would show
/// the raw word before it.
pub fn flux_variables(mut body: Value) -> Value {
    if let Some(variables) = body["variables"].as_array_mut() {
        for variable in variables.iter_mut() {
            flux_value(variable, "value");
        }
        variables.retain(|variable| variable["value"] != "<unbound>");
    }
    body
}

/// An `evaluate` answer, with only the Flux value.
pub fn flux_evaluation(mut body: Value) -> Value {
    flux_value(&mut body, "result");
    body
}

/// lldb-dap shows a value with a summary as `<raw word> <summary>`. For the
/// Flux typedefs the word means nothing to the reader: keep the summary.
fn flux_value(item: &mut Value, field: &str) {
    let is_flux = item["type"]
        .as_str()
        .is_some_and(|ty| ty.starts_with("Flux"));
    if !is_flux {
        return;
    }
    let Some(text) = item[field].as_str() else {
        return;
    };
    if let Some((word, summary)) = text.split_once(' ')
        && (word.parse::<i64>().is_ok() || word.parse::<u64>().is_ok() || word.starts_with("0x"))
    {
        item[field] = json!(summary);
    }
}

/// Source paths as lldb knows them and as the editor wrote them.
///
/// The debug information names each file by its real path, and lldb matches
/// breakpoints on it: one set through a symlink (`/tmp` is `/private/tmp` on
/// macOS) would never resolve. So paths go to `lldb-dap` resolved, and come
/// back in the editor's spelling.
#[derive(Default)]
pub struct SourcePaths {
    to_editor: std::collections::HashMap<String, String>,
}

impl SourcePaths {
    /// Resolves each `path` in the editor's `value`, remembering the
    /// editor's spelling.
    pub fn to_lldb(&mut self, value: &mut Value) {
        self.walk(value, &mut |paths, path| {
            let resolved = std::fs::canonicalize(path).ok()?;
            let resolved = resolved.to_string_lossy().into_owned();
            if resolved == *path {
                return None;
            }
            paths.to_editor.insert(resolved.clone(), path.clone());
            Some(resolved)
        });
    }

    /// Puts back the editor's spelling of each `path` in `lldb-dap`'s `value`.
    pub fn to_editor(&mut self, value: &mut Value) {
        if self.to_editor.is_empty() {
            return;
        }
        self.walk(value, &mut |paths, path| paths.to_editor.get(path).cloned());
    }

    fn walk(
        &mut self,
        value: &mut Value,
        change: &mut impl FnMut(&mut Self, &String) -> Option<String>,
    ) {
        match value {
            Value::Object(object) => {
                for (key, item) in object.iter_mut() {
                    if key == "path"
                        && let Value::String(path) = item
                    {
                        if let Some(changed) = change(self, path) {
                            *path = changed;
                        }
                    } else {
                        self.walk(item, change);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, change);
                }
            }
            _ => {}
        }
    }
}

/// What a Debug Console `.`-command becomes for `lldb-dap`: an lldb command
/// (run through the console's escape prefix), or an answer of its own.
pub enum ConsoleCommand {
    Lldb(String),
    Answer(Result<String, String>),
}

pub fn console_command(command: &str) -> ConsoleCommand {
    let (name, rest) = command
        .split_once(char::is_whitespace)
        .map(|(name, rest)| (name, rest.trim()))
        .unwrap_or((command, ""));
    match name {
        "bt" => ConsoleCommand::Lldb("flux bt".to_string()),
        "locals" => ConsoleCommand::Lldb("flux locals".to_string()),
        "print" | "p" => ConsoleCommand::Lldb(format!("flux print {rest}")),
        "help" => ConsoleCommand::Answer(Ok(NATIVE_HELP.to_string())),
        "where" | "goto" | "events" | "event" | "rs" | "rn" | "rf" | "rc" | "internals" => {
            ConsoleCommand::Answer(Err(format!(
                "`.{name}` needs the VM backend: native programs do not record their run"
            )))
        }
        other => ConsoleCommand::Answer(Err(format!("unknown command `.{other}`; try `.help`"))),
    }
}

const NATIVE_HELP: &str = "\
A variable or a part of one (`xs.1`) shows its value. Commands:
  .bt              the Flux frames
  .locals          the current frame's variables
  .print <name>    a variable and its parts
Commands starting with ` go to lldb itself: `bt all";
