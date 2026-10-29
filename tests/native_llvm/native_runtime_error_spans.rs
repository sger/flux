#![cfg(feature = "llvm")]

use crate::support::flux_runner;

/// What the native run of `examples/runtime_errors/<fixture>` printed,
/// stdout then stderr.
fn run_native(fixture: &str) -> String {
    let rel = format!("examples/runtime_errors/{fixture}");
    let ran = flux_runner::flux_file(&rel).native().env("NO_COLOR", "1").run();
    ran.stdout + &ran.stderr
}

#[test]
fn native_division_by_zero_reports_precise_span() {
    let output = run_native("division_by_zero_int.flx");
    assert!(
        output.contains("examples/runtime_errors/division_by_zero_int.flx:2:19"),
        "expected native runtime error to report precise divide span, got:\n{output}"
    );
    // The program's own trace, from its line table (0188 S10a).
    assert!(
        output.contains(
            "Stack trace:\n  at main (examples/runtime_errors/division_by_zero_int.flx:2:19)\n  at <main>"
        ),
        "expected the native frames, got:\n{output}"
    );
}

#[test]
fn native_modulo_by_zero_reports_precise_span() {
    let output = run_native("division_by_zero_modulo.flx");
    assert!(
        output.contains("examples/runtime_errors/division_by_zero_modulo.flx:2:19"),
        "expected native runtime error to report precise modulo span, got:\n{output}"
    );
    // The program's own trace, from its line table (0188 S10a).
    assert!(
        output.contains(
            "Stack trace:\n  at main (examples/runtime_errors/division_by_zero_modulo.flx:2:19)\n  at <main>"
        ),
        "expected the native frames, got:\n{output}"
    );
}
