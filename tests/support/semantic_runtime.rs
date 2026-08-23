use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// Re-exported so a test including this file gets `Scratch` from here rather
// than declaring its own `mod scratch;` — two `#[path]` declarations of one
// file in the same crate is a `clippy::duplicate_mod` warning.
#[path = "scratch.rs"]
pub mod scratch;
use scratch::Scratch;

pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

pub fn semantic_fixture_root() -> PathBuf {
    workspace_root().join("tests/fixtures/semantic_types")
}

pub fn semantic_fixture_path(rel: &str) -> PathBuf {
    semantic_fixture_root().join(rel)
}

pub fn combined_output(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

pub fn run_fixture(rel: &str, native: bool) -> Output {
    let fixture = semantic_fixture_path(rel);
    let fixture_root = semantic_fixture_root();
    let entry_parent = fixture
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fixture_root.clone());
    let lib_root = workspace_root().join("lib");
    let mut args: Vec<String> = Vec::new();
    if native {
        args.push("--native".into());
    }
    args.push("--no-cache".into());
    args.push(fixture.display().to_string());
    args.push("--roots-only".into());
    args.push("--root".into());
    args.push(entry_parent.display().to_string());
    args.push("--root".into());
    args.push(fixture_root.display().to_string());
    args.push("--root".into());
    args.push(lib_root.display().to_string());

    // `--no-cache` does not isolate native builds, which write shared
    // artifacts under the cache root regardless (KI-010).
    let scratch = Scratch::new("semantic-runtime");
    args.extend(scratch.cache_args());

    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|err| panic!("failed to run fixture {rel}: {err}"))
}
