//! VM Flow.Stream pure pull-stream smoke tests.

use crate::support::flux_runner;

fn run_source(source: &str) -> (String, String, bool) {
    flux_runner::run_flux(source, "stream")
}

#[test]
fn stream_sources_and_next_thread_rest_state() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Async as Async
import Flow.Stream as Stream

fn body() -> Unit with Async, Console {
    let s = Stream.from_array([|1, 2, 3|])
    match Stream.next(s) {
        Some(first) -> do {
            print(first.0)
            match Stream.next(first.1) {
                Some(second) -> print(second.0),
                _ -> print(99)
            }
        },
        _ -> print(98)
    }
    print(Stream.to_array(Stream.from_list([4, 5, 6])))
}

fn main() with IO {
    Async.run_async(body)
}
"#,
    );
    assert!(ok, "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("1"), "{stdout}");
    assert!(stdout.contains("2"), "{stdout}");
    assert!(stdout.contains("[|4, 5, 6|]"), "{stdout}");
}

#[test]
fn stream_adapters_and_consumers_compose() {
    let (stdout, stderr, ok) = run_source(
        r#"
import Flow.Async as Async
import Flow.Stream as Stream

fn body() -> Unit with Async, Console {
    let values = Stream.from_array([|1, 2, 3, 4, 5, 6|])
    let shaped = Stream.take(Stream.drop(Stream.filter(Stream.map(values, fn(x) { x * 2 }), fn(x) { x > 5 }), 1), 2)
    print(Stream.to_array(shaped))
    print(Stream.to_array(Stream.take_while(Stream.from_array([|1, 2, 5, 3|]), fn(x) { x < 4 })))
    print(Stream.count(Stream.take(Stream.repeat("x"), 4)))
}

fn main() with IO {
    Async.run_async(body)
}
"#,
    );
    assert!(ok, "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("[|8, 10|]"), "{stdout}");
    assert!(stdout.contains("[|1, 2|]"), "{stdout}");
    assert!(stdout.contains("4"), "{stdout}");
}
