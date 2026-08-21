//! Integration tests for `Flow.Result`.
//!
//! `Result<a, e>` is the error model every fallible stdlib operation will
//! return, so the properties locked in here are the ones the rest of 0178
//! depends on and that cannot be checked from inside Flux:
//!
//!   * `import Flow.Result` brings the type *and* both constructors into
//!     scope unqualified, so `Result<Int, String>` annotations and bare
//!     `Ok`/`Err` patterns work without qualification.
//!   * `Flow.Result` is NOT auto-injected. Injecting it is the one choice that
//!     cannot be walked back, and it would churn every whole-program IR
//!     snapshot on each stdlib change.
//!   * `Result` is an ordinary declaration, not a compiler built-in: a locally
//!     declared `Ok`/`Err` still shadows the prelude's. Reserving those names
//!     would have broken existing user code, and neither Haskell nor Rust
//!     reserves its result constructors.
//!   * `Flow.Async` declares its own `Result` and must keep working.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

/// Run a snippet through the VM and return `(stdout, stderr, success)`.
fn run_source(name: &str, source: &str) -> (String, String, bool) {
    let dir = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join(name);
    std::fs::write(&file, source).expect("write scratch fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));

    let _ = std::fs::remove_file(&file);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_result_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_result.flx");
    assert!(success, "Flow.Result test suite failed:\n{stdout}");
    assert!(
        stdout.contains("12 tests: 12 passed, 0 failed"),
        "expected all 12 Flow.Result tests to pass, got:\n{stdout}"
    );
}

/// One `import Flow.Result` must be enough for both the type annotation and
/// the bare constructors — no qualification, no separate constructor import.
#[test]
fn one_import_brings_the_type_and_both_constructors_into_scope() {
    let (stdout, stderr, success) = run_source(
        "result_one_import.flx",
        r#"
import Flow.Result as Result

fn classify(r: Result<Int, String>) -> String {
    match r {
        Ok(n) -> "ok:" + to_string(n),
        Err(e) -> "err:" + e,
    }
}

fn main() -> Unit {
    println(classify(Ok(1)))
    println(classify(Err("boom")))
}
"#,
    );
    assert!(
        success,
        "one import should bring Result, Ok, and Err into scope.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("ok:1"), "expected ok:1, got:\n{stdout}");
    assert!(
        stdout.contains("err:boom"),
        "expected err:boom, got:\n{stdout}"
    );
}

/// `Ok`/`Err` are ordinary constructor names, not reserved words. Several
/// corpus files already declare `type Result<T, E> = Ok(T) | Err(E)` of their
/// own; reserving these names would have broken every one of them. Neither
/// Haskell nor Rust reserves its result constructors either.
#[test]
fn a_user_may_declare_its_own_result_type() {
    let (stdout, stderr, success) = run_source(
        "result_user_shadow.flx",
        r#"
type Result<T, E> = Ok(T) | Err(E)

fn main() -> Unit {
    let r: Result<Int, String> = Ok(5)
    match r {
        Ok(n) -> println("local ok " + to_string(n)),
        Err(e) -> println("local err " + e),
    }
}
"#,
    );
    assert!(
        success,
        "a user-declared Result must keep working.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("local ok 5"),
        "expected the local constructor to win, got:\n{stdout}"
    );
}

/// A user `data` declaration reusing `Ok`/`Err` must work alongside an
/// imported `Flow.Result` — this is the form `Flow.Async` uses internally.
#[test]
fn a_user_data_declaration_may_reuse_ok_and_err() {
    let (stdout, stderr, success) = run_source(
        "result_user_data.flx",
        r#"
data MyResult<a> { Ok(a), Err(String) }

fn main() -> Unit {
    match Ok(9) {
        Ok(n) -> println("mine " + to_string(n)),
        Err(e) -> println("err " + e),
    }
}
"#,
    );
    assert!(
        success,
        "a user data decl may reuse Ok/Err.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("mine 9"), "expected mine 9, got:\n{stdout}");
}

/// `Flow.Async` declares its own `Result<a, e>`. Importing both modules must
/// not produce an ambiguity or a resolution failure.
#[test]
fn flow_async_result_coexists_with_flow_result() {
    let (stdout, stderr, success) = run_source(
        "result_async_coexist.flx",
        r#"
import Flow.Async as Async
import Flow.Result as Result

fn main() -> Unit {
    let r: Result<Int, String> = Ok(3)
    match r {
        Ok(n) -> println("flow-result " + to_string(n)),
        Err(e) -> println("err " + e),
    }
}
"#,
    );
    assert!(
        success,
        "Flow.Async's Result must not collide with Flow.Result's.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("flow-result 3"),
        "expected flow-result 3, got:\n{stdout}"
    );
}

/// `Result` carries no effects: it is a plain data type, so a function that
/// only builds and matches on it stays pure and needs no effect annotation.
#[test]
fn result_is_effect_free() {
    let (stdout, stderr, success) = run_source(
        "result_pure.flx",
        r#"
import Flow.Result as Result

fn pure_chain(x: Int) -> Result<Int, String> {
    Result.and_then_result(Ok(x), fn(n) { if n > 0 { Ok(n * 2) } else { Err("neg") } })
}

fn main() -> Unit {
    println(to_string(Result.unwrap_or_result(pure_chain(4), -1)))
    println(to_string(Result.unwrap_or_result(pure_chain(-4), -1)))
}
"#,
    );
    assert!(
        success,
        "Result combinators must be pure.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains('8'), "expected 8, got:\n{stdout}");
    assert!(stdout.contains("-1"), "expected -1, got:\n{stdout}");
}
