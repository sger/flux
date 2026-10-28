//! `flux debug` end to end (proposal 0188 S2, S3).
//!
//! Each fixture in `tests/debug/vm/` is a program and a `.flxdbg` script of
//! debugger commands. The test runs the script against the program and
//! snapshots the transcript: what the debugger said, then what the program
//! printed.

use std::path::Path;
use std::process::{Command, Output};

#[path = "../support/mod.rs"]
mod support;
use crate::support::scratch;
use scratch::Scratch;

fn fixture_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/debug/vm"))
}

/// Copies `name.flx` and its script into a scratch directory, so the
/// transcript names the program the same way on every checkout.
fn prepare(name: &str) -> Scratch {
    let scratch = Scratch::new(&format!("debug-{name}"));
    for ext in ["flx", "flxdbg"] {
        let source = fixture_dir().join(format!("{name}.{ext}"));
        scratch.write(&format!("{name}.{ext}"), &std::fs::read_to_string(source).unwrap());
    }
    scratch
}

fn flux(scratch: &Scratch, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flux"));
    command.current_dir(scratch.path()).args(args).args(scratch.cache_args()).env("NO_COLOR", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run flux")
}

/// The debugger's lines, without the compiler's progress lines.
fn transcript(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let debugger: Vec<&str> = stderr
        .lines()
        .filter(|line| {
            !(line.starts_with('[')
                && (line.contains("Compiling")
                    || line.contains("Cached")
                    || line.contains("Linking")
                    || line.contains("Running via")))
        })
        .collect();
    format!(
        "{}\n--- program output ---\n{}",
        debugger.join("\n"),
        String::from_utf8_lossy(&output.stdout)
    )
}

fn debug_session(name: &str, extra: &[&str], env: &[(&str, &str)]) -> String {
    let scratch = prepare(name);
    let program = format!("{name}.flx");
    let script = format!("{name}.flxdbg");
    let mut args = vec!["debug", program.as_str(), "--script", script.as_str()];
    args.extend_from_slice(extra);
    let output = flux(&scratch, &args, env);
    assert!(
        output.status.success(),
        "flux debug failed for {name}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    transcript(&output)
}

fn assert_session(name: &str) {
    snapshot(name, &debug_session(name, &[], &[]));
}

/// A session whose program ends with a runtime error, after the debugger
/// stopped at it.
fn assert_failing_session(name: &str) {
    let scratch = prepare(name);
    let program = format!("{name}.flx");
    let script = format!("{name}.flxdbg");
    let output = flux(&scratch, &["debug", program.as_str(), "--script", script.as_str()], &[]);
    assert!(
        !output.status.success(),
        "{name} was to end with an error:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    snapshot(name, &transcript(&output));
}

fn snapshot(name: &str, transcript: &str) {
    insta::with_settings!({
        snapshot_path => "../snapshots/debug_vm",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(name, transcript);
    });
}

/// Sites between checkpoints, fixed for a session.
fn checkpoint_every(every: &str) -> [(&'static str, &str); 1] {
    [("FLUX_DEBUG_CHECKPOINT_EVERY", every)]
}

#[test]
fn breakpoints_stack_and_stepping() {
    assert_session("calls");
}

#[test]
fn stepping_into_a_match_arm() {
    assert_session("match_arms");
}

#[test]
fn stepping_through_pipeline_stages() {
    assert_session("pipeline");
}

#[test]
fn locals_and_parts_of_values() {
    assert_session("values");
}

/// `refs` counts a value's references, the debugger's own left out
/// (0188 S10b): with a checkpoint at every site, holding a copy of each
/// slot, the counts are the same.
#[test]
fn reference_counts() {
    let plain = debug_session("refs", &[], &[]);
    snapshot("refs", &plain);
    let checkpointed = debug_session("refs", &[], &checkpoint_every("1"));
    assert_eq!(checkpointed, plain);
}

#[test]
fn a_lambda_and_what_it_captured() {
    assert_session("closure");
}

#[test]
fn tail_calls_are_counted() {
    assert_session("tail_calls");
}

#[test]
fn a_breakpoint_in_a_callback_a_library_runs() {
    assert_session("callback");
}

#[test]
fn a_breakpoint_on_a_line_without_code_moves_down() {
    assert_session("no_code_line");
}

// ── Effects and runtime errors (0188 S7) ─────────────────────────────────

#[test]
fn a_handler_shows_the_frames_it_suspended_and_resume_runs_them_again() {
    assert_session("effect_yield");
}

#[test]
fn stepping_over_perform_and_resume() {
    assert_session("effect_steps");
}

#[test]
fn nested_handlers_and_handler_state() {
    assert_session("effect_handlers");
}

#[test]
fn a_handler_that_never_resumes() {
    assert_session("effect_discard");
}

#[test]
fn a_panic_stops_where_it_happens() {
    assert_failing_session("exception_panic");
}

#[test]
fn division_by_zero_stops_at_the_division() {
    assert_failing_session("exception_division");
}

#[test]
fn catch_none_lets_errors_end_the_program() {
    assert_failing_session("exception_off");
}

#[test]
fn fibers_are_threads_with_what_each_waits_for() {
    assert_session("async_threads");
}

#[test]
fn a_fiber_waiting_on_a_channel() {
    assert_session("async_channel");
}

#[test]
fn stepping_over_a_sleep_stays_in_the_fiber() {
    assert_session("async_step");
}

#[test]
fn a_failing_fiber_stops_where_it_fails() {
    assert_session("async_fail");
}

#[test]
fn a_panic_in_a_fiber_stops_in_that_fiber() {
    assert_failing_session("async_panic");
}

#[test]
fn a_task_is_a_thread_the_debugger_does_not_pause() {
    assert_session("async_task");
}

#[test]
fn compiler_internals_are_hidden_until_asked_for() {
    assert_session("internals");
}

#[test]
fn debugging_a_replay_shows_the_recorded_values() {
    let scratch = prepare("replayed");
    let recorded = flux(
        &scratch,
        &["replayed.flx", "--record", "run.jsonl"],
        &[("DEBUG_FIXTURE_NAME", "recorded")],
    );
    assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let output = flux(
        &scratch,
        &["debug", "replayed.flx", "--script", "replayed.flxdbg", "--replay", "run.jsonl"],
        &[("DEBUG_FIXTURE_NAME", "changed since")],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let transcript = transcript(&output);
    insta::with_settings!({
        snapshot_path => "../snapshots/debug_vm",
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!("replayed", transcript);
    });
}

/// The unit store under `FLUX_HOME` is keyed by content, not path, so a second
/// copy of a program reuses the first copy's unit. The debugger must still
/// show the second copy's source once the first is gone.
#[test]
fn a_unit_built_from_another_copy_shows_this_copys_source() {
    let home = Scratch::new("debug-store-home");
    let home_path = home.path().to_string_lossy().into_owned();
    let replay_in_a_fresh_copy = || {
        let scratch = prepare("replayed");
        let env = [("FLUX_HOME", home_path.as_str()), ("DEBUG_FIXTURE_NAME", "copied")];
        let recorded = flux(&scratch, &["replayed.flx", "--record", "run.jsonl"], &env);
        assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
        let output = flux(
            &scratch,
            &["debug", "replayed.flx", "--script", "replayed.flxdbg", "--replay", "run.jsonl"],
            &env,
        );
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        transcript(&output)
    };
    let first = replay_in_a_fresh_copy();
    let second = replay_in_a_fresh_copy();
    assert_eq!(first, second, "the second copy's session differs");
}

// ── Going back (0188 S3) ─────────────────────────────────────────────────

#[test]
fn going_back_by_step_next_and_finish() {
    assert_session("reverse_steps");
}

#[test]
fn going_back_to_earlier_breakpoint_hits() {
    assert_session("reverse_continue");
}

#[test]
fn going_back_into_a_callback_a_library_runs() {
    assert_session("reverse_callback");
}

#[test]
fn going_back_across_effect_handlers() {
    assert_session("reverse_handler");
}

#[test]
fn going_back_performs_no_effect_twice() {
    let transcript = debug_session("reverse_clock", &[], &[]);
    // The clock is read once: the value after going back and forward again
    // is the one first read.
    let readings: Vec<&str> =
        transcript.lines().filter(|line| line.starts_with("first = ")).collect();
    assert_eq!(readings.len(), 2, "{transcript}");
    assert_eq!(readings[0], readings[1], "{transcript}");
    let transcript: Vec<String> = transcript
        .lines()
        .map(|line| {
            if line.starts_with("first = ") {
                "first = [the clock]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    snapshot("reverse_clock", &transcript.join("\n"));
}

#[test]
fn going_back_into_async_code() {
    assert_session("reverse_async");
}

#[test]
fn going_back_across_a_task() {
    assert_session("reverse_task");
}

#[test]
fn going_back_between_fibers() {
    assert_session("reverse_both");
}

#[test]
fn going_back_while_a_timer_is_in_flight_then_running_live() {
    assert_session("reverse_sleep");
}

#[test]
fn going_back_through_a_channel() {
    assert_session("reverse_channel");
}

#[test]
fn going_back_in_a_replay_of_async_code() {
    let scratch = prepare("reverse_async_replayed");
    let recorded = flux(&scratch, &["reverse_async_replayed.flx", "--record", "run.jsonl"], &[]);
    assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let output = flux(
        &scratch,
        &[
            "debug",
            "reverse_async_replayed.flx",
            "--script",
            "reverse_async_replayed.flxdbg",
            "--replay",
            "run.jsonl",
        ],
        &[],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    snapshot("reverse_async_replayed", &transcript(&output));
}

#[test]
fn going_back_in_a_replay() {
    let scratch = prepare("reverse_replayed");
    let recorded = flux(
        &scratch,
        &["reverse_replayed.flx", "--record", "run.jsonl"],
        &[("DEBUG_FIXTURE_NAME", "recorded")],
    );
    assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let output = flux(
        &scratch,
        &[
            "debug",
            "reverse_replayed.flx",
            "--script",
            "reverse_replayed.flxdbg",
            "--replay",
            "run.jsonl",
        ],
        &[("DEBUG_FIXTURE_NAME", "changed since")],
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    snapshot("reverse_replayed", &transcript(&output));
}

#[test]
fn where_checkpoints_fall_does_not_change_where_going_back_lands() {
    // A checkpoint at every site, and only the one before the program ran:
    // the same transcript either way.
    let every_site = debug_session("reverse_steps", &[], &checkpoint_every("1"));
    let only_the_first = debug_session("reverse_steps", &[], &checkpoint_every("1000000000"));
    assert_eq!(every_site, only_the_first);
    assert_eq!(every_site, debug_session("reverse_steps", &[], &[]));
}

/// The transcript, with the checkpoint count it reports taken out.
fn without_checkpoint_count(transcript: &str) -> (String, Option<usize>) {
    let mut count = None;
    let lines: Vec<&str> = transcript
        .lines()
        .map(|line| match line.strip_suffix(" checkpoints") {
            Some(number) => {
                count = number.parse().ok();
                "[n] checkpoints"
            }
            None => line,
        })
        .collect();
    (lines.join("\n"), count)
}

#[test]
fn checkpoints_stay_bounded_on_a_long_run() {
    // 900,000 sites with a checkpoint due at every one: thinning keeps the
    // count within its bound, and going back from the end still works.
    let (bounded, count) =
        without_checkpoint_count(&debug_session("reverse_far", &[], &checkpoint_every("1")));
    let count = count.expect("the session reports its checkpoints");
    assert!(count <= 256, "{count} checkpoints");
    let (adaptive, _) = without_checkpoint_count(&debug_session("reverse_far", &[], &[]));
    assert_eq!(bounded, adaptive);
    snapshot("reverse_far", &adaptive);
}

#[test]
fn every_site_is_a_breakpoint_instruction() {
    // The tables a -g build records must point at the instructions they
    // describe: each site at an `OpBreakpoint` carrying its own index.
    use flux::bytecode::op_code::OpCode;
    use flux::compiler::Compiler;
    use flux::runtime::value::Value;
    use flux::syntax::{lexer::Lexer, parser::Parser};

    for name in ["calls", "match_arms", "pipeline", "values", "closure"] {
        let source = std::fs::read_to_string(fixture_dir().join(format!("{name}.flx"))).unwrap();
        let mut parser = Parser::new(Lexer::new(&source));
        let program = parser.parse_program();
        let interner = parser.take_interner();
        let mut compiler = Compiler::new_with_interner(format!("{name}.flx"), interner);
        compiler.set_debug_level(flux::debuginfo::DebugLevel::Full);
        compiler.compile(&program).expect("fixture compiles");
        let bytecode = compiler.bytecode();
        let mut checked = 0;
        for constant in &bytecode.constants {
            let Value::Function(function) = constant else {
                continue;
            };
            let Some(info) = &function.debug_info else {
                continue;
            };
            for (index, site) in info.sites.iter().enumerate() {
                let at = site.ip;
                assert_eq!(
                    OpCode::from(function.instructions[at]),
                    OpCode::OpBreakpoint,
                    "{name}: site {index} of {:?}",
                    info.name
                );
                let operand = u16::from_be_bytes([
                    function.instructions[at + 1],
                    function.instructions[at + 2],
                ]);
                assert_eq!(operand as usize, index, "{name}: {:?}", info.name);
                checked += 1;
            }
        }
        assert!(checked > 0, "{name} has no sites");
    }
}
