//! Shared helpers for integration tests that invoke the Flux CLI on inline
//! source fixtures.  Include from a test file with:
//!
//! ```rust
//! #[path = "../support/flux_runner.rs"]
//! mod flux_runner;
//! use flux_runner::{run_flux, run_flux_timed, run_flux_with_env};
//! ```

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Write `source` to a uniquely-named temp file, run `flux --no-cache` on
/// it, delete the file, and return `(stdout, stderr, success)`.
///
/// `tag` is used to distinguish temp-dir names when multiple tests run in
/// parallel within the same binary.
#[allow(dead_code)]
pub fn run_flux(source: &str, tag: &str) -> (String, String, bool) {
    let (stdout, stderr, success, _) = run_flux_timed(source, tag);
    (stdout, stderr, success)
}

/// Same as `run_flux` but also returns the wall-clock elapsed time from
/// process spawn to exit.  Use this only when the test needs to verify
/// timing (e.g. proving that a sleep or cancellation fired on time).
#[allow(dead_code)]
pub fn run_flux_timed(source: &str, tag: &str) -> (String, String, bool, Duration) {
    run_flux_with_env(source, tag, &[])
}

/// Like `run_flux_timed` but also sets additional environment variables on
/// the child process before running.
#[allow(dead_code)]
pub fn run_flux_with_env(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, Duration) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!(
            "flux-runner-{}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test"),
            tag,
        ));
    std::fs::create_dir_all(&dir).expect("create temp dir for flux fixture");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write flux fixture");

    let start = Instant::now();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flux"));
    cmd.current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run flux");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}
