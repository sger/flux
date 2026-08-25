//! Integration tests for `Flume.Resolve.Version` (proposal 0177, Phase 0).
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that every public function in the module is **pure**.

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
fn flume_version_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_version.flx");
}

/// Phase 0's headline property: the package manager's core logic provably
/// cannot touch the machine.
///
/// This compiles a program whose `main` declares **no** effects and which calls
/// the module's parsing, comparison, rendering, and compatibility functions. If
/// any of them acquired an effect — a stray `print`, a file read — the effect
/// would propagate to `main` and the compile would fail with E400.
///
/// A test rather than a convention: "no I/O in the resolver" is only a
/// guarantee if something enforces it.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_version_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Resolve.Version as V
import Flow.Result as Result

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let a = V.version(1, 2, 3)
    let b = V.pre_version(1, 2, 3, "rc.1")
    let parsed = match V.parse("2.0.0-beta+meta") {
        Ok(v) -> V.render(v),
        Err(e) -> e,
    }
    len(parsed) > 0
        && V.less_than(b, a)
        && V.compatible(a, V.version(1, 9, 9))
        && !V.is_prerelease(a)
        && V.equals(a, a)
        && len(V.render(b)) > 0
        && match V.pre(b) { Some(_) -> true, None -> false }
        && V.major(a) + V.minor(a) + V.patch(a) == 6
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
        "Flume.Resolve.Version must be callable from a function with an empty effect \
         row — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}
