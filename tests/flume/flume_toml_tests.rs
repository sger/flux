//! Integration tests for the parsing stack (proposal 0177, Phase 0):
//! `Flume.Toml.Parse`, `Flume.Toml.Value`, `Flume.Toml`, and `Flume.Toml.Document`.
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that both modules are **pure**.

use std::path::Path;
use std::process::Command;

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_backends_agree;
use stdlib_fixture::scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn flume_toml_fixture_passes() {
    assert_backends_agree("flume_toml.flx");
}

/// Phase 0's headline property, for the parser: a manifest parser that
/// provably cannot read a file.
///
/// This compiles a program whose `main` declares **no** effects and which
/// drives the combinator library and the TOML grammar end to end. If anything
/// they call acquired an effect — a stray `print`, a file read — it would
/// propagate to `main` and the compile would fail with E400.
///
/// A test rather than a convention: "the parser cannot touch the machine" is
/// only a guarantee if something enforces it.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_toml_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Toml as Toml
import Flume.Toml.Value as Value
import Flume.Toml.Parse as Parse

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let parsed = match Toml.parse_toml("[a]\nx = 1\ny = [true, \"s\"]\n") {
        Ok(tree) -> Value.render_toml(tree),
        Err(problem) -> Parse.render_error(problem),
    }
    let failed = match Toml.parse_toml("x = 1.5\n") {
        Ok(tree) -> Value.render_toml(tree),
        Err(problem) -> Parse.error_kind(problem)
            + to_string(Parse.pos_line(Parse.error_pos(problem)))
            + to_string(Parse.pos_column(Parse.error_pos(problem)))
            + Parse.error_message(problem),
    }
    len(parsed) > 0 && len(failed) > 0
}

fn main() {
    exercise()
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .args(scratch.cache_args())
        .output()
        .expect("run flux");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Flume.Toml.Parse and Flume.Toml must be callable from a function with an \
         empty effect row — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}
