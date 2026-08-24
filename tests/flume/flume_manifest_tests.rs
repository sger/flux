//! Integration tests for `Flume.Manifest` (proposal 0177, Phase 0).
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that the schema layer is **pure**.

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
fn flume_manifest_fixture_passes() {
    assert_backends_agree("flume_manifest.flx");
}

/// Phase 0's headline property: a manifest reader that provably cannot read a
/// file.
///
/// `main` declares no effects and drives the whole funnel — TOML, schema, and
/// namespace derivation. An effect anywhere beneath would propagate here and
/// fail the compile with E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_manifest_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Manifest as Manifest
import Flume.Parse as Parse

fn exercise() -> Bool {
    let source = "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n"
        + "[dependencies]\na = { path = \"../a\" }\n"
        + "[lib]\npath = \"src/A.flx\"\n"
        + "[[bin]]\nname = \"b\"\npath = \"b.flx\"\n"
    let described = match Manifest.parse(source) {
        Err(problem) -> Parse.error_kind(problem) + Parse.error_message(problem),
        Ok(manifest) -> Manifest.package_name(Manifest.manifest_package(manifest))
            + Manifest.package_version(Manifest.manifest_package(manifest))
            + Manifest.package_edition(Manifest.manifest_package(manifest))
            + Manifest.package_namespace(Manifest.manifest_package(manifest)),
    }
    len(described) > 0 && len(Manifest.derive_namespace("flux-json")) > 0
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
        "Flume.Manifest must be callable from a function with an empty effect \
         row — an effect leaked into the public surface:\n{stdout}{stderr}"
    );
}
