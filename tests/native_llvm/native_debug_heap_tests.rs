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
    let files: Vec<String> = [
        "examples/guide/tuples.flx",
        "examples/guide/records.flx",
        "examples/guide/higher_order_builtins.flx",
        "examples/guide/mini_database.flx",
        "examples/guide/sort_by_string_len.flx",
        "examples/guide/map_builtins.flx",
        "examples/guide/using_list_module.flx",
        "examples/async/15_concurrent_jobs.flx",
        "examples/async/18_task_closure_captures.flx",
    ]
    .map(String::from)
    .into();
    let failures = failures_in_parallel("debug-heap-programs", &files, 4, |scratch, file| {
        let (plain, _, plain_code) = run(scratch, file, &["--native"], None);
        let (heap, stderr, heap_code) = run(scratch, file, &["--native", "--debug-heap"], None);
        ((heap.as_str(), heap_code) != (plain.as_str(), plain_code)).then(|| {
            format!(
                "{file}: plain {:?} (exit {plain_code}), debug heap {:?} (exit {heap_code})\n{}",
                clip(&plain),
                clip(&heap),
                clip(&stderr)
            )
        })
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
/// The fixtures are split over four tests, which nextest runs side by side.
/// Each builds on two workers, each with a scratch directory and so a cache
/// of its own: the native cache is not safe for two builds writing it at
/// once, and one worker building them all takes minutes.
fn owned_moves_part(part: usize) {
    let files: Vec<String> = owned_move_fixtures()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % 4 == part)
        .map(|(_, name)| name)
        .collect();
    assert!(!files.is_empty(), "no fixtures in part {part}");
    let scratch = format!("debug-heap-owned-moves-{part}");
    // CI runners have three cores; more builds there slow the timed tests
    // running beside these.
    let workers = if std::env::var_os("CI").is_some() {
        1
    } else {
        2
    };
    let failures = failures_in_parallel(&scratch, &files, workers, owned_move_failure);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_1() {
    owned_moves_part(0)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_2() {
    owned_moves_part(1)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_3() {
    owned_moves_part(2)
}

#[test]
fn owned_moves_print_what_the_vm_prints_part_4() {
    owned_moves_part(3)
}

/// The fixtures of `OWNED_MOVES`, sorted.
fn owned_move_fixtures() -> Vec<String> {
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
    files
}

/// Runs `check` on each of `files` across up to `workers` threads, each
/// with its own scratch cache: what it reported, sorted.
fn failures_in_parallel<F>(prefix: &str, files: &[String], workers: usize, check: F) -> Vec<String>
where
    F: Fn(&Scratch, &str) -> Option<String> + Sync,
{
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(workers)
        .min(files.len());
    let check = &check;
    let mut failures: Vec<String> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                threads.spawn(move || {
                    let scratch = Scratch::new(&format!("{prefix}-{worker}"));
                    files
                        .iter()
                        .skip(worker)
                        .step_by(workers)
                        .filter_map(|file| check(&scratch, file))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a worker"))
            .collect()
    });
    failures.sort();
    failures
}

/// How fixture `name` fails to print on the debug heap what the VM prints.
fn owned_move_failure(scratch: &Scratch, name: &str) -> Option<String> {
    let file = format!("{OWNED_MOVES}/{name}");
    let (vm, _, vm_code) = run(scratch, &file, &["--vm"], None);
    let (heap, stderr, code) = run(scratch, &file, &["--native", "--debug-heap"], None);
    (vm.is_empty() || vm_code != 0 || (heap.as_str(), code) != (vm.as_str(), 0)).then(|| {
        let (heap, stderr) = (clip(&heap), clip(&stderr));
        format!("{name}: vm {vm:?} (exit {vm_code}), debug heap {heap:?} (exit {code})\n{stderr}")
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

/// A self tail call that hands the list it made to the next turn, which only
/// reads it. Releasing it after the call would take a frame per turn and
/// overflow the stack at ten million turns. Instead `prev` is owned, since a
/// recursive tail call passes it a fresh value: the call gives the list away
/// and stays a jump, and the next turn releases it (KI-128).
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

const LENT_TO_A_TAIL_CALL: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn bump(xs) {
    match xs {
        [h | t] -> [h + 1 | t],
        _ -> xs,
    }
}

fn total(xs) {
    match xs {
        [h | t] -> h + total(t),
        _ -> 0,
    }
}

fn turn(xs) {
    total(bump(xs))
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + turn(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LOCAL_WALKER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(xs) {
    fn go(ys, n) {
        match ys {
            [h | t] -> go(t, n + h),
            _ -> n,
        }
    }
    go(xs, 0)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + walk(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const LIST_COUNT: &str = r"import Flow.List as List

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, acc) {
    if n == 0 { acc } else {
        count(n - 1, acc + List.count(numbers(n), \x -> x > 0))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const CAPTURING_LOCAL_CALLED_LAST: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn run(xs) {
    fn go(k, n) {
        if k == 0 { n + len(xs) } else { go(k - 1, n + 1) }
    }
    go(3, 0)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + run(numbers(n))) }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-128: a value whose last use is a call in tail position, where the call
/// only reads it. The value was kept so the call stayed a tail call, and
/// nothing released it: the list `turn` rebuilds and lends to `total`, the
/// list a local function walks (and `List.count`, written the same way), a
/// local function called last with the list it captured, and the list the
/// lending loop hands to its next turn.
#[test]
fn a_value_lent_to_a_tail_call_is_released() {
    let scratch = Scratch::new("debug-heap-lent-to-tail-call");
    let failures: Vec<String> = [
        ("lent_to_a_tail_call", LENT_TO_A_TAIL_CALL),
        ("local_walker", LOCAL_WALKER),
        ("list_count", LIST_COUNT),
        ("capturing_local_called_last", CAPTURING_LOCAL_CALLED_LAST),
        ("lending_loop", LENDING_LOOP),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const OWNED_TEMPORARY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn keep(xs) {
    xs
}

fn first(ys) {
    match ys {
        [h | _] -> h,
        _ -> 0,
    }
}

fn go(n) {
    let xs = numbers(n)
    let r = [len(keep(xs))]
    first(r) + sum(xs)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() {
    print(count(SIZE, 0))
}
";

const PRINTED_TEMPORARY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn keep(xs) {
    xs
}

fn go(n) with IO {
    let xs = numbers(n)
    print(len(keep(xs)))
    sum(xs)
}

fn count(n, acc) with IO {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() with IO {
    print(count(SIZE, 0))
}
";

const BUILT_STRING: &str = r#"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn sum(xs) {
    match xs {
        [h | t] -> h + sum(t),
        _ -> 0,
    }
}

fn go(n) {
    let xs = numbers(n)
    let name = "n" + to_string(len(xs))
    let pair = (len(xs), name)
    pair.0 + sum(xs) + len(name)
}

fn count(n, acc) {
    if n == 0 { acc } else { count(n - 1, acc + go(n)) }
}

fn main() {
    print(count(SIZE, 0))
}
"#;

/// KI-129: `len(keep(xs))` is computed in a block that copies its result
/// before releasing what `keep` returned. Given straight to an owned use, a
/// list cell or a `print`, that result was taken for a view of `xs`, and
/// `xs` was never released. A string built with `+` from `xs` was taken for
/// one too, and was never released itself.
#[test]
fn a_value_copied_into_a_temporary_given_away_is_released() {
    let scratch = Scratch::new("debug-heap-owned-temporary");
    let failures: Vec<String> = [
        ("owned_temporary", OWNED_TEMPORARY),
        ("printed_temporary", PRINTED_TEMPORARY),
        ("built_string", BUILT_STRING),
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

const GROW_OTHER_SHAPE: &str = r"type Shape = Circle(Int) | Square(Int)

fn make(n) {
    if n % 2 == 0 { Circle(n) } else { Square(n) }
}

fn grow(s) {
    match s {
        Circle(r) -> Circle(r + 1),
        _ -> Circle(0),
    }
}

fn size(s) {
    match s {
        Circle(r) -> r,
        Square(w) -> w,
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let s = grow(make(n))
        count(n - 1, total + size(s))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

const WALK_GIVES_ITS_LIST_AWAY: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn keep(token, v) {
    if len(token) < 0 { match [token] { _ -> v } } else { v }
}

fn walk(xs) {
    match xs {
        [h | t] -> [keep(xs, h) | walk(t)],
        _ -> [],
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = walk(numbers(n))
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a value given away or reused on one path is released on the
/// others: the arm that does not rebuild a circle releases the square it
/// matched, and the arm that gives the list a copy releases the list after
/// its last read.
#[test]
fn a_value_moved_on_one_path_is_released_on_the_others() {
    let scratch = Scratch::new("debug-heap-moved-on-one-path");
    let failures: Vec<String> = [
        ("grow_other_shape", GROW_OTHER_SHAPE),
        ("walk_gives_its_list_away", WALK_GIVES_ITS_LIST_AWAY),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LENS_OF_ROWS: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn rows(n) {
    [numbers(n), numbers(n + 1)]
}

fn lens(xss) {
    match xss {
        [xs | rest] -> [len(xs) | lens(rest)],
        _ -> [],
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = lens(rows(n))
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: a field the arm copies before releasing its cell, and then only
/// reads, is released after that read. Each row is read once and left
/// behind, while the rebuild still reuses the outer list's cells: 8 blocks
/// a turn, all freed.
#[test]
fn a_field_only_read_after_its_cell_is_released_is_released_too() {
    let scratch = Scratch::new("debug-heap-field-only-read");
    let (allocs, _, live) = per_thousand_cells(&scratch, "lens_of_rows", LENS_OF_ROWS);
    assert_eq!((allocs, live), (8000, 0), "(allocs, live) per 1000 turns");
}

const ARRAY_TO_LIST: &str = r"fn to_list(arr, idx) {
    if idx >= len(arr) { [] } else {
        match arr[idx] {
            Some(v) -> [v | to_list(arr, idx + 1)],
            _ -> to_list(arr, idx + 1)
        }
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = to_list([|n, n + 1, n + 2|], 0)
        count(n - 1, total + len(ys))
    }
}

fn main() {
    print(count(SIZE, 0))
}
";

/// KI-121: the option an index returns is matched and its value moved into
/// a new cell; nothing reuses the option, so it is released, whether the
/// cell is built before or after that release.
#[test]
fn an_option_matched_and_not_reused_is_released() {
    let scratch = Scratch::new("debug-heap-array-to-list");
    let (allocs, _, live) = per_thousand_cells(&scratch, "array_to_list", ARRAY_TO_LIST);
    assert_eq!(live, 0, "live per 1000 turns ({allocs} allocs)");
}

const BRANCH_REBUILD: &str = r"fn cell(n) { Some(None) }
fn read(opt) { match opt { Some(_) -> 1, _ -> 0 } }

fn rebuild(opt, keep) {
    match opt {
        Some(v) -> if keep { Some(v) } else { None },
        _ -> None
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let xs = cell(n)
        let ys = rebuild(xs, n % 2 == 0)
        let next = total + read(ys) + EXTRA_READ
        count(n - 1, next)
    }
}

fn main() { print(count(SIZE, 0)) }
";

// A nullary payload isolates shell accounting from extra copies of heap fields.
// The later read retains a second reference to the same cell across rebuild.
#[test]
fn a_matched_cell_reused_on_only_one_path_is_released() {
    let scratch = Scratch::new("debug-heap-branch-rebuild");
    for (name, extra_read) in [("unique", "0"), ("shared", "read(xs)")] {
        let program = BRANCH_REBUILD.replace("EXTRA_READ", extra_read);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        assert_eq!(live, 0, "{name}: live per 1000 turns");
    }
}

const MERGE: &str = r"fn merge(left, right) {
    match left {
        [a | at] -> match right {
            [b | bt] -> if a <= b { [a | merge(at, right)] } else { [b | merge(left, bt)] },
            _ -> left
        },
        _ -> right
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = merge([n, n + 2, n + 4], [n + 1, n + 3, n + 5])
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

/// KI-121: each arm keeps the other list whole and rebuilds in its own cell.
/// The tail it passes on is copied once, before that cell may be released;
/// a second copy where it is passed is a reference nothing gives back.
#[test]
fn a_merge_that_rebuilds_in_its_own_cells_copies_each_field_once() {
    let scratch = Scratch::new("debug-heap-merge");
    let (allocs, _, live) = per_thousand_cells(&scratch, "merge", MERGE);
    assert_eq!(live, 0, "live per 1000 turns ({allocs} allocs)");
}

/// A cons rebuilt in the cell of the list it was matched from, inside a
/// fresh wrapper, is built once. A second, unused copy of it was left
/// behind each turn, holding its fields with references it never had.
#[test]
fn a_forwarded_child_leaves_no_cell_behind() {
    let scratch = Scratch::new("debug-heap-forwarded-child");
    let (_, _, live) = per_thousand_cells(&scratch, "forwarded_child", FORWARDED_CHILD);
    assert_eq!(live, 0, "blocks left live per 1000 turns");
}

const ZEROS_OF_ROWS: &str = r"fn rows(n) {
    [[n], [n + 1], [n + 2]]
}

fn zeros(xss) {
    match xss {
        [_ | rest] -> [0 | zeros(rest)],
        _ -> []
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = zeros(rows(n))
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const KEEP_LEFT: &str = r"type Pair2<T> = Pair2(T, T)

fn make(n) { Pair2([n], [n + 1]) }

fn keep_left(p) {
    match p {
        Pair2(a, _) -> Pair2(a, []),
    }
}

fn size(p) { match p { Pair2(a, b) -> len(a) + len(b) } }

fn count(n, total) {
    if n == 0 { total } else {
        let p = keep_left(make(n))
        count(n - 1, total + size(p))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const RESET_OPTION: &str = r"fn make(n) { Some([n]) }

fn reset(o) {
    match o {
        Some(_) -> Some([0]),
        _ -> None
    }
}

fn size(o) { match o { Some(xs) -> len(xs), _ -> 0 } }

fn count(n, total) {
    if n == 0 { total } else {
        let o = reset(make(n))
        count(n - 1, total + size(o))
    }
}

fn main() { print(count(SIZE, 0)) }
";

const SUM_PAIRS: &str = r"fn numbers(n) {
    [n, n + 1, n + 2, n + 3]
}

fn pairs(xs) {
    match xs {
        [a | [b | t]] -> [a + b | pairs(t)],
        _ -> []
    }
}

fn count(n, total) {
    if n == 0 { total } else {
        let ys = pairs(numbers(n))
        count(n - 1, total + len(ys))
    }
}

fn main() { print(count(SIZE, 0)) }
";

/// A cell rebuilt in place holds new fields, so each old field nothing took
/// over is released first: a wildcard, an unused binder, an inner cell. The
/// first three split into unique and shared paths; the last two reuse the
/// cell where its plain release was.
#[test]
fn a_reused_cell_releases_the_fields_it_does_not_keep() {
    let scratch = Scratch::new("debug-heap-reused-cell-fields");
    let failures: Vec<String> = [
        ("wildcard", ZEROS_OF_ROWS.to_string()),
        ("unused", ZEROS_OF_ROWS.replace("[_ | rest]", "[xs | rest]")),
        ("constructor", KEEP_LEFT.to_string()),
        ("plain", RESET_OPTION.to_string()),
        ("inner_cell", SUM_PAIRS.to_string()),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const RETURN_A_COUNT: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let xs = numbers(n)
    let ys = numbers(n + 1)
    if n > 0 { return len(xs) }
    len(ys)
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + pick(n)) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_A_LIST: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let xs = numbers(n)
    let ys = numbers(n + 1)
    if n > 0 { return xs }
    ys
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(n))) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_PAST_A_PARAMETER: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(xs, n) {
    if n > 0 { return [n] }
    xs
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(numbers(n), n))) }
}

fn main() { print(count(SIZE, 0)) }
";

const RETURN_A_FIELD: &str = r"fn numbers(n) {
    [n, n + 1, n + 2]
}

fn pick(n) {
    let rows = [numbers(n), numbers(n + 1)]
    match rows {
        [r | _] -> if len(r) > 0 { return r },
        _ -> if n < 0 { return numbers(0) }
    }
    numbers(0)
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(pick(n))) }
}

fn main() { print(count(SIZE, 0)) }
";

/// An early return leaves the function with the values it holds still to
/// release: those it never gives away go before it does, and a field it
/// returns is copied out of its list first.
#[test]
fn an_early_return_releases_what_the_function_holds() {
    let scratch = Scratch::new("debug-heap-early-return");
    let failures: Vec<String> = [
        ("count", RETURN_A_COUNT),
        ("list", RETURN_A_LIST),
        ("parameter", RETURN_PAST_A_PARAMETER),
        ("field", RETURN_A_FIELD),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (_, _, live) = per_thousand_cells(&scratch, name, program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LEND_A_FUNCTION: &str = r"fn double(x) { x * 2 }

fn apply(f, x) { f(x) }

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + apply(double, n)) }
}

fn main() { print(count(SIZE, 0)) }
";

const LEND_A_LITERAL: &str = r#"fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len("hello")) }
}

fn main() { print(count(SIZE, 0)) }
"#;

const RETURN_A_LITERAL: &str = r#"fn tag(n) {
    if n % 2 == 0 { "even" } else { "odd" }
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + len(tag(n))) }
}

fn main() { print(count(SIZE, 0)) }
"#;

/// A top-level function used as a value and a string literal are the same
/// value every time, so each is made once, outside the heap, and never
/// released: no use allocates, and a use that only lends one leaves nothing
/// behind.
#[test]
fn a_function_or_literal_used_as_a_value_is_never_allocated() {
    let scratch = Scratch::new("debug-heap-static-values");
    let failures: Vec<String> = [
        ("lent_function", LEND_A_FUNCTION),
        ("lent_literal", LEND_A_LITERAL),
        ("returned_literal", RETURN_A_LITERAL),
    ]
    .into_iter()
    .filter_map(|(name, program)| {
        let (allocs, _, live) = per_thousand_cells(&scratch, name, program);
        (allocs != 0 || live != 0).then(|| {
            format!("{name}: {allocs} blocks allocated and {live} left live per 1000 turns")
        })
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Programs that build a value with `n` fields: a constructor, a tuple, and
/// a closure that captures `n` values; each with what it prints.
fn wide_values(n: usize) -> [(&'static str, String, String); 3] {
    let vals = (0..n)
        .map(|i| format!("n + {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let pattern = std::iter::once("a")
        .chain(std::iter::repeat_n("_", n - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let ints = vec!["Int"; n].join(", ");
    // `keep` is recursive, so it is never inlined: the closure captures each
    // value, where it would copy a cheap `n + i` into its own body.
    let lets = (0..n)
        .map(|i| format!("    let v{i} = keep(n + {i})\n"))
        .collect::<String>();
    let sum = (0..n)
        .map(|i| format!("v{i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    [
        (
            "constructor",
            format!(
                "type Wide = Wide({ints})\n\nfn first(w) {{ match w {{ Wide({pattern}) -> a }} }}\n\n\
                 fn main() {{\n    let n = len([1, 2])\n    print(first(Wide({vals})))\n}}\n"
            ),
            "2".to_string(),
        ),
        (
            "tuple",
            format!(
                "fn first(t) {{ t.0 }}\n\n\
                 fn main() {{\n    let n = len([1, 2])\n    print(first(({vals})))\n}}\n"
            ),
            "2".to_string(),
        ),
        (
            "closure",
            format!(
                "fn keep(k) {{ if k < 0 {{ keep(k + 1) }} else {{ k }} }}\n\n\
                 fn make(n) {{\n{lets}    \\x -> x + {sum}\n}}\n\n\
                 fn main() {{\n    let f = make(len([1, 2]))\n    print(f(1))\n}}\n"
            ),
            // 1 + the sum of 2 + i for i below n.
            (1 + 2 * n + n * (n - 1) / 2).to_string(),
        ),
    ]
}

/// A heap object counts the fields its release walks in one byte, and the
/// VM's instructions count fields and captures in one byte too, so a value
/// with more than 255 is rejected before either backend sees it. Natively it
/// used to build and leak every field past the 255th; the VM failed at run
/// time.
#[test]
fn a_value_with_more_than_255_fields_is_rejected() {
    let scratch = Scratch::new("debug-heap-wide-values");
    let mut failures = Vec::new();
    for (name, program, _) in wide_values(256) {
        let path = scratch.write(&format!("wide_{name}.flx"), &program);
        let file = path.to_string_lossy();
        for (backend, args) in [("native", &["--native"][..]), ("vm", &[][..])] {
            let (_, stderr, code) = run(&scratch, &file, args, None);
            if code == 0 || !stderr.contains("E087") {
                failures.push(format!(
                    "{name} on {backend}: exit {code}\n{}",
                    clip(&stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The largest values that fit, 255 fields or captures, still build and run
/// on both backends.
#[test]
fn a_value_with_255_fields_runs() {
    let scratch = Scratch::new("debug-heap-widest-values");
    let mut failures = Vec::new();
    for (name, program, printed) in wide_values(255) {
        let path = scratch.write(&format!("widest_{name}.flx"), &program);
        let file = path.to_string_lossy();
        for (backend, args) in [("native", &["--native"][..]), ("vm", &[][..])] {
            let (stdout, stderr, code) = run(&scratch, &file, args, None);
            if code != 0 || stdout.lines().last() != Some(printed.as_str()) {
                failures.push(format!(
                    "{name} on {backend}: exit {code}, printed {stdout:?}, want {printed}\n{}",
                    clip(&stderr)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const PERFORM_PER_TURN: &str = r"effect Ask {
    ask : (ARG) -> Int
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let k = perform Ask.ask(SENT)
        count(n - 1, total + k)
    }
}

fn run() -> Int {
    count(500, 0) handle Ask {
        ask(resume, x) -> resume(READ)
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// A perform gives its argument to the clause. The same loop sending a
/// fresh list or its length leaves the same blocks behind: a perform that
/// copied its argument kept every list it sent.
#[test]
fn a_perform_takes_the_argument_it_is_given() {
    let scratch = Scratch::new("debug-heap-perform-argument");
    let program = |arg: &str, sent: &str, read: &str| {
        PERFORM_PER_TURN
            .replace("ARG", arg)
            .replace("SENT", sent)
            .replace("READ", read)
    };
    let (_, _, list) = stats_of(
        &scratch,
        "list.flx",
        &program("List<Int>", "[n, n + 1, n + 2]", "len(x)"),
    );
    let (_, _, int) = stats_of(
        &scratch,
        "int.flx",
        &program("Int", "len([n, n + 1, n + 2])", "x"),
    );
    assert_eq!(list, int, "live blocks: list sent vs its length sent");
}

const RESUME_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let k = PERFORM
        count(n - 1, total + k)
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> CLAUSE
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// The continuation a clause resumes is released once the clause is done:
/// one captured frame, two frames joined into one resume, and a resume
/// that hands the clause back its later work. Nothing heap-allocated lives
/// across the perform, so each turn leaves nothing behind.
#[test]
fn a_resumed_continuation_is_released() {
    let scratch = Scratch::new("debug-heap-resume-continuation");
    let failures: Vec<String> = [
        ("one_frame", "perform Ask.ask(n)", "resume(x)"),
        ("two_frames", "get(n)", "resume(x)"),
        ("later_work", "perform Ask.ask(n)", "resume(x) + 0"),
    ]
    .into_iter()
    .filter_map(|(name, perform, clause)| {
        let program = RESUME_PER_TURN
            .replace("PERFORM", perform)
            .replace("CLAUSE", clause);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const READ_AFTER_PERFORM: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let xs = numbers(n)
        let k = PERFORM
        count(n - 1, total + k + len(xs))
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> CLAUSE
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// Live blocks of `program` at 3 turns and at 4, for a clause that resumes
/// twice: each turn doubles the work, so the loop stays a few turns deep.
fn live_at_three_and_four(scratch: &Scratch, name: &str, program: &str) -> (u64, u64) {
    let at = |n: u64| {
        let source = program.replace("SIZE", &n.to_string());
        stats_of(scratch, &format!("{name}_{n}.flx"), &source).2
    };
    (at(3), at(4))
}

/// A list made before a perform and read after it is released after that
/// read, however the clause resumes. A clause that resumes twice runs the
/// rest of the turn twice, and each run releases the reference it was
/// given.
#[test]
fn a_value_read_after_a_perform_is_released() {
    let scratch = Scratch::new("debug-heap-read-after-perform");
    let program = |clause: &str| {
        READ_AFTER_PERFORM
            .replace("PERFORM", "perform Ask.ask(n)")
            .replace("CLAUSE", clause)
    };
    let mut failures: Vec<String> = [("one_frame", "resume(x)"), ("later_work", "resume(x) + 0")]
        .into_iter()
        .filter_map(|(name, clause)| {
            let (_, _, live) = per_thousand_cells(&scratch, name, &program(clause));
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    let (three, four) =
        live_at_three_and_four(&scratch, "resumed_twice", &program("resume(x) + resume(x)"));
    if three != four {
        failures.push(format!(
            "resumed_twice: {three} blocks live at 3 turns, {four} at 4"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const ABORT_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn attempt(n) -> Int with Ask {
    let xs = numbers(n)
    BODY
}

fn try(n) -> Int {
    attempt(n) handle Ask {
        ask(resume, x) -> 0
    }
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + try(n)) }
}

fn main() with IO {
    print(to_string(count(500, 0)))
}
";

const BORROWED_ACROSS_A_YIELD: &str = r"effect Ask {
    ask : (Int) -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n) + 1
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn walk(xs, n) -> Int with Ask {
    let k = get(n)
    k + len(xs)
}

fn count(n, total) -> Int with Ask {
    if n == 0 { total } else {
        let xs = numbers(n)
        let r = walk(xs, n)
        count(n - 1, total + r + len(xs))
    }
}

fn run() -> Int {
    count(SIZE, 0) handle Ask {
        ask(resume, x) -> resume(x) + resume(x)
    }
}

fn main() with IO {
    print(to_string(run()))
}
";

/// A call that performs inside it yields through the caller's frame, which
/// becomes a continuation. The frame's own values move into it and each
/// resume copies only those, so a list the frame made is released after
/// its last read however the clause resumes, and a list it only borrows is
/// never copied. A clause that never resumes frees the continuation, and
/// with it the frame's list: it leaves no more behind than a frame that
/// released the list before the call.
#[test]
fn a_value_live_across_a_yielding_call_is_released() {
    let scratch = Scratch::new("debug-heap-across-a-yield");
    let program = |clause: &str| {
        READ_AFTER_PERFORM
            .replace("PERFORM", "get(n)")
            .replace("CLAUSE", clause)
    };
    let mut failures: Vec<String> = [("two_frames", "resume(x)"), ("later_work", "resume(x) + 0")]
        .into_iter()
        .filter_map(|(name, clause)| {
            let (_, _, live) = per_thousand_cells(&scratch, name, &program(clause));
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    for (name, program) in [
        ("resumed_twice", program("resume(x) + resume(x)")),
        ("borrowed", BORROWED_ACROSS_A_YIELD.to_string()),
    ] {
        let (three, four) = live_at_three_and_four(&scratch, name, &program);
        if three != four {
            failures.push(format!(
                "{name}: {three} blocks live at 3 turns, {four} at 4"
            ));
        }
    }
    let abort = |body: &str| ABORT_PER_TURN.replace("BODY", body);
    let (_, _, kept) = stats_of(
        &scratch,
        "abort_kept.flx",
        &abort("let k = get(n)\n    k + len(xs)"),
    );
    let (_, _, released) = stats_of(
        &scratch,
        "abort_released.flx",
        &abort("let m = len(xs)\n    get(n) + m"),
    );
    if kept != released {
        failures.push(format!(
            "abort: {kept} blocks live with the list kept across the call, {released} without"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const HANDLE_PER_TURN: &str = r"effect Ask {
    ask : (Int) -> Int
}

effect Counter {
    next : () -> Int
}

fn get(n) -> Int with Ask {
    perform Ask.ask(n)
}

fn count_twice() -> Int with Counter {
    let a = perform Counter.next()
    a + perform Counter.next()
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn attempt(n) -> Int {
    HANDLE
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + attempt(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A `handle` releases what it installed once its body is done: the clause,
/// with what the clause captured, and the handler chain it made. A clause
/// with state gives the next one to `resume`, which replaces it, and can
/// still read the state it was called with afterwards.
#[test]
fn a_handle_releases_its_clause_and_state() {
    let scratch = Scratch::new("debug-heap-handle-per-turn");
    let failures: Vec<String> = [
        (
            "plain",
            "get(n) handle Ask {\n        ask(resume, x) -> resume(x + 1)\n    }",
        ),
        (
            "captures",
            "let xs = numbers(n)\n    get(n) handle Ask {\n        ask(resume, x) -> resume(x + len(xs))\n    }",
        ),
        (
            "state",
            "count_twice() handle Counter(numbers(n)) {\n        next(resume, state) -> resume(len(state), [len(state) | state])\n    }",
        ),
        (
            "state_read_after_resume",
            "count_twice() handle Counter(numbers(n)) {\n        next(resume, state) -> resume(1, [0]) + len(state)\n    }",
        ),
    ]
    .into_iter()
    .filter_map(|(name, handle)| {
        let program = HANDLE_PER_TURN.replace("HANDLE", handle);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const LOCAL_FUNCTION_PER_TURN: &str = r"fn turn(n) {
    LOCAL
}

fn loop(n, total) {
    if n == 0 { total } else { loop(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(loop(SIZE, 0)))
}
";

/// A local recursive function calls itself directly, so it builds no
/// closure of itself on entry: one per entry was left behind. The same for
/// local functions that call each other, and for one that captures.
#[test]
fn a_local_function_leaves_no_closure_behind() {
    let scratch = Scratch::new("debug-heap-local-function");
    let failures: Vec<String> = [
        (
            "own",
            "fn go(i, acc) {\n        if i == 0 { acc } else { go(i - 1, acc + i) }\n    }\n    let r = go(3, n)\n    r + 0",
        ),
        (
            "captures",
            "let k = n % 2\n    fn go(i, acc) {\n        if i == 0 { acc } else { go(i - 1, acc + i + k) }\n    }\n    let r = go(3, n)\n    r + 0",
        ),
        (
            "group",
            "fn even(i) {\n        if i == 0 { 1 } else { odd(i - 1) }\n    }\n    fn odd(i) {\n        if i == 0 { 0 } else { even(i - 1) }\n    }\n    let r = even(n % 4)\n    r + 0",
        ),
    ]
    .into_iter()
    .filter_map(|(name, local)| {
        let program = LOCAL_FUNCTION_PER_TURN.replace("LOCAL", local);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const WIDE_ARRAY: &str = r"import Flow.Array exposing (concat)

fn build(lo, hi) {
    if hi - lo == 1 { [| [lo] |] } else {
        let mid = (lo + hi) / 2
        concat(build(lo, mid), build(mid, hi))
    }
}

fn main() with IO {
    print(len(build(0, SIZE)))
}
";

/// An array releases every element, each a list of its own. Its header
/// counted the elements to release in one byte, capped at 255, so the rest
/// stayed live; 70000 is past the 65535 a 16-bit index reaches too (KI-123).
/// Halving builds it without the quadratic copying of a `push` per element.
#[test]
fn an_array_releases_every_element() {
    let scratch = Scratch::new("debug-heap-wide-array");
    let at = |n: u64| {
        stats_of(
            &scratch,
            &format!("wide_array_{n}.flx"),
            &WIDE_ARRAY.replace("SIZE", &n.to_string()),
        )
        .2
    };
    let (one, wide) = (at(1), at(70000));
    assert_eq!(
        wide, one,
        "an array of 70000 lists left {wide} blocks live, one of 1 left {one}"
    );
}

const MAP_PER_TURN: &str = r"import Flow.Map exposing (set, delete, merge, size)

fn size_of(m) { size(m) }

fn wide(i, n, m) {
    if i == 0 { m } else { wide(i - 1, n, set(m, i, [n + i])) }
}

fn turn(n) {
    TURN
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A map releases its nodes, keys and values, and maps made from one
/// another release what they share once both are gone. A release freed
/// nothing under the root, which was all that kept the uncounted sharing
/// between maps safe; a map of 100 keys has branches inside branches
/// (KI-124).
#[test]
fn a_map_releases_its_nodes_keys_and_values() {
    let scratch = Scratch::new("debug-heap-map");
    let failures: Vec<String> = [
        ("built", r#"size_of(set(set({}, "a", [n]), "b", [n + 1]))"#),
        ("literal", r#"size_of({"a": [n], "b": [n + 1]})"#),
        (
            "shared",
            r#"let m1 = set({}, "a", [n])
    let m2 = set(m1, "c", [n])
    size_of(m1) + size_of(m2)"#,
        ),
        (
            "deleted",
            r#"size_of(delete(set(set({}, "a", [n]), "b", [n]), "a"))"#,
        ),
        (
            "merged",
            r#"size_of(merge(set({}, "a", [n]), set({}, "b", [n])))"#,
        ),
        ("wide", "size_of(wide(100, n, {}))"),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = MAP_PER_TURN.replace("TURN", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const ALIAS_PER_TURN: &str = r"import Flow.Array as Array
import Flow.List as List
import Flow.Map as Map

fn turn(n) {
    TURN
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + turn(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
";

/// A call through a module alias releases what it was lent. The callee was
/// hoisted into a temporary, so Aether gave the arguments away to an
/// unknown function, and a module function that only borrows them, such as
/// `Map.set`, never released them (KI-130). `List.reverse` takes its
/// argument, so it was right before.
#[test]
fn a_call_through_a_module_alias_releases_its_arguments() {
    let scratch = Scratch::new("debug-heap-alias");
    let failures: Vec<String> = [
        ("array_push", "len(Array.push([| [n] |], [n + 1]))"),
        (
            "map_set",
            r#"Map.size(Map.set(Map.set({}, "a", [n]), "b", [n + 1]))"#,
        ),
        (
            "map_get",
            r#"let m = Map.set({}, "a", [n])
    match Map.get(m, "a") { Some(xs) -> len(xs), _ -> 0 }"#,
        ),
        ("list_reverse", "len(List.reverse([n, n + 1]))"),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = ALIAS_PER_TURN.replace("TURN", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const FAILED_FS_CALLS: &str = r#"import Flow.Fs as Fs
import Flow.IoError as Io

fn kind(r) with IO {
    match r {
        Ok(_) -> println("ok"),
        Err(e) -> println(Io.kind_name(Io.error_kind(e))),
    }
}

fn main() with IO {
    let base = "target/test-scratch/debug_heap_failed_fs"
    kind(Fs.remove_file(base + "/absent.txt"))
    kind(Fs.rename(base + "/absent", base + "/t"))
    kind(Fs.write_file(base + "/no/parent/f.txt", "x"))
    kind(Fs.remove_dir_all(base + "/absent"))
    kind(Fs.read_file(base + "/absent.txt"))
}
"#;

/// A failed file operation's error holds the path it was lent: the error
/// took it without counting it, so the caller's release and the error's
/// freed it twice (KI-131). `read_file` builds its error apart from the
/// others.
#[test]
fn a_failed_file_operation_counts_the_path_its_error_holds() {
    let scratch = Scratch::new("debug-heap-failed-fs");
    let (allocs, frees, live) = stats_of(&scratch, "failed_fs.flx", FAILED_FS_CALLS);
    assert_eq!(
        live, 1,
        "{allocs} allocated, {frees} freed: only the block every program that prints leaves should be live (KI-119)"
    );
}

const PANIC_PER_TURN: &str = r#"import Flow.Assert exposing (try_catch)

effect Ask {
    ask : (Int) -> Int
}

effect Tell {
    tell : (Int) -> Int
}

fn numbers(n) {
    [n, n + 1, n + 2]
}

fn explode(n) -> Int {
    if n >= 0 { panic("boom") } else { n }
}

fn boom(n) -> Int {
    let xs = numbers(n)
    let r = explode(n)
    r + len(xs)
}

fn outer(n) -> Int {
    let ys = numbers(n)
    let r = boom(n)
    r + len(ys)
}

fn before_perform(n) -> Int with Ask {
    explode(n) + perform Ask.ask(n)
}

fn after_resume(n) -> Int with Ask {
    perform Ask.ask(n) + explode(n)
}

fn under_two(n) -> Int with Ask, Tell {
    perform Tell.tell(n) + perform Ask.ask(n) + explode(n)
}

fn attempt(n) -> Int {
    let caught = try_catch(\() -> BODY)
    1
}

fn count(n, total) {
    if n == 0 { total } else { count(n - 1, total + attempt(n)) }
}

fn main() with IO {
    print(to_string(count(SIZE, 0)))
}
"#;

/// A panic caught further up releases what the frames between held. It
/// jumped straight to the catch point, past every release still to come in
/// those frames, so `boom`'s list stayed live (KI-126). What a skipped
/// `handle` builds is the effect runtime's, not a frame's (KI-133).
#[test]
#[cfg_attr(
    windows,
    ignore = "Windows catches a panic with SEH, which runs no landing pads"
)]
fn a_caught_panic_releases_what_the_frames_it_skipped_held() {
    let scratch = Scratch::new("debug-heap-caught-panic");
    let failures: Vec<String> = [("held_list", "boom(n)"), ("two_frames", "outer(n)")]
        .into_iter()
        .filter_map(|(name, body)| {
            let program = PANIC_PER_TURN.replace("BODY", body);
            let (_, _, live) = per_thousand_cells(&scratch, name, &program);
            (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A panic caught further up releases what a `handle` it skipped built:
/// the evidence the handle installed, which only its prompt released, and
/// the prompt's own references to the clause's `resume` and argument
/// (KI-133).
#[test]
#[cfg_attr(
    windows,
    ignore = "Windows catches a panic with SEH, which runs no landing pads"
)]
fn a_caught_panic_releases_what_a_skipped_handle_built() {
    let scratch = Scratch::new("debug-heap-skipped-handle");
    let failures: Vec<String> = [
        (
            "in_body",
            "before_perform(n) handle Ask { ask(resume, x) -> resume(x + 1) }",
        ),
        (
            "after_resume",
            "after_resume(n) handle Ask { ask(resume, x) -> resume(x + 1) }",
        ),
        (
            "nested",
            "(under_two(n) handle Ask { ask(resume, x) -> resume(x + 1) }) handle Tell { tell(resume, x) -> resume(x) }",
        ),
    ]
    .into_iter()
    .filter_map(|(name, body)| {
        let program = PANIC_PER_TURN.replace("BODY", body);
        let (_, _, live) = per_thousand_cells(&scratch, name, &program);
        (live != 0).then(|| format!("{name}: {live} blocks left live per 1000 turns"))
    })
    .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
