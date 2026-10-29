//! VM `Async.first` / `Async.first_of` integration tests (proposal 0174
//! Phase 2 slice 2-ii).

use crate::support::flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

#[test]
fn first_of_returns_fastest_index_and_cancels_losers() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(1000)
    1
}

fn fast() -> Int with Async {
    sleep(50)
    2
}

fn body() -> (Int, Int) with Async {
    first_of([slow, fast, slow])
}

fn main() with IO, Time {
    let t0 = now_ms()
    let pair = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "fastest");
    assert!(success, "first_of program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, ["1", "2"]);
    assert!(elapsed < Duration::from_millis(700), "first_of waited for slow losers: {elapsed:?}");
}
