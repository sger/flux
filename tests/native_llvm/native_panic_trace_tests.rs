//! Native panic traces (proposal 0188 S10a): every native build carries a
//! line table, and a program that dies of an error names the Flux frames it
//! was in, whether `flux` runs it or it runs on its own.

#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;
use scratch::workspace_root;

fn fixture(name: &str) -> String {
    format!("tests/native_llvm/panic_trace/{name}")
}

/// What `flux <fixture> --native <extra>` printed, stdout then stderr.
fn run_native(scratch: &Scratch, name: &str, extra: &[&str]) -> (String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .arg(fixture(name))
        .arg("--native")
        .args(extra)
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {name}: {e}"));
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (
        text.replace("\r\n", "\n"),
        output.status.code().unwrap_or(-1),
    )
}

/// Builds `name` into the scratch directory, without running it.
fn build_binary(scratch: &Scratch, name: &str) -> PathBuf {
    let binary = scratch.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });
    let (output, _) = run_native(
        scratch,
        name,
        &["--emit-binary", "-o", binary.to_str().unwrap()],
    );
    assert!(binary.exists(), "no binary built:\n{output}");
    binary
}

/// The lines of the `Stack trace:` section in `output`.
fn trace(output: &str) -> Vec<String> {
    output
        .lines()
        .skip_while(|line| line.trim() != "Stack trace:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .map(|line| line.trim().to_string())
        .collect()
}

const CALLS: [&str; 5] = [
    "at c (tests/native_llvm/panic_trace/calls.flx:3:16)",
    "at b (tests/native_llvm/panic_trace/calls.flx:7:5)",
    "at a (tests/native_llvm/panic_trace/calls.flx:11:5)",
    "at main (tests/native_llvm/panic_trace/calls.flx:15:21)",
    "at <main>",
];

#[test]
fn a_native_panic_names_its_frames() {
    let scratch = Scratch::new("panic-trace");
    let (output, code) = run_native(&scratch, "calls.flx", &[]);
    assert_eq!(code, 1, "{output}");
    assert!(output.contains("error[E1009]: too big"), "{output}");
    // The error points where the panic was, from the innermost frame.
    assert!(
        output.contains("tests/native_llvm/panic_trace/calls.flx:3:16"),
        "{output}"
    );
    assert_eq!(trace(&output), CALLS, "{output}");
}

#[test]
fn a_debug_build_names_the_same_frames() {
    let scratch = Scratch::new("panic-trace-g");
    let (output, _) = run_native(&scratch, "calls.flx", &["-g"]);
    assert_eq!(trace(&output), CALLS, "{output}");
}

#[test]
fn a_program_run_on_its_own_prints_its_trace() {
    let scratch = Scratch::new("panic-trace-binary");
    let binary = build_binary(&scratch, "calls.flx");
    let output = Command::new(&binary).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert!(stderr.starts_with("panic: too big\n"), "{stderr}");
    // The line table names the source by its full path.
    let frames: Vec<String> = trace(&stderr)
        .iter()
        .map(|line| match line.split_once(" (") {
            Some((name, place)) => {
                let file = place.rsplit(['/', '\\']).next().unwrap_or(place);
                format!("{name} (tests/native_llvm/panic_trace/{file}")
            }
            None => line.clone(),
        })
        .collect();
    assert_eq!(frames, CALLS, "{stderr}");
}

#[test]
fn a_deep_trace_keeps_both_ends() {
    let scratch = Scratch::new("panic-trace-deep");
    let (output, _) = run_native(&scratch, "recursion.flx", &[]);
    let frames = trace(&output);
    // 101 frames of `down`, `main` and `<main>`: the first 20 and the last
    // 10, as the VM keeps them.
    assert_eq!(frames.len(), 31, "{output}");
    assert_eq!(
        frames[0],
        "at down (tests/native_llvm/panic_trace/recursion.flx:3:17)"
    );
    assert_eq!(frames[20], "... 73 more frames ...");
    assert_eq!(
        frames[29],
        "at main (tests/native_llvm/panic_trace/recursion.flx:7:21)"
    );
    assert_eq!(frames[30], "at <main>");
}

#[test]
fn a_caught_panic_prints_no_trace() {
    let scratch = Scratch::new("panic-trace-caught");
    let (output, code) = run_native(&scratch, "caught.flx", &[]);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("still running"), "{output}");
    assert!(!output.contains("Stack trace:"), "{output}");
}
