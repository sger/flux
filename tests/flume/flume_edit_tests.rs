//! Integration tests for `Flume.Edit`, the format-preserving `flux.toml` editor.
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here are the two properties the fixture cannot check
//! about itself: that the module is **pure**, and that an edit changes only
//! the line it meant to.

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
fn flume_edit_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_edit.flx");
}

/// Editing is a text-to-text function: reading and writing the manifest are
/// the caller's job, so nothing here may touch the machine.
///
/// This compiles a program whose `main` declares **no** effects and which
/// drives both edits end to end. If either acquired an effect the compile
/// would fail with E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_edit_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Edit as Edit

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let source = "[package]\nname = \"demo\"\n\n[dependencies]\na = \"1\"\n"
    let added = match Edit.add_dependency(source, "dependencies", "b", Edit.version_value("2")) {
        Ok(text) -> text,
        Err(message) -> message,
    }
    let removed = match Edit.remove_dependency(added, "dependencies", "a") {
        Ok(text) -> text,
        Err(message) -> message,
    }
    len(removed) > 0 && len(Edit.path_value("../x")) > 0
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
        "Flume.Edit must be callable from a function with an empty effect row \
         — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}

/// An edit rewrites one line and leaves every other byte alone.
///
/// The fixture asserts that particular comments survive. Asserted here is the
/// stronger form it cannot express: that every line of the original except the
/// removed one appears unchanged, in order, in the result — which is what
/// keeps `flux add` from producing a diff nobody asked for.
#[test]
fn an_edit_changes_only_its_own_line() {
    let scratch = Scratch::new("flume_edit_minimal");
    let file = scratch.write(
        "minimal.flx",
        r##"
import Flume.Edit as Edit
import Flow.Array as Array
import Flow.List as List

fn apply(source: String) -> String {
    match Edit.add_dependency(source, "dependencies", "beta", "\"2.0\"") {
        Ok(text) -> text,
        Err(message) -> "err: " + message,
    }
}

/// Every line of `before` still present, in order, in `after`.
fn preserved(before: List<String>, after: List<String>) -> Bool {
    match before {
        [] -> true,
        [head | rest] -> match after {
            [] -> false,
            [next | more] -> if head == next {
                preserved(rest, more)
            } else {
                preserved(before, more)
            },
        },
    }
}

fn main() with IO {
    let source = "# top\n[package]\nname = \"demo\"   # kept\nversion = \"0.1.0\"\n"
        + "\n[dependencies]\n# note\nalpha = { path = \"../a\" }\n"
    let result = apply(source)
    let before = Array.to_list(split(source, "\n"))
    let after = Array.to_list(split(result, "\n"))
    if preserved(before, after) {
        print("minimal")
    } else {
        print("rewrote:\n" + result)
    }
}
"##,
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
        stdout.contains("minimal"),
        "adding a dependency must leave every existing line untouched:\n{stdout}{stderr}"
    );
}
