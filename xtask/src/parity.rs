//! Running `flux parity-check` over a folder and reading its verdicts, for
//! every command that compares backends: verify, sanitize, rewrite, census
//! and fuzz.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

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

    /// `text` coloured as this verdict: pass green, skip yellow, either
    /// mismatch red.
    fn paint(self, paint: Paint, text: &str) -> String {
        match self {
            Self::Pass => paint.bold_green(text),
            Self::Skip => paint.bold_yellow(text),
            Self::Mismatch | Self::ExpectedOutputMismatch => paint.red(text),
        }
    }
}

/// The column a verdict word is right-aligned in, as nextest aligns `PASS`.
const WORD_COLUMN: usize = 12;

/// A verdict line as nextest shows a test: the word right-aligned and
/// coloured, the fixture's folder and file name, then anything after the
/// path (a skip's reason) dimmed.
fn verdict_line(paint: Paint, verdict: Verdict, line: &str) -> String {
    let rest = line[verdict.word().len()..].trim_start();
    let (path, note) = rest.split_once(' ').unwrap_or((rest, ""));
    let word = verdict.paint(paint, &format!("{:>WORD_COLUMN$}", verdict.word()));
    let mut shown = format!("{word} {}", fixture_path(paint, path));
    if !note.is_empty() {
        shown.push(' ');
        shown.push_str(&paint.dim(note));
    }
    shown
}

/// `SKIP_WAY <path> <way>: <KI> <reason>` laid out as a verdict line, with
/// the way and the KI picked out. A line in another shape stays as it was.
fn skip_way_line(paint: Paint, line: &str) -> String {
    let parts = line
        .strip_prefix("SKIP_WAY ")
        .and_then(|rest| rest.split_once(' '))
        .and_then(|(path, rest)| rest.split_once(": ").map(|(way, why)| (path, way, why)));
    let Some((path, way, why)) = parts else {
        return paint.yellow(line);
    };
    let mut shown = format!(
        "{} {} {}",
        paint.bold_yellow(&format!("{:>WORD_COLUMN$}", "SKIP WAY")),
        fixture_path(paint, path),
        paint.bold(way)
    );
    let (ki, why) = match why.split_once(' ') {
        Some((ki, rest)) if ki.starts_with("KI-") => (Some(ki), rest),
        _ => (None, why),
    };
    if let Some(ki) = ki {
        shown.push(' ');
        shown.push_str(&paint.yellow(ki));
    }
    shown.push(' ');
    shown.push_str(&paint.dim(why));
    shown
}

/// One of `parity-check`'s own `[parity] <label>: <value>` lines: the tag
/// cyan, the label bold, a closing `(…)` dimmed, and paths with `/`
/// separators. A line without a label keeps the tag and plain text.
fn parity_note_line(paint: Paint, line: &str) -> String {
    if !paint.is_on() {
        return line.to_string();
    }
    let rest = line.strip_prefix("[parity]").unwrap_or(line).trim_start().replace('\\', "/");
    let tag = paint.cyan("[parity]");
    let Some((label, value)) = rest.split_once(": ") else {
        return format!("{tag} {rest}");
    };
    let (value, aside) = match value.rfind(" (") {
        Some(at) if value.ends_with(')') => value.split_at(at),
        _ => (value, ""),
    };
    format!("{tag} {}: {value}{}", paint.bold(label), paint.dim(aside))
}

/// A fixture's path with `/` separators, its folder magenta and its file
/// name blue, as nextest colours a test's binary and name.
fn fixture_path(paint: Paint, path: &str) -> String {
    let path = path.replace('\\', "/");
    match path.rsplit_once('/') {
        Some((dir, name)) => format!("{}{}", paint.magenta(&format!("{dir}/")), paint.blue(name)),
        None => paint.blue(&path),
    }
}

/// How far a `parity-check` run has got, for the progress bar.
#[derive(Clone, Copy, Debug, Default)]
struct Tally {
    done: usize,
    total: Option<usize>,
    passed: usize,
    failing: usize,
    skipped: usize,
}

/// The progress bar pinned under the verdicts, after nextest's:
/// `     Running [ 00:00:19] ==========>          30/66: 28 passed, 1 failing, 1 skipped`.
fn progress_bar(paint: Paint, tally: Tally, elapsed: Duration) -> String {
    const WIDTH: usize = 30;
    let secs = elapsed.as_secs();
    let clock = format!("[ {:02}:{:02}:{:02}]", secs / 3600, secs / 60 % 60, secs % 60);
    let (bar, count) = match tally.total {
        Some(total) if total > 0 => {
            let filled = (tally.done * WIDTH / total).min(WIDTH);
            let bar = match filled {
                0 => " ".repeat(WIDTH),
                WIDTH => "=".repeat(WIDTH),
                n => format!("{}>{}", "=".repeat(n - 1), " ".repeat(WIDTH - n)),
            };
            (bar, format!("{}/{total}", tally.done))
        }
        _ => (" ".repeat(WIDTH), tally.done.to_string()),
    };
    let failing = format!("{} failing", tally.failing);
    let skipped = format!("{} skipped", tally.skipped);
    format!(
        "{} {clock} {} {}: {}, {}, {}",
        paint.bold_green(&format!("{:>WORD_COLUMN$}", "Running")),
        paint.cyan(&bar),
        paint.bold(&count),
        paint.green(&format!("{} passed", tally.passed)),
        if tally.failing > 0 { paint.red(&failing) } else { failing },
        if tally.skipped > 0 { paint.yellow(&skipped) } else { skipped },
    )
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
    let mut tally = Tally::default();
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
                tally.total = Some(n);
            }
            let shown = if let Some(verdict) = Verdict::parse(&plain) {
                tally.done += 1;
                match verdict {
                    Verdict::Pass => tally.passed += 1,
                    Verdict::Skip => tally.skipped += 1,
                    Verdict::Mismatch | Verdict::ExpectedOutputMismatch => tally.failing += 1,
                }
                counts.tally(verdict, &plain);
                if paint.is_on() {
                    Some(verdict_line(paint, verdict, &plain))
                } else {
                    // Piped, every verdict keeps its position, for a log
                    // read later; there is no bar to show it.
                    Some(format!("{} {plain}", progress(tally.done, tally.total, started)))
                }
            } else if plain.starts_with("SKIP_WAY ") {
                Some(skip_way_line(paint, &plain))
            } else if plain.starts_with("UNEXPECTED_PASS ") {
                counts.unexpected.push(plain.clone());
                Some(paint.yellow(&plain))
            } else if plain.starts_with("LEAK ") {
                // Nearly every file leaks for now (KI-119, KI-121): kept
                // for the note and the log, not scrolled past.
                counts.leaks.push(plain.clone());
                None
            } else if plain.starts_with("[parity]") {
                Some(parity_note_line(paint, &plain))
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
                if tally.total.is_some() {
                    status.update(&mut stdout, &progress_bar(paint, tally, started.elapsed()));
                }
            }
        },
    )?;
    status.clear(&mut stdout);
    if let Some(e) = log_error {
        return Err(e);
    }
    counts.exit = exit;
    let result = counts.describe();
    let closing =
        format!("{dir}: {} fixture(s) in {}", tally.done, format_duration(started.elapsed()));
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
    fn a_verdict_line_aligns_its_word_and_splits_the_path() {
        let line = r"PASS tests/parity\a.flx";
        let verdict = Verdict::parse(line).unwrap();
        assert_eq!(verdict_line(Paint::plain(), verdict, line), "        PASS tests/parity/a.flx");
        assert_eq!(
            verdict_line(Paint::always(), verdict, line),
            "\x1b[1;32m        PASS\x1b[0m \x1b[35mtests/parity/\x1b[0m\x1b[34ma.flx\x1b[0m"
        );
        let skip = "SKIP tests/parity_debug/b.flx (native debug parity skipped)";
        assert_eq!(
            verdict_line(Paint::plain(), Verdict::parse(skip).unwrap(), skip),
            "        SKIP tests/parity_debug/b.flx (native debug parity skipped)"
        );
        let failed = "EXPECTED_OUTPUT_MISMATCH c.flx";
        assert!(
            verdict_line(Paint::always(), Verdict::parse(failed).unwrap(), failed)
                .starts_with("\x1b[1;31mEXPECTED_OUTPUT_MISMATCH\x1b[0m")
        );
    }

    #[test]
    fn parity_check_notes_pick_out_their_label_and_aside() {
        let on = Paint::always();
        assert_eq!(
            parity_note_line(
                on,
                r"[parity] binary vm: target/parity_vm/debug\flux.exe (stale -> rebuilt)"
            ),
            "\x1b[36m[parity]\x1b[0m \x1b[1mbinary vm\x1b[0m: target/parity_vm/debug/flux.exe\x1b[2m (stale -> rebuilt)\x1b[0m"
        );
        assert_eq!(
            parity_note_line(on, "[parity] rebuilding VM binary in target/parity_vm"),
            "\x1b[36m[parity]\x1b[0m rebuilding VM binary in target/parity_vm"
        );
        let plain = r"[parity] binary vm: target/parity_vm/debug\flux.exe (stale -> rebuilt)";
        assert_eq!(parity_note_line(Paint::plain(), plain), plain);
    }

    #[test]
    fn a_skipped_way_names_its_way_and_ki() {
        let line = r"SKIP_WAY examples/type_classes\c.flx vm_opt: KI-102 the example's own effect";
        assert_eq!(
            skip_way_line(Paint::plain(), line),
            "    SKIP WAY examples/type_classes/c.flx vm_opt KI-102 the example's own effect"
        );
        let no_ki = "SKIP_WAY d.flx llvm: native only on unix";
        assert_eq!(
            skip_way_line(Paint::plain(), no_ki),
            "    SKIP WAY d.flx llvm native only on unix"
        );
        assert_eq!(skip_way_line(Paint::plain(), "SKIP_WAY odd"), "SKIP_WAY odd");
    }

    #[test]
    fn the_progress_bar_fills_with_the_fixtures_done() {
        let tally = Tally { done: 30, total: Some(60), passed: 28, failing: 1, skipped: 1 };
        assert_eq!(
            progress_bar(Paint::plain(), tally, Duration::from_secs(3725)),
            format!(
                "     Running [ 01:02:05] {}>{} 30/60: 28 passed, 1 failing, 1 skipped",
                "=".repeat(14),
                " ".repeat(15)
            )
        );
        let start = Tally { total: Some(60), ..Tally::default() };
        assert!(progress_bar(Paint::plain(), start, Duration::ZERO).contains(&" ".repeat(30)));
        let done = Tally { done: 60, total: Some(60), passed: 60, ..Tally::default() };
        assert!(progress_bar(Paint::plain(), done, Duration::ZERO).contains(&"=".repeat(30)));
        // A count of zero failing stays plain; one is red.
        assert!(
            progress_bar(Paint::always(), tally, Duration::ZERO).contains("\x1b[1;31m1 failing")
        );
        let clean = Tally { failing: 0, ..tally };
        assert!(progress_bar(Paint::always(), clean, Duration::ZERO).contains(", 0 failing, "));
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
