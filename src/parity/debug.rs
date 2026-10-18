//! Debug parity (proposal 0188 S6): the same scripted debugging session on
//! the VM debugger and on lldb, with normalized transcripts compared.
//!
//! A fixture scripts the session in its header:
//!
//! ```text
//! // dbg: break 9
//! // dbg: continue
//! // dbg: locals
//! // dbg: next
//! // dbg: bt
//! ```
//!
//! `vm_debug` runs the commands with `flux debug <file> --script`.
//! `llvm_debug` builds the program with `--native -g` and runs the same
//! commands in `lldb -b`, through `flux_lldb.py`'s `flux` command. Each
//! transcript is reduced to what both debuggers mean the same way:
//!
//! - `stop <function> <file>:<line>` for each stop, with `thread <n>` when
//!   it is in a fiber other than the root (S8), then `exception <kind>`
//!   when it is a runtime error the debugger caught;
//! - `thread <n> <name> [<state>] <file>:<line>` for each thread of
//!   `threads` (S8);
//! - `frame <function> <file>:<line>` for each Flux frame of `bt`, the
//!   frames an effect handler suspended among them (S7);
//! - `handler <effect>[ state = <value>]` for each handler of `handlers`;
//! - `<name> = <value>` for each variable of `locals` or part of `print`,
//!   each group sorted by name;
//! - `finished` when the program ends.
//!
//! Columns, stop reasons, positions, the VM's tail-call labels, the
//! `⟂ handling …` labels, the `(suspended)` and `[inlined]` marks and the
//! program's own output are left out. The way's "stdout" is that reduced
//! transcript, so the usual stdout comparison checks the two match.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{ExitKind, RunResult, Way};

/// The commands a fixture's `// dbg:` lines give, in order.
pub fn debug_script(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("// dbg:"))
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .collect()
}

/// Runs `file`'s debug script under `way` (`vm_debug` or `llvm_debug`).
pub fn run_debug_way(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> RunResult {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(err) => return failure(way, &format!("cannot read {}: {err}", file.display())),
    };
    let script = debug_script(&source);
    if script.is_empty() {
        return failure(way, "the fixture has no `// dbg:` commands");
    }
    let dir = work_dir(file, way);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return failure(way, &format!("cannot make {}: {err}", dir.display()));
    }
    let outcome = match way {
        Way::VmDebug => run_vm(vm_binary, file, &script, &dir, extra_args, timeout),
        Way::LlvmDebug => run_lldb(llvm_binary, file, &script, &dir, extra_args, timeout),
        other => Err(format!("`{other}` is not a debug way")),
    };
    let _ = std::fs::remove_dir_all(&dir);
    match outcome {
        Ok((transcript, raw)) => RunResult {
            way,
            exit_kind: ExitKind::Success,
            exit_code: 0,
            stdout: raw,
            stderr: String::new(),
            normalized_stdout: transcript,
            normalized_stderr: String::new(),
            cache_observations: vec![],
        },
        Err(message) => failure(way, &message),
    }
}

fn work_dir(file: &Path, way: Way) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fixture".to_string());
    PathBuf::from("target/parity-cache")
        .join(way.to_string())
        .join(stem)
}

fn failure(way: Way, message: &str) -> RunResult {
    RunResult {
        way,
        exit_kind: ExitKind::ToolFailure,
        exit_code: -1,
        stdout: String::new(),
        stderr: message.to_string(),
        normalized_stdout: String::new(),
        normalized_stderr: message.to_string(),
        cache_observations: vec![],
    }
}

/// What `run` returns for a failed tool: its output, for the report.
fn run(
    mut command: Command,
    timeout: Duration,
    what: &str,
) -> Result<(i32, String, String), String> {
    let mut child = command
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot run {what}: {err}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let read = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    };
    let stdout = read(Box::new(stdout));
    let stderr = read(Box::new(stderr));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{what} timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok((
        status.code().unwrap_or(-1),
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    ))
}

// ── The VM ───────────────────────────────────────────────────────────────

fn run_vm(
    binary: &Path,
    file: &Path,
    script: &[String],
    dir: &Path,
    extra_args: &[String],
    timeout: Duration,
) -> Result<(String, String), String> {
    let commands = dir.join("session.flxdbg");
    std::fs::write(&commands, script.join("\n") + "\n").map_err(|err| err.to_string())?;
    let mut command = Command::new(binary);
    command
        .arg("debug")
        .arg(file)
        .arg("--script")
        .arg(&commands)
        .arg("--no-cache")
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .args(extra_args);
    let (code, _stdout, stderr) = run(command, timeout, "flux debug")?;
    if code != 0 && !stderr.contains("program finished") {
        return Err(format!("flux debug failed ({code}):\n{stderr}"));
    }
    Ok((normalize_vm(&stderr), stderr))
}

/// The VM debugger's transcript (its stderr), reduced.
pub fn normalize_vm(transcript: &str) -> String {
    let mut out = Transcript::default();
    for line in transcript.lines() {
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("stopped: ") {
            // stopped: <reason> at <file>:<l>:<c> in <fn> ([thread <t>, ]position <n>)
            push_stop(&mut out, rest);
        } else if let Some(thread) = parse_thread(line) {
            out.push(thread);
        } else if let Some(handler) = parse_handler(line) {
            out.push(handler);
        } else if let Some(frame) = parse_frame(line) {
            out.push(frame);
        } else if line == "program finished" {
            out.push("finished".to_string());
        } else if line == "no variables" || line == "no handlers" {
            out.push(line.to_string());
        } else if let Some(value) = parse_value(line) {
            out.value(value);
        }
    }
    out.finish()
}

// ── lldb ─────────────────────────────────────────────────────────────────

/// The lldb to run: `FLUX_LLDB`, else one on PATH, else Xcode's.
pub fn find_lldb() -> Option<Vec<String>> {
    if let Some(path) = std::env::var_os("FLUX_LLDB") {
        return Some(vec![path.to_string_lossy().into_owned()]);
    }
    for name in [
        "lldb", "lldb-18", "lldb-19", "lldb-20", "lldb-21", "lldb-22",
    ] {
        let found = Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if found {
            return Some(vec![name.to_string()]);
        }
    }
    if cfg!(target_os = "macos") {
        let found = Command::new("xcrun")
            .args(["--find", "lldb"])
            .output()
            .is_ok_and(|output| output.status.success());
        if found {
            return Some(vec!["xcrun".to_string(), "lldb".to_string()]);
        }
    }
    None
}

/// How a `llvm_debug` run that cannot compare starts its message, which the
/// parity check reports as a skip; the reason follows.
pub const NATIVE_DEBUG_SKIPPED: &str = "native debug parity skipped";

fn run_lldb(
    binary: &Path,
    file: &Path,
    script: &[String],
    dir: &Path,
    extra_args: &[String],
    timeout: Duration,
) -> Result<(String, String), String> {
    // Windows builds carry CodeView for Visual Studio and cdb, which show
    // call stacks only (0188 S11). lldb reads it, but names frames by their
    // symbols, so its transcripts cannot match the VM's.
    if cfg!(windows) {
        return Err(format!(
            "{NATIVE_DEBUG_SKIPPED}: Windows debug builds are for Visual Studio and cdb"
        ));
    }
    let Some(lldb) = find_lldb() else {
        return Err(format!(
            "{NATIVE_DEBUG_SKIPPED}: lldb not found (set FLUX_LLDB)"
        ));
    };
    let dir = std::fs::canonicalize(dir).map_err(|err| err.to_string())?;
    let program = dir.join("app");
    let mut build = Command::new(binary);
    build
        .arg(file)
        .args(["--native", "-g", "--emit-binary", "-o"])
        .arg(&program)
        .arg("--no-cache")
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .args(extra_args);
    let (code, _, stderr) = run(build, timeout * 4, "the native build")?;
    if code != 0 {
        return Err(format!("llvm compilation failed:\n{stderr}"));
    }
    let lldb_script = dir.join("flux_lldb.py");
    std::fs::write(&lldb_script, crate::debuginfo::LLDB_SCRIPT).map_err(|err| err.to_string())?;
    let file_name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut commands = vec![
        "settings set target.load-script-from-symbol-file false".to_string(),
        format!("command script import \"{}\"", lldb_script.display()),
        // The runtime errors `flux debug` stops at unless told otherwise.
        "flux catch panic division unhandled fibers".to_string(),
    ];
    let mut started = false;
    for line in script {
        let (name, rest) = line
            .split_once(char::is_whitespace)
            .map(|(name, rest)| (name, rest.trim()))
            .unwrap_or((line.as_str(), ""));
        let command = match name {
            "break" | "b" => {
                let line = rest.rsplit(':').next().unwrap_or(rest);
                format!("breakpoint set --file {file_name} --line {line}")
            }
            "delete" | "d" => {
                let line = rest.rsplit(':').next().unwrap_or(rest);
                format!("breakpoint clear --file {file_name} --line {line}")
            }
            "continue" | "c" if !started => {
                started = true;
                // The program's own output stays out of the transcript.
                commands.push(format!(
                    "process launch -o \"{}\"",
                    dir.join("output.txt").display()
                ));
                WHERE.to_string()
            }
            "continue" | "c" => {
                commands.push("flux forget-steps".to_string());
                commands.push("process continue".to_string());
                WHERE.to_string()
            }
            // `flux_lldb.py` steps as `flux debug` does: once per line, and
            // following a perform to where its frame goes on.
            "step" | "s" => {
                commands.push("flux step".to_string());
                WHERE.to_string()
            }
            "next" | "n" => {
                commands.push("flux next".to_string());
                WHERE.to_string()
            }
            "finish" | "f" => {
                commands.push("flux finish".to_string());
                WHERE.to_string()
            }
            "bt" | "backtrace" => "flux bt".to_string(),
            "locals" => "flux locals".to_string(),
            "print" | "p" => format!("flux print {rest}"),
            "handlers" => "flux handlers".to_string(),
            "threads" => "flux threads".to_string(),
            "thread" => format!("flux thread {rest}"),
            "catch" => format!("flux catch {rest}"),
            other => return Err(format!("`// dbg: {other}` has no lldb equivalent")),
        };
        commands.push(command);
    }
    let mut command = Command::new(&lldb[0]);
    command.args(&lldb[1..]).args(["-b", "--no-lldbinit"]);
    for line in &commands {
        command.arg("-o").arg(line);
    }
    // One worker, as `flux debug` runs async code (S8).
    command
        .arg(&program)
        .current_dir(&dir)
        .env("FLUX_WORKERS", "1");
    let (_, stdout, stderr) = run(command, timeout * 2, "lldb")?;
    let transcript = format!("{stdout}{stderr}");
    if transcript.contains("script interpreter") {
        return Err(format!(
            "{NATIVE_DEBUG_SKIPPED}: this lldb runs no Python scripts"
        ));
    }
    Ok((normalize_lldb(&transcript), transcript))
}

/// Says where the program stopped, as `flux debug` says it, after each
/// command that runs it: lldb's own stop lines name the runtime's frame at
/// a caught error, and stop at every step of a script step that took
/// several.
const WHERE: &str = "flux where";

/// An lldb batch session's transcript, reduced.
pub fn normalize_lldb(transcript: &str) -> String {
    let mut out = Transcript::default();
    for line in transcript.lines() {
        let line = line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("(lldb)") {
            out.end_group();
            continue;
        }
        // `flux where`: stopping out of `main`, in the runtime, says no
        // stop.
        if let Some(rest) = line.strip_prefix("stopped: ") {
            push_stop(&mut out, rest);
            continue;
        }
        if line.starts_with("Process ") && line.contains(" exited with status") {
            out.push("finished".to_string());
        } else if let Some(thread) = parse_thread(line) {
            out.push(thread);
        } else if let Some(handler) = parse_handler(line) {
            out.push(handler);
        } else if let Some(frame) = parse_frame(line) {
            out.push(frame);
        } else if line == "no variables" || line == "no handlers" {
            out.push(line.to_string());
        } else if let Some(value) = parse_value(line) {
            // A value native code has freed: the VM has moved it out of its
            // slot by then, and does not show it either.
            if !line.ends_with(" = <dropped>") {
                out.value(value);
            }
        }
    }
    out.finish()
}

/// `<why> at <file>:<l>:<c> in <fn>[ (<notes>)]`, from either debugger, as
/// `stop`, with the thread a note names (`thread 3, position 19` on the
/// VM, `thread 3` natively), and, for a caught runtime error,
/// `exception <kind>`.
fn push_stop(out: &mut Transcript, rest: &str) {
    let (rest, notes) = match rest
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
    {
        Some((rest, notes)) if !notes.contains(" at ") => (rest, notes),
        _ => (rest, ""),
    };
    let thread = notes
        .split(", ")
        .find_map(|note| note.strip_prefix("thread "))
        .map(|thread| format!(" thread {thread}"))
        .unwrap_or_default();
    let Some((why_place, function)) = rest.rsplit_once(" in ") else {
        return;
    };
    let Some((why, place)) = why_place.rsplit_once(" at ") else {
        return;
    };
    out.push(format!("stop {}{thread}", place_of(function, place)));
    if let Some(error) = why.strip_prefix("exception (") {
        let kind = [
            "panic",
            "division by zero",
            "unhandled effect",
            "fiber failure",
            "runtime error",
        ]
        .into_iter()
        .find(|kind| error.starts_with(kind))
        .unwrap_or("runtime error");
        out.push(format!("exception {kind}"));
    }
}

// ── Comparing ────────────────────────────────────────────────────────────

/// Whether two ways' outputs match. For the two debug ways, the native
/// session must say what the VM's does, line for line, except that a group
/// of variables may hold more natively: native code keeps a value in its
/// slot after the VM has moved it out of its own, and the VM then no longer
/// shows it. Every variable the VM shows must be there with its value.
pub fn outputs_match(left: &RunResult, right: &RunResult) -> bool {
    let (vm, native) = match (left.way, right.way) {
        (Way::VmDebug, Way::LlvmDebug) => (left, right),
        (Way::LlvmDebug, Way::VmDebug) => (right, left),
        _ => return left.normalized_stdout == right.normalized_stdout,
    };
    let vm = items(&vm.normalized_stdout);
    let native = items(&native.normalized_stdout);
    let mut native = native.iter().peekable();
    for vm in &vm {
        // A handler arm whose `resume` is its last call waits natively
        // under the continuation that call runs; on the VM the tail resume
        // has replaced it, and a tail-resumptive arm has already returned.
        while let Some(Item::Line(line)) = native.peek()
            && line.starts_with("frame <handler ")
            && !matches!(vm, Item::Line(vm) if vm == line)
        {
            native.next();
        }
        let Some(native) = native.next() else {
            return false;
        };
        let same = match (vm, native) {
            (Item::Line(vm), Item::Line(native)) => vm == native,
            (Item::Values(vm), Item::Values(native)) => {
                vm.iter().all(|value| native.contains(value))
            }
            _ => false,
        };
        if !same {
            return false;
        }
    }
    native.next().is_none()
}

/// A reduced transcript, as lines and groups of values (each a variable
/// with its parts).
#[derive(Debug, PartialEq)]
enum Item {
    Line(String),
    Values(Vec<Vec<String>>),
}

fn items(transcript: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for line in transcript.lines() {
        let is_part = line.starts_with("  .");
        let is_value = !is_part
            && line.contains(" = ")
            && !line.starts_with("stop ")
            && !line.starts_with("frame ");
        if line == "no variables" {
            items.push(Item::Values(Vec::new()));
        } else if is_part {
            if let Some(Item::Values(values)) = items.last_mut()
                && let Some(value) = values.last_mut()
            {
                value.push(line.to_string());
            }
        } else if is_value {
            match items.last_mut() {
                Some(Item::Values(values)) => values.push(vec![line.to_string()]),
                _ => items.push(Item::Values(vec![vec![line.to_string()]])),
            }
        } else {
            items.push(Item::Line(line.to_string()));
        }
    }
    items
}

// ── Shared parsing ───────────────────────────────────────────────────────

/// `app.flx:12:5` → `app.flx:12`.
fn file_line(place: &str) -> String {
    let mut parts = place.trim().rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let middle = parts.next();
    let first = parts.next();
    match (first, middle) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (None, Some(file)) => format!("{file}:{last}"),
        _ => place.trim().to_string(),
    }
}

/// `#0 square at app.flx:3:13` → `frame square app.flx:3`. The VM's
/// `#N <entry>` and tail-call labels are not frames.
fn parse_frame(line: &str) -> Option<String> {
    let rest = line.strip_prefix('#')?;
    let (number, rest) = rest.split_once(' ')?;
    number.parse::<usize>().ok()?;
    // `   (suspended)` and `   (suspended, not resumable)`: native code
    // cannot tell whether a handler will resume. `   [inlined]`: the VM
    // inlines nothing, and native code's lambda is the VM's frame.
    let rest = rest
        .split_once("   (suspended")
        .map_or(rest, |(rest, _)| rest);
    let rest = rest
        .split_once("   [inlined]")
        .map_or(rest, |(rest, _)| rest);
    let (function, place) = rest.rsplit_once(" at ")?;
    Some(format!("frame {}", place_of(function, place)))
}

/// `<function> <file>:<line>`, or `<library>` for a place in the Flux
/// library (`Async.flx`, `List.flx`): the backends name and inline the
/// library's functions differently, so a stop or frame there, such as
/// `sleep` or `fail` a fiber is in, reads the same on both only as that.
/// A fixture's own file is named in lower case.
fn place_of(function: &str, place: &str) -> String {
    let file = file_line(place);
    if is_library_file(&file) {
        "<library>".to_string()
    } else {
        format!("{function} {file}")
    }
}

fn is_library_file(file_line: &str) -> bool {
    file_line.chars().next().is_some_and(char::is_uppercase)
}

/// `* 3 fast [stopped] at app.flx:14:5` → `thread 3 fast [stopped]
/// app.flx:14`, from `threads` (S8).
fn parse_thread(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("* ")
        .or_else(|| line.strip_prefix("  "))?;
    let (number, rest) = rest.split_once(' ')?;
    number.parse::<u64>().ok()?;
    let (name, rest) = rest.split_once(" [")?;
    let (state, rest) = rest.split_once(']')?;
    let place = rest
        .strip_prefix(" at ")
        .map(|place| {
            let file = file_line(place);
            if is_library_file(&file) {
                " <library>".to_string()
            } else {
                format!(" {file}")
            }
        })
        .unwrap_or_default();
    Some(format!("thread {number} {name} [{state}]{place}"))
}

/// `#0 Counter state = 1 — handled in main` → `handler Counter state = 1`:
/// native code does not know where a handler was installed.
fn parse_handler(line: &str) -> Option<String> {
    let rest = line.strip_prefix('#')?;
    let (number, rest) = rest.split_once(' ')?;
    number.parse::<usize>().ok()?;
    let rest = rest.split_once(" — ").map_or(rest, |(rest, _)| rest);
    let effect = rest.split(' ').next()?;
    let is_handler = !rest.contains(" at ")
        && effect.chars().next().is_some_and(char::is_uppercase)
        && effect
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && (rest == effect || rest.starts_with(&format!("{effect} state = ")));
    is_handler.then(|| format!("handler {rest}"))
}

/// A variable, `name = value`, or a part of the one before,
/// `  .0 = value`.
enum Value {
    Variable(String),
    Part(String),
}

fn parse_value(line: &str) -> Option<Value> {
    let (name, value) = line.split_once(" = ")?;
    let value = value.trim();
    if let Some(part) = name.strip_prefix("  .") {
        let valid = !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_');
        return valid.then(|| Value::Part(format!("  .{part} = {value}")));
    }
    let valid = !name.is_empty()
        && !name.starts_with(char::is_whitespace)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '\'');
    valid.then(|| Value::Variable(format!("{name} = {value}")))
}

/// A reduced transcript: lines, with each run of variables sorted by name,
/// each keeping its parts after it.
#[derive(Default)]
struct Transcript {
    lines: Vec<String>,
    values: Vec<(String, Vec<String>)>,
}

impl Transcript {
    fn push(&mut self, line: String) {
        self.end_group();
        // Library frames, one after another, read as one (see `place`); and
        // one exit, which `flux_lldb.py` reports after a step that ran off
        // the end and lldb may report again.
        if (line == "frame <library>" || line == "finished") && self.lines.last() == Some(&line) {
            return;
        }
        self.lines.push(line);
    }

    fn value(&mut self, value: Value) {
        match value {
            Value::Variable(line) => self.values.push((line, Vec::new())),
            Value::Part(line) => match self.values.last_mut() {
                Some((_, parts)) => parts.push(line),
                None => self.lines.push(line),
            },
        }
    }

    fn end_group(&mut self) {
        self.values.sort_by(|a, b| a.0.cmp(&b.0));
        for (line, parts) in self.values.drain(..) {
            self.lines.push(line);
            self.lines.extend(parts);
        }
    }

    fn finish(mut self) -> String {
        self.end_group();
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixture_scripts_its_session_in_dbg_lines() {
        let source = "// dbg: break 3\n// dbg: continue\n//dbg: no\n// dbg:\nfn main() {}\n";
        assert_eq!(debug_script(source), ["break 3", "continue"]);
    }

    #[test]
    fn an_inlined_lambda_is_the_same_frame_on_both() {
        assert_eq!(
            parse_frame("#0 <lambda app.flx:2:14> at app.flx:4:9   [inlined]"),
            parse_frame("#0 <lambda app.flx:2:14> at app.flx:4:9"),
        );
        assert_eq!(
            parse_frame("#0 <lambda app.flx:2:14> at app.flx:4:9   [inlined]").as_deref(),
            Some("frame <lambda app.flx:2:14> app.flx:4")
        );
    }

    #[test]
    fn vm_and_lldb_transcripts_reduce_to_the_same_lines() {
        let vm = "\
> break 9
breakpoint at calls.flx:9
> continue
stopped: breakpoint at calls.flx:9:5 in sum_squares (position 7)
     9 |     let sb = square(b)
> locals
b = 4
a = 3
sa = 9 (captured)
> bt
#0 sum_squares at calls.flx:9:5
   … 1 call elided by tail calls (last: go at calls.flx:2:3)
#1 main at calls.flx:15:17
#2 <entry>
> print xs
xs = [1, 2]
  .0 = 1
  .1 = 2
program finished
";
        let lldb = "\
(lldb) breakpoint set --file calls.flx --line 9
Breakpoint 1: where = app`sum_squares + 40 at calls.flx:9:14, address = 0x1
(lldb) process launch -o \"out.txt\"
Process 42 launched: '/tmp/app' (arm64)
Process 42 stopped
* thread #1, queue = 'com.apple.main-thread', stop reason = breakpoint 1.1
    frame #0: 0x0000000100000dcc app`sum_squares(a=3, b=4) at calls.flx:9:14
   8   \t    let sa = square(a)
-> 9   \t    let sb = square(b)
(lldb) flux where
stopped: breakpoint at calls.flx:9:14 in sum_squares
(lldb) flux locals
a = 3
b = 4
sa = 9 (captured)
(lldb) flux bt
#0 sum_squares at calls.flx:9:14
#1 main at calls.flx:15:17
(lldb) flux print xs
xs = [1, 2]
  .0 = 1
  .1 = 2
(lldb) process continue
Process 42 resuming
Process 42 exited with status = 0 (0x00000000)
(lldb) flux where
not stopped in Flux code
";
        let expected = "\
stop sum_squares calls.flx:9
a = 3
b = 4
sa = 9 (captured)
frame sum_squares calls.flx:9
frame main calls.flx:15
xs = [1, 2]
  .0 = 1
  .1 = 2
finished";
        assert_eq!(normalize_vm(vm), expected);
        assert_eq!(normalize_lldb(lldb), expected);
    }

    fn run(way: Way, stdout: &str) -> RunResult {
        RunResult {
            way,
            exit_kind: ExitKind::Success,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            normalized_stdout: stdout.to_string(),
            normalized_stderr: String::new(),
            cache_observations: vec![],
        }
    }

    #[test]
    fn native_may_still_show_what_the_vm_moved_but_nothing_else_differs() {
        let vm = run(Way::VmDebug, "stop area a.flx:3\nh = 4\nw = 3\nfinished");
        let more = run(
            Way::LlvmDebug,
            "stop area a.flx:3\nh = 4\nshape = Rect(3, 4)\nw = 3\nfinished",
        );
        assert!(outputs_match(&vm, &more));
        assert!(outputs_match(&more, &vm));
        let other_value = run(Way::LlvmDebug, "stop area a.flx:3\nh = 5\nw = 3\nfinished");
        assert!(!outputs_match(&vm, &other_value));
        let missing = run(Way::LlvmDebug, "stop area a.flx:3\nh = 4\nfinished");
        assert!(!outputs_match(&vm, &missing));
        let elsewhere = run(Way::LlvmDebug, "stop area a.flx:4\nh = 4\nw = 3\nfinished");
        assert!(!outputs_match(&vm, &elsewhere));
    }
}
