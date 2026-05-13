//! VM Flow.Stream pure pull-stream smoke tests.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn write_fixture(source: &str) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-vm-stream-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: &str) -> (String, String, bool) {
    let path = write_fixture(source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
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
    assert!(
        ok,
        "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
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
    assert!(
        ok,
        "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[|8, 10|]"), "{stdout}");
    assert!(stdout.contains("[|1, 2|]"), "{stdout}");
    assert!(stdout.contains("4"), "{stdout}");
}

#[test]
fn stream_chunk_append_merge_and_async_pull() {
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
        "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[[1, 2], [3, 4], [5]]"), "{stdout}");
    assert!(stdout.contains("[|1, 2, 3, 4, 5|]"), "{stdout}");
    assert!(stdout.contains("[|7|]"), "{stdout}");
}

#[test]
fn stream_flat_map_and_zip_compose() {
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

    print(Stream.to_array(Stream.zip(Stream.empty(), Stream.from_array([|"a"|]))))
    print(Stream.to_array(Stream.zip(Stream.from_array([|1|]), Stream.empty())))
}

fn main() with IO {
    run_async(body)
}
"#,
    );
    assert!(
        ok,
        "stream fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("[|1, 10, 2, 20, 3, 30|]"), "{stdout}");
    assert!(
        stdout.contains(r#"[|(1, "a"), (2, "b"), (3, "c")|]"#),
        "{stdout}"
    );
    assert_eq!(stdout.matches(r#"[|(1, "a")|]"#).count(), 2, "{stdout}");
    assert_eq!(stdout.matches("[||]").count(), 2, "{stdout}");
}
