//! Verify one step of a change: lint, the full suite, the corpus sweeps and
//! backend parity. Each run's outputs are tagged, so a step's results compare
//! with the previous step's.
//!
//! Outputs go to the run's own folder, `.verify/<tag>/`, which git
//! ignores (see `store.rs`):
//! - `test.txt`: the suite
//! - `sweep_cfg/`: every example compiled with `--dump-cfg`
//! - `sweep_core/`: ... with `--dump-core`
//! - `sweep_core-opt/`: ... with `--optimize --dump-core`
//! - `parity.txt`: vm, vm_cached, vm_strict, vm_opt, llvm and llvm_opt
//!   compared
//! - `parity_g.txt`: the guide natively with and without `-g`
//! - `parity_debug.txt`: the VM debugger and lldb compared
//! - `parity_heap.txt`: native runs on the debug heap against plain ones,
//!   with the files that leak
//! - `summary.txt`: each step's result and time, for a later `--base`
//!
//! With `--base`, each sweep is compared with the same sweep of the earlier
//! tag. A sweep's base must come from the same tools, so run one on main
//! first: check out main, build, and `cargo xtask verify main --skip-suite`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::compare;
use crate::metrics;
use crate::outcome::{
    Outcome, Steps, TOTAL, format_duration, parse_summary_file, summary_is_partial,
};
use crate::parity::{ParityCounts, parity, parity_with_env};
use crate::proc::{run, run_tee};
use crate::store;
use crate::sweep::{self, Mode};
use crate::text::Paint;

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    /// Fail when a sweep's dumps differ from the base's, for a change that
    /// claims to change no output (0190 S4).
    pub cmp: Cmp,
    /// The checks that run, in [`Check::ALL`]'s order (`choose_checks`).
    pub checks: Vec<Check>,
    /// How many files each sweep reruns to check its dumps are reproducible
    /// (0190 §9); 0 checks none.
    pub det_sample: usize,
    /// Pass the metrics step when counts are worse than `--base`'s, noting
    /// them instead, for a change that costs them on purpose (0190 §12).
    pub accept_metrics: bool,
}

/// Which changed dumps `--cmp` fails on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// Changed dumps are noted, not failed.
    Off,
    /// `--cmp`: any changed dump fails.
    Any,
    /// `--cmp=structure`: only a changed program fails, not a renumbering,
    /// for a change that renumbers on purpose.
    Structure,
}

impl Cmp {
    /// Whether `comparison`'s changed dumps fail the step.
    fn fails(self, comparison: &compare::Comparison) -> bool {
        match self {
            Self::Off => false,
            Self::Any => !comparison.changed.is_empty(),
            Self::Structure => comparison.changed.len() > comparison.numbering,
        }
    }
}

/// One check a run can take, named for `--only` and `--skip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Fmt,
    Clippy,
    Suite,
    /// The five sweeps, their determinism checks, the internal-error tally
    /// and the comparisons with the base.
    Sweeps,
    Parity,
    /// The counts from parity's vm runs against the base's.
    Metrics,
    DebugBuildParity,
    DebugParity,
    HeapParity,
}

impl Check {
    /// Every check, in the order a run takes them.
    pub const ALL: [Check; 9] = [
        Check::Fmt,
        Check::Clippy,
        Check::Suite,
        Check::Sweeps,
        Check::Parity,
        Check::Metrics,
        Check::DebugBuildParity,
        Check::DebugParity,
        Check::HeapParity,
    ];

    pub fn parse(name: &str) -> Option<Check> {
        Check::ALL.into_iter().find(|check| check.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Check::Fmt => "fmt",
            Check::Clippy => "clippy",
            Check::Suite => "suite",
            Check::Sweeps => "sweeps",
            Check::Parity => "parity",
            Check::Metrics => "metrics",
            Check::DebugBuildParity => "debug-build-parity",
            Check::DebugParity => "debug-parity",
            Check::HeapParity => "heap-parity",
        }
    }
}

/// The checks a run takes, from `--only` and `--skip` (each a list of
/// comma-separated names, `--skip-suite` and `--skip-sweeps` already added
/// to `skip`). A check that needs another, or a flag that needs a check,
/// is an error here rather than a step that quietly does nothing.
pub fn choose_checks(
    only: &[String],
    skip: &[String],
    base: bool,
    cmp: Cmp,
    accept_metrics: bool,
) -> Result<Vec<Check>, String> {
    let names = |lists: &[String]| -> Result<Vec<Check>, String> {
        lists
            .iter()
            .flat_map(|list| list.split(','))
            .map(|name| {
                Check::parse(name.trim()).ok_or_else(|| {
                    let known: Vec<&str> = Check::ALL.iter().map(|c| c.name()).collect();
                    format!("unknown check {name}; the checks are {}", known.join(", "))
                })
            })
            .collect()
    };
    if !only.is_empty() && !skip.is_empty() {
        return Err("--only or --skip (--skip-suite, --skip-sweeps), not both".into());
    }
    let (only, skip) = (names(only)?, names(skip)?);
    let checks: Vec<Check> = Check::ALL
        .into_iter()
        .filter(|check| if only.is_empty() { !skip.contains(check) } else { only.contains(check) })
        // Without a base there are no counts to compare with, so a full
        // run leaves metrics out; only naming it is an error.
        .filter(|check| *check != Check::Metrics || base || only.contains(check))
        .collect();
    let runs = |check| checks.contains(&check);
    if checks.is_empty() {
        return Err("nothing to run".into());
    }
    if runs(Check::Metrics) && !base {
        return Err("metrics compares the counts with --base's, so it needs --base".into());
    }
    if runs(Check::Metrics) && !runs(Check::Parity) {
        return Err("metrics compares the counts parity writes, so it needs parity".into());
    }
    if accept_metrics && !runs(Check::Metrics) {
        return Err(
            "--accept-metrics accepts counts worse than --base's, so it needs --base and metrics"
                .into(),
        );
    }
    if cmp != Cmp::Off && (!base || !runs(Check::Sweeps)) {
        return Err(
            "--cmp compares the sweeps with --base's, so it needs --base and the sweeps".into()
        );
    }
    Ok(checks)
}

impl Options {
    pub fn runs(&self, check: Check) -> bool {
        self.checks.contains(&check)
    }

    /// The checks a full run with the same base takes that this one does
    /// not.
    fn not_run(&self) -> Vec<Check> {
        Check::ALL
            .into_iter()
            .filter(|check| *check != Check::Metrics || self.base.is_some())
            .filter(|check| !self.runs(*check))
            .collect()
    }

    /// Whether the suite does not build `target/debug/flux` but a later
    /// check runs it.
    fn needs_build(&self) -> bool {
        !self.runs(Check::Suite)
            && [
                Check::Sweeps,
                Check::Parity,
                Check::DebugBuildParity,
                Check::DebugParity,
                Check::HeapParity,
            ]
            .into_iter()
            .any(|check| self.runs(check))
    }
}

/// How many numbered steps a run with `options` takes; the summary is not one.
fn step_count(options: &Options) -> usize {
    let sweeps = if options.runs(Check::Sweeps) {
        // Each sweep and its determinism check, the internal-error tally,
        // and each comparison.
        let compares = if options.base.is_some() { SWEEPS.len() } else { 0 };
        let checks = if options.det_sample > 0 { SWEEPS.len() } else { 0 };
        SWEEPS.len() + checks + 1 + compares
    } else {
        0
    };
    // fmt and clippy share a step; the suite or a build; then one step per
    // parity check, and with a base the comparison of the counts.
    let lint = usize::from(options.runs(Check::Fmt) || options.runs(Check::Clippy));
    let suite = usize::from(options.runs(Check::Suite) || options.needs_build());
    let later = [
        Check::Parity,
        Check::Metrics,
        Check::DebugBuildParity,
        Check::DebugParity,
        Check::HeapParity,
    ]
    .into_iter()
    .filter(|check| options.runs(*check))
    .count();
    lint + suite + sweeps + later
}

pub(crate) fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

pub fn flux_binary(root: &Path) -> PathBuf {
    root.join("target/debug").join(format!("flux{}", std::env::consts::EXE_SUFFIX))
}

/// The summary file a run saves in its folder, for a later run to compare
/// with.
fn summary_file(run: &Path) -> PathBuf {
    run.join("summary.txt")
}

/// Returns whether every check passed.
pub fn run_verify(root: &Path, options: &Options) -> Result<bool, String> {
    let out = store::run_dir(root, &options.tag)?;
    let base_out = options.base.as_deref().map(|base| store::run_dir(root, base)).transpose()?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("outputs: {}", store::display(&out));
    let mut outcome = Outcome::new();
    let mut steps = Steps::new(step_count(options));
    let io = |e: std::io::Error| e.to_string();

    // Lint and test with the flags CI uses, so the llvm-feature tests run too.
    let lint: Vec<&str> = [Check::Fmt, Check::Clippy]
        .into_iter()
        .filter(|check| options.runs(*check))
        .map(Check::name)
        .collect();
    if !lint.is_empty() {
        steps.start(&lint.join(", "));
    }
    if options.runs(Check::Fmt) {
        let started = Instant::now();
        let fmt =
            run(cargo().current_dir(root).args(["fmt", "--all", "--", "--check"])).map_err(io)?;
        if fmt != 0 {
            println!("FMT: run cargo fmt");
        }
        outcome.record("fmt", fmt == 0, "fmt".into(), started.elapsed());
    }

    if options.runs(Check::Clippy) {
        let started = Instant::now();
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
        let clippy_findings = grep_with_context(&clippy_lines, 6, 20);
        for line in &clippy_findings {
            println!("{line}");
        }
        outcome.record("clippy", clippy == 0, "clippy".into(), started.elapsed()).details =
            clippy_findings
                .iter()
                .filter(|line| line.starts_with("warning") || line.starts_with("error"))
                .map(|line| (*line).to_string())
                .collect();
    }

    if options.needs_build() {
        // The suite builds target/debug/flux with every feature; without it,
        // build it here, for the sweeps and parity.
        let started = steps.start("build");
        let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
            .map_err(io)?;
        outcome.record("build", build == 0, "build".into(), started.elapsed());
        if build != 0 {
            return Ok(finish(&mut outcome, &out, base_out.as_deref(), options));
        }
    } else if options.runs(Check::Suite) {
        let started = steps.start("suite");
        let log = out.join("test.txt");
        let _ = fs::remove_file(&log);
        let tests = run_tee(
            cargo().current_dir(root).env("INSTA_FORCE_PASS", "1").args([
                "test",
                "--all",
                "--all-features",
                "--no-fail-fast",
            ]),
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
        let failed: Vec<String> = text
            .lines()
            .filter(|l| l.starts_with("test result: FAILED"))
            .map(str::to_string)
            .collect();
        for line in &failed {
            println!("{line}");
        }
        let mut snaps = Vec::new();
        find_snap_new(root, root, &mut snaps);
        snaps.sort();
        for snap in &snaps {
            println!("{snap}");
        }
        let entry =
            outcome.record("tests", tests == 0, format!("tests (exit {tests})"), started.elapsed());
        if tests != 0 {
            entry.details = capped(&failed, &log);
        }
        outcome.note("snapshots", format!("{} .snap.new file(s) to review", snaps.len()), None);
    }

    let flux = flux_binary(root);
    if options.runs(Check::Sweeps) {
        let sweeps = SWEEPS.map(|mode| (sweep_dir(mode), mode));
        let mut internal = Vec::new();
        for (dir, mode) in &sweeps {
            let started = steps.start(&format!("sweep {}", mode.name()));
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
            let took = started.elapsed();
            println!("took {}", format_duration(took));
            let step = format!("sweep {}", mode.name());
            outcome.note(&step, step.clone(), Some(took));
            if options.det_sample > 0 {
                check_determinism(
                    root,
                    &out.join(dir),
                    *mode,
                    &results,
                    options,
                    &mut steps,
                    &mut outcome,
                );
            }
            let count = results.iter().filter(|r| compare::INTERNAL.contains(&r.status)).count();
            internal.push((*mode, count));
        }

        steps.start("internal errors");
        for (mode, count) in &internal {
            let line = format!("{}: {count} internal error(s)", mode.name());
            println!("{line}");
            outcome.note(&format!("internal {}", mode.name()), line, None);
        }

        if let (Some(base), Some(base_out)) = (&options.base, &base_out) {
            for (dir, mode) in &sweeps {
                let started = steps.start(&format!("compare {dir} with {base}"));
                let base_dir = base_out.join(dir);
                let step = format!("compare {}", mode.name());
                let head_dir = out.join(dir);
                if !base_dir.is_dir() {
                    // A base from before this mode was swept.
                    let line = format!("no {} sweep in {base} to compare with", mode.name());
                    println!("{line}");
                    outcome.note(&step, line, None);
                    continue;
                }
                match compare::run(root, &base_dir, &head_dir, true) {
                    Ok(comparison) => {
                        let changed = comparison.changed.len();
                        let fails_on_content = options.cmp.fails(&comparison);
                        let entry = outcome.record(
                            &step,
                            !comparison.failed && !fails_on_content,
                            format!("compare {dir} with {base} ({changed} dump(s) changed)"),
                            started.elapsed(),
                        );
                        let diff_file = compare::Comparison::diff_file(&head_dir);
                        if fails_on_content {
                            entry.details = capped(&comparison.changed, &diff_file);
                        } else if changed > 0 {
                            outcome.note(
                                &format!("dumps {}", mode.name()),
                                format!(
                                    "{changed} {} dump(s) changed, {} only in numbering{}; --cmp fails on that ({})",
                                    mode.name(),
                                    comparison.numbering,
                                    if comparison.unknown > 0 {
                                        format!(
                                            ", {} unknown ({base} has no normalized hashes)",
                                            comparison.unknown
                                        )
                                    } else {
                                        String::new()
                                    },
                                    diff_file.display()
                                ),
                                None,
                            );
                        }
                    }
                    Err(e) => {
                        println!("{e}");
                        outcome.record(
                            &step,
                            false,
                            format!("compare {dir} with {base}: {e}"),
                            started.elapsed(),
                        );
                    }
                }
            }
        }
    }

    if options.runs(Check::Parity) {
        let started = steps.start("parity");
        println!(
            "each fixture runs on the VM (fresh, cached, strict, optimized) and natively (plain, optimized); their outputs must agree"
        );
        let log_path = out.join("parity.txt");
        let mut log = open_log(&log_path).map_err(io)?;
        let mut counts = ParityCounts::default();
        let mut failed_runs = 0;
        let dirs = ["examples/type_classes", "examples/generics", "tests/flux", "tests/parity"];
        let mut metric_parts = Vec::new();
        for (n, dir) in dirs.iter().enumerate() {
            println!("{}", Paint::detect().bold(&format!("=== {dir}")));
            writeln!(log, "=== {dir}").map_err(io)?;
            // The vm and vm_opt runs write their counts, for the metrics step.
            let part = out.join(format!("metrics_{n}.tsv"));
            let extra = ["--metrics".to_string(), part.to_string_lossy().into_owned()];
            let run = parity_with_env(root, &flux, dir, MAIN_WAYS, true, &mut log, &[], &extra)
                .map_err(io)?;
            if run.exit != 0 {
                failed_runs += 1;
            }
            counts.absorb(run);
            metric_parts.push(part);
        }
        let metrics_file = out.join("metrics.tsv");
        join_metrics(&metric_parts, &metrics_file).map_err(io)?;
        let took = started.elapsed();
        println!("took {}", format_duration(took));
        // A known divergence is recorded in its fixture with `// skip-way:` and
        // a KI (0190 S2), so anything left here is new and fails the run.
        let ok = counts.mismatches == 0 && counts.expected == 0 && failed_runs == 0;
        let entry = outcome.record(
        "parity",
        ok,
        format!(
            "parity ({} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, {failed_runs} of {} dir(s) exited non-zero)",
            counts.mismatches,
            counts.expected,
            dirs.len()
        ),
        took,
    );
        if !ok {
            entry.details = capped(&counts.failures, &log_path);
        }
        note_unexpected_passes(&mut outcome, "parity", &counts, &log_path);

        if options.runs(Check::Metrics)
            && let (Some(base), Some(base_out)) = (&options.base, &base_out)
        {
            let started = steps.start(&format!("metrics against {base}"));
            compare_metrics(&mut outcome, options, base, base_out, &out, &metrics_file, started)
                .map_err(io)?;
        }
    }

    // A debug build runs as a plain one does (0188 S9c): every guide
    // example prints the same natively with and without `-g`.
    if options.runs(Check::DebugBuildParity) {
        let started = steps.start("debug build parity");
        println!("every guide example built natively with and without -g; output must agree");
        let log_path = out.join("parity_g.txt");
        let mut log = open_log(&log_path).map_err(io)?;
        let counts =
            parity(root, &flux, "examples/guide", "llvm,llvm_g", true, &mut log).map_err(io)?;
        record_parity(&mut outcome, "debug build parity", &counts, &log_path, started);
    }

    // The VM debugger and lldb on the same scripted sessions (0188 S6b).
    // Every fixture is expected to match, so a mismatch fails the run.
    if options.runs(Check::DebugParity) {
        let started = steps.start("debug parity");
        println!("each // dbg: script run in the VM debugger and in lldb; transcripts must agree");
        let log_path = out.join("parity_debug.txt");
        let mut log = open_log(&log_path).map_err(io)?;
        let counts =
            parity(root, &flux, "tests/parity_debug", "vm_debug,llvm_debug", true, &mut log)
                .map_err(io)?;
        record_parity(&mut outcome, "debug parity", &counts, &log_path, started);
    }

    // The debug heap (0190 S15): each native run again on a heap that stops
    // at a use after free and names the blocks live at exit. A fault or a
    // different result fails; a leak is a note while nearly every program
    // leaks (KI-119, KI-121).
    if options.runs(Check::HeapParity) {
        let started = steps.start("heap parity");
        println!(
            "each fixture natively on the plain heap and on the debug heap; output must agree, leaks are counted"
        );
        let log_path = out.join("parity_heap.txt");
        let mut log = open_log(&log_path).map_err(io)?;
        let mut counts = ParityCounts::default();
        let mut failed_runs = 0;
        let dirs = ["tests/parity", "examples/aether"];
        for dir in dirs {
            println!("{}", Paint::detect().bold(&format!("=== {dir}")));
            writeln!(log, "=== {dir}").map_err(io)?;
            let run = parity(root, &flux, dir, "llvm,llvm_heap", true, &mut log).map_err(io)?;
            if run.exit != 0 {
                failed_runs += 1;
            }
            counts.absorb(run);
        }
        let took = started.elapsed();
        println!("took {}", format_duration(took));
        let ok = counts.mismatches == 0 && counts.expected == 0 && failed_runs == 0;
        let entry = outcome.record(
        "heap parity",
        ok,
        format!(
            "heap parity ({} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, {} file(s) leak, {failed_runs} of {} dir(s) exited non-zero)",
            counts.mismatches,
            counts.expected,
            counts.leaks.len(),
            dirs.len()
        ),
        took,
    );
        if !ok {
            entry.details = capped(&counts.failures, &log_path);
        }
        note_unexpected_passes(&mut outcome, "heap parity", &counts, &log_path);
        if let Some(line) = listed_note(&counts.leaks, "LEAK ", "file(s) leak", &log_path) {
            outcome.note("heap parity", line, None);
        }
    }

    Ok(finish(&mut outcome, &out, base_out.as_deref(), options))
}

/// Records a one-directory parity step, listing its failures when it failed.
fn record_parity(
    outcome: &mut Outcome,
    step: &str,
    counts: &ParityCounts,
    log: &Path,
    started: Instant,
) {
    let took = started.elapsed();
    println!("took {}", format_duration(took));
    let entry = outcome.record(step, counts.ok(), format!("{step} ({})", counts.describe()), took);
    if !counts.ok() {
        entry.details = capped(&counts.failures, log);
    }
    note_unexpected_passes(outcome, step, counts, log);
}

/// A note, never a failure, for skipped ways that now pass: their KI may be
/// fixed, perhaps on another branch (0190 §8).
fn note_unexpected_passes(outcome: &mut Outcome, step: &str, counts: &ParityCounts, log: &Path) {
    if let Some(line) = unexpected_note(&counts.unexpected, log) {
        outcome.note(step, line, None);
    }
}

/// `2 skipped way(s) now pass: <first>, <second>`, the lines cut to their
/// file, way and KI; past three, how many more and the log to read.
fn unexpected_note(lines: &[String], log: &Path) -> Option<String> {
    listed_note(lines, "UNEXPECTED_PASS ", "skipped way(s) now pass", log)
}

/// `<n> <what>: <first>; <second>; <third>; <more> more in <log>`, each line
/// without its `prefix`; `None` when there are no lines.
fn listed_note(lines: &[String], prefix: &str, what: &str, log: &Path) -> Option<String> {
    const SHOWN: usize = 3;
    if lines.is_empty() {
        return None;
    }
    let mut named: Vec<&str> =
        lines.iter().take(SHOWN).map(|line| line.strip_prefix(prefix).unwrap_or(line)).collect();
    let more = lines.len().saturating_sub(SHOWN);
    let rest = (more > 0).then(|| format!("{more} more in {}", log.display()));
    named.extend(rest.as_deref());
    Some(format!("{} {what}: {}", lines.len(), named.join("; ")))
}

/// Joins parity-check's `--metrics` files, one per directory, into `into`
/// with a single header, and removes them. A directory whose run wrote none
/// adds nothing.
fn join_metrics(parts: &[PathBuf], into: &Path) -> std::io::Result<()> {
    let mut joined = format!("{}\n", metrics::HEADER);
    for part in parts {
        let Ok(text) = fs::read_to_string(part) else { continue };
        for line in text.lines().skip(1) {
            joined.push_str(line);
            joined.push('\n');
        }
        fs::remove_file(part)?;
    }
    fs::write(into, joined)
}

/// The metrics step (0190 §12): the head's counts against `base`'s. A count
/// that got worse fails it, unless `--accept-metrics`, which notes them
/// instead. Every changed count is listed in `metrics_diff.txt`.
fn compare_metrics(
    outcome: &mut Outcome,
    options: &Options,
    base: &str,
    base_out: &Path,
    out: &Path,
    head_file: &Path,
    started: Instant,
) -> std::io::Result<()> {
    let Ok(base_text) = fs::read_to_string(base_out.join("metrics.tsv")) else {
        let line = format!("metrics: {base} kept no counts (recorded before 0190 S11)");
        println!("{line}");
        outcome.record("metrics", true, line, started.elapsed());
        return Ok(());
    };
    let head = metrics::parse(&fs::read_to_string(head_file)?);
    let report = metrics::compare(&metrics::parse(&base_text), &head);
    let diff_file = out.join("metrics_diff.txt");
    let listing: String = report
        .worse
        .iter()
        .map(|line| format!("worse   {line}\n"))
        .chain(report.better.iter().map(|line| format!("better  {line}\n")))
        .collect();
    fs::write(&diff_file, listing)?;

    let (worse, better) = (report.worse.len(), report.better.len());
    let line = format!(
        "metrics ({worse} count(s) worse than {base}, {better} better, {} only on one side)",
        report.unmatched
    );
    println!("{line}");
    let fails = worse > 0 && !options.accept_metrics;
    let entry = outcome.record("metrics", !fails, line, started.elapsed());
    if fails {
        entry.details = capped(&report.worse, &diff_file);
    } else if let Some(note) =
        listed_note(&report.worse, "", "count(s) worse, accepted by --accept-metrics", &diff_file)
    {
        outcome.note("metrics accepted", note, None);
    }
    if let Some(note) = listed_note(&report.better, "", "count(s) better", &diff_file) {
        outcome.note("metrics better", note, None);
    }
    Ok(())
}

/// At most [`MAX_DETAILS`] of `items`, then how many more, then the log to
/// read for the rest.
fn capped(items: &[String], log: &Path) -> Vec<String> {
    let mut details: Vec<String> = items.iter().take(MAX_DETAILS).cloned().collect();
    if items.len() > MAX_DETAILS {
        details.push(format!("… and {} more", items.len() - MAX_DETAILS));
    }
    details.push(format!("log: {}", log.display()));
    details
}

const MAX_DETAILS: usize = 10;

/// Prints the summary, comparing times with the base run's saved summary
/// when there is one, and saves this run's. Returns whether every step
/// passed.
fn finish(outcome: &mut Outcome, out: &Path, base_out: Option<&Path>, options: &Options) -> bool {
    let paint = Paint::detect();
    let not_run = options.not_run();
    for check in &not_run {
        outcome.skip(check.name(), format!("{} (not run)", check.name()));
    }
    let base_times: Option<HashMap<String, Duration>> = base_out.and_then(|base_out| {
        let path = summary_file(base_out);
        let text = fs::read_to_string(&path).ok();
        if text.is_none() {
            println!(
                "{}",
                paint.dim(&format!("no {} to compare times with", store::display(&path)))
            );
        }
        text.map(|text| {
            let mut times = parse_summary_file(&text);
            // Two runs that took different checks have totals that do not
            // compare; their common steps still do.
            if !not_run.is_empty() || summary_is_partial(&text) {
                times.remove(TOTAL);
            }
            times
        })
    });
    outcome.print(&options.tag, &store::display(out), base_times.as_ref());
    let path = summary_file(out);
    if !not_run.is_empty() {
        let names: Vec<&str> = not_run.iter().map(|check| check.name()).collect();
        println!(
            "{}",
            paint.yellow(&format!(
                "partial run: {} of {} checks not run ({}); a merge needs the full verify",
                not_run.len(),
                not_run.len() + options.checks.len(),
                names.join(", ")
            ))
        );
        if fs::read_to_string(&path).is_ok_and(|text| !summary_is_partial(&text)) {
            println!(
                "{}",
                paint.yellow(&format!(
                    "replaces {}, from a run that skipped no check",
                    store::display(&path)
                ))
            );
        }
    }
    if let Err(e) = fs::write(&path, outcome.to_file()) {
        println!("could not save {}: {e}", path.display());
    } else {
        println!("{}", paint.dim(&format!("summary saved: {}", store::display(&path))));
    }
    println!("all-done: {}", options.tag);
    !outcome.failed()
}

const SWEEPS: [Mode; 5] = [Mode::Cfg, Mode::Core, Mode::CoreOpt, Mode::Aether, Mode::Lir];

/// Reruns a sample of the sweep in `dir` with the compiler it ran, and
/// fails the step when any dump comes out different: then some output
/// depends on more than its input, such as `HashMap` order (0190 §9).
fn check_determinism(
    root: &Path,
    dir: &Path,
    mode: Mode,
    results: &[sweep::FileResult],
    options: &Options,
    steps: &mut Steps,
    outcome: &mut Outcome,
) {
    let step = format!("determinism {}", mode.name());
    let started = steps.start(&step);
    let sample = sweep::determinism_sample(results, options.det_sample);
    let differing =
        sweep::recheck(root, dir, mode, &sample, sweep::default_jobs(), Duration::from_secs(120));
    let took = started.elapsed();
    println!("took {}", format_duration(took));
    let entry = outcome.record(
        &step,
        differing.is_empty(),
        format!("{step} ({} of {} rerun file(s) differ)", differing.len(), sample.len()),
        took,
    );
    entry.details = differing
        .iter()
        .take(MAX_DETAILS)
        .map(|(file, first, again)| {
            format!("{file} [{}]", compare::Change::of(first, again).name())
        })
        .collect();
}

/// The main parity step's ways. The opt ways run every fixture under
/// `--optimize`, so a miscompile shows up as a difference from the plain way
/// (0190 S3).
const MAIN_WAYS: &str = "vm,vm_cached,vm_strict,vm_opt,llvm,llvm_opt";

/// The folder a sweep in `mode` writes to, in a run's folder.
fn sweep_dir(mode: Mode) -> String {
    format!("sweep_{}", mode.name())
}

/// Creates a step's log file and says where it is.
fn open_log(path: &Path) -> std::io::Result<fs::File> {
    println!("{}", Paint::detect().dim(&format!("  log: {}", store::display(path))));
    fs::File::create(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// `^\s+Running `: cargo's line naming each test binary.
fn is_running_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() < line.len() && trimmed.starts_with("Running ")
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
        assert_eq!(grep_with_context(&lines, 1, 10), ["warning: x", "b", "error: y", "e"]);
        assert_eq!(grep_with_context(&lines, 6, 3), ["warning: x", "b", "c"]);
    }

    #[test]
    fn unexpected_passes_are_noted_three_at_a_time() {
        let log = Path::new("parity.txt");
        assert_eq!(unexpected_note(&[], log), None);
        let lines: Vec<String> = (1..=4)
            .map(|n| format!("UNEXPECTED_PASS f{n}.flx vm: KI-10{n} may be fixed"))
            .collect();
        assert_eq!(
            unexpected_note(&lines[..1], log).as_deref(),
            Some("1 skipped way(s) now pass: f1.flx vm: KI-101 may be fixed")
        );
        let note = unexpected_note(&lines, log).expect("four lines");
        assert!(note.starts_with("4 skipped way(s) now pass: f1.flx"));
        assert!(note.ends_with("f3.flx vm: KI-103 may be fixed; 1 more in parity.txt"));
    }

    #[test]
    fn leaking_files_are_noted_without_their_prefix() {
        let lines = ["LEAK a.flx llvm_heap: 5 blocks live at exit, first x".to_string()];
        assert_eq!(
            listed_note(&lines, "LEAK ", "file(s) leak", Path::new("parity_heap.txt")).as_deref(),
            Some("1 file(s) leak: a.flx llvm_heap: 5 blocks live at exit, first x")
        );
    }

    #[test]
    fn each_sweep_has_its_own_folder_in_a_run() {
        assert_eq!(sweep_dir(Mode::Cfg), "sweep_cfg");
        assert_eq!(sweep_dir(Mode::CoreOpt), "sweep_core-opt");
    }

    #[test]
    fn counts_the_steps_each_option_leaves() {
        let options = |only: &[&str], skip: &[&str], base: Option<&str>| {
            let list = |names: &[&str]| -> Vec<String> {
                names.iter().map(|name| name.to_string()).collect()
            };
            Options {
                tag: "t".into(),
                base: base.map(str::to_string),
                cmp: Cmp::Off,
                checks: choose_checks(&list(only), &list(skip), base.is_some(), Cmp::Off, false)
                    .expect("valid checks"),
                det_sample: 50,
                accept_metrics: false,
            }
        };
        // fmt+clippy, suite or build, then four parity steps.
        assert_eq!(step_count(&options(&[], &["suite", "sweeps"], None)), 6);
        // With a base, the counts are compared too.
        assert_eq!(step_count(&options(&[], &["sweeps"], Some("main"))), 7);
        // Five sweeps, each with its determinism check, and the
        // internal-error tally.
        assert_eq!(step_count(&options(&[], &[], None)), 17);
        // And five comparisons with the base, and the counts'.
        assert_eq!(step_count(&options(&[], &["suite"], Some("main"))), 23);
        let unchecked = Options { det_sample: 0, ..options(&[], &[], None) };
        assert_eq!(step_count(&unchecked), 12);
        // One parity check needs the compiler built first.
        assert_eq!(step_count(&options(&["debug-build-parity"], &[], None)), 2);
        // Lint alone builds nothing.
        assert_eq!(step_count(&options(&["clippy"], &[], None)), 1);
    }

    fn choose(only: &str, skip: &str, base: bool) -> Result<Vec<Check>, String> {
        let list = |names: &str| -> Vec<String> {
            names.split(' ').filter(|n| !n.is_empty()).map(str::to_string).collect()
        };
        choose_checks(&list(only), &list(skip), base, Cmp::Off, false)
    }

    #[test]
    fn with_no_flags_every_check_runs_and_metrics_only_with_a_base() {
        let all = choose("", "", true).unwrap();
        assert_eq!(all, Check::ALL);
        let without_base = choose("", "", false).unwrap();
        assert!(!without_base.contains(&Check::Metrics));
        assert_eq!(without_base.len(), Check::ALL.len() - 1);
    }

    #[test]
    fn only_runs_its_checks_in_run_order() {
        assert_eq!(
            choose("heap-parity,fmt parity", "", false).unwrap(),
            [Check::Fmt, Check::Parity, Check::HeapParity]
        );
    }

    #[test]
    fn skip_runs_the_rest() {
        let checks = choose("", "suite,sweeps heap-parity", false).unwrap();
        assert_eq!(
            checks,
            [Check::Fmt, Check::Clippy, Check::Parity, Check::DebugBuildParity, Check::DebugParity]
        );
    }

    #[test]
    fn a_check_that_needs_another_or_a_base_is_refused() {
        let error = |only: &str, skip: &str, base: bool| choose(only, skip, base).unwrap_err();
        assert!(error("nope", "", false).starts_with("unknown check nope; the checks are fmt,"));
        assert!(error("fmt", "suite", false).contains("not both"));
        assert_eq!(
            error(
                "",
                "fmt,clippy,suite,sweeps,parity,debug-build-parity,debug-parity,heap-parity",
                false
            ),
            "nothing to run"
        );
        assert!(error("parity,metrics", "", false).contains("needs --base"));
        assert!(error("metrics", "", true).contains("needs parity"));
        assert!(error("", "parity", true).contains("needs parity"));
        // Skipping metrics with a base is fine: the counts are only kept.
        assert!(choose("", "metrics", true).is_ok());
    }

    #[test]
    fn a_flag_that_needs_a_check_is_refused_without_it() {
        let only =
            |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
        let parity = only(&["parity"]);
        assert!(
            choose_checks(&parity, &[], true, Cmp::Off, true)
                .unwrap_err()
                .contains("--accept-metrics")
        );
        assert!(
            choose_checks(&[], &[], false, Cmp::Off, true)
                .unwrap_err()
                .contains("--accept-metrics")
        );
        assert!(choose_checks(&[], &[], true, Cmp::Off, true).is_ok());
        assert!(choose_checks(&parity, &[], true, Cmp::Any, false).unwrap_err().contains("--cmp"));
        assert!(
            choose_checks(&[], &[], false, Cmp::Structure, false).unwrap_err().contains("--cmp")
        );
        assert!(choose_checks(&[], &[], true, Cmp::Any, false).is_ok());
    }

    #[test]
    fn a_run_that_leaves_out_a_check_lists_what_it_left_out() {
        let options = |checks: Vec<Check>, base: Option<&str>| Options {
            tag: "t".into(),
            base: base.map(str::to_string),
            cmp: Cmp::Off,
            checks,
            det_sample: 50,
            accept_metrics: false,
        };
        assert!(options(choose("", "", false).unwrap(), None).not_run().is_empty());
        assert!(options(choose("", "", true).unwrap(), Some("main")).not_run().is_empty());
        let partial = options(choose("", "metrics,heap-parity", true).unwrap(), Some("main"));
        assert_eq!(partial.not_run(), [Check::Metrics, Check::HeapParity]);
    }

    #[test]
    fn cmp_structure_fails_only_on_a_changed_program() {
        let comparison = |changed: usize, numbering| compare::Comparison {
            failed: false,
            changed: (0..changed).map(|n| format!("f{n}.flx")).collect(),
            numbering,
            unknown: 0,
        };
        assert!(!Cmp::Off.fails(&comparison(3, 0)));
        assert!(Cmp::Any.fails(&comparison(2, 2)));
        assert!(!Cmp::Structure.fails(&comparison(2, 2)));
        assert!(Cmp::Structure.fails(&comparison(3, 2)));
        assert!(!Cmp::Any.fails(&comparison(0, 0)));
    }

    #[test]
    fn caps_the_failures_listed_and_points_at_the_log() {
        let failures: Vec<String> = (0..12).map(|i| format!("MISMATCH f{i}.flx")).collect();
        let details = capped(&failures, Path::new("out/parity_t.txt"));
        assert_eq!(details.len(), MAX_DETAILS + 2);
        assert_eq!(details[MAX_DETAILS], "… and 2 more");
        assert_eq!(details.last().unwrap(), "log: out/parity_t.txt");
        assert_eq!(capped(&[], Path::new("l")), ["log: l"]);
    }

    #[test]
    fn recognises_running_lines_only_when_indented() {
        assert!(is_running_line("     Running unittests src/lib.rs"));
        assert!(!is_running_line("Running"));
    }
}
