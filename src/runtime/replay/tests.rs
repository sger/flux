use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

/// The tape is process-global; tests that drive it run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    enlist_in_test_tape();
    guard
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
    EffectRequest::ReadFile { path: path.to_string() }
}

fn stdout(text: &str) -> EffectRequest {
    EffectRequest::Stdout { text: text.into() }
}

fn text(value: &str) -> Outcome {
    Outcome::Text(value.into())
}

/// Records `run`'s effects to a tape and loads it back.
fn record(name: &str, run: impl FnOnce()) -> LoadedTape {
    enlist_in_test_tape();
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
        EffectRequest::EnvVar { name: "HOME".into() },
        EffectRequest::EnvArgs,
        EffectRequest::EnvCwd,
        EffectRequest::EnvHomeDir,
        EffectRequest::ProcRun { cmd: "echo".into(), argv: vec!["hi".into()] },
        EffectRequest::ClockNow,
        EffectRequest::Time,
        EffectRequest::WriteFile { path: "a".into(), contents: "x".into() },
        EffectRequest::FsWriteFile { path: "a".into(), contents: TapeBytes(vec![0xff, 0x00]) },
        EffectRequest::FsCreateDirAll { path: "a".into() },
        EffectRequest::FsRemoveFile { path: "a".into() },
        EffectRequest::FsRemoveDirAll { path: "a".into() },
        EffectRequest::FsRename { from: "a".into(), to: "b".into() },
        stdout("hello\n"),
        EffectRequest::Stderr { text: "oops\n".into() },
        EffectRequest::Exit { failed: true, message: Some("panic: boom".into()) },
        EffectRequest::EventClock,
        EffectRequest::TimerStart { req: 1, ms: 30 },
        EffectRequest::DnsResolve { req: 2, host: "example.com".into(), port: 80 },
        EffectRequest::TcpConnect { req: 3, addr: "127.0.0.1:8080".into() },
        EffectRequest::TcpListen { req: 4, addr: "127.0.0.1:0".into() },
        EffectRequest::TcpAccept { req: 5, handle: 1 },
        EffectRequest::TcpRead { req: 6, handle: 2, max: 64 },
        EffectRequest::TcpWrite { req: 7, handle: 2, bytes: TapeBytes(vec![0xff]) },
        EffectRequest::TcpClose { handle: 2 },
        EffectRequest::AsyncComplete { req: 7 },
        EffectRequest::TaskSpawn { id: 1 },
        EffectRequest::TaskStart { id: 1 },
        EffectRequest::TaskEnd { id: 1, failed: true },
        EffectRequest::TaskCancel { id: 2 },
        EffectRequest::TaskJoin { id: 2 },
        EffectRequest::TaskAwait { id: 2 },
        EffectRequest::Wake { waiter: 3 },
        EffectRequest::Channel { chan: 1, op: "try_recv".into() },
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
        Outcome::Proc(ProcOut { status: 0, stdout: "out".into(), stderr: TapeBytes(vec![0xfe]) }),
        Outcome::Fail("read_file failed".into()),
        Outcome::Bytes(TapeBytes(vec![0x68, 0x69])),
        Outcome::Addrs(vec!["127.0.0.1:80".into(), "[::1]:80".into()]),
    ];
    for request in requests {
        let event = TapeEvent { seq: 3, fiber: 0, task: 0, request, result: None };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<TapeEvent>(&json).unwrap(), event, "{json}");
    }
    for outcome in outcomes {
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(serde_json::from_str::<Outcome>(&json).unwrap(), outcome, "{json}");
    }
}

#[test]
fn events_are_written_as_flat_json_lines() {
    let event = TapeEvent {
        seq: 0,
        fiber: 0,
        task: 0,
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
    assert_eq!(serde_json::to_string(&TapeBytes::from("ok")).unwrap(), r#""ok""#);
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
    let replayed = effect(read_file("in.txt"), || panic!("replay must not perform effects"));
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
    assert!(matches!(err, TapeError::KindMismatch { index: 0, .. }), "{err:?}");
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
    assert!(matches!(err, TapeError::Leftover { count: 2, first_index: 1, .. }), "{err:?}");

    // A truncated tape: no `exit`, then one effect too many.
    let mut truncated = tape;
    truncated.events.truncate(2);
    assert!(truncated.is_truncated());
    start_replay(truncated);
    effect(EffectRequest::ClockNow, || unreachable!()).unwrap();
    effect(EffectRequest::ClockNow, || unreachable!()).unwrap();
    let err = effect(EffectRequest::ClockNow, || unreachable!()).unwrap_err();
    reset();
    assert!(matches!(err, TapeError::Exhausted { count: 2, .. }), "{err:?}");
}

#[test]
fn replay_compares_how_the_run_ended() {
    let _guard = serial();
    let tape = record("ending.jsonl", || {
        finish(true, Some("panic: boom".into())).unwrap();
    });
    start_replay(tape.clone());
    assert_eq!(finish(true, Some("a differently worded panic".into())), Ok(()));
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
    let future = header_line.replace("\"version\":3", "\"version\":99");
    assert!(reason(&write("future.jsonl", &format!("{future}\n"))).contains("version 99"));
    // Version 1 tapes have no task field and no hand-overs (0188 S8c);
    // version 2 tapes name a hand-over by a backend's request (0188 S9d).
    for older in ["1", "2"] {
        let line = header_line.replace("\"version\":3", &format!("\"version\":{older}"));
        let path = write(&format!("older{older}.jsonl"), &format!("{line}\n"));
        assert!(reason(&path).contains(&format!("version {older}")));
    }
    let gap = format!("{header_line}\n{}\n", r#"{"seq":1,"kind":"clock_now","result":{"int":1}}"#);
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

#[test]
fn async_events_are_written_with_logical_numbers() {
    let event = TapeEvent {
        seq: 4,
        fiber: 2,
        task: 0,
        request: EffectRequest::TimerStart { req: 1, ms: 30 },
        result: None,
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"seq":4,"fiber":2,"kind":"timer_start","req":1,"ms":30}"#
    );
    let event = TapeEvent {
        seq: 5,
        fiber: 0,
        task: 0,
        request: EffectRequest::AsyncComplete { req: 1 },
        result: Some(Outcome::Bytes("hello".into())),
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"seq":5,"kind":"async_complete","req":1,"result":{"bytes":"hello"}}"#
    );
}

#[test]
fn every_completion_payload_survives_the_tape() {
    use crate::runtime::r#async::backend::{CompletionPayload, IoHandle};
    for payload in [
        CompletionPayload::Unit,
        CompletionPayload::Bytes(vec![0, 159, 146, 150]),
        CompletionPayload::Error("connection refused".into()),
        CompletionPayload::TcpHandle(IoHandle(9)),
        CompletionPayload::AddressList(vec![
            "127.0.0.1:80".parse().unwrap(),
            "[::1]:443".parse().unwrap(),
        ]),
    ] {
        assert_eq!(async_tape::payload_round_trip(&payload), Some(payload.clone()), "{payload:?}");
    }
}

#[test]
fn async_replay_answers_from_the_tape_without_the_backend() {
    use crate::runtime::r#async::backend::{Completion, CompletionPayload, RequestId};
    use async_tape::{Submission, deliver, submit};
    let _guard = serial();
    // Real request ids differ between runs; the tape numbers them itself.
    let tape = record("async.jsonl", || {
        assert!(submit(Some(RequestId(100)), Submission::Timer { ms: 5 }));
        let completion = deliver(|| {
            Some(Completion { request_id: RequestId(100), payload: CompletionPayload::Unit })
        });
        assert!(completion.is_some());
        // Nothing arrived: nothing is written.
        assert_eq!(deliver(|| None), None);
        console(stdout("woke\n")).unwrap();
        finish(false, None).unwrap();
    });
    assert_eq!(tape.events[0].request, EffectRequest::TimerStart { req: 1, ms: 5 });

    start_replay(tape);
    // A replay submits nothing to the backend...
    assert!(!submit(Some(RequestId(200)), Submission::Timer { ms: 5 }));
    // ...and gets the recorded completion back under this run's id.
    let completion = deliver(|| panic!("replay must not poll the backend"));
    assert_eq!(
        completion,
        Some(Completion { request_id: RequestId(200), payload: CompletionPayload::Unit })
    );
    // The tape's next event is console text, not a completion: none yet.
    assert_eq!(deliver(|| panic!("replay must not poll the backend")), None);
    console(stdout("woke\n")).unwrap();
    assert_eq!(finish(false, None), Ok(()));
    reset();
}

/// The fiber `test_fiber` reports as running: 0 for none.
static TEST_FIBER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn test_fiber() -> Option<u64> {
    match TEST_FIBER.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        fiber => Some(fiber),
    }
}

fn run_fiber(fiber: u64) {
    TEST_FIBER.store(fiber, std::sync::atomic::Ordering::Relaxed);
}

#[test]
fn async_names_do_not_depend_on_the_backend() {
    use crate::runtime::r#async::backend::RequestId;
    use async_tape::{Submission, deliver_cross, note_request, set_fiber_source, submit};
    let _guard = serial();
    set_fiber_source(test_fiber);
    // Fiber 70 makes a request it never submits, then a timer, which the
    // scheduler submits while no fiber runs; fiber 71 waits for a hand-over.
    let tape = record("names.jsonl", || {
        run_fiber(70);
        note_request(100);
        note_request(101);
        run_fiber(71);
        note_request(102);
        run_fiber(0);
        assert!(submit(Some(RequestId(101)), Submission::Timer { ms: 5 }));
        let handed = deliver_cross(|want| {
            assert_eq!(want, None);
            Some((102, "handed"))
        });
        assert_eq!(handed, Some("handed"));
        finish(false, None).unwrap();
    });
    // The timer is request 1, the hidden request numbered nothing, and it
    // is fiber 1's; the hand-over is named by the fiber waiting for it.
    assert_eq!(tape.events[0].fiber, 1);
    assert_eq!(tape.events[0].request, EffectRequest::TimerStart { req: 1, ms: 5 });
    assert_eq!(tape.events[1].request, EffectRequest::Wake { waiter: 2 });

    // Another backend: other ids, and no hidden request.
    start_replay(tape);
    run_fiber(9);
    note_request(7);
    run_fiber(8);
    note_request(8);
    run_fiber(0);
    assert!(!submit(Some(RequestId(7)), Submission::Timer { ms: 5 }));
    let handed = deliver_cross(|want| {
        assert_eq!(want, Some(8));
        Some((8, "handed"))
    });
    assert_eq!(handed, Some("handed"));
    assert_eq!(finish(false, None), Ok(()));
    reset();
}

#[test]
fn a_tape_forces_one_worker() {
    let _guard = serial();
    assert_eq!(worker_count(4), 4);
    start_replay(LoadedTape { path: "t".into(), header: header(), events: Vec::new() });
    assert_eq!(worker_count(4), 1);
    reset();
}

// ── Going back in a debug session (0188 S3) ─────────────────────────────

#[test]
fn a_kept_recording_answers_its_past_and_then_records_again() {
    let _guard = serial();
    start_memory_record();
    assert_eq!(mode(), ReplayMode::Record);
    let start = mark().expect("a kept recording can be marked");
    assert_eq!(effect(read_file("a"), || text("first")), Ok(text("first")));
    assert_eq!(console_shown(stdout("one\n")), Ok(true));

    // Back to the start: the past is answered from memory, not performed,
    // and its output is not printed again.
    rewind_to(start);
    assert_eq!(mode(), ReplayMode::Replay);
    let again = effect(read_file("a"), || panic!("going back must not perform effects"));
    assert_eq!(again, Ok(text("first")));
    assert_eq!(console_shown(stdout("one\n")), Ok(false));

    // Past the end of what was kept, the run records again.
    assert_eq!(mode(), ReplayMode::Record);
    assert_eq!(effect(read_file("b"), || text("second")), Ok(text("second")));
    assert_eq!(console_shown(stdout("two\n")), Ok(true));
    assert_eq!(event_list().len(), 4);
    reset();
}

#[test]
fn a_kept_recording_reports_a_run_that_goes_another_way() {
    let _guard = serial();
    start_memory_record();
    let start = mark().unwrap();
    effect(read_file("a"), || text("first")).unwrap();
    rewind_to(start);
    let err = effect(read_file("elsewhere"), || text("other")).unwrap_err();
    assert!(matches!(err, TapeError::PayloadMismatch { .. }), "{err:?}");
    reset();
}

#[test]
fn a_replay_goes_back_without_printing_twice() {
    let _guard = serial();
    let tape = record("rewind.jsonl", || {
        effect(read_file("in.txt"), || text("recorded")).unwrap();
        console(stdout("once\n")).unwrap();
        finish(false, None).unwrap();
    });
    start_replay(tape);
    let start = mark().unwrap();
    effect(read_file("in.txt"), || unreachable!()).unwrap();
    assert_eq!(console_shown(stdout("once\n")), Ok(true));
    rewind_to(start);
    effect(read_file("in.txt"), || unreachable!()).unwrap();
    assert_eq!(console_shown(stdout("once\n")), Ok(false));
    assert_eq!(finish(false, None), Ok(()));
    reset();
}

#[test]
fn a_debug_sessions_own_recording_can_be_dropped() {
    let _guard = serial();
    start_memory_record();
    assert!(is_implicit());
    stop_implicit();
    assert!(!is_active());
    assert_eq!(mark(), None);

    // A recording asked for is never dropped this way.
    let path = scratch_tape("asked.jsonl");
    start_record(&path, &header()).unwrap();
    stop_implicit();
    assert!(is_active());
    reset();
}

// ── Tasks (0188 S8c) ────────────────────────────────────────────────────

#[test]
fn an_event_names_its_task_only_off_the_main_program() {
    let event = |task| TapeEvent {
        seq: 2,
        fiber: 0,
        task,
        request: EffectRequest::ClockNow,
        result: Some(Outcome::Int(5)),
    };
    let main = serde_json::to_string(&event(0)).unwrap();
    assert!(!main.contains("task"), "{main}");
    let in_task = serde_json::to_string(&event(3)).unwrap();
    assert!(in_task.contains(r#""task":3"#), "{in_task}");
    assert_eq!(serde_json::from_str::<TapeEvent>(&in_task).unwrap(), event(3));
}

/// Records, on two threads, the main program spawning task 1 and each
/// printing; returns the tape.
fn record_two_threads(name: &str) -> LoadedTape {
    record(name, || {
        ordered(EffectRequest::TaskSpawn { id: 1 }, || None).unwrap();
        console(stdout("main\n")).unwrap();
        std::thread::spawn(|| {
            enlist_in_test_tape();
            async_tape::enter_task(1);
            ordered(EffectRequest::TaskStart { id: 1 }, || Some(Outcome::Bool(true))).unwrap();
            console(stdout("task\n")).unwrap();
            ordered(EffectRequest::TaskEnd { id: 1, failed: false }, || None).unwrap();
        })
        .join()
        .unwrap();
        console(stdout("after\n")).unwrap();
        finish(false, None).unwrap();
    })
}

#[test]
fn a_replay_runs_threads_in_the_recorded_order() {
    let _serial = serial();
    let tape = record_two_threads("two_threads.jsonl");
    let tasks: Vec<u32> = tape.events.iter().map(|event| event.task).collect();
    assert_eq!(tasks, [0, 0, 1, 1, 1, 0, 0]);

    // Replaying, the task's thread starts first and must wait for the main
    // program's first two events.
    start_replay(tape);
    let order = std::sync::Arc::new(Mutex::new(Vec::new()));
    let task_order = order.clone();
    let task = std::thread::spawn(move || {
        enlist_in_test_tape();
        async_tape::enter_task(1);
        ordered(EffectRequest::TaskStart { id: 1 }, || Some(Outcome::Bool(true))).unwrap();
        console_in_turn(stdout("task\n"), || task_order.lock().unwrap().push("task")).unwrap();
        ordered(EffectRequest::TaskEnd { id: 1, failed: false }, || None).unwrap();
    });
    std::thread::sleep(Duration::from_millis(30));
    ordered(EffectRequest::TaskSpawn { id: 1 }, || None).unwrap();
    console_in_turn(stdout("main\n"), || order.lock().unwrap().push("main")).unwrap();
    task.join().unwrap();
    console_in_turn(stdout("after\n"), || order.lock().unwrap().push("after")).unwrap();
    assert_eq!(finish(false, None), Ok(()));
    assert_eq!(*order.lock().unwrap(), ["main", "task", "after"]);
    reset();
}

#[test]
fn ordered_work_must_do_what_it_did_when_recorded() {
    let _serial = serial();
    let tape = record("ordered.jsonl", || {
        ordered(EffectRequest::TaskStart { id: 1 }, || Some(Outcome::Bool(false))).unwrap();
        finish(false, None).unwrap();
    });
    start_replay(tape);
    let err =
        ordered(EffectRequest::TaskStart { id: 1 }, || Some(Outcome::Bool(true))).unwrap_err();
    assert!(
        matches!(&err, TapeError::PayloadMismatch { what, .. } if what.contains("task_start")),
        "{err:?}"
    );
    reset();
}

#[test]
fn a_thread_whose_turn_never_comes_stops_the_replay() {
    let _serial = serial();
    let tape = record_two_threads("stall.jsonl");
    start_replay(tape);
    ordered(EffectRequest::TaskSpawn { id: 1 }, || None).unwrap();
    // The main program skips its print: the task waits for it, in vain.
    TURN_LIMIT_MS.store(200, Ordering::Relaxed);
    let waited = std::thread::spawn(|| {
        enlist_in_test_tape();
        async_tape::enter_task(1);
        ordered(EffectRequest::TaskStart { id: 1 }, || Some(Outcome::Bool(true)))
    })
    .join()
    .unwrap();
    TURN_LIMIT_MS.store(30_000, Ordering::Relaxed);
    let err = waited.unwrap_err();
    assert!(
        matches!(&err, TapeError::Stalled { owner, waiting, .. }
            if owner == "the main program" && waiting == "task 1"),
        "{err:?}"
    );
    assert!(err.message().contains("stdout"), "{}", err.message());
    reset();
}

#[test]
fn a_tape_is_checked_against_the_program_replaying_it() {
    let _guard = serial();
    let tape = record("checked.jsonl", || {
        effect(read_file("in.txt"), || text("recorded")).unwrap();
        finish(false, None).unwrap();
    });
    assert_eq!(tape.check_for("sha256:test"), Ok(Vec::new()));
    // A program that does not know its hash skips that check.
    assert_eq!(tape.check_for(""), Ok(Vec::new()));
    let Err(TapeError::BadTape { reason, .. }) = tape.check_for("sha256:other") else {
        panic!("another program's tape must be refused");
    };
    assert!(reason.contains("different program"), "{reason}");

    let mut unfinished = tape.clone();
    unfinished.events.pop();
    unfinished.header.flux_version = "0.0.0".to_string();
    let warnings = unfinished.check_for("sha256:test").unwrap();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("recorded by flux 0.0.0"), "{warnings:?}");
    assert!(warnings[1].contains("incomplete"), "{warnings:?}");
}

#[test]
fn a_debugger_reads_the_tape_a_page_at_a_time() {
    let _guard = serial();
    let tape = record("paged.jsonl", || {
        for i in 0..5 {
            console(stdout(&format!("{i}\n"))).unwrap();
        }
        finish(false, None).unwrap();
    });
    start_replay(tape);
    console(stdout("0\n")).unwrap();
    let page = event_page(0, 4).unwrap();
    assert!(page.replaying);
    assert_eq!(page.cursor, 1);
    assert_eq!(page.events.len(), 4);
    assert_eq!(page.events[0], (0, stdout("0\n").to_string()));
    assert_eq!(page.next, Some(4));
    let rest = event_page(4, 4).unwrap();
    assert_eq!(rest.events.len(), 2);
    assert_eq!(rest.next, None);
    reset();
    assert!(event_page(0, 4).is_none());

    // A recording lists what it has recorded, once asked to keep it.
    let path = scratch_tape("titles.jsonl");
    start_record(&path, &header()).unwrap();
    keep_titles();
    console(stdout("a\n")).unwrap();
    let page = event_page(0, 10).unwrap();
    assert!(!page.replaying);
    assert_eq!(page.cursor, 1);
    assert_eq!(page.events, vec![(0, stdout("a\n").to_string())]);
    reset();
}
