//! Running `flux parity-check` over a folder and reading its verdicts, for
//! every command that compares backends: verify, sanitize, rewrite, census
//! and fuzz.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::outcome::format_duration;
use crate::proc::run_tee;
use crate::text::{Paint, StatusLine, strip_ansi};

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
    pub(crate) unexpected: Vec<String>,
    /// Each `LEAK` line: a file with blocks live at exit on a heap way.
    pub(crate) leaks: Vec<String>,
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

    pub(crate) fn describe(&self) -> String {
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
    parity_with_env(root, flux, dir, ways, check_skips, log, &[], &[])
}

/// [`parity`], with `env` set for `parity-check` and the runs it starts, and
/// `extra` arguments for `parity-check` itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parity_with_env(
    root: &Path,
    flux: &Path,
    dir: &str,
    ways: &str,
    check_skips: bool,
    log: &mut fs::File,
    env: &[(&str, &str)],
    extra: &[String],
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
            .args(extra)
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
                // Nearly every file leaks for now (KI-119, KI-121): kept
                // for the note and the log, not scrolled past.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(is_cargo_progress("   Compiling flux v0.0.7"));
        assert!(!is_cargo_progress("Finished"));
    }
}
