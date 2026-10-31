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

use crate::support::scratch;
use scratch::Scratch;
use scratch::workspace_root;

const FIXTURE: &str = "tests/debug/heap/use_after_free.flx";

/// What `flux <file> <args>` printed (stdout, stderr) and its exit code,
/// with `fault` as `FLUX_AETHER_FAULT` when given.
fn run(scratch: &Scratch, file: &str, args: &[&str], fault: Option<&str>) -> (String, String, i32) {
    let env: Vec<(&str, &str)> =
        fault.map(|fault| ("FLUX_AETHER_FAULT", fault)).into_iter().collect();
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
        .args(scratch.seeded_cache_args())
        .env("NO_COLOR", "1")
        .env_remove("FLUX_AETHER_FAULT")
        .env_remove("FLUX_HEAP_STATS")
        .envs(env.iter().copied());
    let output = command.output().unwrap_or_else(|e| panic!("failed to run flux on {file}: {e}"));
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    (text(&output.stdout), text(&output.stderr), output.status.code().unwrap_or(-1))
}

/// The debug heap's counts (`FLUX_HEAP_STATS`): blocks allocated, freed, and
/// still live at exit.
fn heap_stats(stderr: &str) -> Option<(u64, u64, u64)> {
    let line = stderr.lines().find_map(|line| line.strip_prefix("heap: "))?;
    let mut parts = line.split(' ');
    let mut count = |key: &str| parts.next()?.strip_prefix(key)?.parse().ok();
    Some((count("allocs=")?, count("frees=")?, count("live=")?))
}

/// Runs `check` on each of `files` across up to `workers` threads, each
/// with its own scratch cache: what it reported, sorted.
fn failures_in_parallel<F>(prefix: &str, files: &[String], workers: usize, check: F) -> Vec<String>
where
    F: Fn(&Scratch, &str) -> Option<String> + Sync,
{
    let workers =
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(workers).min(files.len());
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
        handles.into_iter().flat_map(|handle| handle.join().expect("a worker")).collect()
    });
    failures.sort();
    failures
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

/// `(allocs, frees, live)` of `source` run on the debug heap.
fn stats_of(scratch: &Scratch, name: &str, source: &str) -> (u64, u64, u64) {
    let path = scratch.write(name, source);
    let file = path.to_string_lossy();
    let (_, stderr, code) =
        run_env(scratch, &file, &["--native", "--debug-heap"], &[("FLUX_HEAP_STATS", "1")]);
    assert_eq!(code, 0, "{name}:\n{stderr}");
    heap_stats(&stderr).unwrap_or_else(|| panic!("{name}:\n{stderr}"))
}

/// `stats_of(program)` at 2000 cells minus at 1000.
fn per_thousand_cells(scratch: &Scratch, name: &str, program: &str) -> (u64, u64, u64) {
    let at = |n: u64| {
        stats_of(scratch, &format!("{name}_{n}.flx"), &program.replace("SIZE", &n.to_string()))
    };
    let (small, large) = (at(1000), at(2000));
    let diff = |a: u64, b: u64| {
        b.checked_sub(a)
            .unwrap_or_else(|| panic!("{name}: 1000 cells {small:?}, 2000 cells {large:?}"))
    };
    (diff(small.0, large.0), diff(small.1, large.1), diff(small.2, large.2))
}

mod collections_and_panics;
mod effects;
mod leaks_and_moves;
mod rebuild;
mod reuse;
