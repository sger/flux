//! Shared driver for the `Flow.*` standard-library fixtures in `tests/flux/`.
//!
//! Each stdlib module has a `tests/flux/stdlib_<name>.flx` fixture holding its
//! behavioural coverage, and a thin Rust target that runs it. Include from a
//! test file with:
//!
//! ```rust
//! #[path = "../support/stdlib_fixture.rs"]
//! mod stdlib_fixture;
//! use stdlib_fixture::{assert_fixture_passes, assert_backends_agree};
//! ```
//!
//! Two things are asserted per module. `assert_fixture_passes` runs the
//! fixture on the VM and requires every test in it to pass. `assert_backends_agree`
//! runs it again natively and requires the same tests to pass there — backend
//! parity is the main risk called out by proposal 0178, and the `Flow.Fs` work
//! surfaced a real divergence this way that no VM-only test could have caught.

use std::path::Path;
use std::process::Command;

// Re-exported so a test including this file gets `Scratch` from here rather
// than declaring its own `mod scratch;` — two `#[path]` declarations of one
// file in the same crate is a `clippy::duplicate_mod` warning.
#[path = "scratch.rs"]
pub mod scratch;
use scratch::Scratch;

#[allow(dead_code)]
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run one fixture through `flux --test`, optionally on the native backend.
/// Returns `(stdout, success)`. Stderr is folded into the returned text when
/// the run fails, so a compile or link error is visible in the panic message
/// rather than surfacing as an unexplained empty summary.
///
/// Each call gets its own cache directory. `--no-cache` alone is not enough
/// for `--native`: the native backend also writes shared build artifacts under
/// the cache root, and concurrent test binaries were clobbering each other's
/// (KI-010 in `docs/known_issues.md`).
#[allow(dead_code)]
pub fn run_fixture(fixture: &str, native: bool) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let scratch = Scratch::new(&format!(
        "fixture-{}{}",
        fixture.trim_end_matches(".flx"),
        if native { "-native" } else { "" }
    ));
    let cache = scratch.cache_dir();

    let mut args: Vec<String> = vec![
        "--test".to_string(),
        path.to_string_lossy().into_owned(),
        "--no-cache".to_string(),
        "--cache-dir".to_string(),
        cache.to_string_lossy().into_owned(),
    ];
    if native {
        args.push("--native".to_string());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        // `stdlib_process.flx` spawns this rather than a system command, so
        // its expectations hold on every host. Passed by path because only
        // cargo knows where it was built; the native leg inherits it through
        // the compiled fixture's own environment.
        .env("FLUX_PROC_HELPER", env!("CARGO_BIN_EXE_flux_proc_helper"))
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    let success = output.status.success();
    if success {
        return (stdout, true);
    }
    // The failure detail is almost always on stderr; without it the caller can
    // only report "no summary", which says nothing about what went wrong.
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    let combined = if stderr.is_empty() {
        stdout
    } else {
        format!("{stdout}\n── stderr ──\n{stderr}")
    };
    (combined, false)
}

/// Parse the runner's trailing `"N tests: N passed, M failed"` line.
/// Returns `(passed, failed)`.
#[allow(dead_code)]
pub fn parse_summary(stdout: &str) -> Option<(u32, u32)> {
    let line = stdout.lines().rev().find(|l| l.contains(" tests: "))?;
    let (_, rest) = line.split_once(" tests: ")?;
    let (passed, rest) = rest.split_once(" passed, ")?;
    let (failed, _) = rest.split_once(" failed")?;
    Some((passed.trim().parse().ok()?, failed.trim().parse().ok()?))
}

/// Run `fixture` on the VM and require every test in it to pass.
///
/// Asserts a non-zero test count as well, so a fixture that silently stops
/// being discovered fails loudly instead of vacuously passing.
#[allow(dead_code)]
pub fn assert_fixture_passes(fixture: &str) {
    let (stdout, success) = run_fixture(fixture, false);
    let summary = parse_summary(&stdout);
    assert!(success, "{fixture} failed on the VM:\n{stdout}");
    match summary {
        Some((passed, failed)) => {
            assert_eq!(
                failed, 0,
                "{fixture} had {failed} failing test(s):\n{stdout}"
            );
            assert!(passed > 0, "{fixture} ran no tests at all:\n{stdout}");
        }
        None => panic!("could not parse a test summary from {fixture}:\n{stdout}"),
    }
}

/// Run `fixture` on both backends and require identical pass/fail counts.
///
/// The counts are compared rather than raw stdout because the runner prints
/// per-test timings, which legitimately differ between backends.
///
/// Skips when the `llvm` feature is off: `--native` is rejected at flag
/// validation there, so the run produces no summary to compare and the
/// assertion would fail for a reason unrelated to the fixture.
#[allow(dead_code)]
pub fn assert_backends_agree(fixture: &str) {
    if !cfg!(feature = "llvm") {
        eprintln!("skipping native parity for {fixture}: built without the `llvm` feature");
        return;
    }
    let (vm_out, vm_ok) = run_fixture(fixture, false);
    let (native_out, native_ok) = run_fixture(fixture, true);

    let vm =
        parse_summary(&vm_out).unwrap_or_else(|| panic!("no VM summary for {fixture}:\n{vm_out}"));
    let native = parse_summary(&native_out)
        .unwrap_or_else(|| panic!("no native summary for {fixture}:\n{native_out}"));

    assert_eq!(
        vm, native,
        "{fixture} disagrees between backends: VM {vm:?} vs native {native:?}\n\
         ── VM ──\n{vm_out}\n── native ──\n{native_out}"
    );
    assert!(
        vm_ok && native_ok,
        "{fixture} must pass on both backends (vm_ok={vm_ok}, native_ok={native_ok})"
    );
}
