//! The C runtime's own tests: C programs in `runtime/c` that call the
//! runtime directly, built and linked the way a native Flux program is.

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;

use flux::llvm::pipeline::{RuntimeVariant, build_c_program};

fn runtime_c_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime").join("c")
}

/// Builds `runtime/c/<source>` against the runtime and runs it, failing
/// with its output when it fails.
fn run_c_test(source: &str, variant: RuntimeVariant) -> String {
    let dir = runtime_c_dir();
    let stem = source.trim_end_matches(".c");
    let exe = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{stem}_{}", variant.stem()));
    build_c_program(&[dir.join(source)], &dir, variant, &exe)
        .unwrap_or_else(|err| panic!("building {source}: {err:?}"));
    let exe = flux::llvm::pipeline::linked_exe_path(&exe);
    let output =
        Command::new(&exe).output().unwrap_or_else(|err| panic!("running {source}: {err}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{source} failed ({}):\n--- stdout\n{stdout}\n--- stderr\n{stderr}",
        output.status
    );
    stdout
}

#[test]
fn the_c_runtime_smoke_test_passes() {
    let stdout = run_c_test("smoke_test.c", RuntimeVariant::Plain);
    assert!(stdout.contains("tests passed"), "unexpected output: {stdout}");
}
