//! Integration tests for `Flow.Path`.
//!
//! `Flow.Path` is pure Flux with no primops, so the behavioral coverage lives
//! in the Flux fixture and is driven here through the `flux --test` runner.
//! These tests additionally assert the two properties that cannot be checked
//! from inside Flux: that the module is *not* auto-injected into the prelude,
//! and that it stays effect-free.

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
    // Own scratch dir per run: a literal filename in one shared directory let
    // concurrent test binaries overwrite each other (KI-010).
    let scratch = Scratch::new(name.trim_end_matches(".flx"));
    let file = scratch.write(name, source);

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_path_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_path.flx");
    assert!(success, "Flow.Path test suite failed:\n{stdout}");
    assert!(
        stdout.contains("28 tests: 28 passed, 0 failed"),
        "expected all 28 Flow.Path tests to pass, got:\n{stdout}"
    );
}

#[test]
fn flow_path_is_not_auto_injected_into_the_prelude() {
    // `Flow.Path` must be an explicit import. If it were added to
    // FLOW_PRELUDE_MODULES, `Path` would resolve here and this would pass
    // compilation — which would also mean every program pays to compile it.
    let (_stdout, stderr, success) = run_source(
        "path_not_prelude.flx",
        "fn main() -> Unit with Console {\n    println(Path.separator())\n}\n",
    );
    assert!(
        !success,
        "Flow.Path resolved without an explicit import; it must not be in the prelude"
    );
    let _ = stderr;
}

#[test]
fn flow_path_functions_are_pure() {
    // The whole point of stage 0 is that path manipulation needs no
    // capability. A caller with no effect row at all must be able to use it;
    // if any Flow.Path function acquired an effect, this stops compiling.
    let (stdout, _stderr, success) = run_source(
        "path_is_pure.flx",
        r#"import Flow.Path as Path

fn pure_join(a: String, b: String) -> String {
    Path.normalize(Path.join(a, b))
}

fn main() -> Unit with Console {
    println(pure_join("a/", "./b/../c"))
}
"#,
    );
    assert!(
        success,
        "Flow.Path is no longer usable from a pure function:\n{stdout}"
    );
    assert!(
        stdout.contains("a/c"),
        "expected normalized join to print a/c, got:\n{stdout}"
    );
}
