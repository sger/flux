//! Integration tests for `Flow.Process` (proposal 0178, item 6).
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. What is asserted here is what the fixture cannot check about
//! itself: that `Process` is a real effect the checker enforces, and that it
//! is distinct from `FileSystem` — the property that makes the label worth
//! having at all.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::{assert_backends_agree, assert_fixture_passes};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn scratch_file(name: &str, source: &str) -> PathBuf {
    let dir = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join(name);
    std::fs::write(&file, source).expect("write scratch fixture");
    file
}

fn compile(file: &Path) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux");
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_process_fixture_passes_on_the_vm() {
    assert_fixture_passes("stdlib_process.flx");
}

#[test]
fn stdlib_process_agrees_across_backends() {
    assert_backends_agree("stdlib_process.flx");
}

/// Running a subprocess is a capability, so it must be declared.
#[test]
fn running_a_subprocess_requires_the_process_effect() {
    let file = scratch_file(
        "proc_effect.flx",
        r#"
import Flow.Process as Proc
import Flow.List as List

fn sneaky() -> Bool {
    match Proc.run("true", List.to_array([])) {
        Ok(_) -> true,
        Err(_) -> false,
    }
}

fn main() -> Unit with Console {
    print(to_string(sneaky()))
}
"#,
    );
    let (stdout, stderr, success) = compile(&file);
    let _ = std::fs::remove_file(&file);
    assert!(
        !success,
        "an undeclared Process effect must be rejected:\n{stdout}"
    );
    assert!(
        format!("{stdout}{stderr}").contains("Process"),
        "the diagnostic should name the Process effect:\n{stdout}{stderr}"
    );
}

/// The whole point of a distinct label: `FileSystem` must not authorise
/// spawning a process. If this ever passes, `Process` has silently collapsed
/// into another capability and signatures no longer mean what they say.
#[test]
fn the_filesystem_effect_does_not_authorise_subprocesses() {
    let file = scratch_file(
        "proc_not_fs.flx",
        r#"
import Flow.Process as Proc
import Flow.List as List

fn run_it() -> Bool with FileSystem {
    match Proc.run("true", List.to_array([])) {
        Ok(_) -> true,
        Err(_) -> false,
    }
}

fn main() -> Unit with IO {
    print(to_string(run_it()))
}
"#,
    );
    let (stdout, stderr, success) = compile(&file);
    let _ = std::fs::remove_file(&file);
    assert!(
        !success,
        "FileSystem must not cover Process:\n{stdout}{stderr}"
    );
    assert!(
        format!("{stdout}{stderr}").contains("Process"),
        "the diagnostic should name the Process effect:\n{stdout}{stderr}"
    );
}

/// `Process` coarsens to `IO`, so a function declaring `with IO` may spawn a
/// subprocess without naming `Process` separately.
#[test]
fn the_io_alias_covers_the_process_effect() {
    let file = scratch_file(
        "proc_io_alias.flx",
        r#"
import Flow.Process as Proc
import Flow.List as List

fn main() -> Unit with IO {
    match Proc.run("/bin/echo", List.to_array(["covered"])) {
        Ok(out) -> print(Proc.stdout_of(out)),
        Err(_) -> print("failed"),
    }
}
"#,
    );
    let (stdout, stderr, success) = compile(&file);
    let _ = std::fs::remove_file(&file);
    assert!(success, "IO must cover Process:\n{stdout}\n{stderr}");
    assert!(stdout.contains("covered"), "got:\n{stdout}");
}
