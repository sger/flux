//! Integration tests for `Flume.Resolve.Solver` (proposal 0177, Phase 0).
//!
//! The behavioural coverage lives in the Flux fixture and runs on both
//! backends. Asserted here is the property the fixture cannot check about
//! itself: that the resolver is **pure**.

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
fn flume_resolve_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_resolve.flx");
}

/// The resolver's purity is the sharpest form of Phase 0's claim.
///
/// The candidate set is a parameter rather than a fetch, so the solver cannot
/// reach a registry — which is what makes a resolution reproducible: it cannot
/// depend on when it ran. `main` declares no effects, so any I/O reachable from
/// `resolve` would fail this compile with E400.
#[test]
fn every_public_function_is_pure() {
    let scratch = Scratch::new("flume_resolve_purity");
    let file = scratch.write(
        "purity.flx",
        r#"
import Flume.Resolve.Solver as Resolve
import Flume.Resolve.Version as Version
import Flume.Resolve.Version exposing (Range)

// `exposing` rather than `Version.Range`: a qualified type is not accepted in
// type position.
fn caret(major: Int) -> Range {
    Caret(Version.version(major, 0, 0))
}

fn exercise() -> Bool {
    let roots = [Resolve.dep("app", caret(1), Resolve.from_root())]
    let available = [
        Resolve.package(
            "app",
            Version.version(1, 0, 0),
            [Resolve.dep(
                "lib",
                caret(1),
                Resolve.from_package("app", Version.version(1, 0, 0)),
            )],
        ),
        Resolve.package("lib", Version.version(1, 2, 0), []),
    ]
    let rendered = match Resolve.resolve(roots, available) {
        Ok(resolution) -> Resolve.render_resolution(resolution),
        Err(conflict) -> Resolve.render_conflict(conflict),
    }
    let failed = match Resolve.resolve(
        [Resolve.dep("ghost", caret(1), Resolve.from_lockfile())],
        available,
    ) {
        Ok(resolution) -> Resolve.render_resolution(resolution),
        Err(conflict) -> Resolve.render_conflict(conflict),
    }
    len(rendered) > 0
        && len(failed) > 0
        && len(Resolve.render_origin(Resolve.from_command_line())) > 0
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
        "Flume.Resolve.Solver must be callable from a function with an empty effect \
         row — an effect leaked into the resolver:\n{stdout}{stderr}"
    );
}
