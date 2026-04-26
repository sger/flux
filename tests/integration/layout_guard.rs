//! Enforces the phase-subdirectory layout defined in proposal 0169.
//!
//! New integration tests must live under a phase subdirectory
//! (`tests/lexer/`, `tests/parser/`, …); dropping a bare `tests/foo.rs`
//! bypasses the phase grouping and the `[[test]]`-binary-name discipline
//! that keeps insta snapshot filenames stable.
//!
//! Shared helpers belong in `tests/support/`, snapshots under
//! `tests/snapshots/<phase>/`, and fixtures under `tests/fixtures/`,
//! `tests/parity/`, or `tests/flux/`. None of those require top-level
//! `.rs` files either.

use std::{fs, path::Path};

#[test]
fn no_new_top_level_rs_tests() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let entries = fs::read_dir(&tests_dir).expect("tests/ should be readable");

    let mut offenders = Vec::new();
    for entry in entries {
        let entry = entry.expect("dir entry should be readable");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            offenders.push(
                path.file_name()
                    .expect("file has a name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    offenders.sort();
    assert!(
        offenders.is_empty(),
        "proposal 0169 forbids top-level `tests/*.rs` files; move these into a phase subdirectory:\n  - {}",
        offenders.join("\n  - ")
    );
}
