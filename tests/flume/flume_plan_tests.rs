//! Integration tests for `Flume.Resolve.Plan`, the join between the manifest, the
//! registry index, the resolver, and the lockfile.
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that the entire resolution path is **pure**.

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
fn flume_plan_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_plan.flx");
}

/// Resolution cannot reach the network, which is what makes a resolution
/// reproducible: a solver that can fetch can produce a different answer
/// depending on when it ran.
///
/// The index arrives as text and the lockfile leaves as text, so the whole
/// path — requirements, resolution, lockfile rendering — sits inside a
/// function with an empty effect row. A leaked effect fails the compile with
/// E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_plan_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Resolve.Plan as Plan
import Flume.Schema.Index as Index
import Flume.Schema.Lock as Lock

// No `with` clause anywhere: if any callee were effectful, this would not
// compile.
fn exercise() -> Bool {
    let index = "{\"name\":\"json\",\"version\":\"1.0.0\",\"checksum\":\"sha256:ab\"}"
    let entries = match Index.from_text(index) {
        Ok(found) -> found,
        Err(_) -> [],
    }
    let deps = match Plan.root_deps([("json", "^1.0")]) {
        Ok(found) -> found,
        Err(_) -> [],
    }
    match Plan.resolve(deps, entries, None) {
        Err(_) -> false,
        Ok(outcome) -> len(Lock.render(Plan.outcome_lock(outcome))) > 0,
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
        "the resolution path must be callable from a function with an empty \
         effect row — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}
