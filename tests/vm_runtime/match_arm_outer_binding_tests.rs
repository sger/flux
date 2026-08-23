//! A binding declared outside a `match` and read inside an arm must survive the
//! match (KI-001).
//!
//! The VM compiles a read that is an arm's only use as `OpConsumeLocal`, which
//! *moves* the value out of its stack slot and leaves the `Uninit` sentinel
//! behind. That is right when the arm body is genuinely the binding's last use,
//! and wrong when it is read again after the match — which the arm alone cannot
//! decide, because it is one branch of the function rather than the whole of it.
//!
//! `compile_match` merged each arm body's use counts into the enclosing map with
//! `or_insert`. For the arm's own pattern bindings that is correct. For a
//! binding declared outside, the arm-body count of 1 became the *whole-function*
//! count whenever the outer map had no entry — and the later read then found an
//! emptied slot.
//!
//! The failure was silent for `Int` (`<uninit>` printed where a number belonged)
//! and loud for `String` (`E1009: Cannot add String and Uninit`). The LLVM
//! backend was always correct, so this was also a parity divergence — hence the
//! parity assertion below rather than a VM-only check.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "match_arm_outer_binding.flx";

/// What the fixture must print, in order. Every pair is a read inside an arm
/// followed by a read after the match; before the fix the second of each pair
/// was `<uninit>` or aborted the run.
const EXPECTED: &[&str] = &[
    "42",
    "42",
    "hello",
    "hello again",
    "7",
    "7",
    "9",
    "9",
    "found work",
    "cleaning work",
    "5",
    "ab",
];

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run(native: bool) -> (String, bool) {
    let path = workspace_root().join("tests").join("parity").join(FIXTURE);
    let mut args: Vec<String> = vec![
        path.to_string_lossy().into_owned(),
        "--no-cache".to_string(),
    ];
    if native {
        args.push("--native".to_string());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {FIXTURE}: {e}"));

    // Stderr is folded in on failure so an abort shows its diagnostic rather
    // than surfacing as unexplained empty output.
    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    if output.status.success() {
        return (stdout, true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (format!("{stdout}\n── stderr ──\n{stderr}"), false)
}

/// Printed values, unquoted, in order.
fn printed_values(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|line| line.trim().trim_matches('"').to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn an_outer_binding_survives_being_read_inside_a_match_arm_on_the_vm() {
    let (stdout, success) = run(false);
    assert!(success, "the fixture must run to completion:\n{stdout}");

    let values = printed_values(&stdout);
    assert!(
        !values.iter().any(|value| value.contains("uninit")),
        "a consumed binding leaked the VM's `Uninit` sentinel, which must never \
         be observable at language level:\n{stdout}"
    );
    assert_eq!(values, EXPECTED, "unexpected output:\n{stdout}");
}

#[cfg(feature = "llvm")]
#[test]
fn the_vm_and_native_backends_agree() {
    let (vm_stdout, vm_success) = run(false);
    assert!(vm_success, "VM run failed:\n{vm_stdout}");
    let (native_stdout, native_success) = run(true);
    assert!(native_success, "native run failed:\n{native_stdout}");

    assert_eq!(
        printed_values(&vm_stdout),
        printed_values(&native_stdout),
        "backends disagree — KI-001 was a parity divergence, and this is the \
         assertion that catches a recurrence:\nVM:\n{vm_stdout}\nnative:\n{native_stdout}"
    );
}
