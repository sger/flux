//! Native LLVM Flow.Stream pure pull-stream parity smoke tests.

#![cfg(feature = "llvm")]

use std::sync::{Mutex, OnceLock};

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use flux_runner::flux_source;

static NATIVE_STREAM_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn run_source(source: &str) -> (String, String, bool) {
    let _guard = NATIVE_STREAM_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native Stream test lock poisoned");
    let ran = flux_source(&source, "native_stream_tests").native().run();
    (ran.stdout, ran.stderr, ran.success)
}

#[test]
fn native_stream_sources_and_adapters() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Async as Async
import Flow.Stream as Stream

fn body() -> Unit with Async, Console {
    print(Stream.to_array(Stream.take(Stream.from_array([|1, 2, 3, 4|]), 3)))
    print(Stream.to_array(Stream.filter(Stream.map(Stream.from_list([1, 2, 3, 4]), fn(x) { x * 2 }), fn(x) { x > 4 })))
    print(Stream.fold(Stream.from_array([|1, 2, 3, 4|]), 0, fn(acc, x) { acc + x }))
}

fn main() with IO {
    Async.run_async(body)
}
"#,
    );
    assert!(
        ok,
        "native Stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[|1, 2, 3|]"), "{stdout}");
    assert!(stdout.contains("[|6, 8|]"), "{stdout}");
    assert!(stdout.contains("10"), "{stdout}");
}
