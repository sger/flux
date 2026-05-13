//! Surface-level tests for Flow.Async user ergonomics:
//! - `import Flow.Async exposing (..)` brings public ADT type names
//!   (`Result`, `AsyncError`, `Scope`, ...) and their constructors
//!   (`Ok`, `Err`, `Canceled`, ...) into scope without qualification.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_source(source: &str, tag: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!(
        "flux-flow-async-surface-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        tag
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for Flow.Async surface test");
    let path = dir.join(format!("{tag}.flx"));
    std::fs::write(&path, source).expect("write Flow.Async surface fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on Flow.Async surface fixture");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

#[test]
fn exposing_all_brings_adt_type_and_constructors_into_scope() {
    let source = r#"
import Flow.Async exposing (..)

fn helper(s: Scope) -> Bool { true }

fn main() with IO {
    let r: Result<Int, AsyncError> = Ok(42)
    match r {
        Ok(v) -> print(v),
        Err(_) -> print(0)
    }
    let _ = helper
}
"#;
    let (stdout, stderr, ok) = run_flux_source(source, "exposing_all");
    assert!(
        ok,
        "exposing (..) must bring Result/Scope and Ok/Err into scope:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn zero_arg_constructor_resolves_with_explicit_type_annotation() {
    let source = r#"
import Flow.Async exposing (..)

fn main() with IO {
    let e: AsyncError = Canceled
    match e {
        Canceled -> print(1),
        _ -> print(0)
    }
}
"#;
    let (stdout, stderr, ok) = run_flux_source(source, "zero_arg_canceled");
    assert!(
        ok,
        "Canceled must resolve as a zero-arg constructor with explicit type:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "1");
}

#[test]
fn try_recovers_async_failures() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async { 9 }

fn main() with IO {
    let r = run_async(fn() {
        match try(body) {
            Ok(v) -> v,
            Err(_) -> 0
        }
    })
    print(r)
}
"#;
    let (stdout, stderr, ok) = run_flux_source(source, "try_basic");
    assert!(
        ok,
        "try must compile and recover from fail:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "9");
}
