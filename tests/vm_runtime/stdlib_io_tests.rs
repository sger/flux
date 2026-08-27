//! Integration tests for `Flow.IoError`.
//!
//! `Flow.IoError` declares the error type every fallible OS capability reports:
//! `Result<a, IoError>`, with a machine-readable `kind` beside the human
//! message. The behavioural coverage lives in the Flux fixture; asserted here
//! are the properties that cannot be checked from inside Flux.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let scratch = Scratch::new("io-suite");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_source(name: &str, source: &str) -> (String, String, bool) {
    // Own scratch dir per run: a literal filename in one shared directory let
    // concurrent test binaries overwrite each other (KI-010).
    let scratch = Scratch::new(name.trim_end_matches(".flx"));
    let file = scratch.write(name, source);

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_io_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_io.flx");
    assert!(success, "Flow.IoError test suite failed:\n{stdout}");
    assert!(
        stdout.contains("12 tests: 12 passed, 0 failed"),
        "expected all 12 Flow.IoError tests to pass, got:\n{stdout}"
    );
}

/// The whole point of a structured `kind`: a caller recovers from one failure
/// and propagates another without ever inspecting the message text.
#[test]
fn a_caller_can_branch_on_kind_without_reading_the_message() {
    let (stdout, stderr, success) = run_source(
        "io_branch_on_kind.flx",
        r#"
import Flow.IoError as Io
import Flow.Result as Result

fn recover(r: Result<Int, IoError>) -> String {
    match r {
        Ok(v) -> "ok:" + to_string(v),
        Err(e) -> match Io.is_not_found(e) {
            true -> "default",
            false -> "fatal:" + Io.kind_name(Io.error_kind(e)),
        },
    }
}

fn main() -> Unit {
    println(recover(Ok(3)))
    println(recover(Err(Io.io_error(NotFound, "gone", "/p"))))
    println(recover(Err(Io.io_error(PermissionDenied, "denied", "/p"))))
}
"#,
    );
    assert!(success, "branching on kind failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ok:3"), "expected ok:3:\n{stdout}");
    assert!(stdout.contains("default"), "expected default:\n{stdout}");
    assert!(
        stdout.contains("fatal:PermissionDenied"),
        "expected fatal:PermissionDenied:\n{stdout}"
    );
}

/// `IoError` is a record constructor exported from a stdlib module, so callers
/// matching it directly exercise cross-module named-field patterns — the exact
/// combination that used to fail with a bogus arity mismatch.
#[test]
fn callers_may_pattern_match_the_record_across_the_module_boundary() {
    let (stdout, stderr, success) = run_source(
        "io_named_pattern.flx",
        r#"
import Flow.IoError as Io

fn main() -> Unit {
    match Io.io_error(AlreadyExists, "exists", "/d") {
        IoError { kind, message, path } ->
            println(Io.kind_name(kind) + "|" + message + "|" + path),
    }
}
"#,
    );
    assert!(
        success,
        "named-field pattern on Flow.IoError's IoError failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("AlreadyExists|exists|/d"),
        "expected AlreadyExists|exists|/d:\n{stdout}"
    );
}

/// `Flow.Async` declares a *positional* `IoError(Int, String, String)` of its
/// own. Importing both must not collide.
#[test]
fn flow_async_io_error_coexists_with_flow_io_error() {
    let (stdout, stderr, success) = run_source(
        "io_async_coexist.flx",
        r#"
import Flow.Async as Async
import Flow.IoError as Io

fn main() -> Unit {
    println(Io.describe(Io.io_error(NotFound, "m", "/p")))
}
"#,
    );
    assert!(
        success,
        "Flow.Async's IoError must not collide with Flow.IoError's:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("NotFound: m (/p)"),
        "expected NotFound: m (/p):\n{stdout}"
    );
}

/// `Flow.IoError` is pure — declaring and inspecting an error touches nothing, so a
/// function that only builds one needs no effect annotation.
#[test]
fn flow_io_is_effect_free() {
    let (stdout, stderr, success) = run_source(
        "io_pure.flx",
        r#"
import Flow.IoError as Io

fn classify(path: String) -> String {
    Io.describe(Io.io_error(NotFound, "missing", path))
}

fn main() -> Unit {
    println(classify("/a"))
}
"#,
    );
    assert!(success, "Flow.IoError must be pure:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("NotFound: missing (/a)"),
        "expected NotFound: missing (/a):\n{stdout}"
    );
}

/// `println` must render collections, not drop them (KI-002).
///
/// The reported symptom — no output for a list or array — did not reproduce;
/// it was an artifact of filtering terminal output, where a `[1, 2, 3]` line
/// looks like the compiler's `[ 1 of 12]` progress lines. This asserts against
/// raw stdout so a real regression cannot hide the same way.
#[test]
fn println_renders_lists_and_arrays() {
    let (stdout, stderr, success) = run_source(
        "println_collections.flx",
        r#"
fn main() with IO {
    println([1, 2, 3])
    println([|1, 2, 3|])
    println([])
    println(["a", "b"])
}
"#,
    );
    assert!(success, "run failed:\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let printed: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        printed,
        vec!["[1, 2, 3]", "[|1, 2, 3|]", "[]", r#"["a", "b"]"#],
        "println dropped or misrendered a collection:\n{stdout}"
    );
}

/// A constrained stdlib function must reject the wrong container type (KI-003).
///
/// `List.contains` is `(List<a>, a) -> Bool`, so an `Array` argument is an
/// E300 — but the diagnostic used to be suppressed whenever the expected type
/// still held a free variable, which an `Eq`-constrained element type always
/// does. The call then compiled and returned a wrong answer instead of failing:
/// `not_elem` reported a present element as absent.
#[test]
fn a_constrained_stdlib_function_rejects_the_wrong_container() {
    for (call, func) in [
        ("contains(arr, 1)", "contains"),
        ("not_elem(arr, 1)", "not_elem"),
        ("nub(arr)", "nub"),
    ] {
        let source = format!(
            r#"
fn main() with IO {{
    let arr = [|1, 2|]
    println({call})
}}
"#
        );
        let (stdout, stderr, success) = run_source("wrong_container.flx", &source);
        let combined = format!("{stdout}{stderr}");
        assert!(
            !success,
            "`{func}` must reject an Array where List<a> is declared:\n{combined}"
        );
        assert!(
            combined.contains("E300"),
            "expected E300 for `{func}`, got:\n{combined}"
        );
    }
}

/// The KI-003 fix must not reject calls that are actually well-typed.
///
/// The suppression it removed existed to hide transient mismatches while a
/// type was still being solved, so these cover the shapes most at risk:
/// numeric defaulting, an untyped stdlib function whose inferred type is only
/// an approximation (`List.first`), and locally-declared unannotated functions.
#[test]
fn well_typed_calls_still_compile() {
    let (stdout, stderr, success) = run_source(
        "still_compiles.flx",
        r#"
fn find_in(xs, target) {
    if contains(xs, target) { Some(target) } else { None }
}

fn main() with IO {
    println(contains([1, 2, 3], 2))
    println(nub([1, 1, 2]))
    println(contains([1.5], 1.5))
    println(contains(["a"], "a"))
    println(min(1, 2))
    println(find_in([1, 2], 2))
}
"#,
    );
    assert!(
        success,
        "well-typed program must still compile:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
