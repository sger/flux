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
//! - `parity_g_<tag>.txt`: the guide natively with and without `-g`
//! - `parity_debug_<tag>.txt`: the VM debugger and lldb compared
//!
//! With `--base`, each sweep is compared with the same sweep of the earlier
//! tag. A sweep's base must come from the same tools, so run one on main
//! first: check out main, build, and `cargo xtask verify main --skip-suite`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::compare;
use crate::proc::{run, run_tee};
use crate::sweep::{self, Mode};
use crate::text::{Paint, strip_ansi};

pub struct Options {
    pub tag: String,
    pub base: Option<String>,
    pub skip_suite: bool,
    pub skip_sweeps: bool,
}

/// How a summary line counts towards the run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ok,
    Fail,
    /// Informational: never fails the run.
    Note,
}

/// The closing summary, one marked line per step.
#[derive(Default)]
struct Outcome {
    lines: Vec<(Mark, String)>,
}

impl Outcome {
    fn record(&mut self, ok: bool, line: String) {
        self.lines
            .push((if ok { Mark::Ok } else { Mark::Fail }, line));
    }

    fn note(&mut self, line: String) {
        self.lines.push((Mark::Note, line));
    }

    fn failed(&self) -> bool {
        self.lines.iter().any(|(mark, _)| *mark == Mark::Fail)
    }

    fn print(&self) {
        let paint = Paint::detect();
        for (mark, line) in &self.lines {
            match mark {
                Mark::Ok => println!("{} {line}", paint.green("ok  ")),
                Mark::Fail => println!("{} {line}", paint.red("FAIL")),
                Mark::Note => println!("     {}", paint.yellow(line)),
            }
        }
    }
}

fn section(title: &str) {
    println!("{}", Paint::detect().heading(&format!("== {title}")));
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
        let sweeps = SWEEPS.map(|mode| (sweep_dir(tag, mode), mode));
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
            for (dir, mode) in &sweeps {
                section(&format!("compare {dir} with {base}"));
                let base_dir = out.join(sweep_dir(base, *mode));
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
    println!(
        "each fixture runs on the VM (fresh, cached, strict) and natively; their outputs must agree"
    );
    let mut log = open_log(&out.join(format!("parity_{tag}.txt"))).map_err(io)?;
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
        let run = parity(root, &flux, dir, "vm,vm_cached,vm_strict,llvm", &mut log).map_err(io)?;
        counts.mismatches += run.mismatches;
        counts.expected += run.expected;
        if run.exit != 0 {
            failed_runs += 1;
        }
    }
    // A known divergence is recorded in its fixture with `// skip-way:` and
    // a KI (0190 S2), so anything left here is new and fails the run.
    outcome.record(
        counts.mismatches == 0 && counts.expected == 0 && failed_runs == 0,
        format!(
            "parity ({} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, {failed_runs} of {} dir(s) exited non-zero)",
            counts.mismatches,
            counts.expected,
            dirs.len()
        ),
    );

    // A debug build runs as a plain one does (0188 S9c): every guide
    // example prints the same natively with and without `-g`.
    section("debug build parity");
    println!("every guide example built natively with and without -g; output must agree");
    let mut log = open_log(&out.join(format!("parity_g_{tag}.txt"))).map_err(io)?;
    let counts = parity(root, &flux, "examples/guide", "llvm,llvm_g", &mut log).map_err(io)?;
    outcome.record(
        counts.ok(),
        format!("debug build parity ({})", counts.describe()),
    );

    // The VM debugger and lldb on the same scripted sessions (0188 S6b).
    // Every fixture is expected to match, so a mismatch fails the run.
    section("debug parity");
    println!("each // dbg: script run in the VM debugger and in lldb; transcripts must agree");
    let mut log = open_log(&out.join(format!("parity_debug_{tag}.txt"))).map_err(io)?;
    let counts = parity(
        root,
        &flux,
        "tests/parity_debug",
        "vm_debug,llvm_debug",
        &mut log,
    )
    .map_err(io)?;
    outcome.record(counts.ok(), format!("debug parity ({})", counts.describe()));

    section(&format!("summary {tag}"));
    outcome.print();
    println!("all-done: {tag}");
    Ok(!outcome.failed())
}

const SWEEPS: [Mode; 3] = [Mode::Cfg, Mode::Core, Mode::CoreOpt];

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
}

impl ParityCounts {
    /// Counts `verdict` if it is a failing one.
    fn tally(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Mismatch => self.mismatches += 1,
            Verdict::ExpectedOutputMismatch => self.expected += 1,
            Verdict::Pass | Verdict::Skip => {}
        }
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
/// timed, so a long run shows where it is. Returns the failing verdicts and
/// the exit code.
fn parity(
    root: &Path,
    flux: &Path,
    dir: &str,
    ways: &str,
    log: &mut fs::File,
) -> std::io::Result<ParityCounts> {
    let started = Instant::now();
    let paint = Paint::detect();
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
            if let Some(verdict) = Verdict::parse(&plain) {
                fixtures += 1;
                counts.tally(verdict);
                let counter = progress(fixtures, total, started);
                println!("{} {}", paint.dim(&counter), verdict.paint(paint, &plain));
            } else if plain.starts_with("SKIP_WAY ") {
                println!("{}", paint.yellow(&plain));
            } else if plain.starts_with("[parity]") {
                println!("{}", paint.dim(&plain));
            } else if is_parity_line(&plain) {
                println!("{plain}");
            }
        },
    )?;
    if let Some(e) = log_error {
        return Err(e);
    }
    counts.exit = exit;
    let result = counts.describe();
    println!(
        "{dir}: {fixtures} fixture(s) in {} ({})",
        elapsed(started),
        if counts.ok() {
            paint.green(&result)
        } else {
            paint.red(&result)
        }
    );
    Ok(counts)
}

/// `fixture 3/23 · 0:41 elapsed`, the position of a verdict in its run;
/// without the total when `parity-check` did not announce one.
fn progress(done: usize, total: Option<usize>, started: Instant) -> String {
    let of = total.map(|t| format!("/{t}")).unwrap_or_default();
    format!("fixture {done}{of} · {} elapsed", elapsed(started))
}

/// `m:ss` since `started`.
fn elapsed(started: Instant) -> String {
    let secs = started.elapsed().as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
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
    fn only_a_failed_step_fails_the_run() {
        let mut outcome = Outcome::default();
        outcome.record(true, "fmt".into());
        outcome.note("parity: 9 MISMATCH".into());
        assert!(!outcome.failed());
        outcome.record(false, "debug parity".into());
        assert!(outcome.failed());
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
                counts.tally(verdict);
            }
        }
        assert_eq!(counts.mismatches, 1);
        assert_eq!(counts.expected, 2);
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
