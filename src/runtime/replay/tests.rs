use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

/// The tape is process-global; tests that drive it run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn scratch_tape(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flux-replay-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn header() -> TapeHeader {
    TapeHeader::new("sha256:test".to_string(), vec!["app.flx".to_string()], "vm")
}

fn read_file(path: &str) -> EffectRequest {
    EffectRequest::ReadFile {
        path: path.to_string(),
    }
}

fn stdout(text: &str) -> EffectRequest {
    EffectRequest::Stdout { text: text.into() }
}

fn text(value: &str) -> Outcome {
    Outcome::Text(value.into())
}

/// Records `run`'s effects to a tape and loads it back.
fn record(name: &str, run: impl FnOnce()) -> LoadedTape {
    let path = scratch_tape(name);
    start_record(&path, &header()).unwrap();
    run();
    reset();
    LoadedTape::load(&path).unwrap()
}

#[test]
fn every_request_and_outcome_round_trips_through_json() {
    let requests = vec![
        EffectRequest::ReadStdin,
        read_file("in.txt"),
        EffectRequest::ReadLines { path: "a".into() },
        EffectRequest::TryReadFile { path: "a".into() },
        EffectRequest::FsExists { path: "a".into() },
        EffectRequest::FsIsDir { path: "a".into() },
        EffectRequest::FsIsFile { path: "a".into() },
        EffectRequest::FsListDir { path: "a".into() },
        EffectRequest::FsMetadata { path: "a".into() },
        EffectRequest::Sha256File { path: "a".into() },
        EffectRequest::EnvVar {
            name: "HOME".into(),
        },
        EffectRequest::EnvArgs,
        EffectRequest::EnvCwd,
        EffectRequest::EnvHomeDir,
        EffectRequest::ProcRun {
            cmd: "echo".into(),
            argv: vec!["hi".into()],
        },
        EffectRequest::ClockNow,
        EffectRequest::Time,
        EffectRequest::WriteFile {
            path: "a".into(),
            contents: "x".into(),
        },
        EffectRequest::FsWriteFile {
            path: "a".into(),
            contents: TapeBytes(vec![0xff, 0x00]),
        },
        EffectRequest::FsCreateDirAll { path: "a".into() },
        EffectRequest::FsRemoveFile { path: "a".into() },
        EffectRequest::FsRemoveDirAll { path: "a".into() },
        EffectRequest::FsRename {
            from: "a".into(),
            to: "b".into(),
        },
        stdout("hello\n"),
        EffectRequest::Stderr {
            text: "oops\n".into(),
        },
        EffectRequest::Exit {
            failed: true,
            message: Some("panic: boom".into()),
        },
    ];
    let outcomes = vec![
        Outcome::Unit,
        Outcome::Bool(true),
        Outcome::Int(-7),
        text("hi"),
        Outcome::List(vec!["a".into(), "b".into()]),
        Outcome::Some("v".into()),
        Outcome::None,
        Outcome::Ok(Box::new(text("ok"))),
        Outcome::Err(TapeIoError {
            kind: IoKind::NotFound,
            message: "no such file".into(),
            path: "in.txt".into(),
        }),
        Outcome::Meta(FsMeta {
            size: 3,
            modified: 1_700_000_000_000,
            is_dir: false,
            is_file: true,
        }),
        Outcome::Proc(ProcOut {
            status: 0,
            stdout: "out".into(),
            stderr: TapeBytes(vec![0xfe]),
        }),
        Outcome::Fail("read_file failed".into()),
    ];
    for request in requests {
        let event = TapeEvent {
            seq: 3,
            fiber: 0,
            request,
            result: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<TapeEvent>(&json).unwrap(),
            event,
            "{json}"
        );
    }
    for outcome in outcomes {
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            serde_json::from_str::<Outcome>(&json).unwrap(),
            outcome,
            "{json}"
        );
    }
}

#[test]
fn events_are_written_as_flat_json_lines() {
    let event = TapeEvent {
        seq: 0,
        fiber: 0,
        request: read_file("in.txt"),
        result: Some(Outcome::Ok(Box::new(text("hello\n")))),
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"seq":0,"kind":"read_file","path":"in.txt","result":{"ok":{"text":"hello\n"}}}"#
    );
}

#[test]
fn bytes_that_are_not_utf8_are_written_as_hex() {
    let bytes = TapeBytes(vec![0xff, 0x00, 0x41]);
    let json = serde_json::to_string(&bytes).unwrap();
    assert_eq!(json, r#"{"hex":"ff0041"}"#);
    assert_eq!(serde_json::from_str::<TapeBytes>(&json).unwrap(), bytes);
    assert_eq!(
        serde_json::to_string(&TapeBytes::from("ok")).unwrap(),
        r#""ok""#
    );
}

#[test]
fn replay_returns_what_was_recorded_without_performing_it() {
    let _guard = serial();
    let tape = record("returns.jsonl", || {
        effect(read_file("in.txt"), || text("recorded")).unwrap();
        console(stdout("recorded\n")).unwrap();
        finish(false, None).unwrap();
    });
    assert!(!tape.is_truncated());

    start_replay(tape);
    let replayed = effect(read_file("in.txt"), || {
        panic!("replay must not perform effects")
    });
    assert_eq!(replayed.unwrap(), text("recorded"));
    console(stdout("recorded\n")).unwrap();
    assert_eq!(finish(false, None), Ok(()));
    reset();
}

#[test]
fn replay_reports_a_different_kind_of_effect() {
    let _guard = serial();
    let tape = record("kind.jsonl", || {
        effect(EffectRequest::ClockNow, || Outcome::Int(5)).unwrap();
        finish(false, None).unwrap();
    });
    start_replay(tape);
    let err = effect(EffectRequest::EnvArgs, || unreachable!()).unwrap_err();
    reset();
    assert!(
        matches!(err, TapeError::KindMismatch { index: 0, .. }),
        "{err:?}"
    );
    assert_eq!(err.code().code, "E1022");
}

#[test]
fn replay_reports_different_details_of_the_same_effect() {
    let _guard = serial();
    let tape = record("payload.jsonl", || {
        effect(read_file("a.txt"), || text("x")).unwrap();
        console(stdout("one\ntwo\n")).unwrap();
        finish(false, None).unwrap();
    });

    start_replay(tape.clone());
    let err = effect(read_file("b.txt"), || unreachable!()).unwrap_err();
    reset();
    assert_eq!(err.code().code, "E1023");
    assert!(err.message().contains("\"a.txt\""), "{err}");
    assert!(err.message().contains("\"b.txt\""), "{err}");

    start_replay(tape);
    effect(read_file("a.txt"), || unreachable!()).unwrap();
    let err = console(stdout("one\nTWO\n")).unwrap_err();
    reset();
    assert_eq!(err.code().code, "E1023");
    assert!(err.message().contains("the printed text"), "{err}");
    assert!(err.message().contains("\"two\""), "{err}");
    assert!(err.message().contains("\"TWO\""), "{err}");
}

#[test]
fn replay_reports_a_tape_that_ran_out_and_events_left_over() {
    let _guard = serial();
    let tape = record("short.jsonl", || {
        effect(EffectRequest::ClockNow, || Outcome::Int(1)).unwrap();
        effect(EffectRequest::ClockNow, || Outcome::Int(2)).unwrap();
        finish(false, None).unwrap();
    });

    // Fewer effects than recorded: leftovers at the end.
    start_replay(tape.clone());
    effect(EffectRequest::ClockNow, || unreachable!()).unwrap();
    let err = finish(false, None).unwrap_err();
    reset();
    assert!(
        matches!(
            err,
            TapeError::Leftover {
                count: 2,
                first_index: 1,
                ..
            }
        ),
        "{err:?}"
    );

    // A truncated tape: no `exit`, then one effect too many.
    let mut truncated = tape;
    truncated.events.truncate(2);
    assert!(truncated.is_truncated());
    start_replay(truncated);
    effect(EffectRequest::ClockNow, || unreachable!()).unwrap();
    effect(EffectRequest::ClockNow, || unreachable!()).unwrap();
    let err = effect(EffectRequest::ClockNow, || unreachable!()).unwrap_err();
    reset();
    assert!(
        matches!(err, TapeError::Exhausted { count: 2, .. }),
        "{err:?}"
    );
}

#[test]
fn replay_compares_how_the_run_ended() {
    let _guard = serial();
    let tape = record("ending.jsonl", || {
        finish(true, Some("panic: boom".into())).unwrap();
    });
    start_replay(tape.clone());
    assert_eq!(
        finish(true, Some("a differently worded panic".into())),
        Ok(())
    );
    reset();

    start_replay(tape);
    let err = finish(false, None).unwrap_err();
    reset();
    assert_eq!(err.code().code, "E1023");
}

#[test]
fn finishing_twice_writes_one_exit() {
    let _guard = serial();
    let tape = record("twice.jsonl", || {
        finish(false, None).unwrap();
        finish(true, None).unwrap();
    });
    assert_eq!(tape.events.len(), 1);
}

#[test]
fn loading_rejects_tapes_that_cannot_be_replayed() {
    let write = |name: &str, contents: &str| -> PathBuf {
        let path = scratch_tape(name);
        std::fs::write(&path, contents).unwrap();
        path
    };
    let header_line = serde_json::to_string(&header()).unwrap();
    let reason = |path: &Path| match LoadedTape::load(path).unwrap_err() {
        TapeError::BadTape { reason, .. } => reason,
        other => panic!("expected a bad tape, got {other:?}"),
    };

    assert!(reason(&write("not_tape.jsonl", "hello\n")).contains("not a Flux tape"));
    let future = header_line.replace("\"version\":1", "\"version\":99");
    assert!(reason(&write("future.jsonl", &format!("{future}\n"))).contains("version 99"));
    let gap = format!(
        "{header_line}\n{}\n",
        r#"{"seq":1,"kind":"clock_now","result":{"int":1}}"#
    );
    assert!(reason(&write("gap.jsonl", &gap)).contains("expected #0"));
    let missing = format!("{header_line}\n{}\n", r#"{"seq":0,"kind":"clock_now"}"#);
    assert!(reason(&write("missing.jsonl", &missing)).contains("missing a result"));
    let broken = format!("{header_line}\n{{not json\n");
    assert!(reason(&write("broken.jsonl", &broken)).contains("line 2 is malformed"));
}

#[test]
fn native_errors_cross_stderr_intact() {
    let err = TapeError::PayloadMismatch {
        index: 4,
        what: "the printed text".into(),
        tape: "\"a\"".into(),
        program: "\"b\"".into(),
    };
    let stderr = format!("some output\n{}\nmore\n", err.to_wire());
    assert_eq!(TapeError::from_stderr(&stderr), Some(err));
    assert_eq!(TapeError::from_stderr("panic: unrelated"), None);
}

#[test]
fn every_primop_with_an_outside_effect_is_on_the_tape_or_refused() {
    use crate::syntax::builtin_effects::primop_fine_effect_label;
    for op in (0..=u8::MAX).filter_map(CorePrimOp::from_id) {
        let Some(label) = primop_fine_effect_label(op) else {
            continue;
        };
        // Panics and arithmetic faults end the run; the tape records that as
        // the run's `exit`, not at the primop. `sha256` is pure.
        if matches!(label, "Panic" | "Div") {
            continue;
        }
        assert_ne!(
            tape_class(op),
            TapeClass::Untaped,
            "{op:?} has the `{label}` effect but the tape does not handle it"
        );
    }
}
