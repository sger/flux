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

#[test]
fn native_stream_chunk_merge_and_async_pull() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Async exposing (..)
import Flow.Stream as Stream

fn delayed_once(value) {
    Stream.make(fn() {
        sleep(5)
        Some((value, Stream.empty()))
    })
}

fn body() -> Unit with Async, Console {
    print(Stream.to_list(Stream.chunk(Stream.from_array([|1, 2, 3, 4, 5|]), 2)))
    print(Stream.to_array(Stream.merge(Stream.from_array([|1, 3, 5|]), Stream.from_array([|2, 4|]))))
    print(Stream.to_array(delayed_once(7)))
}

fn main() with IO {
    run_async(body)
}
"#,
    );
    assert!(
        ok,
        "native Stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[[1, 2], [3, 4], [5]]"), "{stdout}");
    assert!(stdout.contains("[|1, 2, 3, 4, 5|]"), "{stdout}");
    assert!(stdout.contains("[|7|]"), "{stdout}");
}

#[test]
fn native_stream_flat_map_and_zip_compose() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Async exposing (..)
import Flow.Stream as Stream

fn body() -> Unit with Async, Console {
    let expanded = Stream.flat_map(Stream.from_array([|1, 2, 3|]), fn(x) {
        Stream.from_array([|x, x * 10|])
    })
    print(Stream.to_array(expanded))

    let zipped = Stream.zip(Stream.from_array([|1, 2, 3|]), Stream.from_array([|"a", "b", "c"|]))
    print(Stream.to_array(zipped))

    let short_left = Stream.zip(Stream.from_array([|1|]), Stream.from_array([|"a", "b"|]))
    print(Stream.to_array(short_left))

    let short_right = Stream.zip(Stream.from_array([|1, 2|]), Stream.from_array([|"a"|]))
    print(Stream.to_array(short_right))
}

fn main() with IO {
    run_async(body)
}
"#,
    );
    assert!(
        ok,
        "native Stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[|1, 10, 2, 20, 3, 30|]"), "{stdout}");
    assert!(
        stdout.contains(r#"[|(1, "a"), (2, "b"), (3, "c")|]"#),
        "{stdout}"
    );
    assert_eq!(stdout.matches(r#"[|(1, "a")|]"#).count(), 2, "{stdout}");
}
