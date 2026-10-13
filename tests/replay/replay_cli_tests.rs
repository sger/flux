//! `flux run --record` / `--replay` end to end (proposal 0188 S1).
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
        let program = scratch.write("app.flx", &std::fs::read_to_string(source).unwrap());
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

#[test]
fn async_code_cannot_be_recorded_yet() {
    let case = Case::new("async_refused");
    let tape = case.tape("run.jsonl");
    let run = case.record(Backend::Vm, &tape, "", &[]);
    assert_tape_error(&run, "E1027", "`run_async` cannot be recorded");
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

    #[test]
    fn native_async_code_cannot_be_recorded_yet() {
        let case = Case::new("async_refused");
        let tape = case.tape("run.jsonl");
        let run = case.record(Backend::Native, &tape, "", &[]);
        assert_tape_error(&run, "E1027", "`run_async` cannot be recorded");
    }
}
