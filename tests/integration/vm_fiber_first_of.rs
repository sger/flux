//! VM `Async.first` / `Async.first_of` integration tests (proposal 0174
//! Phase 2 slice 2-ii).

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-fiber-first-of-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        fixture_tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for first_of fixture");
    let path = dir.join("vm_fiber_first_of.flx");
    std::fs::write(&path, source).expect("write first_of fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on first_of fixture");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
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
    assert!(
        elapsed < Duration::from_millis(1500),
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
