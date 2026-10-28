//! Shared helpers for integration tests that run the `flux` CLI on a
//! program: inline source, a `tests/parity` fixture or a `tests/flux` test
//! file, on the VM or natively. Include from a test file with:
//!
//! ```rust
//! #[path = "../support/flux_runner.rs"]
//! mod flux_runner;
//! use flux_runner::{run_flux, run_native, run_vm};
//! ```
//!
//! The `run_*` functions cover the common cases. `flux_source` and
//! `flux_file` build a run with any mix of options.
#![allow(dead_code)]

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Re-exported so a test including this file gets `Scratch` from here rather
// than declaring its own `mod scratch;` — two `#[path]` declarations of one
// file in the same crate is a `clippy::duplicate_mod` warning.
#[path = "scratch.rs"]
pub mod scratch;
use scratch::Scratch;

#[allow(unused_imports)]
pub use scratch::workspace_root;

enum Program<'a> {
    Source(&'a str),
    File(PathBuf),
}

/// One `flux --no-cache` run, built up with options. Every run gets its own
/// `Scratch`: `--no-cache` alone does not isolate, because the native backend
/// writes shared artifacts under the cache root regardless (KI-010).
pub struct FluxRun<'a> {
    program: Program<'a>,
    tag: &'a str,
    args: Vec<&'a str>,
    env: Vec<(&'a str, &'a str)>,
    native: bool,
    timeout: Option<Duration>,
}

/// What a run printed, with `\r\n` turned into `\n`. `success` is false
/// when the run timed out.
pub struct Ran {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub timed_out: bool,
    pub elapsed: Duration,
}

/// A run of `source`, written to a file in the run's scratch directory.
/// `tag` names the directory, to tell runs apart when debugging.
pub fn flux_source<'a>(source: &'a str, tag: &'a str) -> FluxRun<'a> {
    FluxRun::new(Program::Source(source), tag)
}

/// A run of the file at `rel`, relative to the workspace root.
pub fn flux_file(rel: &str) -> FluxRun<'_> {
    FluxRun::new(Program::File(workspace_root().join(rel)), rel)
}

impl<'a> FluxRun<'a> {
    fn new(program: Program<'a>, tag: &'a str) -> Self {
        Self { program, tag, args: Vec::new(), env: Vec::new(), native: false, timeout: None }
    }

    pub fn native(mut self) -> Self {
        self.native = true;
        self
    }

    pub fn args(mut self, args: &[&'a str]) -> Self {
        self.args.extend_from_slice(args);
        self
    }

    pub fn env(mut self, key: &'a str, value: &'a str) -> Self {
        self.env.push((key, value));
        self
    }

    pub fn envs(mut self, env: &[(&'a str, &'a str)]) -> Self {
        self.env.extend_from_slice(env);
        self
    }

    /// Kill the run after `timeout`, so a deadlock fails the test instead of
    /// hanging it.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn run(self) -> Ran {
        let label: String =
            self.tag.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let scratch = Scratch::new(&format!("flux-runner-{label}"));
        let path = match self.program {
            Program::Source(source) => scratch.write("fixture.flx", source),
            Program::File(path) => path,
        };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_flux"));
        cmd.current_dir(workspace_root())
            .arg(&path)
            .args(&self.args)
            .args(self.native.then_some("--native"))
            .arg("--no-cache")
            .args(scratch.cache_args())
            .envs(self.env.iter().copied());

        let start = Instant::now();
        let (stdout, stderr, success, timed_out) = match self.timeout {
            None => {
                let output = cmd
                    .output()
                    .unwrap_or_else(|e| panic!("failed to run flux on {}: {e}", self.tag));
                (output.stdout, output.stderr, output.status.success(), false)
            }
            Some(timeout) => run_with_timeout(&mut cmd, timeout, self.tag),
        };
        Ran {
            stdout: String::from_utf8_lossy(&stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&stderr).replace("\r\n", "\n"),
            success,
            timed_out,
            elapsed: start.elapsed(),
        }
    }
}

/// Runs `cmd`, killing it once `timeout` has passed. The pipes are drained on
/// their own threads, so a chatty child cannot block on a full pipe.
fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    tag: &str,
) -> (Vec<u8>, Vec<u8>, bool, bool) {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to run flux on {tag}: {e}"));
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("wait for flux") {
            break (Some(status), false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let success = status.is_some_and(|s| s.success());
    (stdout.join().unwrap(), stderr.join().unwrap(), success, timed_out)
}

/// Run `source` on the VM and return `(stdout, stderr, success)`. `tag`
/// tells runs apart when several tests in one binary run at once.
pub fn run_flux(source: &str, tag: &str) -> (String, String, bool) {
    let (stdout, stderr, success, _) = run_flux_timed(source, tag);
    (stdout, stderr, success)
}

/// Same as `run_flux` but also returns the wall-clock elapsed time from
/// process spawn to exit.
pub fn run_flux_timed(source: &str, tag: &str) -> (String, String, bool, Duration) {
    run_flux_with_env(source, tag, &[])
}

/// Like `run_flux_timed` but also sets additional environment variables on
/// the child process before running.
pub fn run_flux_with_env(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, Duration) {
    let ran = flux_source(source, tag).envs(env).run();
    (ran.stdout, ran.stderr, ran.success, ran.elapsed)
}

/// The time a fixture printed as `elapsed_ms=<n>` around its `run_async`,
/// and its output without that line. A timed run's wall clock also counts
/// the compile, which on a loaded machine can take longer than the sleeps
/// a test compares.
pub fn program_elapsed(stdout: &str) -> (String, Duration) {
    let mut elapsed = None;
    let mut rest = String::new();
    for line in stdout.lines() {
        match line.trim_matches('"').strip_prefix("elapsed_ms=") {
            Some(ms) => elapsed = ms.parse().ok().map(Duration::from_millis),
            None => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    let elapsed = elapsed.unwrap_or_else(|| panic!("no elapsed_ms= line in:\n{stdout}"));
    (rest, elapsed)
}

/// Run `source` on the default **cached** path, with an isolated cache root.
///
/// The other helpers pass `--no-cache`, which bypasses the parallel module
/// linker entirely. A bug in how imported instance methods are linked is
/// therefore invisible to them — that is how KI-051 survived: `--no-cache`
/// printed the right answer while the default cached run failed with
/// `missing imported global`.
///
/// Runs twice against the same cache root so both the cold (write) and warm
/// (replay) paths are exercised; a stale artifact replaying its own binding
/// classification is its own failure mode.
pub fn run_flux_cached(source: &str, tag: &str) -> (String, String, bool) {
    let scratch = Scratch::new(&format!("flux-runner-cached-{tag}"));
    let path = scratch.write("fixture.flx", source);

    let mut last = None;
    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .arg(path.to_str().unwrap())
            .args(scratch.cache_args())
            .output()
            .expect("run flux");
        last = Some((
            String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
            output.status.success(),
        ));
    }
    last.expect("at least one run")
}

/// Run `tests/parity/<fixture>` on the VM. Returns trimmed stdout.
pub fn run_vm(fixture: &str) -> String {
    let rel = format!("tests/parity/{fixture}");
    flux_file(&rel).run().stdout.trim().to_string()
}

/// Run `tests/parity/<fixture>` natively. Returns trimmed stdout and
/// success; on failure the stderr is added to the text, so an assert on
/// success shows why.
pub fn run_native(fixture: &str) -> (String, bool) {
    run_native_with_env(fixture, &[])
}

/// `run_native` with extra environment variables.
pub fn run_native_with_env(fixture: &str, env: &[(&str, &str)]) -> (String, bool) {
    let rel = format!("tests/parity/{fixture}");
    let ran = flux_file(&rel).native().envs(env).run();
    let stdout = ran.stdout.trim().to_string();
    if ran.success {
        return (stdout, true);
    }
    let stderr = ran.stderr.trim();
    if stderr.is_empty() {
        (stdout, false)
    } else if stdout.is_empty() {
        (stderr.to_string(), false)
    } else {
        (format!("{stdout}\n── stderr ──\n{stderr}"), false)
    }
}

/// Run `flux --test tests/flux/<fixture>`. Returns trimmed stdout and success.
pub fn run_flux_test(fixture: &str) -> (String, bool) {
    run_flux_test_with(fixture, &[], &[])
}

/// `run_flux_test` with extra arguments and environment variables.
pub fn run_flux_test_with(fixture: &str, args: &[&str], env: &[(&str, &str)]) -> (String, bool) {
    let rel = format!("tests/flux/{fixture}");
    let ran = flux_file(&rel).args(&["--test"]).args(args).envs(env).run();
    (ran.stdout.trim().to_string(), ran.success)
}
