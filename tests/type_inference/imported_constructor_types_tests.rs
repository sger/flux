//! A constructor imported from another module must infer as its ADT (KI-014).
//!
//! Constructor applications route through `adt_constructor_types`, which was
//! populated only from local `data` statements. An imported constructor missed
//! that lookup, fell through to the ordinary function-call path, and inferred
//! as an unconstrained type variable — so class dispatch, which keys on the
//! argument's type, had nothing to select on and panicked at runtime.
//!
//! Module interfaces now carry constructor field types, which seed inference on
//! import. The fixture exercises every constructor shape (positional, nullary,
//! named-field, generic) because they take different inference paths.
//!
//! It also covers a field whose declared type is a transparent alias. Field
//! types are collected from the raw AST, which still names the alias, while
//! schemes are built after expansion — so an unexpanded export made an importer
//! see `Bytes` where inference produced `String`, breaking `Flow.Http`.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

const FIXTURE: &str = "imported_constructor_types.flx";

/// What the fixture must print, in order. Before the fix the run aborted at the
/// first imported-constructor dispatch, having printed only the local one.
const EXPECTED: &[&str] = &[
    "int 7", "shape 12", "shape 0", "rect 12", "5", "tag!", "shape 3",
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
    // Private cache root: `--no-cache` does not isolate native builds, which
    // write shared artifacts under the cache root regardless (KI-010).
    let scratch = Scratch::new("fixture-run");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        .args(scratch.cache_args())
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
fn an_imported_constructor_dispatches_on_its_own_type() {
    let (stdout, success) = run(false);
    assert!(success, "the fixture must run to completion:\n{stdout}");
    assert_eq!(
        printed_values(&stdout),
        EXPECTED,
        "unexpected output:\n{stdout}"
    );
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
        "backends disagree:\nVM:\n{vm_stdout}\n\nnative:\n{native_stdout}"
    );
}
