//! Integration tests for `Flume.Index`, the registry index reader.
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that reading the index is **pure**.

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
fn flume_index_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_index.flx");
}

/// Turning index text into resolver candidates must not be able to reach the
/// network or the disk.
///
/// The seam that makes this true is `from_text` taking the index *text*
/// rather than a path: fetching belongs to the caller. This compiles a program
/// whose `main` declares **no** effects and drives the reader end to end, so a
/// leaked effect would fail the compile with E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_index_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Index as Index
import Flow.List as List

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let text = "{\"name\":\"json\",\"version\":\"1.0.0\",\"checksum\":\"sha256:ab\","
        + "\"deps\":[{\"name\":\"core\",\"req\":\"^1.0\"}]}"
    match Index.from_text(text) {
        Err(_) -> false,
        Ok(entries) -> List.length(Index.to_candidates(entries)) == 1,
    }
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
        "Flume.Index must be callable from a function with an empty effect row \
         — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}
