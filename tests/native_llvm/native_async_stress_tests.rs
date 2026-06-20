//! Native async stress/soak harness.
//!
//! The native twin of `tests/integration/async_stress.rs`. Native fibers use
//! real cross-worker stealing with a C effect-context snapshot per fiber, so
//! the migration/cancel paths are a *different* implementation from the VM's
//! and need their own stress coverage. Same contract, same fixtures, same
//! exact-total invariants — see the VM file's module doc for the rationale
//! (no panics ⇒ no leaked continuations; exact total ⇒ no lost/duplicated
//! completions; the only timing bound is a deadlock guard, not a margin).
//!
//!
//! ```sh
//! for i in $(seq 1 100); do \
//!   cargo test --features llvm --test native_async_stress_tests --quiet || break; \
//! done
//! ```

#![cfg(feature = "llvm")]

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Force migration + stealing on so native fibers actually move across workers.
const CHAOS_ENV: &[(&str, &str)] = &[("FLUX_FIBER_MIGRATION", "1"), ("FLUX_WORK_STEALING", "1")];
const REPEATS: usize = 3;

/// Hard wall-clock kill deadline per run (native links + runs, so it is roomier
/// than the VM's). A deadlock fails the test loudly instead of hanging CI.
const HANG_KILL: Duration = Duration::from_secs(90);

/// Spawn native `flux` on `source`, polling until exit or `HANG_KILL` (then
/// kill). Returns `(stdout, stderr, exited_ok, timed_out)`.
fn run_until_deadline(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, bool) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!("flux-async-stress-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux native");

    let start = Instant::now();
    let (mut exited_ok, mut timed_out) = (false, false);
    loop {
        match child.try_wait().expect("try_wait on flux child") {
            Some(status) => {
                exited_ok = status.success();
                break;
            }
            None => {
                if start.elapsed() > HANG_KILL {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let _ = std::fs::remove_file(&path);
    (
        stdout.replace("\r\n", "\n"),
        stderr.replace("\r\n", "\n"),
        exited_ok,
        timed_out,
    )
}
