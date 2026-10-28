use std::path::PathBuf;
use std::process::{Command, Output};

use crate::support::scratch;
use scratch::Scratch;
use scratch::workspace_root;

fn fixture_path(name: &str) -> PathBuf {
    workspace_root().join("tests").join("flux").join(name)
}

fn example_path(rel: &str) -> PathBuf {
    workspace_root().join("examples").join(rel)
}

fn run_flux(args: &[&str]) -> Output {
    // Private cache root: `--no-cache` does not isolate native builds, which
    // write shared artifacts under the cache root regardless (KI-010).
    let scratch = Scratch::new("test-runner-cli");
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(args)
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args))
}

fn run_flux_strict(args: &[&str]) -> Output {
    let mut full_args = vec!["--strict"];
    full_args.extend_from_slice(args);
    let scratch = Scratch::new("test-runner-cli");
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(&full_args)
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args))
}

#[cfg(feature = "llvm")]
fn cli_supports_flag(flag: &str) -> bool {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --help: {e}"));
    combined_output(&output).contains(flag)
}

fn combined_output(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[cfg(feature = "llvm")]
fn write_temp_flux_file(prefix: &str, source: &str) -> (Scratch, PathBuf) {
    let scratch = Scratch::new(prefix);
    let path = scratch.write(&format!("flux_{prefix}.flx"), source);
    (scratch, path)
}

#[allow(dead_code)]
fn extract_function_ir<'a>(llvm: &'a str, name: &str) -> &'a str {
    // Try both internal (single-module) and non-internal (per-module) linkage.
    let needle_internal = format!("define internal fastcc i64 @{name}");
    let needle_public = format!("define fastcc i64 @{name}");
    let start = llvm
        .find(&needle_internal)
        .or_else(|| llvm.find(&needle_public))
        .unwrap_or_else(|| panic!("expected function `{name}` in LLVM output:\n{llvm}"));
    let rest = &llvm[start..];
    if let Some(next_define) = rest[1..].find("\ndefine ") {
        &rest[..next_define + 1]
    } else {
        rest
    }
}

// ── `flux repl` — interactive session (proposal 0175, Phase 1) ───────────────
//
// The REPL prints evaluation results to stdout and prompts/errors to stderr, so
// stdout carries only the values an expression line produced.

/// Drive a scripted `flux repl` session: feed `script` on stdin and capture the
/// process output. `script` should end each line with `\n` and finish with
/// `:quit` (or rely on EOF).
fn run_flux_repl(script: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let scratch = Scratch::new("test-runner-repl");
    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .arg("repl")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `flux repl`");
    child
        .stdin
        .take()
        .expect("repl stdin")
        .write_all(script.as_bytes())
        .expect("write repl script");
    child.wait_with_output().expect("wait for `flux repl`")
}

/// The non-empty stdout lines of a REPL session (the printed results).
fn repl_results(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Assert that some printed (trimmed) stdout line of a REPL session equals
/// `expected`, with the full output in the failure message.
fn assert_repl_line(output: &Output, expected: &str) {
    let results = repl_results(output);
    assert!(
        results.iter().any(|line| line == expected),
        "expected a line `{expected}` in REPL stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Write `source` to a `.flx` file in its own scratch directory. The file
/// lives as long as the returned `Scratch`.
fn repl_temp_flx(tag: &str, source: &str) -> (Scratch, std::path::PathBuf) {
    let scratch = Scratch::new(&format!("repl-{tag}"));
    let path = scratch.write(&format!("flux_repl_{tag}.flx"), source);
    (scratch, path)
}

mod dumps;
mod examples_manifest;
mod repl_commands;
mod repl_session;
mod test_mode;
