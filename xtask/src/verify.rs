//! Verify one step of a change: lint, the full suite, the corpus sweeps and
//! backend parity. Each run's outputs are tagged, so a step's results compare
//! with the previous step's.
//!
//! Outputs go to `.git/parked/out/` (untracked, survives `cargo clean`):
//! - `test_<tag>.txt`: the suite
//! - `sweep_<tag>/`: every example compiled with `--dump-cfg`
//! - `dump_<tag>__dump_core/`: ... with `--dump-core`
//! - `dump_<tag>__optimize___dump_core/`: ... with `--optimize --dump-core`
//! - `parity_<tag>.txt`: vm, vm_cached, vm_strict, vm_opt, llvm and llvm_opt
//!   compared
//! - `parity_g_<tag>.txt`: the guide natively with and without `-g`
//! - `parity_debug_<tag>.txt`: the VM debugger and lldb compared
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
use crate::sweep::{self, Mode};
use crate::text::{Paint, StatusLine, strip_ansi};

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    /// Fail when a sweep's dumps differ from the base's, for a change that
    /// claims to change no output (0190 S4).
    pub cmp: bool,
    pub skip_suite: bool,
    pub skip_sweeps: bool,
}

/// How many numbered steps a run with `options` takes; the summary is not one.
fn step_count(options: &Options) -> usize {
    let sweeps = if options.skip_sweeps {
        0
    } else {
        // Each sweep, the internal-error tally, and each comparison.
        let compares = if options.base.is_some() {
            SWEEPS.len()
        } else {
            0
        };
        SWEEPS.len() + 1 + compares
    };
    // fmt and clippy; the suite or a build; then the three parity steps.
    2 + sweeps + 3
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

pub fn flux_binary(root: &Path) -> PathBuf {
    root.join("target/debug")
        .join(format!("flux{}", std::env::consts::EXE_SUFFIX))
}

/// The summary file a run of `tag` saves, for a later run to compare with.
fn summary_file(out: &Path, tag: &str) -> PathBuf {
    out.join(format!("verify_{tag}.txt"))
}

/// Returns whether every check passed.
pub fn run_verify(root: &Path, options: &Options) -> Result<bool, String> {
    let tag = &options.tag;
    let out = root.join(".git/parked/out");
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut outcome = Outcome::new();
    let mut steps = Steps::new(step_count(options));
    let io = |e: std::io::Error| e.to_string();

    // Lint and test with the flags CI uses, so the llvm-feature tests run too.
    steps.start("fmt, clippy");
    let started = Instant::now();
    let fmt = run(cargo()
        .current_dir(root)
        .args(["fmt", "--all", "--", "--check"]))
    .map_err(io)?;
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
    outcome
        .record("clippy", clippy == 0, "clippy".into(), started.elapsed())
        .details = clippy_findings
        .iter()
        .filter(|line| line.starts_with("warning") || line.starts_with("error"))
        .map(|line| (*line).to_string())
        .collect();

    if options.skip_suite {
        // The suite builds target/debug/flux with every feature; without it,
        // build it here, for the sweeps and parity.
        let started = steps.start("build");
        let build = run(cargo()
            .current_dir(root)
            .args(["build", "--all-features", "--quiet"]))
        .map_err(io)?;
        outcome.record("build", build == 0, "build".into(), started.elapsed());
        if build != 0 {
            return Ok(finish(&outcome, root, &out, options));
        }
    } else {
        let started = steps.start("suite");
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
        let entry = outcome.record(
            "tests",
            tests == 0,
            format!("tests (exit {tests})"),
            started.elapsed(),
        );
        if tests != 0 {
            entry.details = capped(&failed, &log);
        }
        outcome.note(
            "snapshots",
            format!("{} .snap.new file(s) to review", snaps.len()),
            None,
        );
    }

    let flux = flux_binary(root);
    if !options.skip_sweeps {
        let sweeps = SWEEPS.map(|mode| (sweep_dir(tag, mode), mode));
        let mut internal = Vec::new();
        for (dir, mode) in &sweeps {
            let started = steps.start(&format!("sweep {dir} ({})", mode.name()));
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
            outcome.note(
                &format!("sweep {}", mode.name()),
                format!("sweep {dir} ({})", mode.name()),
                Some(took),
            );
            let count = results
                .iter()
                .filter(|r| compare::INTERNAL.contains(&r.status))
                .count();
            internal.push((dir, *mode, count));
        }

        steps.start("internal errors");
        for (dir, mode, count) in &internal {
            println!("{dir}: {count}");
            outcome.note(
                &format!("internal {}", mode.name()),
                format!("{dir}: {count} internal"),
                None,
            );
        }

        if let Some(base) = &options.base {
            for (dir, mode) in &sweeps {
                let started = steps.start(&format!("compare {dir} with {base}"));
                let base_dir = out.join(sweep_dir(base, *mode));
                let step = format!("compare {}", mode.name());
                let head_dir = out.join(dir);
                match compare::run(root, &base_dir, &head_dir, true) {
                    Ok(comparison) => {
                        let changed = comparison.changed.len();
                        let fails_on_content = options.cmp && changed > 0;
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
                                    "{changed} {} dump(s) changed; --cmp fails on that ({})",
                                    mode.name(),
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
    let log_path = out.join(format!("parity_{tag}.txt"));
    let mut log = open_log(&log_path).map_err(io)?;
    let mut counts = ParityCounts::default();
    let mut failed_runs = 0;
    let dirs = [
        "examples/type_classes",
        "examples/generics",
        "tests/flux",
        "tests/parity",
    ];
    for dir in dirs {
        println!("{}", Paint::detect().bold(&format!("=== {dir}")));
        writeln!(log, "=== {dir}").map_err(io)?;
        let run = parity(root, &flux, dir, MAIN_WAYS, &mut log).map_err(io)?;
        counts.mismatches += run.mismatches;
        counts.expected += run.expected;
        counts.failures.extend(run.failures);
        if run.exit != 0 {
            failed_runs += 1;
        }
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

    // A debug build runs as a plain one does (0188 S9c): every guide
    // example prints the same natively with and without `-g`.
    let started = steps.start("debug build parity");
    println!("every guide example built natively with and without -g; output must agree");
    let log_path = out.join(format!("parity_g_{tag}.txt"));
    let mut log = open_log(&log_path).map_err(io)?;
    let counts = parity(root, &flux, "examples/guide", "llvm,llvm_g", &mut log).map_err(io)?;
    record_parity(
        &mut outcome,
        "debug build parity",
        &counts,
        &log_path,
        started,
    );

    // The VM debugger and lldb on the same scripted sessions (0188 S6b).
    // Every fixture is expected to match, so a mismatch fails the run.
    let started = steps.start("debug parity");
    println!("each // dbg: script run in the VM debugger and in lldb; transcripts must agree");
    let log_path = out.join(format!("parity_debug_{tag}.txt"));
    let mut log = open_log(&log_path).map_err(io)?;
    let counts = parity(
        root,
        &flux,
        "tests/parity_debug",
        "vm_debug,llvm_debug",
        &mut log,
    )
    .map_err(io)?;
    record_parity(&mut outcome, "debug parity", &counts, &log_path, started);

    Ok(finish(&outcome, root, &out, options))
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
    let entry = outcome.record(
        step,
        counts.ok(),
        format!("{step} ({})", counts.describe()),
        took,
    );
    if !counts.ok() {
        entry.details = capped(&counts.failures, log);
    }
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
fn finish(outcome: &Outcome, root: &Path, out: &Path, options: &Options) -> bool {
    let paint = Paint::detect();
    let base_times: Option<HashMap<String, Duration>> = options.base.as_ref().and_then(|base| {
        let path = summary_file(out, base);
        let text = fs::read_to_string(&path).ok();
        if text.is_none() {
            println!(
                "{}",
                paint.dim(&format!("no {} to compare times with", path.display()))
            );
        }
        text.map(|text| parse_summary_file(&text))
    });
    let out_display = out.strip_prefix(root).unwrap_or(out).display().to_string();
    outcome.print(&options.tag, &out_display, base_times.as_ref());
    let path = summary_file(out, &options.tag);
    if let Err(e) = fs::write(&path, outcome.to_file()) {
        println!("could not save {}: {e}", path.display());
    } else {
        println!(
            "{}",
            paint.dim(&format!(
                "summary saved: {}",
                path.strip_prefix(root).unwrap_or(&path).display()
            ))
        );
    }
    println!("all-done: {}", options.tag);
    !outcome.failed()
}

const SWEEPS: [Mode; 3] = [Mode::Cfg, Mode::Core, Mode::CoreOpt];

/// The main parity step's ways. The opt ways run every fixture under
/// `--optimize`, so a miscompile shows up as a difference from the plain way
/// (0190 S3).
const MAIN_WAYS: &str = "vm,vm_cached,vm_strict,vm_opt,llvm,llvm_opt";

/// The folder a sweep of `tag` in `mode` writes to, under the out dir.
fn sweep_dir(tag: &str, mode: Mode) -> String {
    match mode {
        Mode::Cfg => format!("sweep_{tag}"),
        Mode::Core => format!("dump_{tag}__dump_core"),
        Mode::CoreOpt => format!("dump_{tag}__optimize___dump_core"),
    }
}

/// Creates a step's log file and says where it is.
fn open_log(path: &Path) -> std::io::Result<fs::File> {
    println!(
        "{}",
        Paint::detect().dim(&format!("  log: {}", path.display()))
    );
    fs::File::create(path)
        .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// A fixture's verdict, the first word of the line `print_result` writes
/// for it. One list of the words, so counting, numbering and colouring
/// cannot disagree about what a verdict line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Skip,
    Mismatch,
    ExpectedOutputMismatch,
}

impl Verdict {
    const ALL: [Self; 4] = [
        Self::Pass,
        Self::Skip,
        Self::Mismatch,
        Self::ExpectedOutputMismatch,
    ];

    fn word(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Skip => "SKIP",
            Self::Mismatch => "MISMATCH",
            Self::ExpectedOutputMismatch => "EXPECTED_OUTPUT_MISMATCH",
        }
    }

    /// The verdict `line` (without colour) reports, if it is a verdict line.
    fn parse(line: &str) -> Option<Self> {
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
struct ParityCounts {
    /// Fixtures whose ways disagreed (`MISMATCH`).
    mismatches: usize,
    /// Fixtures whose ways agreed but missed the fixture's declared
    /// outcome or expected stdout (`EXPECTED_OUTPUT_MISMATCH`).
    expected: usize,
    /// `parity-check`'s exit code: 1 on any failure, when nothing passed,
    /// or when it could not run at all.
    exit: i32,
    /// Each failing verdict line, as it arrived.
    failures: Vec<String>,
}

impl ParityCounts {
    /// Counts `verdict`, and keeps `line` when it is a failing one.
    fn tally(&mut self, verdict: Verdict, line: &str) {
        match verdict {
            Verdict::Mismatch => self.mismatches += 1,
            Verdict::ExpectedOutputMismatch => self.expected += 1,
            Verdict::Pass | Verdict::Skip => return,
        }
        self.failures.push(line.to_string());
    }

    fn ok(&self) -> bool {
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
/// Returns the failing verdicts and the exit code.
fn parity(
    root: &Path,
    flux: &Path,
    dir: &str,
    ways: &str,
    log: &mut fs::File,
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
            .args(["parity-check", dir, "--ways", ways]),
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
                .and_then(|n| n.trim().parse().ok())
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
                    Some(format!(
                        "{} {}",
                        paint.dim(&counter),
                        verdict.paint(paint, &plain)
                    ))
                }
            } else if plain.starts_with("SKIP_WAY ") {
                Some(paint.yellow(&plain))
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
    let closing = format!(
        "{dir}: {fixtures} fixture(s) in {}",
        format_duration(started.elapsed())
    );
    writeln!(log, "{closing} ({result})")?;
    println!(
        "{closing} ({})",
        if counts.ok() {
            paint.green(&result)
        } else {
            paint.red(&result)
        }
    );
    Ok(counts)
}

/// The last `max` characters of `text`, marked with `…` when cut.
fn tail(text: &str, max: usize) -> String {
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
        "Skip" | "SkippedWays" if count > 0 => paint.yellow(&digits),
        "Skip" | "SkippedWays" => paint.dim(&digits),
        _ => return None,
    };
    let spacing = &number[..number.len() - number.trim_start().len()];
    Some(format!("{label}:{spacing}{painted}"))
}

/// `fixture 3/23 · 0:41 elapsed`, the position of a verdict in its run;
/// without the total when `parity-check` did not announce one.
fn progress(done: usize, total: Option<usize>, started: Instant) -> String {
    let of = total.map(|t| format!("/{t}")).unwrap_or_default();
    format!(
        "fixture {done}{of} · {} elapsed",
        format_duration(started.elapsed())
    )
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
        assert_eq!(
            grep_with_context(&lines, 1, 10),
            ["warning: x", "b", "error: y", "e"]
        );
        assert_eq!(grep_with_context(&lines, 6, 3), ["warning: x", "b", "c"]);
    }

    #[test]
    fn a_base_sweep_is_named_from_its_own_tag() {
        // A tag that is part of a folder name must not change the rest of it.
        assert_eq!(sweep_dir("core", Mode::Core), "dump_core__dump_core");
        assert_eq!(sweep_dir("main", Mode::Core), "dump_main__dump_core");
        assert_eq!(
            sweep_dir("dump", Mode::CoreOpt),
            "dump_dump__optimize___dump_core"
        );
        assert_eq!(sweep_dir("main", Mode::Cfg), "sweep_main");
    }

    #[test]
    fn counts_the_steps_each_option_leaves() {
        let options = |skip_suite, skip_sweeps, base: Option<&str>| Options {
            tag: "t".into(),
            base: base.map(str::to_string),
            cmp: false,
            skip_suite,
            skip_sweeps,
        };
        // fmt+clippy, suite or build, then three parity steps.
        assert_eq!(step_count(&options(true, true, None)), 5);
        assert_eq!(step_count(&options(false, true, Some("main"))), 5);
        // Three sweeps and the internal-error tally.
        assert_eq!(step_count(&options(false, false, None)), 9);
        // And three comparisons with the base.
        assert_eq!(step_count(&options(true, false, Some("main"))), 12);
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
        assert_eq!(
            Verdict::parse(line).unwrap().paint(on, line),
            "\x1b[32mPASS\x1b[0m a.flx"
        );
    }

    #[test]
    fn a_failing_exit_alone_is_not_ok() {
        // parity-check exits 1 when nothing passed or it could not start,
        // with no verdict line to count.
        let counts = ParityCounts {
            exit: 1,
            ..ParityCounts::default()
        };
        assert!(!counts.ok());
        assert!(ParityCounts::default().ok());
        assert_eq!(
            counts.describe(),
            "0 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, exit 1"
        );
    }

    #[test]
    fn recognises_cargo_lines_only_when_indented() {
        assert!(is_running_line("     Running unittests src/lib.rs"));
        assert!(!is_running_line("Running"));
        assert!(is_cargo_progress("   Compiling flux v0.0.7"));
        assert!(!is_cargo_progress("Finished"));
    }
}
