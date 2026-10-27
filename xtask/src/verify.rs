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
use crate::outcome::{Outcome, Steps, format_duration, parse_summary_file};
use crate::proc::{run, run_tee};
use crate::store;
use crate::sweep::{self, Mode};
use crate::text::{Paint, StatusLine, strip_ansi};

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    /// Fail when a sweep's dumps differ from the base's, for a change that
    /// claims to change no output (0190 S4).
    pub cmp: Cmp,
    pub skip_suite: bool,
    pub skip_sweeps: bool,
    /// How many files each sweep reruns to check its dumps are reproducible
    /// (0190 §9); 0 checks none.
    pub det_sample: usize,
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

/// How many numbered steps a run with `options` takes; the summary is not one.
fn step_count(options: &Options) -> usize {
    let sweeps = if options.skip_sweeps {
        0
    } else {
        // Each sweep and its determinism check, the internal-error tally,
        // and each comparison.
        let compares = if options.base.is_some() { SWEEPS.len() } else { 0 };
        let checks = if options.det_sample > 0 { SWEEPS.len() } else { 0 };
        SWEEPS.len() + checks + 1 + compares
    };
    // fmt and clippy; the suite or a build; then the four parity steps.
    2 + sweeps + 4
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
    steps.start("fmt, clippy");
    let started = Instant::now();
    let fmt = run(cargo().current_dir(root).args(["fmt", "--all", "--", "--check"])).map_err(io)?;
    if fmt != 0 {
        println!("FMT: run cargo fmt");
    }
    outcome.record("fmt", fmt == 0, "fmt".into(), started.elapsed());

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

    if options.skip_suite {
        // The suite builds target/debug/flux with every feature; without it,
        // build it here, for the sweeps and parity.
        let started = steps.start("build");
        let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
            .map_err(io)?;
        outcome.record("build", build == 0, "build".into(), started.elapsed());
        if build != 0 {
            return Ok(finish(&outcome, &out, base_out.as_deref(), options));
        }
    } else {
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
    if !options.skip_sweeps {
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

    let started = steps.start("parity");
    println!(
        "each fixture runs on the VM (fresh, cached, strict, optimized) and natively (plain, optimized); their outputs must agree"
    );
    let log_path = out.join("parity.txt");
    let mut log = open_log(&log_path).map_err(io)?;
    let mut counts = ParityCounts::default();
    let mut failed_runs = 0;
    let dirs = ["examples/type_classes", "examples/generics", "tests/flux", "tests/parity"];
    for dir in dirs {
        println!("{}", Paint::detect().bold(&format!("=== {dir}")));
        writeln!(log, "=== {dir}").map_err(io)?;
        let run = parity(root, &flux, dir, MAIN_WAYS, true, &mut log).map_err(io)?;
        if run.exit != 0 {
            failed_runs += 1;
        }
        counts.absorb(run);
    }
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

    // A debug build runs as a plain one does (0188 S9c): every guide
    // example prints the same natively with and without `-g`.
    let started = steps.start("debug build parity");
    println!("every guide example built natively with and without -g; output must agree");
    let log_path = out.join("parity_g.txt");
    let mut log = open_log(&log_path).map_err(io)?;
    let counts =
        parity(root, &flux, "examples/guide", "llvm,llvm_g", true, &mut log).map_err(io)?;
    record_parity(&mut outcome, "debug build parity", &counts, &log_path, started);

    // The VM debugger and lldb on the same scripted sessions (0188 S6b).
    // Every fixture is expected to match, so a mismatch fails the run.
    let started = steps.start("debug parity");
    println!("each // dbg: script run in the VM debugger and in lldb; transcripts must agree");
    let log_path = out.join("parity_debug.txt");
    let mut log = open_log(&log_path).map_err(io)?;
    let counts = parity(root, &flux, "tests/parity_debug", "vm_debug,llvm_debug", true, &mut log)
        .map_err(io)?;
    record_parity(&mut outcome, "debug parity", &counts, &log_path, started);

    // The debug heap (0190 S15): each native run again on a heap that stops
    // at a use after free and names the blocks live at exit. A fault, a
    // different result or a leak no `expect-leak` marker allows fails; the
    // leaks a marker allows are listed in a note.
    let started = steps.start("heap parity");
    println!(
        "each fixture natively on the plain heap and on the debug heap; output must agree, and a leak needs an expect-leak marker"
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

    Ok(finish(&outcome, &out, base_out.as_deref(), options))
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
fn finish(outcome: &Outcome, out: &Path, base_out: Option<&Path>, options: &Options) -> bool {
    let paint = Paint::detect();
    let base_times: Option<HashMap<String, Duration>> = base_out.and_then(|base_out| {
        let path = summary_file(base_out);
        let text = fs::read_to_string(&path).ok();
        if text.is_none() {
            println!(
                "{}",
                paint.dim(&format!("no {} to compare times with", store::display(&path)))
            );
        }
        text.map(|text| parse_summary_file(&text))
    });
    outcome.print(&options.tag, &store::display(out), base_times.as_ref());
    let path = summary_file(out);
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

/// A fixture's verdict, the first word of the line `print_result` writes
/// for it. One list of the words, so counting, numbering and colouring
/// cannot disagree about what a verdict line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Skip,
    Mismatch,
    ExpectedOutputMismatch,
}

impl Verdict {
    const ALL: [Self; 4] = [Self::Pass, Self::Skip, Self::Mismatch, Self::ExpectedOutputMismatch];

    fn word(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Skip => "SKIP",
            Self::Mismatch => "MISMATCH",
            Self::ExpectedOutputMismatch => "EXPECTED_OUTPUT_MISMATCH",
        }
    }

    /// The verdict `line` (without colour) reports, if it is a verdict line.
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let (word, _) = line.split_once(' ')?;
        Self::ALL.into_iter().find(|v| v.word() == word)
    }

    /// `line` with its verdict word coloured: pass green, skip yellow,
    /// either mismatch red.
    fn paint(self, paint: Paint, line: &str) -> String {
        let word = self.word();
        let painted = match self {
            Self::Pass => paint.green(word),
            Self::Skip => paint.yellow(word),
            Self::Mismatch | Self::ExpectedOutputMismatch => paint.red(word),
        };
        format!("{painted}{}", &line[word.len()..])
    }
}

/// What one `parity-check` run reported.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ParityCounts {
    /// Fixtures whose ways disagreed (`MISMATCH`).
    pub(crate) mismatches: usize,
    /// Fixtures whose ways agreed but missed the fixture's declared
    /// outcome or expected stdout (`EXPECTED_OUTPUT_MISMATCH`).
    pub(crate) expected: usize,
    /// `parity-check`'s exit code: 1 on any failure, when nothing passed,
    /// or when it could not run at all.
    pub(crate) exit: i32,
    /// Each failing verdict line, as it arrived.
    pub(crate) failures: Vec<String>,
    /// Each `UNEXPECTED_PASS` line: a skipped way that now passes.
    unexpected: Vec<String>,
    /// Each `LEAK` line: a file with blocks live at exit on a heap way.
    leaks: Vec<String>,
}

impl ParityCounts {
    /// Adds another directory's counts and lines to these, for a step over
    /// several; the caller counts the runs that exited non-zero.
    pub(crate) fn absorb(&mut self, run: ParityCounts) {
        self.mismatches += run.mismatches;
        self.expected += run.expected;
        self.failures.extend(run.failures);
        self.unexpected.extend(run.unexpected);
        self.leaks.extend(run.leaks);
    }

    /// Counts `verdict`, and keeps `line` when it is a failing one.
    fn tally(&mut self, verdict: Verdict, line: &str) {
        match verdict {
            Verdict::Mismatch => self.mismatches += 1,
            Verdict::ExpectedOutputMismatch => self.expected += 1,
            Verdict::Pass | Verdict::Skip => return,
        }
        self.failures.push(line.to_string());
    }

    pub(crate) fn ok(&self) -> bool {
        self.mismatches == 0 && self.expected == 0 && self.exit == 0
    }

    fn describe(&self) -> String {
        format!(
            "{} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, exit {}",
            self.mismatches, self.expected, self.exit
        )
    }
}

/// Runs `flux parity-check <dir> --ways <ways>`, writing its output, without
/// cargo's progress lines, to `log`. Prints, as they arrive, its `[parity]`
/// lines (such as rebuilding its binaries) and each verdict, numbered and
/// timed, so a long run shows where it is. On a terminal a passing fixture
/// only redraws one status line, so what scrolls is what needs attention.
/// Returns the failing verdicts and the exit code. With `check_skips`, the
/// skipped ways run too and report `UNEXPECTED_PASS` (§8); `rewrite` turns it
/// off, since a rewritten copy passing says nothing about the KI.
pub(crate) fn parity(
    root: &Path,
    flux: &Path,
    dir: &str,
    ways: &str,
    check_skips: bool,
    log: &mut fs::File,
) -> std::io::Result<ParityCounts> {
    parity_with_env(root, flux, dir, ways, check_skips, log, &[])
}

/// [`parity`], with `env` set for `parity-check` and the runs it starts.
pub(crate) fn parity_with_env(
    root: &Path,
    flux: &Path,
    dir: &str,
    ways: &str,
    check_skips: bool,
    log: &mut fs::File,
    env: &[(&str, &str)],
) -> std::io::Result<ParityCounts> {
    let started = Instant::now();
    let paint = Paint::detect();
    let mut status = StatusLine::new(paint.is_on());
    let mut stdout = std::io::stdout();
    let mut counts = ParityCounts::default();
    let mut fixtures = 0;
    let mut total: Option<usize> = None;
    let mut log_error = None;
    let exit = run_tee(
        Command::new(flux)
            .current_dir(root)
            .envs(env.iter().copied())
            .args(["parity-check", dir, "--ways", ways])
            // 0190 S5: a skip must cite an open KI, and a skipped way that
            // passes is reported.
            .args(["--known-issues", "docs/known_issues.md"])
            .args(check_skips.then_some("--check-skips"))
            // The debug heap records a stack at every allocation and free,
            // so a program that allocates a lot runs several times slower
            // on it than parity-check's default allows.
            .args(
                ways.split(',')
                    .any(|way| way == "llvm_heap")
                    .then_some(["--timeout", "60"])
                    .into_iter()
                    .flatten(),
            ),
        None,
        |line| {
            if is_cargo_progress(line) {
                return;
            }
            let plain = strip_ansi(line);
            if let Err(e) = writeln!(log, "{plain}")
                && log_error.is_none()
            {
                log_error = Some(e);
            }
            if let Some(n) = plain
                .strip_prefix("[parity] fixtures: ")
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse().ok())
            {
                total = Some(n);
            }
            let shown = if let Some(verdict) = Verdict::parse(&plain) {
                fixtures += 1;
                counts.tally(verdict, &plain);
                let counter = progress(fixtures, total, started);
                if verdict == Verdict::Pass && paint.is_on() {
                    let failing = counts.mismatches + counts.expected;
                    let file = tail(&plain["PASS ".len()..], 48);
                    status.update(
                        &mut stdout,
                        &paint.dim(&format!("{counter} · {failing} failing · {file}")),
                    );
                    None
                } else {
                    Some(format!("{} {}", paint.dim(&counter), verdict.paint(paint, &plain)))
                }
            } else if plain.starts_with("SKIP_WAY ") {
                Some(paint.yellow(&plain))
            } else if plain.starts_with("UNEXPECTED_PASS ") {
                counts.unexpected.push(plain.clone());
                Some(paint.yellow(&plain))
            } else if plain.starts_with("LEAK ") {
                // A leak no marker allows also arrives as a failing verdict;
                // the rest, most of them KI-119's, are kept for the note and
                // the log, not scrolled past.
                counts.leaks.push(plain.clone());
                None
            } else if plain.starts_with("[parity]") {
                Some(paint.dim(&plain))
            } else if let Some(painted) = paint_summary_line(paint, &plain) {
                Some(painted)
            } else if is_parity_line(&plain) {
                Some(plain.clone())
            } else {
                None
            };
            if let Some(text) = shown {
                status.clear(&mut stdout);
                println!("{text}");
            }
        },
    )?;
    status.clear(&mut stdout);
    if let Some(e) = log_error {
        return Err(e);
    }
    counts.exit = exit;
    let result = counts.describe();
    let closing = format!("{dir}: {fixtures} fixture(s) in {}", format_duration(started.elapsed()));
    writeln!(log, "{closing} ({result})")?;
    println!("{closing} ({})", if counts.ok() { paint.green(&result) } else { paint.red(&result) });
    Ok(counts)
}

/// The last `max` characters of `text`, marked with `…` when cut.
pub(crate) fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - max + 1).collect();
    format!("…{kept}")
}

/// A line of `parity-check`'s closing summary with its number coloured by
/// what it means: passes green, failures red when there are any, skips
/// yellow when there are any. Labels stay plain.
fn paint_summary_line(paint: Paint, line: &str) -> Option<String> {
    if line == "All compiled examples match!" {
        return Some(paint.green(line));
    }
    let (label, number) = line.split_once(':')?;
    let count: usize = number.trim().parse().ok()?;
    let digits = count.to_string();
    let painted = match label {
        "Total" => paint.bold(&digits),
        "Mismatch" | "ExpectedMismatch" if count > 0 => paint.red(&digits),
        // Passes, and failure counts that are zero, are good news.
        "Pass" | "Mismatch" | "ExpectedMismatch" => paint.green(&digits),
        "Skip" | "SkippedWays" | "UnexpectedPass" | "Leaks" if count > 0 => paint.yellow(&digits),
        "Leaks" => paint.green(&digits),
        "Skip" | "SkippedWays" => paint.dim(&digits),
        _ => return None,
    };
    let spacing = &number[..number.len() - number.trim_start().len()];
    Some(format!("{label}:{spacing}{painted}"))
}

/// `fixture 3/23 · 0:41 elapsed`, the position of a verdict in its run;
/// without the total when `parity-check` did not announce one.
pub(crate) fn progress(done: usize, total: Option<usize>, started: Instant) -> String {
    let of = total.map(|t| format!("/{t}")).unwrap_or_default();
    format!("fixture {done}{of} · {} elapsed", format_duration(started.elapsed()))
}

/// `^\s+Running `: cargo's line naming each test binary.
fn is_running_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() < line.len() && trimmed.starts_with("Running ")
}

/// `^\s+Compiling|^\s+Finished`.
pub(crate) fn is_cargo_progress(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.len() < line.len()
        && (trimmed.starts_with("Compiling") || trimmed.starts_with("Finished"))
}

fn is_parity_line(line: &str) -> bool {
    [
        "PASS",
        "MISMATCH",
        "EXPECTED",
        "SKIP",
        "Total",
        "Pass",
        "Mismatch",
        "ExpectedMismatch",
        "Skip",
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
        let options = |skip_suite, skip_sweeps, base: Option<&str>| Options {
            tag: "t".into(),
            base: base.map(str::to_string),
            cmp: Cmp::Off,
            skip_suite,
            skip_sweeps,
            det_sample: 50,
        };
        // fmt+clippy, suite or build, then four parity steps.
        assert_eq!(step_count(&options(true, true, None)), 6);
        assert_eq!(step_count(&options(false, true, Some("main"))), 6);
        // Five sweeps, each with its determinism check, and the
        // internal-error tally.
        assert_eq!(step_count(&options(false, false, None)), 17);
        // And five comparisons with the base.
        assert_eq!(step_count(&options(true, false, Some("main"))), 22);
        let unchecked = Options { det_sample: 0, ..options(false, false, None) };
        assert_eq!(step_count(&unchecked), 12);
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
    fn colours_summary_numbers_by_meaning() {
        let on = Paint::always();
        assert_eq!(
            paint_summary_line(on, "Pass:     132").as_deref(),
            Some("Pass:     \x1b[32m132\x1b[0m")
        );
        assert_eq!(
            paint_summary_line(on, "Mismatch: 2").as_deref(),
            Some("Mismatch: \x1b[1;31m2\x1b[0m")
        );
        assert_eq!(
            paint_summary_line(on, "Mismatch: 0").as_deref(),
            Some("Mismatch: \x1b[32m0\x1b[0m")
        );
        assert_eq!(
            paint_summary_line(on, "SkippedWays: 0").as_deref(),
            Some("SkippedWays: \x1b[2m0\x1b[0m")
        );
        assert_eq!(paint_summary_line(on, "Leaks: 4").as_deref(), Some("Leaks: \x1b[33m4\x1b[0m"));
        assert_eq!(paint_summary_line(on, "Leaks: 0").as_deref(), Some("Leaks: \x1b[32m0\x1b[0m"));
        assert_eq!(paint_summary_line(on, "Mismatch: lots"), None);
        assert_eq!(paint_summary_line(on, "error: Mismatch: 2"), None);
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
    fn a_long_path_keeps_its_end() {
        assert_eq!(tail("short.flx", 48), "short.flx");
        let cut = tail("examples/type_classes/a_very_long_fixture_name.flx", 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.starts_with('…') && cut.ends_with("_name.flx"));
    }

    #[test]
    fn counts_both_failing_verdicts_and_nothing_else() {
        let mut counts = ParityCounts::default();
        for line in [
            "PASS tests/parity/a.flx",
            "MISMATCH tests/parity/b.flx",
            "EXPECTED_OUTPUT_MISMATCH tests/parity/c.flx",
            "EXPECTED_OUTPUT_MISMATCH tests/parity/d.flx",
            "SKIP tests/parity/e.flx (KI-073)",
            "Mismatch: 1",
            "ExpectedMismatch: 2",
            "MISMATCH_KIND b.flx stdout vm/llvm",
        ] {
            if let Some(verdict) = Verdict::parse(line) {
                counts.tally(verdict, line);
            }
        }
        assert_eq!(counts.mismatches, 1);
        assert_eq!(counts.expected, 2);
        assert_eq!(
            counts.failures,
            [
                "MISMATCH tests/parity/b.flx",
                "EXPECTED_OUTPUT_MISMATCH tests/parity/c.flx",
                "EXPECTED_OUTPUT_MISMATCH tests/parity/d.flx",
            ]
        );
    }

    #[test]
    fn colours_only_the_verdict_word() {
        let on = Paint::always();
        let line = "EXPECTED_OUTPUT_MISMATCH b.flx";
        assert_eq!(
            Verdict::parse(line).unwrap().paint(on, line),
            "\x1b[1;31mEXPECTED_OUTPUT_MISMATCH\x1b[0m b.flx"
        );
        let line = "PASS a.flx";
        assert_eq!(Verdict::parse(line).unwrap().paint(on, line), "\x1b[32mPASS\x1b[0m a.flx");
    }

    #[test]
    fn a_failing_exit_alone_is_not_ok() {
        // parity-check exits 1 when nothing passed or it could not start,
        // with no verdict line to count.
        let counts = ParityCounts { exit: 1, ..ParityCounts::default() };
        assert!(!counts.ok());
        assert!(ParityCounts::default().ok());
        assert_eq!(counts.describe(), "0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, exit 1");
    }

    #[test]
    fn recognises_cargo_lines_only_when_indented() {
        assert!(is_running_line("     Running unittests src/lib.rs"));
        assert!(!is_running_line("Running"));
        assert!(is_cargo_progress("   Compiling flux v0.0.7"));
        assert!(!is_cargo_progress("Finished"));
    }
}
