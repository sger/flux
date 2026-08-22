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

#[allow(dead_code)]
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run one fixture through `flux --test`, optionally on the native backend.
/// Returns `(stdout, success)`.
#[allow(dead_code)]
pub fn run_fixture(fixture: &str, native: bool) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let mut args = vec!["--test", path.to_str().unwrap(), "--no-cache"];
    if native {
        args.push("--native");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
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
#[allow(dead_code)]
pub fn assert_backends_agree(fixture: &str) {
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
