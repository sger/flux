//! Verify one step of a change: lint, the full suite, the corpus sweeps and
//! backend parity. Each run's outputs are tagged, so a step's results compare
//! with the previous step's.
//!
//! Outputs go to `.git/parked/out/` (untracked, survives `cargo clean`):
//! - `test_<tag>.txt`: the suite
//! - `sweep_<tag>/`: every example compiled with `--dump-cfg`
//! - `dump_<tag>__dump_core/`: ... with `--dump-core`
//! - `dump_<tag>__optimize___dump_core/`: ... with `--optimize --dump-core`
//! - `parity_<tag>.txt`: vm, vm_cached, vm_strict and llvm compared
//!
//! With `--base`, each sweep is compared with the same sweep of the earlier
//! tag. A sweep's base must come from the same tools, so run one on main
//! first: check out main, build, and `cargo xtask verify main --skip-suite`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::compare;
use crate::proc::{run, run_tee};
use crate::sweep::{self, Mode};
use crate::text::strip_ansi;

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    pub skip_suite: bool,
    pub skip_sweeps: bool,
}

/// What failed, for the closing summary.
#[derive(Default)]
struct Outcome {
    lines: Vec<String>,
    failed: bool,
}

impl Outcome {
    fn record(&mut self, ok: bool, line: String) {
        self.failed |= !ok;
        self.lines
            .push(format!("{} {line}", if ok { "ok  " } else { "FAIL" }));
    }

    fn note(&mut self, line: String) {
        self.lines.push(format!("     {line}"));
    }
}

fn section(title: &str) {
    println!("== {title}");
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

pub fn flux_binary(root: &Path) -> PathBuf {
    root.join("target/debug")
        .join(format!("flux{}", std::env::consts::EXE_SUFFIX))
}

/// Returns whether every check passed.
pub fn run_verify(root: &Path, options: &Options) -> Result<bool, String> {
    let tag = &options.tag;
    let out = root.join(".git/parked/out");
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut outcome = Outcome::default();
    let io = |e: std::io::Error| e.to_string();

    // Lint and test with the flags CI uses, so the llvm-feature tests run too.
    section("fmt, clippy");
    let fmt = run(cargo()
        .current_dir(root)
        .args(["fmt", "--all", "--", "--check"]))
    .map_err(io)?;
    if fmt != 0 {
        println!("FMT: run cargo fmt");
    }
    outcome.record(fmt == 0, "fmt".into());

    let mut clippy_lines = Vec::new();
    let clippy = run_tee(
        cargo().current_dir(root).args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ]),
        None,
        |line| clippy_lines.push(line.to_string()),
    )
    .map_err(io)?;
    for line in grep_with_context(&clippy_lines, 6, 20) {
        println!("{line}");
    }
    outcome.record(clippy == 0, "clippy".into());

    if !options.skip_suite {
        section("suite");
        let log = out.join(format!("test_{tag}.txt"));
        let _ = fs::remove_file(&log);
        let tests = run_tee(
            cargo()
                .current_dir(root)
                .env("INSTA_FORCE_PASS", "1")
                .args(["test", "--all", "--all-features", "--no-fail-fast"]),
            Some(&log),
            |line| {
                if is_running_line(line) || line.starts_with("test result") {
                    println!("{line}");
                }
            },
        )
        .map_err(io)?;
        println!("tests exit {tests}");
        let text = fs::read_to_string(&log).unwrap_or_default();
        for line in text
            .lines()
            .filter(|l| l.starts_with("test result: FAILED"))
        {
            println!("{line}");
        }
        let mut snaps = Vec::new();
        find_snap_new(root, root, &mut snaps);
        snaps.sort();
        for snap in &snaps {
            println!("{snap}");
        }
        outcome.record(tests == 0, format!("tests (exit {tests})"));
        outcome.note(format!("{} .snap.new file(s) to review", snaps.len()));
    } else {
        // The suite builds target/debug/flux with every feature; without it,
        // build it here, for the sweeps and parity.
        let build = run(cargo()
            .current_dir(root)
            .args(["build", "--all-features", "--quiet"]))
        .map_err(io)?;
        outcome.record(build == 0, "build".into());
        if build != 0 {
            return Ok(false);
        }
    }

    let flux = flux_binary(root);
    if !options.skip_sweeps {
        let sweeps = [
            (format!("sweep_{tag}"), Mode::Cfg),
            (format!("dump_{tag}__dump_core"), Mode::Core),
            (format!("dump_{tag}__optimize___dump_core"), Mode::CoreOpt),
        ];
        let mut internal = Vec::new();
        for (dir, mode) in &sweeps {
            section(&format!("sweep {dir} ({})", mode.name()));
            let results = sweep::run(
                root,
                &sweep::Options {
                    out_dir: out.join(dir),
                    mode: *mode,
                    files: None,
                    jobs: sweep::default_jobs(),
                    timeout: Duration::from_secs(120),
                    flux: flux.clone(),
                },
            )?;
            let count = results
                .iter()
                .filter(|r| compare::INTERNAL.contains(&r.status))
                .count();
            internal.push((dir, count));
        }

        section("internal errors");
        for (dir, count) in &internal {
            println!("{dir}: {count}");
            outcome.note(format!("{dir}: {count} internal"));
        }

        if let Some(base) = &options.base {
            for (dir, _) in &sweeps {
                section(&format!("compare {dir} with {base}"));
                let base_dir = out.join(dir.replace(tag.as_str(), base));
                match compare::run(&base_dir, &out.join(dir)) {
                    Ok(failed) => outcome.record(!failed, format!("compare {dir} with {base}")),
                    Err(e) => {
                        println!("{e}");
                        outcome.record(false, format!("compare {dir} with {base}: {e}"));
                    }
                }
            }
        }
    }

    section("parity");
    let mut log = fs::File::create(out.join(format!("parity_{tag}.txt"))).map_err(io)?;
    let mut mismatches = 0;
    for dir in [
        "examples/type_classes",
        "examples/generics",
        "tests/flux",
        "tests/parity",
    ] {
        println!("=== {dir}");
        writeln!(log, "=== {dir}").map_err(io)?;
        let mut lines = Vec::new();
        run_tee(
            Command::new(&flux).current_dir(root).args([
                "parity-check",
                dir,
                "--ways",
                "vm,vm_cached,vm_strict,llvm",
            ]),
            None,
            |line| lines.push(line.to_string()),
        )
        .map_err(io)?;
        for line in lines.iter().filter(|l| !is_cargo_progress(l)) {
            writeln!(log, "{line}").map_err(io)?;
            let plain = strip_ansi(line);
            if is_parity_line(&plain) {
                println!("{plain}");
            }
            if plain.starts_with("MISMATCH") {
                mismatches += 1;
            }
        }
    }
    outcome.note(format!(
        "parity: {mismatches} MISMATCH line(s); compare with the base's parity log"
    ));

    section(&format!("summary {tag}"));
    for line in &outcome.lines {
        println!("{line}");
    }
    println!("all-done: {tag}");
    Ok(!outcome.failed)
}

/// `^\s+Running `: cargo's line naming each test binary.
fn is_running_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() < line.len() && trimmed.starts_with("Running ")
}

/// `^\s+Compiling|^\s+Finished`.
fn is_cargo_progress(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() < line.len()
        && (trimmed.starts_with("Compiling") || trimmed.starts_with("Finished"))
}

fn is_parity_line(line: &str) -> bool {
    [
        "PASS", "MISMATCH", "EXPECTED", "SKIP", "Total", "Pass", "Mismatch",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

/// The lines starting `warning` or `error`, each with the `after` lines that
/// follow it, as `grep -A`; at most `limit` lines.
fn grep_with_context(lines: &[String], after: usize, limit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut until = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("warning") || line.starts_with("error") {
            until = i + after + 1;
        }
        if i < until {
            out.push(line.as_str());
            if out.len() == limit {
                break;
            }
        }
    }
    out
}

/// Every `*.snap.new` under `dir`, outside `target` and `.git`.
fn find_snap_new(root: &Path, dir: &Path, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" {
                find_snap_new(root, &path, found);
            }
        } else if name.to_string_lossy().ends_with(".snap.new") {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            found.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_matches_with_their_context_up_to_the_limit() {
        let lines: Vec<String> = ["a", "warning: x", "b", "c", "d", "error: y", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            grep_with_context(&lines, 1, 10),
            ["warning: x", "b", "error: y", "e"]
        );
        assert_eq!(grep_with_context(&lines, 6, 3), ["warning: x", "b", "c"]);
    }

    #[test]
    fn recognises_cargo_lines_only_when_indented() {
        assert!(is_running_line("     Running unittests src/lib.rs"));
        assert!(!is_running_line("Running"));
        assert!(is_cargo_progress("   Compiling flux v0.0.7"));
        assert!(!is_cargo_progress("Finished"));
    }
}
