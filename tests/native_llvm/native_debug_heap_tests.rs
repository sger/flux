//! The debug heap (proposal 0188 S10c): `--debug-heap` links a runtime that
//! keeps and poisons freed blocks, and stops a program at the next dup or
//! drop of one, naming where it was allocated and freed.
//!
//! The fixture's use after free is made on purpose: `FLUX_AETHER_FAULT`
//! (a test-only hook in LIR lowering) leaves out every Dup of `victim`.
//!
//! `--leak-check` (0190 S15) adds a report at the program's end of the blocks
//! nothing freed; `skip-drop:` plants one.

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
    assert_eq!(stdout, "6\n");
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
             freed at main (tests/native_llvm/debug_heap/use_after_free.flx:16:16))"
        ),
        "{stderr}"
    );
    // Where the freed list was used again: the call that lends it to `total`.
    assert!(stderr.contains("Stack trace:"), "{stderr}");
    assert!(
        stderr.contains("at main (tests/native_llvm/debug_heap/use_after_free.flx:17:23)"),
        "{stderr}"
    );
}

#[test]
fn without_the_debug_heap_the_fault_goes_unnoticed() {
    // What the debug heap is for: the arena never reuses a block, so the
    // faulted program prints its answer and exits cleanly.
    let scratch = Scratch::new("debug-heap-unnoticed");
    let (stdout, stderr, code) = run(&scratch, FIXTURE, &["--native"], Some("skip-dup:victim"));
    assert_eq!((stdout.as_str(), code), ("6\n", 0), "{stderr}");
}

#[test]
fn the_fixture_runs_clean_without_the_fault() {
    let scratch = Scratch::new("debug-heap-clean");
    let (stdout, stderr, code) = run(&scratch, FIXTURE, &["--native", "--debug-heap"], None);
    assert_eq!((stdout.as_str(), code), ("6\n", 0), "{stderr}");
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
        "examples/guide/using_list_module.flx",
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

/// A nested match rebuilding the inner list, over lists built at run time,
/// prints what the VM prints on the debug heap: Aether used to drop the
/// outer list twice on every path.
#[test]
fn a_nested_match_releases_each_list_once() {
    let scratch = Scratch::new("debug-heap-nested-match");
    let file = "tests/native_llvm/debug_heap/nested_match.flx";
    let (vm, _, _) = run(&scratch, file, &["--vm"], None);
    let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
    assert_eq!((heap.as_str(), code), (vm.as_str(), 0), "{stderr}");
    assert_eq!(vm, "[34, 23, 12]\n[510, 405]\n[]\n");
}

/// Functions that rebuild a list, on lists built at run time that the
/// caller keeps (and on a worker thread): each prints what the VM prints on
/// the debug heap. A function inferred to borrow its list used to release
/// it, and the caller's guard released it again.
#[test]
fn kept_lists_survive_the_functions_they_are_lent_to() {
    let scratch = Scratch::new("debug-heap-shared-lists");
    for file in [
        "tests/native_llvm/debug_heap/shared_lists.flx",
        "tests/native_llvm/debug_heap/shared_lists_tasks.flx",
    ] {
        let (vm, _, _) = run(&scratch, file, &["--vm"], None);
        let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
        assert!(!vm.is_empty(), "{file}");
        assert_eq!((heap.as_str(), code), (vm.as_str(), 0), "{file}:\n{stderr}");
    }
}

/// A variable lent and given in the same call: the given reference is the
/// callee's to free, so the lent one needs a reference of its own.
#[test]
fn a_list_lent_and_given_in_one_call_survives_the_call() {
    let scratch = Scratch::new("debug-heap-lent-and-owned");
    let file = "tests/native_llvm/debug_heap/lent_and_owned.flx";
    let (vm, _, _) = run(&scratch, file, &["--vm"], None);
    assert_eq!(vm, "3\n");
    let (heap, stderr, code) = run(&scratch, file, &["--native", "--debug-heap"], None);
    assert_eq!((heap.as_str(), code), ("3\n", 0), "{stderr}");
}

const LEAK_FIXTURE: &str = "tests/native_llvm/debug_heap/leak.flx";

/// The lines of a leak report that name an allocation site.
fn site_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.contains(" allocated at "))
        .collect()
}

#[test]
fn a_leak_is_reported_at_exit_with_its_allocation_site() {
    let scratch = Scratch::new("debug-heap-leak");
    let (_, stderr, code) = run(
        &scratch,
        LEAK_FIXTURE,
        &["--native", "--leak-check"],
        Some("skip-drop:leaked"),
    );
    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("leak: 3 blocks still live at exit"),
        "{stderr}"
    );
    let sites = site_lines(&stderr);
    assert_eq!(sites.len(), 3, "{stderr}");
    for line in sites {
        assert_eq!(
            line,
            "  a constructor value (24 bytes) allocated at numbers \
             (tests/native_llvm/debug_heap/leak.flx:5:5)",
            "{stderr}"
        );
    }
}

#[test]
fn a_leak_report_names_at_most_ten_sites() {
    let scratch = Scratch::new("debug-heap-leak-many");
    let (_, stderr, code) = run(
        &scratch,
        LEAK_FIXTURE,
        &["--native", "--leak-check"],
        Some("skip-drop:many"),
    );
    assert_eq!(code, 3, "{stderr}");
    assert!(
        stderr.contains("leak: 12 blocks still live at exit"),
        "{stderr}"
    );
    assert_eq!(site_lines(&stderr).len(), 10, "{stderr}");
    assert!(stderr.contains("\n  and 2 more\n"), "{stderr}");
}

#[test]
fn the_leak_fixture_frees_everything_without_the_fault() {
    let scratch = Scratch::new("debug-heap-leak-clean");
    let (_, stderr, code) = run(&scratch, LEAK_FIXTURE, &["--native", "--leak-check"], None);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

#[test]
fn a_value_passed_by_ownership_is_freed_by_its_callee() {
    let scratch = Scratch::new("debug-heap-owned-arg");
    let (stdout, stderr, code) = run(
        &scratch,
        "tests/native_llvm/debug_heap/owned_arg.flx",
        &["--native", "--leak-check"],
        None,
    );
    assert_eq!((stdout.as_str(), code), ("", 0), "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

/// `--debug-heap` alone does not report leaks: nearly every program still
/// leaks (KI-119, KI-121).
#[test]
fn the_debug_heap_alone_does_not_report_leaks() {
    let scratch = Scratch::new("debug-heap-leak-off");
    let (_, stderr, code) = run(
        &scratch,
        LEAK_FIXTURE,
        &["--native", "--debug-heap"],
        Some("skip-drop:leaked"),
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains("leak:"), "{stderr}");
}

#[test]
fn without_the_debug_heap_a_leak_goes_unnoticed() {
    let scratch = Scratch::new("debug-heap-leak-unnoticed");
    let (_, stderr, code) = run(
        &scratch,
        LEAK_FIXTURE,
        &["--native"],
        Some("skip-drop:leaked"),
    );
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn the_leak_check_needs_the_native_backend() {
    let scratch = Scratch::new("debug-heap-leak-vm");
    let (_, stderr, code) = run(&scratch, LEAK_FIXTURE, &["--vm", "--leak-check"], None);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("Error: --leak-check applies to a program run or built with --native."),
        "{stderr}"
    );
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
