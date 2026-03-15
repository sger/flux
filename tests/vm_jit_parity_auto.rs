//! Automated VM/JIT parity test.
//!
//! Scans example directories and verifies that every `.flx` file produces
//! identical exit codes and stdout between the bytecode VM and the Cranelift
//! JIT backend.  Files listed in `JIT_EXCLUDE` are known JIT gaps and are
//! skipped (they should be removed from the list as gaps are fixed).
//!
//! This test is the safety net that prevents VM/JIT divergence from sneaking
//! in through new examples or compiler changes.

#![cfg(feature = "jit")]

use std::path::Path;
use std::process::Command;

/// Example directories to scan for `.flx` files.
const EXAMPLE_DIRS: &[&str] = &[
    "examples/basics",
    "examples/advanced",
    "examples/functions",
    "examples/patterns",
    "examples/tail_call",
    "examples/perf",
    "examples/primop",
];

/// Files with known JIT gaps — excluded from automatic parity checks.
/// Each entry should reference the tracking issue or proposal.
/// Remove entries as gaps are fixed.
const JIT_EXCLUDE: &[&str] = &[
    // Pattern validation with complex match arms
    "examples/patterns/pattern_validation_happy_path.flx",
    // VM print uses Display (adds quotes around strings), JIT does not
    "examples/patterns/match_wildcard_non_last_error.flx",
    "examples/patterns/match_non_exhaustive_error.flx",
    // VM uses [|...|] for arrays and (a, b) for tuples; JIT uses [...] and [a, b]
    "examples/advanced/list_map_filter.flx",
];

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_flux(file: &str, jit: bool) -> (i32, String, String) {
    let mut args = vec!["--no-cache", file];
    if jit {
        args.push("--jit");
    }
    let output = Command::new(flux_bin())
        .current_dir(workspace_root())
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for `{file}` (jit={jit}): {e}"));

    let status = output.status.code().unwrap_or(-1);
    let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    (status, stdout, stderr)
}

fn collect_flx_files() -> Vec<String> {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in EXAMPLE_DIRS {
        let dir_path = root.join(dir);
        if !dir_path.is_dir() {
            continue;
        }
        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("flx") {
                    Some(format!("{}/{}", dir, path.file_name()?.to_str()?))
                } else {
                    None
                }
            })
            .collect();
        entries.sort();
        files.extend(entries);
    }
    files
}

#[test]
fn vm_jit_parity_auto_examples() {
    let files = collect_flx_files();
    assert!(
        !files.is_empty(),
        "no .flx files found — check EXAMPLE_DIRS paths"
    );

    let mut tested = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    for file in &files {
        if JIT_EXCLUDE.iter().any(|exc| file.ends_with(exc)) {
            skipped += 1;
            continue;
        }

        let (vm_status, vm_stdout, _vm_stderr) = run_flux(file, false);
        let (jit_status, jit_stdout, jit_stderr) = run_flux(file, true);

        if vm_status != jit_status {
            failures.push(format!(
                "{file}: exit code mismatch (vm={vm_status}, jit={jit_status})\n  JIT stderr: {}",
                jit_stderr.lines().next().unwrap_or("<empty>")
            ));
        } else if vm_status == 0 && vm_stdout != jit_stdout {
            failures.push(format!(
                "{file}: stdout mismatch\n  VM:  {}\n  JIT: {}",
                vm_stdout.lines().next().unwrap_or("<empty>"),
                jit_stdout.lines().next().unwrap_or("<empty>")
            ));
        }
        // For error cases (vm_status != 0), we only check exit code parity,
        // not stderr text — error formatting may differ between backends.

        tested += 1;
    }

    if !failures.is_empty() {
        panic!(
            "VM/JIT parity failures ({} of {} tested, {} skipped):\n\n{}",
            failures.len(),
            tested,
            skipped,
            failures.join("\n\n")
        );
    }

    // Sanity: ensure we actually tested a meaningful number of files.
    assert!(
        tested >= 50,
        "only {tested} files tested — expected at least 50 (found {} total, {skipped} excluded)",
        files.len()
    );
}
