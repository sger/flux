//! Integration tests for `Flow.Env` (proposal 0178, item 5).
//!
//! Two things cannot be asserted from inside the Flux fixture and are covered
//! here: that `Env` is a real effect the checker enforces, and that program
//! arguments actually arrive — which needs the CLI to be invoked with a `--`
//! separator, something a `--test` run cannot do to itself.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::scratch::Scratch;

use stdlib_fixture::assert_fixture_passes;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Write a fixture into a scratch dir unique to this process, and return both
/// the path and the guard that removes the dir on drop. The name alone is not
/// unique: concurrent test binaries writing the same literal filename into one
/// shared directory clobbered each other (KI-010 in docs/known_issues.md).
fn scratch_file(name: &str, source: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(name.trim_end_matches(".flx"));
    let file = scratch.write(name, source);
    (scratch, file)
}

/// Run a program with extra arguments after `--`.
fn run_with_args(file: &Path, scratch: &Scratch, program_args: &[&str]) -> (String, String, bool) {
    let cache_args = scratch.cache_args();
    let mut args = vec!["run", file.to_str().unwrap(), "--no-cache"];
    // Cache options must precede the argument separator; everything after it
    // is intentionally forwarded to the Flux program as user argv (KI-010).
    args.extend(cache_args.iter().map(String::as_str));
    if !program_args.is_empty() {
        args.push("--");
        args.extend_from_slice(program_args);
    }
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        .output()
        .expect("run flux");
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

const ARGV_PROGRAM: &str = r#"
import Flow.Env as Env
import Flow.String as Str

fn main() -> Unit with Env, Console {
    print("argc=" + to_string(len(Env.args())))
    print("argv=" + Str.join(Env.args(), "|"))
}
"#;

#[test]
fn stdlib_env_fixture_passes_on_the_vm() {
    assert_fixture_passes("stdlib_env.flx");
}

/// With no `--`, the program still sees its own path as argv[0].
#[test]
fn args_contains_only_the_program_path_when_none_are_passed() {
    let (guard, file) = scratch_file("env_argv_none.flx", ARGV_PROGRAM);
    let (stdout, stderr, success) = run_with_args(&file, &guard, &[]);
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("argc=1"), "got:\n{stdout}");
    assert!(
        stdout.contains("env_argv_none.flx"),
        "argv[0] should be the script path:\n{stdout}"
    );
}

/// Arguments after `--` reach the program, in order, after the script path.
#[test]
fn arguments_after_the_separator_reach_the_program() {
    let (guard, file) = scratch_file("env_argv_some.flx", ARGV_PROGRAM);
    let (stdout, stderr, success) = run_with_args(&file, &guard, &["build", "release"]);
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("argc=3"), "got:\n{stdout}");
    assert!(stdout.contains("|build|release"), "got:\n{stdout}");
}

/// The point of the `--` separator: a program may take flags that `flux`
/// itself would otherwise claim, or reject as unknown.
#[test]
fn a_program_can_receive_flags_that_flux_would_otherwise_reject() {
    let (guard, file) = scratch_file("env_argv_flags.flx", ARGV_PROGRAM);
    let (stdout, stderr, success) = run_with_args(&file, &guard, &["--native", "--verbose"]);
    assert!(
        success,
        "flags after `--` must not be parsed by flux:\n{stdout}\n{stderr}"
    );
    assert!(stdout.contains("argc=3"), "got:\n{stdout}");
    assert!(stdout.contains("|--native|--verbose"), "got:\n{stdout}");
}

/// Reading the environment is a capability, so it must be declared.
#[test]
fn reading_the_environment_requires_the_env_effect() {
    let (guard, file) = scratch_file(
        "env_effect.flx",
        r#"
import Flow.Env as Env

fn sneaky() -> Bool {
    Env.has_var("PATH")
}

fn main() -> Unit with Console {
    print(to_string(sneaky()))
}
"#,
    );
    let (stdout, stderr, success) = run_with_args(&file, &guard, &[]);
    assert!(
        !success,
        "an undeclared Env effect must be rejected:\n{stdout}"
    );
    assert!(
        format!("{stdout}{stderr}").contains("Env"),
        "the diagnostic should name the Env effect:\n{stdout}{stderr}"
    );
}

/// `Env` coarsens to `IO`, so a function declaring `with IO` may read the
/// environment without naming `Env` separately.
#[test]
fn the_io_alias_covers_the_env_effect() {
    let (guard, file) = scratch_file(
        "env_io_alias.flx",
        r#"
import Flow.Env as Env

fn main() -> Unit with IO {
    print(to_string(Env.has_var("PATH")))
}
"#,
    );
    let (stdout, stderr, success) = run_with_args(&file, &guard, &[]);
    assert!(success, "IO must cover Env:\n{stdout}\n{stderr}");
    assert!(stdout.contains("true"), "got:\n{stdout}");
}
