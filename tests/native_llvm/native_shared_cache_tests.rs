//! Natively, two programs built into one cache, as every program in a
//! project is. Each module's object is cached by its own key, so a later
//! program links objects an earlier one compiled (KI-145).

#![cfg(feature = "llvm")]

use std::process::Command;

use crate::support::scratch;
use scratch::Scratch;

/// Imports `Flow.Result` alongside `Flow.Channel`, so the cached
/// `Flow.Result` object is compiled in a program other than `SECOND`.
const FIRST: &str = r"import Flow.Channel as C
import Flow.Result as R

fn main() with IO {
    print(1)
}
";

const SECOND: &str = r#"import Flow.Fs as Fs

fn main() with IO {
    let _ = Fs.write_file("f.txt", "payload")
    println(Fs.read_file_or("f.txt", "MISSING"))
}
"#;

/// What `file` printed when built natively on `scratch`'s cache.
fn run_native(scratch: &Scratch, file: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(scratch.path())
        .arg(file)
        .arg("--native")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .output()
        .expect("run flux");
    assert!(
        output.status.success(),
        "flux {file} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

/// The cache keys for a checked build agree between the stage that loads
/// interfaces and the one that saves them, so an edit costs one module from the
/// very next build. Under `-g` they differ and it takes three more (KI-166).
#[test]
fn an_edit_recompiles_only_the_edited_file_on_the_next_build() {
    let scratch = Scratch::new("edit-one-file");
    let build = |source: &str| {
        scratch.write("program.flx", source);
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(scratch.path())
            .args(["program.flx", "--native"])
            .args(scratch.cache_args())
            .env("NO_COLOR", "1")
            .env_remove("FLUX_AETHER_VERIFY")
            .output()
            .expect("run flux");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{stderr}");
        stderr
    };
    build("fn main() with IO {\n    print(1)\n}\n");
    let stderr = build("fn main() with IO {\n    print(2)\n}\n");
    let compiled: Vec<&str> =
        stderr.lines().filter(|line| line.contains("] Compiling  ")).collect();
    assert_eq!(compiled.len(), 1, "{stderr}");
    assert!(compiled[0].ends_with("Compiling  program"), "{stderr}");
}

/// `v` is the imported module's highest binder, so the temporary holding the
/// indexed value used to get the same id, and clang rejected the module as
/// broken (KI-167).
#[test]
fn a_module_ending_in_a_match_on_a_temporary_builds_ki167() {
    let scratch = Scratch::new("module-temporary");
    scratch.write(
        "Shared.flx",
        r"module Shared {
    public fn at(arr, idx) {
        match arr[idx] {
            Some(v) -> v,
            _ -> [],
        }
    }
}
",
    );
    scratch.write(
        "program.flx",
        r"import Shared

fn main() with IO {
    print(Shared.at([|[1, 2], [3, 4, 5]|], 1))
}
",
    );
    assert_eq!(run_native(&scratch, "program.flx").trim(), "[3, 4, 5]");
}

#[test]
#[ignore = "KI-145: a cached stdlib object keeps the constructor tags of the program that built it"]
fn a_program_reads_its_file_after_another_built_the_stdlib() {
    let scratch = Scratch::new("shared-cache");
    scratch.write("first.flx", FIRST);
    scratch.write("second.flx", SECOND);
    run_native(&scratch, "first.flx");
    assert_eq!(run_native(&scratch, "second.flx").trim(), "\"payload\"");
}

/// Each scratch's seeded cache is its own copy of the stdlib template, and
/// the copy keeps the stdlib archive no older than its members, which is
/// what lets a build trust the archive instead of making it again.
#[test]
fn a_seeded_cache_is_a_private_copy_that_keeps_the_archive_current() {
    let first = Scratch::new("seeded-first");
    let second = Scratch::new("seeded-second");
    first.seeded_cache_args();
    second.seeded_cache_args();
    let native = |scratch: &Scratch| scratch.cache_dir().join("native");
    let modified = |path: &std::path::Path| {
        std::fs::metadata(path).and_then(|metadata| metadata.modified()).expect("an mtime")
    };
    let archive = native(&first).join("libflux_std_O0.a");
    assert!(archive.exists(), "the seeded cache has no stdlib archive");
    for entry in std::fs::read_dir(native(&first)).expect("the native cache") {
        let member = entry.expect("an entry").path();
        if member.extension().is_some_and(|ext| ext == "o" || ext == "obj") {
            assert!(modified(&member) <= modified(&archive), "{} is newer", member.display());
        }
    }
    std::fs::remove_dir_all(first.cache_dir()).expect("remove the first cache");
    assert!(native(&second).join("libflux_std_O0.a").exists(), "the caches are shared");
}

/// The runners take the seeded cache by default; `no_cache()` keeps the
/// `--no-cache` path, and both print the same, on the VM and natively.
#[test]
fn a_fixture_prints_the_same_with_and_without_the_cache() {
    use crate::support::flux_runner::flux_file;
    let fixture = "tests/parity/adt_either_match.flx";
    for native in [false, true] {
        let run = |no_cache: bool| {
            let run = flux_file(fixture);
            let run = if native { run.native() } else { run };
            let ran = if no_cache { run.no_cache() } else { run }.run();
            assert!(ran.success, "{fixture} failed:\n{}", ran.stderr);
            ran.stdout
        };
        assert_eq!(run(false), run(true), "native: {native}");
    }
}
