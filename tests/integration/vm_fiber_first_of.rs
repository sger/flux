//! VM `Async.first` / `Async.first_of` integration tests.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

#[test]
fn first_of_returns_fastest_index_and_cancels_losers() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(30000)
    1
}

fn fast() -> Int with Async {
    sleep(50)
    2
}

fn body() -> (Int, Int) with Async {
    first_of([slow, fast, slow])
}

fn main() with IO {
    let pair = run_async(body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "fastest");
    assert!(
        success,
        "first_of program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, ["1", "2"]);
    // Wide-gap deadlock guard (proposal 0177 T1.4): a working run returns on the
    // fast branch in ~50ms + compile; a regression that waits for the slow
    // losers blocks on their 30s sleeps and trips this. Not load-sensitive.
    // See vm_fiber_cancel_loser.rs and docs/internals/concurrency_model.md §1.
    assert!(
        elapsed < Duration::from_secs(8),
        "first_of waited for slow losers: {elapsed:?}"
    );
}

#[test]
fn first_of_immediate_children_are_source_ordered() {
    let source = r#"
import Flow.Async exposing (..)

fn ten() -> Int with Async { 10 }
fn twenty() -> Int with Async { 20 }
fn thirty() -> Int with Async { 30 }

fn body() -> (Int, Int) with Async {
    first_of([ten, twenty, thirty])
}

fn main() with IO {
    let pair = run_async(body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "tie");
    assert!(
        success,
        "first_of tie program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, ["0", "10"]);
}
