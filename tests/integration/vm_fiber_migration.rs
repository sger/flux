//! VM fiber migration integration tests.
//!
//! These exercise the public `run_async_with_workers` path with
//! `FLUX_FIBER_MIGRATION` both enabled and disabled. Unit tests in
//! `runtime::async::scheduler` pin the exact steal-from-back behavior; these
//! tests keep the user-visible VM behavior stable while migration is active.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source_with_env(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, Duration) {
    flux_runner::run_flux_with_env(source, tag, env)
}

const MIGRATION_ON: &[(&str, &str)] = &[("FLUX_FIBER_MIGRATION", "1")];
const MIGRATION_OFF: &[(&str, &str)] = &[("FLUX_FIBER_MIGRATION", "0")];

const PARKED_WORK_FIXTURE: &str = r#"
import Flow.Async exposing (..)

fn one() -> Int with Async {
    yield_now()
    1
}

fn two() -> Int with Async {
    yield_now()
    2
}

fn pair() -> (Int, Int) with Async {
    both(one, two)
}

fn body() -> Int with Async {
    let a = pair()
    let b = pair()
    let c = pair()
    a.0 + a.1 + b.0 + b.1 + c.0 + c.1
}

fn main() with IO {
    print(run_async_with_workers(4, body))
}
"#;

#[test]
fn migration_enabled_completes_parked_work() {
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(PARKED_WORK_FIXTURE, "enabled_parked", MIGRATION_ON);
    assert!(
        success,
        "migration-enabled parked work must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "9");
}

#[test]
fn migration_disabled_keeps_existing_behavior() {
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(PARKED_WORK_FIXTURE, "disabled_parked", MIGRATION_OFF);
    assert!(
        success,
        "migration-disabled parked work must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "9");
}

#[test]
fn race_immediate_fifo_is_preserved_under_migration() {
    let source = r#"
import Flow.Async exposing (..)

fn first() -> Int with Async { 10 }
fn second() -> Int with Async { 20 }

fn body() -> Int with Async {
    race(first, second)
}

fn main() with IO {
    print(run_async_with_workers(4, body))
}
"#;
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(source, "race_fifo", MIGRATION_ON);
    assert!(
        success,
        "race FIFO fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "10");
}

#[test]
fn first_of_immediate_fifo_is_preserved_under_migration() {
    let source = r#"
import Flow.Async exposing (..)

fn ten() -> Int with Async { 10 }
fn twenty() -> Int with Async { 20 }
fn thirty() -> Int with Async { 30 }

fn body() -> (Int, Int) with Async {
    first_of([ten, twenty, thirty])
}

fn main() with IO {
    let pair = run_async_with_workers(4, body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(source, "first_of_fifo", MIGRATION_ON);
    assert!(
        success,
        "first_of FIFO fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, ["0", "10"]);
}

#[test]
fn parked_sleep_fibers_resume_under_migration() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async {
    sleep(25)
    4
}

fn right() -> Int with Async {
    sleep(25)
    5
}

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let pair = run_async_with_workers(4, body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, elapsed) =
        run_source_with_env(source, "sleep_resume", MIGRATION_ON);
    assert!(
        success,
        "parked sleep fibers must resume under migration:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, ["4", "5"]);
    assert!(
        elapsed < Duration::from_millis(8000),
        "sleep migration fixture took too long: {elapsed:?}"
    );
}
