//! Surface-level tests for Flow.Async user ergonomics:
//! - `import Flow.Async exposing (..)` brings public ADT type names
//!   (`Result`, `AsyncError`, `Scope`, ...) and their constructors
//!   (`Ok`, `Err`, `Canceled`, ...) into scope without qualification.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

fn run_flux_source(source: &str, tag: &str) -> (String, String, bool) {
    flux_runner::run_flux(source, tag)
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
