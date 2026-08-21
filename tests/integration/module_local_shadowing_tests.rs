//! Regression tests: module-level definitions shadow same-named builtins.
//!
//! A bare call inside `module M` used to resolve to the builtin rather than to
//! a sibling member of `M`, because module members are stored in the symbol
//! table under a qualified key (`M.name`) that bare-name shadow checks missed.
//! The builtin then ran in place of the local definition with no diagnostic.
//!
//! Two independent channels were affected and are both covered here:
//! `route_effectful_primops` (bare call → `perform`) and identifier
//! compilation falling through to `exposed_bindings`.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let dir = workspace_root().join("tests").join("flux");
    let path = dir.join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            "--test",
            path.to_str().unwrap(),
            "--root",
            dir.to_str().unwrap(),
            "--no-cache",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

/// Compile and run a two-file program in an isolated scratch directory.
///
/// A module must live in a file named after it (`module Text` → `Text.flx`),
/// so the module and the entry point are written separately and the scratch
/// directory is passed as a module root.
fn run_program(case: &str, module_name: &str, module_src: &str, main_src: &str) -> ProgramRun {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");

    let module_file = dir.join(format!("{module_name}.flx"));
    let main_file = dir.join("main.flx");
    std::fs::write(&module_file, module_src).expect("write module fixture");
    std::fs::write(&main_file, main_src).expect("write entry fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            "run",
            main_file.to_str().unwrap(),
            "--root",
            dir.to_str().unwrap(),
            "--no-cache",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for {case}: {e}"));

    let _ = std::fs::remove_dir_all(&dir);
    ProgramRun {
        stdout: String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        stderr: String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        success: output.status.success(),
    }
}

struct ProgramRun {
    stdout: String,
    stderr: String,
    success: bool,
}

impl ProgramRun {
    /// Diagnostics may land on either stream depending on the phase.
    fn output(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

#[test]
fn module_local_shadowing_flux_suite_passes() {
    let (stdout, success) = run_flux_test("module_local_shadowing.flx");
    assert!(success, "module shadowing suite failed:\n{stdout}");
    assert!(
        stdout.contains("8 tests: 8 passed, 0 failed"),
        "expected all 8 shadowing tests to pass, got:\n{stdout}"
    );
}

#[test]
fn bare_and_qualified_calls_to_a_shadowing_local_agree() {
    // The original bug: the same function called two ways gave two different
    // answers, because only the bare call fell through to the builtin.
    let run = run_program(
        "bare_vs_qualified",
        "Text",
        r#"module Text {
    public fn trim(s: String) -> String { "LOCAL" }
    public fn bare() -> String { trim("  hi  ") }
    public fn qualified() -> String { Text.trim("  hi  ") }
}
"#,
        r#"import Text

fn main() -> Unit with Console {
    println(Text.qualified())
    println(Text.bare())
}
"#,
    );
    assert!(
        run.success,
        "shadowing program failed to run:\n{}",
        run.output()
    );
    let stdout = run.stdout.clone();
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "expected two lines of output, got:\n{stdout}"
    );
    assert_eq!(
        lines[0], lines[1],
        "bare and qualified calls disagreed — the local was shadowed by a builtin:\n{stdout}"
    );
    assert!(
        lines[1].contains("LOCAL"),
        "bare call did not reach the local definition:\n{stdout}"
    );
}

#[test]
fn shadowing_a_routed_effect_builtin_drops_its_effect_requirement() {
    // `read_file` normally routes to `perform FileSystem.read_file`, so a
    // caller must declare `with FileSystem`. Once shadowed by a pure local,
    // that requirement must disappear — otherwise this fails to compile (E400)
    // rather than printing.
    let run = run_program(
        "effect_builtin",
        "Fs",
        r#"module Fs {
    public fn read_file(p: String) -> String { "LOCAL" }
    public fn pure_caller() -> String { read_file("/nonexistent/path") }
}
"#,
        r#"import Fs

fn main() -> Unit with Console {
    println(Fs.pure_caller())
}
"#,
    );
    assert!(
        run.success,
        "pure caller of a shadowed effectful builtin failed:\n{}",
        run.output()
    );
    assert!(
        run.stdout.contains("LOCAL"),
        "expected the local definition to run, got:\n{}",
        run.output()
    );
    assert!(
        !run.output().contains("E400"),
        "shadowed builtin still imposed its effect requirement:\n{}",
        run.output()
    );
}

#[test]
fn unshadowed_builtins_are_unaffected() {
    // The guard must be narrow: a module that does not define `trim` still
    // reaches the builtin, and effectful builtins keep their requirements.
    let run = run_program(
        "unaffected",
        "M",
        r#"module M {
    public fn use_builtin(s: String) -> String { trim(s) }
}
"#,
        r#"import M

fn main() -> Unit with Console {
    println(M.use_builtin("  padded  "))
}
"#,
    );
    assert!(
        run.success,
        "unshadowed builtin call failed:\n{}",
        run.output()
    );
    assert!(
        run.stdout.contains("padded") && !run.stdout.contains("  padded  "),
        "expected the builtin trim to run, got:\n{}",
        run.output()
    );
}

#[test]
fn effect_requirement_still_enforced_for_unshadowed_builtins() {
    // Narrowness in the other direction: an unshadowed `read_file` must still
    // demand `with FileSystem`.
    let run = run_program(
        "effect_still_required",
        "M",
        r#"module M {
    public fn reader(p: String) -> String { read_file(p) }
}
"#,
        r#"import M

fn main() -> Unit with Console {
    println(M.reader("x"))
}
"#,
    );
    assert!(
        !run.success,
        "an unshadowed read_file should still require `with FileSystem`, got:\n{}",
        run.output()
    );
    assert!(
        run.output().contains("E400") || run.output().contains("FileSystem"),
        "expected a missing-effect diagnostic, got:\n{}",
        run.output()
    );
}
