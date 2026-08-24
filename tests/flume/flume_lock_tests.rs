//! Integration tests for `Flume.Lock`, the `flux.lock` reader and writer.
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here are the two properties the fixture cannot check
//! about itself: that the module is **pure**, and that rendering a lockfile
//! and reading it back is a fixed point.

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
fn flume_lock_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_lock.flx");
}

/// A lockfile is read and written, never fetched: nothing in this module may
/// touch the machine.
///
/// This compiles a program whose `main` declares **no** effects and which
/// drives parsing and rendering end to end. If either acquired an effect the
/// compile would fail with E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_lock_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Lock as Lock

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let text = "version = 1\n\n[[package]]\nname = \"json\"\nversion = \"1.2.0\"\n"
    let rendered = match Lock.parse(text) {
        Ok(parsed) -> Lock.render(parsed),
        Err(message) -> message,
    }
    len(rendered) > 0 && Lock.max_format() >= Lock.default_format()
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
        "Flume.Lock must be callable from a function with an empty effect row \
         — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}

/// Rendering is a fixed point: `render(parse(render(l)))` equals `render(l)`.
///
/// The fixture asserts this for one hand-built lockfile. Asserted here is the
/// stronger form the fixture cannot express — that the rendered text is
/// *byte-identical* across a round trip, which is what keeps a lockfile from
/// churning in version control when nothing about the resolution changed.
#[test]
fn rendering_is_stable_across_a_round_trip() {
    let scratch = Scratch::new("flume_lock_stable");
    let file = scratch.write(
        "stable.flx",
        r#"
import Flume.Lock as Lock

fn round_trip(text: String) -> String {
    match Lock.parse(text) {
        Ok(parsed) -> Lock.render(parsed),
        Err(message) -> "err: " + message,
    }
}

fn main() with IO {
    let source = "version = 1\n"
        + "\n[[package]]\nname = \"zeta\"\nversion = \"0.3.1\"\n"
        + "\n[[package]]\nname = \"alpha\"\nversion = \"1.2.0\"\n"
        + "source = \"registry+https://example.org\"\nchecksum = \"sha256:ab\"\n"
        + "dependencies = [\"zeta\"]\n"
    let once = round_trip(source)
    let twice = round_trip(once)
    if once == twice { print("stable") } else { print("churned:\n" + once + "\n---\n" + twice) }
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
        stdout.contains("stable"),
        "a rendered lockfile must re-render byte-identically:\n{stdout}{stderr}"
    );
}
