//! `flux run --record` / `--replay` end to end (proposal 0188 S1a and S1b).
//!
//! Each test copies a fixture from `tests/replay/fixtures/` into a scratch
//! directory and runs it there, so the files a fixture reads and writes are
//! its own. A replay must print what the recorded run printed, without
//! reading or changing anything outside the program, on either backend.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

#[derive(Clone, Copy, Debug)]
enum Backend {
    Vm,
    #[cfg(feature = "llvm")]
    Native,
}

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

/// A fixture copied into its own scratch directory as `app.flx`.
struct Case {
    scratch: Scratch,
    program: PathBuf,
}

impl Case {
    fn new(fixture: &str) -> Case {
        let scratch = Scratch::new(&format!("replay-{fixture}"));
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/replay/fixtures")
            .join(format!("{fixture}.flx"));
        let text = std::fs::read_to_string(source)
            .unwrap()
            .replace("PORT", &free_port().to_string());
        let program = scratch.write("app.flx", &text);
        Case { scratch, program }
    }

    fn tape(&self, name: &str) -> PathBuf {
        self.scratch.join(name)
    }

    fn run(&self, backend: Backend, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Run {
        let mut command = Command::new(env!("CARGO_BIN_EXE_flux"));
        command
            .current_dir(self.scratch.path())
            .arg(&self.program)
            .args(self.scratch.cache_args())
            .args(args)
            .env("NO_COLOR", "1")
            .env("FLUX_PROC_HELPER", env!("CARGO_BIN_EXE_flux_proc_helper"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(feature = "llvm")]
        if matches!(backend, Backend::Native) {
            command.arg("--native");
        }
        let _ = backend;
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("run flux");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
        }
    }

    fn record(&self, backend: Backend, tape: &Path, stdin: &str, env: &[(&str, &str)]) -> Run {
        self.run(backend, &["--record", tape.to_str().unwrap()], stdin, env)
    }

    fn replay(&self, backend: Backend, tape: &Path, env: &[(&str, &str)]) -> Run {
        self.run(backend, &["--replay", tape.to_str().unwrap()], "", env)
    }
}

/// A local port nothing listens on right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The fixtures whose inputs a test sets up before recording.
fn prepare_inputs(case: &Case) {
    std::fs::create_dir_all(case.scratch.join("data")).unwrap();
    std::fs::write(case.scratch.join("data/in.txt"), "alpha\nbeta\n").unwrap();
}

const STDIN: &str = "piped input";
const ENV: &[(&str, &str)] = &[("REPLAY_FIXTURE_VAR", "set when recorded")];

/// Records `fixture` on `from`, replays it on `to`, and checks that the
/// replay printed and exited exactly as the recording did.
fn assert_replays(fixture: &str, from: Backend, to: Backend) {
    let case = Case::new(fixture);
    prepare_inputs(&case);
    let tape = case.tape("run.jsonl");
    let recorded = case.record(from, &tape, STDIN, ENV);
    assert!(
        recorded.code == 0 || fixture == "panic",
        "{fixture}: recording on {from:?} failed:\n{}",
        recorded.stderr
    );
    let replayed = case.replay(to, &tape, &[]);
    assert_eq!(
        replayed.stdout, recorded.stdout,
        "{fixture}: {from:?} → {to:?} replay printed something else.\nstderr:\n{}",
        replayed.stderr
    );
    assert_eq!(
        replayed.code, recorded.code,
        "{fixture}: {from:?} → {to:?} replay exited differently.\nstderr:\n{}",
        replayed.stderr
    );
    assert!(
        !replayed.stderr.contains("error[E102"),
        "{fixture}: {from:?} → {to:?} replay reported a tape error:\n{}",
        replayed.stderr
    );
}

const FIXTURES: &[&str] = &[
    "console",
    "fs_reads",
    "fs_writes",
    "env",
    "proc_run",
    "stdin",
    "clock",
    "panic",
];

#[test]
fn each_fixture_replays_on_the_vm() {
    for fixture in FIXTURES.iter().chain(&["trace"]) {
        assert_replays(fixture, Backend::Vm, Backend::Vm);
    }
}

#[test]
fn replay_reads_nothing_from_outside_the_program() {
    // Files: the recorded inputs are gone.
    let case = Case::new("fs_reads");
    prepare_inputs(&case);
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", &[]);
    std::fs::remove_dir_all(case.scratch.join("data")).unwrap();
    assert_eq!(case.replay(Backend::Vm, &tape, &[]).stdout, recorded.stdout);

    // Environment and stdin: different now, recorded values come back.
    let case = Case::new("env");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", ENV);
    let replayed = case.replay(
        Backend::Vm,
        &tape,
        &[
            ("REPLAY_FIXTURE_VAR", "different now"),
            ("REPLAY_FIXTURE_ABSENT", "x"),
        ],
    );
    assert_eq!(replayed.stdout, recorded.stdout);
    assert!(
        replayed.stdout.contains("set when recorded"),
        "{}",
        replayed.stdout
    );

    let case = Case::new("stdin");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, STDIN, &[]);
    assert_eq!(case.replay(Backend::Vm, &tape, &[]).stdout, recorded.stdout);

    // Processes: the program it ran no longer exists.
    let case = Case::new("proc_run");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", &[]);
    let replayed = case.replay(
        Backend::Vm,
        &tape,
        &[("FLUX_PROC_HELPER", "/nonexistent/helper")],
    );
    assert_eq!(replayed.stdout, recorded.stdout);
    assert!(
        replayed.stdout.contains("from the helper"),
        "{}",
        replayed.stdout
    );
}

#[test]
fn replay_changes_nothing_outside_the_program() {
    let case = Case::new("fs_writes");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", &[]);
    assert!(case.scratch.join("out/a/g.txt").exists());
    std::fs::remove_dir_all(case.scratch.join("out")).unwrap();

    let replayed = case.replay(Backend::Vm, &tape, &[]);
    assert_eq!(replayed.stdout, recorded.stdout);
    assert!(
        !case.scratch.join("out").exists(),
        "replay re-created files the recorded run wrote"
    );
}

/// Records the clock fixture and returns its tape's lines.
fn recorded_lines(case: &Case) -> (PathBuf, Vec<String>) {
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", &[]);
    assert_eq!(recorded.code, 0, "{}", recorded.stderr);
    let lines = std::fs::read_to_string(&tape)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    (tape, lines)
}

fn write_lines(path: &Path, lines: &[String]) {
    std::fs::write(path, lines.join("\n") + "\n").unwrap();
}

fn assert_tape_error(run: &Run, code: &str, needle: &str) {
    assert_ne!(
        run.code, 0,
        "the replay should have failed:\n{}",
        run.stdout
    );
    assert!(
        run.stderr.contains(&format!("error[{code}]")),
        "expected {code}, got:\n{}",
        run.stderr
    );
    assert!(
        run.stderr.contains(needle),
        "expected `{needle}` in:\n{}",
        run.stderr
    );
}

#[test]
fn an_edited_tape_stops_the_replay_where_it_diverges() {
    let case = Case::new("fs_reads");
    prepare_inputs(&case);
    let (tape, lines) = recorded_lines(&case);
    let edited = case.tape("edited.jsonl");

    // A different path: same kind of effect, different details.
    let mut changed = lines.clone();
    changed[1] = changed[1].replace("data/in.txt", "data/other.txt");
    write_lines(&edited, &changed);
    let run = case.replay(Backend::Vm, &edited, &[]);
    assert_tape_error(&run, "E1023", "\"data/other.txt\"");
    assert!(
        run.stderr.contains("app.flx:"),
        "the VM names the line:\n{}",
        run.stderr
    );

    // A different kind of effect.
    let mut changed = lines.clone();
    changed[1] = r#"{"seq":0,"kind":"clock_now","result":{"int":1}}"#.to_string();
    write_lines(&edited, &changed);
    assert_tape_error(
        &case.replay(Backend::Vm, &edited, &[]),
        "E1022",
        "clock_now()",
    );

    // The tape stops early.
    write_lines(&edited, &lines[..3]);
    assert_tape_error(
        &case.replay(Backend::Vm, &edited, &[]),
        "E1024",
        "The tape ended",
    );

    // Different printed text.
    let mut changed = lines.clone();
    let stdout = changed
        .iter()
        .position(|line| line.contains("\"stdout\""))
        .unwrap();
    changed[stdout] = changed[stdout].replace("alpha", "ALPHA");
    write_lines(&edited, &changed);
    assert_tape_error(
        &case.replay(Backend::Vm, &edited, &[]),
        "E1023",
        "the printed text",
    );

    // More recorded than the program does: an extra event before `exit`.
    let mut changed = lines.clone();
    let exit = changed.pop().unwrap();
    let next = changed.len() - 1;
    changed.push(format!(
        r#"{{"seq":{next},"kind":"stdout","text":"extra\n"}}"#
    ));
    changed.push(exit.replace(&format!("\"seq\":{next}"), &format!("\"seq\":{}", next + 1)));
    write_lines(&edited, &changed);
    assert_tape_error(
        &case.replay(Backend::Vm, &edited, &[]),
        "E1025",
        "never replayed",
    );

    // Not a tape at all.
    std::fs::write(&edited, "hello\n").unwrap();
    assert_tape_error(
        &case.replay(Backend::Vm, &edited, &[]),
        "E1026",
        "not a Flux tape",
    );

    // The program changed since recording.
    let source = std::fs::read_to_string(&case.program).unwrap();
    std::fs::write(&case.program, source + "\n// edited\n").unwrap();
    assert_tape_error(
        &case.replay(Backend::Vm, &tape, &[]),
        "E1026",
        "different program",
    );
}

/// Async code: fibers, sleeps, channels, events, sockets and file system
/// calls inside `run_async`. A tape of async code replays on either backend
/// (0188 S9d).
const ASYNC_FIXTURES: &[&str] = &[
    "async_sleep",
    "async_channels",
    "async_events",
    "async_scopes",
    "async_fs",
    "async_tcp",
    "async_race_loser",
];

#[test]
fn each_async_fixture_replays_on_the_vm() {
    for fixture in ASYNC_FIXTURES {
        assert_replays(fixture, Backend::Vm, Backend::Vm);
    }
}

/// Replays a tape of `async_long_sleep` and returns how long the recording
/// and the replay took.
fn time_long_sleep(backend: Backend) -> (std::time::Duration, std::time::Duration) {
    let case = Case::new("async_long_sleep");
    let tape = case.tape("run.jsonl");
    let started = std::time::Instant::now();
    let recorded = case.record(backend, &tape, "", &[]);
    let recording = started.elapsed();
    assert_eq!(recorded.code, 0, "{}", recorded.stderr);
    let started = std::time::Instant::now();
    let replayed = case.replay(backend, &tape, &[]);
    let replaying = started.elapsed();
    assert_eq!(replayed.stdout, recorded.stdout, "{}", replayed.stderr);
    (recording, replaying)
}

#[test]
fn async_replay_does_not_wait_for_timers() {
    let (recording, replaying) = time_long_sleep(Backend::Vm);
    assert!(
        recording >= std::time::Duration::from_millis(2500),
        "the recorded run sleeps 2.5 s, took {recording:?}"
    );
    assert!(
        replaying + std::time::Duration::from_millis(1500) < recording,
        "replay took {replaying:?}, recording {recording:?}: it waited for the timer"
    );
}

/// Records `async_tcp`, then replays it while its port is taken: a replay
/// that opened a socket would fail to listen.
fn assert_tcp_replays_without_sockets(backend: Backend) {
    let case = Case::new("async_tcp");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(backend, &tape, "", &[]);
    assert_eq!(recorded.code, 0, "{}", recorded.stderr);
    let source = std::fs::read_to_string(&case.program).unwrap();
    let port = source
        .split("\"127.0.0.1\", ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .unwrap();
    let _taken = std::net::TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    let replayed = case.replay(backend, &tape, &[]);
    assert_eq!(replayed.stdout, recorded.stdout, "{}", replayed.stderr);
    assert!(
        recorded.stdout.contains("client got echo hello"),
        "{}",
        recorded.stdout
    );
}

#[test]
fn async_replay_opens_no_sockets() {
    assert_tcp_replays_without_sockets(Backend::Vm);
}

#[test]
fn async_replay_writes_no_files() {
    let case = Case::new("async_fs");
    prepare_inputs(&case);
    let tape = case.tape("run.jsonl");
    let recorded = case.record(Backend::Vm, &tape, "", &[]);
    assert!(case.scratch.join("out-async.txt").exists());
    std::fs::remove_file(case.scratch.join("out-async.txt")).unwrap();
    std::fs::remove_dir_all(case.scratch.join("data")).unwrap();
    let replayed = case.replay(Backend::Vm, &tape, &[]);
    assert_eq!(replayed.stdout, recorded.stdout, "{}", replayed.stderr);
    assert!(!case.scratch.join("out-async.txt").exists());
}

/// Records `fixture` with several workers requested, then replays the tape.
fn assert_replays_with_workers(fixture: &str, backend: Backend) {
    for attempt in 0..5 {
        let case = Case::new(fixture);
        let tape = case.tape("run.jsonl");
        let recorded = case.record(backend, &tape, "", &[("FLUX_WORKERS", "4")]);
        assert_eq!(recorded.code, 0, "{}", recorded.stderr);
        // A native program's stderr is shown only when it fails, so only the
        // VM's note is visible here.
        assert!(
            !matches!(backend, Backend::Vm) || recorded.stderr.contains("on one worker"),
            "the run should say it uses one worker:\n{}",
            recorded.stderr
        );
        let replayed = case.replay(backend, &tape, &[("FLUX_WORKERS", "4")]);
        assert_eq!(
            replayed.stdout, recorded.stdout,
            "attempt {attempt}: {}",
            replayed.stderr
        );
    }
}

#[test]
fn async_runs_on_one_worker_under_a_tape() {
    assert_replays_with_workers("async_sleep", Backend::Vm);
}

/// Rewrites the `seq` numbers of a tape's events after one was removed.
fn renumber(lines: &[String]) -> Vec<String> {
    let mut out = vec![lines[0].clone()];
    for (seq, line) in lines[1..].iter().enumerate() {
        let rest = line.split_once(',').unwrap().1;
        out.push(format!("{{\"seq\":{seq},{rest}"));
    }
    out
}

fn assert_async_tape_divergence(backend: Backend) {
    let case = Case::new("async_sleep");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(backend, &tape, "", &[]);
    assert_eq!(recorded.code, 0, "{}", recorded.stderr);
    let lines: Vec<String> = std::fs::read_to_string(&tape)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let edited = case.tape("edited.jsonl");

    // A timer of another length.
    let timer = lines
        .iter()
        .position(|line| line.contains("\"timer_start\""))
        .unwrap();
    let mut changed = lines.clone();
    changed[timer] = changed[timer].replace("\"ms\":60", "\"ms\":61");
    assert_ne!(changed[timer], lines[timer], "{}", lines[timer]);
    write_lines(&edited, &changed);
    assert_tape_error(&case.replay(backend, &edited, &[]), "E1023", "timer_start");

    // A completion that never arrives: the program waits for it while the
    // tape goes on with something else.
    let completion = lines
        .iter()
        .position(|line| line.contains("\"async_complete\""))
        .unwrap();
    let mut changed = lines.clone();
    changed.remove(completion);
    write_lines(&edited, &renumber(&changed));
    assert_tape_error(
        &case.replay(backend, &edited, &[]),
        "E1022",
        "waiting for an async completion",
    );
}

#[test]
fn an_edited_async_tape_stops_the_replay() {
    assert_async_tape_divergence(Backend::Vm);
}

/// Tasks (0188 S8c): each runs on a thread of its own, and what crosses
/// threads (spawns, starts and ends, cancels, joins, hand-overs to fibers,
/// channel operations) is ordered on the tape, so a replay runs the threads
/// in the recorded order.
const TASK_FIXTURES: &[&str] = &["task_join", "task_await", "task_cancel", "task_channel"];

#[test]
fn each_task_fixture_replays_on_the_vm() {
    for fixture in TASK_FIXTURES {
        assert_replays(fixture, Backend::Vm, Backend::Vm);
    }
}

/// Records `fixture` once on `from`, with several workers asked for, then
/// replays the tape `times` times on `to`: every replay prints exactly what
/// the recording did.
fn assert_replays_identically(fixture: &str, from: Backend, to: Backend, times: usize) {
    let workers = [("FLUX_WORKERS", "4")];
    let case = Case::new(fixture);
    let tape = case.tape("run.jsonl");
    let recorded = case.record(from, &tape, "", &workers);
    assert_eq!(recorded.code, 0, "{fixture}: {}", recorded.stderr);
    for run in 0..times {
        let replayed = case.replay(to, &tape, &workers);
        assert_eq!(
            replayed.stdout, recorded.stdout,
            "{fixture}: replay {run} printed something else:\n{}",
            replayed.stderr
        );
        assert_eq!(
            replayed.code, 0,
            "{fixture}: replay {run}: {}",
            replayed.stderr
        );
    }
}

#[test]
fn a_replay_with_tasks_is_identical_a_hundred_times() {
    for fixture in ["task_cancel", "task_channel"] {
        assert_replays_identically(fixture, Backend::Vm, Backend::Vm, 100);
    }
}

/// A tape whose task starts before it is spawned: the replay stops at the
/// first event no thread can perform.
fn assert_task_tape_divergence(backend: Backend) {
    let case = Case::new("task_join");
    let tape = case.tape("run.jsonl");
    let recorded = case.record(backend, &tape, "", &[]);
    assert_eq!(recorded.code, 0, "{}", recorded.stderr);
    let mut lines: Vec<String> = std::fs::read_to_string(&tape)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let spawn = lines
        .iter()
        .position(|line| line.contains("\"task_spawn\""))
        .unwrap();
    let start = lines
        .iter()
        .position(|line| line.contains("\"task_start\""))
        .unwrap();
    lines.swap(spawn, start);
    let edited = case.tape("edited.jsonl");
    write_lines(&edited, &renumber(&lines));
    assert_tape_error(&case.replay(backend, &edited, &[]), "E1022", "task_start");
}

#[test]
fn an_edited_task_tape_stops_the_replay() {
    assert_task_tape_divergence(Backend::Vm);
}

#[test]
fn run_async_inside_a_task_cannot_be_recorded_yet() {
    let case = Case::new("task_nested_async");
    let tape = case.tape("run.jsonl");
    let run = case.record(Backend::Vm, &tape, "", &[]);
    assert_tape_error(&run, "E1027", "run_async inside a task");
}

#[test]
fn record_and_replay_are_for_running_a_program() {
    let case = Case::new("clock");
    let tape = case.tape("run.jsonl");
    let tape = tape.to_str().unwrap();
    let both = case.run(Backend::Vm, &["--record", tape, "--replay", tape], "", &[]);
    assert_ne!(both.code, 0);
    assert!(
        both.stderr.contains("one of --record and --replay"),
        "{}",
        both.stderr
    );

    let test_mode = case.run(Backend::Vm, &["--test", "--record", tape], "", &[]);
    assert_ne!(test_mode.code, 0);
    assert!(
        test_mode.stderr.contains("apply only to running a program"),
        "{}",
        test_mode.stderr
    );
}

#[cfg(feature = "llvm")]
mod native {
    use super::*;

    #[test]
    fn each_fixture_replays_on_native() {
        for fixture in FIXTURES {
            assert_replays(fixture, Backend::Native, Backend::Native);
        }
    }

    #[test]
    fn a_vm_tape_replays_on_native() {
        for fixture in FIXTURES {
            assert_replays(fixture, Backend::Vm, Backend::Native);
        }
    }

    #[test]
    fn a_native_tape_replays_on_the_vm() {
        for fixture in FIXTURES {
            assert_replays(fixture, Backend::Native, Backend::Vm);
        }
    }

    #[test]
    fn native_replay_changes_nothing_outside_the_program() {
        let case = Case::new("fs_writes");
        let tape = case.tape("run.jsonl");
        let recorded = case.record(Backend::Native, &tape, "", &[]);
        std::fs::remove_dir_all(case.scratch.join("out")).unwrap();
        let replayed = case.replay(Backend::Native, &tape, &[]);
        assert_eq!(replayed.stdout, recorded.stdout);
        assert!(!case.scratch.join("out").exists());
    }

    #[test]
    fn native_replay_stops_where_the_tape_diverges() {
        let case = Case::new("fs_reads");
        prepare_inputs(&case);
        let tape = case.tape("run.jsonl");
        let recorded = case.record(Backend::Native, &tape, "", &[]);
        assert_eq!(recorded.code, 0, "{}", recorded.stderr);
        let edited = case.tape("edited.jsonl");
        let text = std::fs::read_to_string(&tape).unwrap();
        std::fs::write(&edited, text.replacen("data/in.txt", "data/other.txt", 1)).unwrap();
        let run = case.replay(Backend::Native, &edited, &[]);
        assert_tape_error(&run, "E1023", "\"data/other.txt\"");
    }

    /// Builds the case's program as a native binary, as an editor does.
    fn build_binary(case: &Case) -> PathBuf {
        let binary = case.scratch.join("app_bin");
        let run = case.run(
            Backend::Native,
            &["--emit-binary", "-o", binary.to_str().unwrap()],
            "",
            &[],
        );
        assert_eq!(run.code, 0, "{}", run.stderr);
        binary
    }

    /// Runs a native binary the driver built, without the driver.
    fn run_binary(case: &Case, binary: &Path, env: &[(&str, &Path)]) -> Run {
        let mut command = Command::new(binary);
        command
            .current_dir(case.scratch.path())
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().expect("run the native binary");
        Run {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
        }
    }

    fn header(tape: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(tape).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    #[test]
    fn a_native_program_records_on_its_own() {
        // Under lldb or an editor, nothing but the program writes the tape.
        let case = Case::new("clock");
        let binary = build_binary(&case);
        let by_driver = case.tape("driver.jsonl");
        let recorded = case.record(Backend::Native, &by_driver, "", &[]);
        assert_eq!(recorded.code, 0, "{}", recorded.stderr);
        let tape = case.tape("own.jsonl");
        // A file already there is replaced, not added to.
        std::fs::write(&tape, "left from another run\n").unwrap();
        let run = run_binary(&case, &binary, &[("FLUX_TAPE_RECORD", &tape)]);
        assert_eq!(run.code, 0, "{}", run.stderr);
        let own = header(&tape);
        assert_eq!(own["program_hash"], header(&by_driver)["program_hash"]);
        assert_eq!(own["backend"], "native");
        assert_eq!(own["magic"], "FLXTAPE");
        for backend in [Backend::Vm, Backend::Native] {
            let replayed = case.replay(backend, &tape, &[]);
            assert_eq!(replayed.code, 0, "{backend:?}: {}", replayed.stderr);
            assert_eq!(replayed.stdout, run.stdout, "{backend:?}");
        }
        let replayed = run_binary(&case, &binary, &[("FLUX_TAPE_REPLAY", &tape)]);
        assert_eq!(replayed.stdout, run.stdout, "{}", replayed.stderr);
    }

    #[test]
    fn a_native_program_checks_the_tape_it_replays() {
        let other = Case::new("console");
        let tape = other.tape("console.jsonl");
        let recorded = other.record(Backend::Native, &tape, STDIN, &[]);
        assert_eq!(recorded.code, 0, "{}", recorded.stderr);
        let case = Case::new("clock");
        let binary = build_binary(&case);
        let run = run_binary(&case, &binary, &[("FLUX_TAPE_REPLAY", &tape)]);
        assert_eq!(run.code, 1);
        // Rendered for the person running it, and as the line tools read.
        assert!(run.stderr.contains("error[E1026]"), "{}", run.stderr);
        assert!(
            run.stderr.contains("recorded from a different program"),
            "{}",
            run.stderr
        );
        assert!(run.stderr.contains("flux-tape-error: {"), "{}", run.stderr);
        // Through the driver, the error is shown once.
        let run = case.replay(Backend::Native, &tape, &[]);
        assert_eq!(
            run.stderr.matches("error[E1026]").count(),
            1,
            "{}",
            run.stderr
        );
        assert!(!run.stderr.contains("flux-tape-error:"), "{}", run.stderr);
    }

    #[test]
    fn each_async_fixture_replays_on_native() {
        for fixture in ASYNC_FIXTURES {
            assert_replays(fixture, Backend::Native, Backend::Native);
        }
    }

    #[test]
    fn native_async_replay_does_not_wait_for_timers() {
        let (recording, replaying) = time_long_sleep(Backend::Native);
        assert!(
            replaying + std::time::Duration::from_millis(1500) < recording,
            "replay took {replaying:?}, recording {recording:?}: it waited for the timer"
        );
    }

    #[test]
    fn native_async_replay_opens_no_sockets() {
        assert_tcp_replays_without_sockets(Backend::Native);
    }

    #[test]
    fn native_async_runs_on_one_worker_under_a_tape() {
        assert_replays_with_workers("async_sleep", Backend::Native);
    }

    #[test]
    fn an_edited_async_tape_stops_the_native_replay() {
        assert_async_tape_divergence(Backend::Native);
    }

    /// Both schedulers follow the same rules, so a tape of async code or
    /// tasks replays on the other backend too (0188 S9d).
    #[test]
    fn a_vm_async_tape_replays_on_native() {
        for fixture in ASYNC_FIXTURES.iter().chain(TASK_FIXTURES) {
            assert_replays(fixture, Backend::Vm, Backend::Native);
        }
    }

    #[test]
    fn a_native_async_tape_replays_on_the_vm() {
        for fixture in ASYNC_FIXTURES.iter().chain(TASK_FIXTURES) {
            assert_replays(fixture, Backend::Native, Backend::Vm);
        }
    }

    #[test]
    fn a_cross_backend_replay_with_tasks_is_identical_a_hundred_times() {
        for fixture in ["task_cancel", "task_channel"] {
            assert_replays_identically(fixture, Backend::Vm, Backend::Native, 100);
            assert_replays_identically(fixture, Backend::Native, Backend::Vm, 100);
        }
    }

    #[test]
    fn each_task_fixture_replays_on_native() {
        for fixture in TASK_FIXTURES {
            assert_replays(fixture, Backend::Native, Backend::Native);
        }
    }

    #[test]
    fn a_native_replay_with_tasks_is_identical_a_hundred_times() {
        for fixture in ["task_cancel", "task_channel"] {
            assert_replays_identically(fixture, Backend::Native, Backend::Native, 100);
        }
    }

    #[test]
    fn an_edited_task_tape_stops_the_native_replay() {
        assert_task_tape_divergence(Backend::Native);
    }

    #[test]
    fn native_run_async_inside_a_task_cannot_be_recorded_yet() {
        let case = Case::new("task_nested_async");
        let tape = case.tape("run.jsonl");
        let run = case.record(Backend::Native, &tape, "", &[]);
        assert_tape_error(&run, "E1027", "run_async inside a task");
    }
}
