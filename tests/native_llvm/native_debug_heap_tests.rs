//! The debug heap (proposal 0188 S10c): `--debug-heap` links a runtime that
//! keeps and poisons freed blocks, and stops a program at the next dup or
//! drop of one, naming where it was allocated and freed.
//!
//! The fixture's use after free is made on purpose: `FLUX_AETHER_FAULT`
//! (a test-only hook in LIR lowering) leaves out every Dup of `victim`.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

const FIXTURE: &str = "tests/native_llvm/debug_heap/use_after_free.flx";

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// What `flux <file> <args>` printed (stdout, stderr) and its exit code,
/// with `fault` as `FLUX_AETHER_FAULT` when given.
fn run(scratch: &Scratch, file: &str, args: &[&str], fault: Option<&str>) -> (String, String, i32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flux"));
    command
        .current_dir(workspace_root())
        .arg(file)
        .args(args)
        .arg("--quiet")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .env_remove("FLUX_AETHER_FAULT");
    if let Some(fault) = fault {
        command.env("FLUX_AETHER_FAULT", fault);
    }
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {file}: {e}"));
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    (
        text(&output.stdout),
        text(&output.stderr),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn a_use_after_free_is_caught() {
    let scratch = Scratch::new("debug-heap-fault");
    // A sound build first: the fault keys its own cache entries, so the
    // faulted build below cannot be served this one's objects.
    let (stdout, _, _) = run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    assert_eq!(stdout, "16\n");
    let (_, stderr, code) = run(
        &scratch,
        FIXTURE,
        &["--native", "--debug-heap"],
        Some("skip-dup:victim"),
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains(
            "use after free: copying a constructor value that was already freed \
             (allocated at numbers (tests/native_llvm/debug_heap/use_after_free.flx:2:5), \
             freed at main (tests/native_llvm/debug_heap/use_after_free.flx:15:21))"
        ),
        "{stderr}"
    );
    // Where the freed list was used again, and how the program got there.
    assert!(stderr.contains("Stack trace:"), "{stderr}");
    assert!(
        stderr.contains("at total (tests/native_llvm/debug_heap/use_after_free.flx:10:5)"),
        "{stderr}"
    );
    assert!(
        stderr.contains("at main (tests/native_llvm/debug_heap/use_after_free.flx:18:17)"),
        "{stderr}"
    );
}

#[test]
fn without_the_debug_heap_the_fault_goes_unnoticed() {
    // What the debug heap is for: the arena never reuses a block, so the
    // faulted program prints its answer and exits cleanly.
    let scratch = Scratch::new("debug-heap-unnoticed");
    let (stdout, stderr, code) = run(&scratch, FIXTURE, &["--native"], Some("skip-dup:victim"));
    assert_eq!((stdout.as_str(), code), ("16\n", 0), "{stderr}");
}

#[test]
fn the_fixture_runs_clean_without_the_fault() {
    let scratch = Scratch::new("debug-heap-clean");
    let (stdout, stderr, code) = run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    assert_eq!((stdout.as_str(), code), ("16\n", 0), "{stderr}");
}

/// Programs over lists, tuples, records, closures, strings, maps, reuse and
/// worker threads print the same on the debug heap: it reports no fault in
/// a sound program.
#[test]
fn programs_run_the_same_on_the_debug_heap() {
    let scratch = Scratch::new("debug-heap-programs");
    for file in [
        "examples/guide/tuples.flx",
        "examples/guide/records.flx",
        "examples/guide/higher_order_builtins.flx",
        "examples/guide/mini_database.flx",
        "examples/guide/sort_by_string_len.flx",
        "examples/guide/map_builtins.flx",
        "examples/async/15_concurrent_jobs.flx",
        "examples/async/18_task_closure_captures.flx",
    ] {
        let (plain, _, plain_code) = run(&scratch, file, &["--native"], None);
        let (heap, stderr, heap_code) = run(&scratch, file, &["--native", "--debug-heap"], None);
        assert_eq!(
            (heap.as_str(), heap_code),
            (plain.as_str(), plain_code),
            "{file}:\n{stderr}"
        );
    }
}

#[test]
fn the_debug_heap_needs_the_native_backend() {
    let scratch = Scratch::new("debug-heap-vm");
    for args in [&["--vm", "--debug-heap"][..], &["--debug-heap"][..]] {
        let (_, stderr, code) = run(&scratch, FIXTURE, args, None);
        assert_eq!(code, 1);
        assert!(
            stderr.contains("Error: --debug-heap applies to a program run or built with --native."),
            "{stderr}"
        );
    }
}

#[test]
fn debug_heap_binaries_are_cached_apart() {
    let scratch = Scratch::new("debug-heap-cache");
    run(&scratch, FIXTURE, &["--native"], None);
    run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    let binaries: Vec<String> = walk(scratch.path())
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            name.ends_with(".bin").then_some(name)
        })
        .collect();
    assert!(
        binaries.contains(&"use_after_free.bin".to_string()),
        "{binaries:?}"
    );
    assert!(
        binaries.contains(&"use_after_free.debug-heap.bin".to_string()),
        "{binaries:?}"
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
