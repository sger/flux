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
    let env: Vec<(&str, &str)> = fault
        .map(|fault| ("FLUX_AETHER_FAULT", fault))
        .into_iter()
        .collect();
    run_env(scratch, file, args, &env)
}

/// [`run`] with `env` set; the variables the tests read are cleared first,
/// so a value in the caller's environment cannot leak into a run.
fn run_env(
    scratch: &Scratch,
    file: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> (String, String, i32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flux"));
    command
        .current_dir(workspace_root())
        .arg(file)
        .args(args)
        .arg("--quiet")
        .args(scratch.cache_args())
        .env("NO_COLOR", "1")
        .env_remove("FLUX_AETHER_FAULT")
        .env_remove("FLUX_HEAP_STATS")
        .envs(env.iter().copied());
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

/// The debug heap's counts (`FLUX_HEAP_STATS`): blocks allocated, freed, and
/// still live at exit.
fn heap_stats(stderr: &str) -> Option<(u64, u64, u64)> {
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("heap: "))?;
    let mut parts = line.split(' ');
    let mut count = |key: &str| parts.next()?.strip_prefix(key)?.parse().ok();
    Some((count("allocs=")?, count("frees=")?, count("live=")?))
}

#[test]
fn heap_stats_are_reported_only_when_asked() {
    let scratch = Scratch::new("debug-heap-stats");
    let file = "tests/native_llvm/debug_heap/owned_arg.flx";
    let args = ["--native", "--debug-heap"];
    let (_, stderr, code) = run_env(&scratch, file, &args, &[("FLUX_HEAP_STATS", "1")]);
    assert_eq!(code, 0, "{stderr}");
    let (allocs, frees, live) = heap_stats(&stderr).unwrap_or_else(|| panic!("{stderr}"));
    assert!(allocs > 0, "{stderr}");
    assert_eq!((frees, live), (allocs, 0), "{stderr}");

    // A leak exit forwards all of stderr, so these see what the runtime
    // printed, not what the driver kept on a normal exit.
    let leak_args = ["--native", "--leak-check"];
    let leaked = ("FLUX_AETHER_FAULT", "skip-drop:leaked");
    let (_, stderr, code) = run_env(
        &scratch,
        LEAK_FIXTURE,
        &leak_args,
        &[leaked, ("FLUX_HEAP_STATS", "1")],
    );
    assert_eq!(code, 3, "{stderr}");
    assert_eq!(
        heap_stats(&stderr).map(|(_, _, live)| live),
        Some(3),
        "{stderr}"
    );
    assert!(stderr.find("heap: ") < stderr.find("leak: "), "{stderr}");
    let (_, stderr, code) = run_env(
        &scratch,
        LEAK_FIXTURE,
        &leak_args,
        &[leaked, ("FLUX_HEAP_STATS", "0")],
    );
    assert_eq!(code, 3, "{stderr}");
    assert!(!stderr.contains("heap:"), "{stderr}");
}

const OWNED_MOVES: &str = "tests/native_llvm/debug_heap/owned_moves";

/// Proposal 0189's hazards V1, V2, V4, V6 and V7, and the step-1 review's
/// repros: places a value is handed out or shared without being counted. Each
/// fixture prints on the debug heap what it prints on the VM: a freed value
/// read back shows up as poison there.
///
/// The fixtures build on a few workers, each with a scratch directory and so
/// a cache of its own: the native cache is not safe for two builds writing
/// it at once, and one worker building them all takes minutes.
#[test]
fn owned_moves_print_what_the_vm_prints() {
    let mut files: Vec<String> = std::fs::read_dir(workspace_root().join(OWNED_MOVES))
        .expect("owned_moves fixtures")
        .filter_map(|entry| {
            entry
                .ok()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "flx"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {OWNED_MOVES}");
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    let mut failures: Vec<(String, String)> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let files = &files;
                threads.spawn(move || {
                    let scratch = Scratch::new(&format!("debug-heap-owned-moves-{worker}"));
                    files
                        .iter()
                        .skip(worker)
                        .step_by(workers)
                        .filter_map(|name| owned_move_failure(&scratch, name))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("owned_moves worker"))
            .collect()
    });
    failures.sort();
    let report: Vec<String> = failures.into_iter().map(|(_, failure)| failure).collect();
    assert!(report.is_empty(), "{}", report.join("\n"));
}

/// How fixture `name` fails to print on the debug heap what the VM prints,
/// keyed by its name.
fn owned_move_failure(scratch: &Scratch, name: &str) -> Option<(String, String)> {
    let file = format!("{OWNED_MOVES}/{name}");
    let (vm, _, vm_code) = run(scratch, &file, &["--vm"], None);
    let (heap, stderr, code) = run(scratch, &file, &["--native", "--debug-heap"], None);
    (vm.is_empty() || vm_code != 0 || (heap.as_str(), code) != (vm.as_str(), 0)).then(|| {
        let (heap, stderr) = (clip(&heap), clip(&stderr));
        (
            name.to_string(),
            format!(
                "{name}: vm {vm:?} (exit {vm_code}), debug heap {heap:?} (exit {code})\n{stderr}"
            ),
        )
    })
}

/// The start of a program's output: a freed string read back can print
/// megabytes of whatever its length field now says.
fn clip(text: &str) -> String {
    const SHOWN: usize = 300;
    match text.char_indices().nth(SHOWN) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// Blocks a program allocates on the debug heap. `source` is written to the
/// scratch directory, so one shape can be measured at two sizes and the
/// blocks every program allocates (KI-119) cancel out.
fn allocs_of(scratch: &Scratch, name: &str, source: &str) -> u64 {
    stats_of(scratch, name, source).0
}

/// `(allocs, frees, live)` of `source` run on the debug heap.
fn stats_of(scratch: &Scratch, name: &str, source: &str) -> (u64, u64, u64) {
    let path = scratch.write(name, source);
    let file = path.to_string_lossy();
    let (_, stderr, code) = run_env(
        scratch,
        &file,
        &["--native", "--debug-heap"],
        &[("FLUX_HEAP_STATS", "1")],
    );
    assert_eq!(code, 0, "{name}:\n{stderr}");
    heap_stats(&stderr).unwrap_or_else(|| panic!("{name}:\n{stderr}"))
}

const LINEAR_BUMP: &str = r"fn build(n, acc) {
    if n == 0 { acc } else { build(n - 1, [n | acc]) }
}

fn bump(xs) {
    match xs {
        [h | t] -> [h + 1 | bump(t)],
        _ -> []
    }
}

fn main() {
    let ys = bump(build(SIZE, []))
    print(len(ys))
}
";

/// `source` with a guard on the rebuilding arm, which lowers through the
/// pattern-check chain instead of the multi-arm match.
fn guarded(source: &str) -> String {
    let out = source.replace("[h | t] ->", "[h | t] if h >= 0 ->");
    assert_ne!(out, source, "guarded: no rebuilding arm");
    out
}

/// `source`, then the rebuilt list released. `discard` owns its argument and
/// releases it there, so the release happens before the program's end and
/// nothing else holds the list.
fn released(source: &str) -> String {
    let body = source.replace(
        "    let ys = bump(build(SIZE, []))\n    print(len(ys))",
        "    let ys = bump(build(SIZE, []))\n    let d = discard(ys, false)\n    print(len(d))",
    );
    assert_ne!(body, source, "released: no `main` to change");
    body + "\nfn discard(xs, keep) {\n    if keep { xs } else { [] }\n}\n"
}

/// `stats_of(program)` at 2000 cells minus at 1000.
fn per_thousand_cells(scratch: &Scratch, name: &str, program: &str) -> (u64, u64, u64) {
    let at = |n: u64| {
        stats_of(
            scratch,
            &format!("{name}_{n}.flx"),
            &program.replace("SIZE", &n.to_string()),
        )
    };
    let (small, large) = (at(1000), at(2000));
    let diff = |a: u64, b: u64| {
        b.checked_sub(a)
            .unwrap_or_else(|| panic!("{name}: 1000 cells {small:?}, 2000 cells {large:?}"))
    };
    (
        diff(small.0, large.0),
        diff(small.1, large.1),
        diff(small.2, large.2),
    )
}

/// A list built and then rebuilt cell by cell, with nothing else holding it.
/// Building takes one block per cell, and the rebuild reuses each cell in
/// place. A match that copied the fields it binds made every tail shared, so
/// the rebuild took a new block per cell (0189 F3-4).
#[test]
fn a_linear_rebuild_allocates_once_per_cell() {
    let scratch = Scratch::new("debug-heap-linear-bump");
    for (name, program) in [
        ("bump", LINEAR_BUMP.to_string()),
        ("guarded_bump", guarded(LINEAR_BUMP)),
    ] {
        let (allocs, _, _) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(allocs, 1000, "{name}: blocks per 1000 cells");
    }
}

/// Releasing the rebuilt list frees every cell: none is left behind by a copy
/// the match made and nothing released (0189 F3-4).
#[test]
fn a_rebuilt_list_released_leaves_nothing_behind() {
    let scratch = Scratch::new("debug-heap-linear-bump-released");
    for (name, program) in [
        ("bump", released(LINEAR_BUMP)),
        ("guarded_bump", released(&guarded(LINEAR_BUMP))),
    ] {
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(live, 0, "{name}: blocks left live per 1000 cells");
    }
}

const COUNTING_LOOP: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let l = len(xs)
        count(n - 1, acc + l)
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// A self tail call that makes a list on each turn: it must stay a jump, so
/// ten million turns run in constant stack natively, and each turn allocates
/// exactly its three cells.
#[test]
fn a_loop_that_makes_a_list_each_turn_stays_a_tail_call() {
    let scratch = Scratch::new("debug-heap-counting-loop");
    // Native binaries get a 64 MB stack on macOS and Windows: a frame per turn
    // still fits at a million turns, and overflows well before ten million.
    // The VM takes too long at this size, so the sum is checked directly.
    let path = scratch.write("count_long.flx", &COUNTING_LOOP.replace("SIZE", "10000000"));
    let file = path.to_string_lossy();
    let (native, stderr, code) = run(&scratch, &file, &["--native"], None);
    assert_eq!((native.as_str(), code), ("30000000\n", 0), "{stderr}");

    let at = |n: u64| {
        allocs_of(
            &scratch,
            &format!("count_{n}.flx"),
            &COUNTING_LOOP.replace("SIZE", &n.to_string()),
        )
    };
    let (small, large) = (at(1000), at(2000));
    assert_eq!(
        large.saturating_sub(small),
        3000,
        "1000 turns: {small}, 2000 turns: {large}"
    );
}

const LENDING_LOOP: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(n, acc, prev) {
    if n == 0 { acc } else {
        walk(n - 1, acc + len(prev), numbers(n))
    }
}

fn main() {
    print(walk(SIZE, 0, []))
}
";

/// A self tail call that lends the list it made to the next turn: `prev` is
/// only read, so the caller still holds the list while the call runs.
/// Releasing it after the call would take a frame per turn and overflow the
/// stack at ten million turns, so the call stays a jump (PA R1).
#[test]
fn a_loop_that_lends_a_fresh_list_to_its_tail_call_stays_a_tail_call() {
    let scratch = Scratch::new("debug-heap-lending-loop");
    let path = scratch.write("lend_long.flx", &LENDING_LOOP.replace("SIZE", "10000000"));
    let file = path.to_string_lossy();
    let (native, stderr, code) = run(&scratch, &file, &["--native"], None);
    assert_eq!((native.as_str(), code), ("29999997\n", 0), "{stderr}");
}

const CLOSURE_PER_TURN: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let f = \() -> len(xs)
        count(n - 1, acc + f())
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LAMBDA_READS_ITS_PARAMETER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc, f) {
    if n == 0 { acc } else {
        count(n - 1, acc + f(numbers(n)), f)
    }
}

fn main() {
    print(count(SIZE, 0, \ys -> len(ys)))
}
";

const OWNED_THEN_READ: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn discard(xs, keep) {
    if keep { xs } else { [] }
}

fn keep_len(xs) {
    len(discard(xs, true))
}

fn count(n, acc) {
    if n == 0 { acc } else {
        let xs = numbers(n)
        let a = keep_len(xs)
        count(n - 1, acc + a + len(xs))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a value whose last use reads it is released after that read: a
/// list only lent, a closure only called and the list it captured, a
/// lambda's parameter, and a list given away once and then read. Each turn
/// makes the same blocks, so none is left at 2000 turns that was not at
/// 1000.
#[test]
fn values_last_used_by_a_read_are_released() {
    let scratch = Scratch::new("debug-heap-last-read");
    let failures: Vec<String> = [
        ("counting_loop", COUNTING_LOOP),
        ("closure_per_turn", CLOSURE_PER_TURN),
        ("lambda_reads_its_parameter", LAMBDA_READS_ITS_PARAMETER),
        ("owned_then_read", OWNED_THEN_READ),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const FORWARDED_CHILD: &str = r"type Pair2<T> = Pair2(T, T)

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn forwarded_child(xs, acc) {
    match xs {
        [y | ys] -> Pair2([y | acc], ys),
        _ -> Pair2(xs, acc),
    }
}

fn lengths(p) {
    match p {
        Pair2(a, b) -> len(a) + len(b),
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let p = forwarded_child(numbers(n), [n])
        count(n - 1, total + lengths(p))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// A cons rebuilt in the cell of the list it was matched from, inside a
/// fresh wrapper, is built once. A second, unused copy of it was left
/// behind each turn, holding its fields with references it never had.
#[test]
fn a_forwarded_child_leaves_no_cell_behind() {
    let scratch = Scratch::new("debug-heap-forwarded-child");
    let (_, _, live) = per_thousand_cells(&scratch, "forwarded_child", FORWARDED_CHILD);
    assert_eq!(live, 0, "blocks left live per 1000 turns");
}
