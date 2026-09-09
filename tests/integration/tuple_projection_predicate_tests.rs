//! Tuple projection is a solver predicate, not a guessed tuple shape
//! (proposal 0185 stage 5).
//!
//! These run the compiler as a subprocess rather than calling `infer_program`
//! directly. The predicate lives in the reserved `__field` module, whose name is
//! interned by `register_prelude_classes` — so a unit harness that builds no
//! class environment never raises one, and an assertion written there would
//! pass whatever the compiler did. That is not a hypothetical: the first
//! version of these two tests was written against that harness, where the
//! cascade assertion held vacuously.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn compile_output(label: &str, source: &str) -> String {
    let scratch = Scratch::new(label);
    let program = scratch.write("main.flx", source);
    let output = Command::new(Path::new(env!("CARGO_BIN_EXE_flux")))
        .arg(&program)
        .arg("--no-cache")
        .output()
        .expect("run flux");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A receiver no call site ever determines is reported, rather than silently
/// acquiring the width the projection happened to imply.
#[test]
fn an_undetermined_tuple_receiver_is_reported() {
    let out = compile_output(
        "e491-undetermined",
        "fn fst(t) { t.0 }\n\nfn main() with IO { print(\"unused helper\") }\n",
    );
    assert!(
        out.contains("E491"),
        "expected E491 for a receiver nothing determines, got:\n{out}"
    );
}

/// A projection on an *undefined* name reports only that the name is undefined.
/// The receiver is unknown because of an error already reported, so a second
/// diagnostic saying the projection cannot be resolved adds nothing — the guard
/// the field predicate already had, and the tuple predicate initially lacked.
#[test]
fn a_projection_on_an_undefined_name_does_not_cascade() {
    let out = compile_output(
        "e491-cascade",
        "fn main() -> Unit {\n    let value: Int = mystery.0\n}\n",
    );
    assert!(
        out.contains("E004"),
        "expected E004 for the undefined name, got:\n{out}"
    );
    assert!(
        !out.contains("E491"),
        "an undefined receiver is already reported as E004, got:\n{out}"
    );
}

/// A projection past the last element is rejected. The guessed shape used to
/// accept it by making the receiver wider.
#[test]
fn a_projection_past_the_end_of_a_tuple_is_rejected() {
    let out = compile_output(
        "e492-out-of-range",
        "fn third(t) { t.2 }\n\nfn main() with IO { print(third((1, 2))) }\n",
    );
    assert!(
        out.contains("E492") || out.contains("E491"),
        "expected the projection to be rejected, got:\n{out}"
    );
}
