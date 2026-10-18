//! `cargo xtask bisect-opt` (proposal 0190 §13): finds the optimizing
//! rewrite that makes an opt way (`vm_opt`, `llvm_opt`) behave differently
//! from its plain way.
//!
//! The compiler numbers its optimizing rewrites as they fire and skips those
//! above `--opt-bisect-limit` (`src/shared/opt_bisect.rs`). With the plain
//! way's run as the reference, a binary search finds the smallest limit at
//! which the opt run differs; that rewrite is the culprit. The report shows
//! its `opt-bisect:` line and its definition's code without it and with it.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::outcome::format_duration;
use crate::proc::run;
use crate::store;
use crate::sweep::{Ran, run_with_timeout};
use crate::verify::cargo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    VmOpt,
    LlvmOpt,
}

impl Way {
    pub fn parse(name: &str) -> Option<Way> {
        match name {
            "vm_opt" => Some(Way::VmOpt),
            "llvm_opt" => Some(Way::LlvmOpt),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Way::VmOpt => "vm_opt",
            Way::LlvmOpt => "llvm_opt",
        }
    }

    /// The flags of the plain way this one is compared with.
    fn plain_flags(self) -> &'static [&'static str] {
        match self {
            Way::VmOpt => &[],
            Way::LlvmOpt => &["--native"],
        }
    }
}

pub struct Options {
    /// From the repository root.
    pub fixture: String,
    pub way: Way,
    pub timeout: Duration,
    pub flux: PathBuf,
}

/// Returns `Ok(true)` when it named a rewrite or found nothing to bisect,
/// `Ok(false)` when the difference is outside the counted rewrites.
pub fn run_bisect(root: &Path, options: &Options) -> Result<bool, String> {
    let started = Instant::now();
    let stem = Path::new(&options.fixture)
        .file_stem()
        .map_or_else(|| "fixture".into(), |s| s.to_string_lossy().into_owned());
    let out = store::run_dir(root, &format!("bisect_{stem}"))?;
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;

    let build = run(cargo()
        .current_dir(root)
        .args(["build", "--all-features", "--quiet"]))
    .map_err(|e| e.to_string())?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }

    let flux = |flags: &[&str], show: Option<&str>| {
        let mut command = Command::new(&options.flux);
        command
            .args(["--no-cache", "--quiet"])
            .args(options.way.plain_flags())
            .args(flags)
            .arg(&options.fixture);
        if let Some(def) = show {
            command.env("FLUX_OPT_BISECT_SHOW", def);
        }
        run_with_timeout(command, root, options.timeout)
    };
    let mut runs = 0;
    let mut opt = |limit: i64, show: Option<&str>| {
        runs += 1;
        flux(
            &["--optimize", "--opt-bisect-limit", &limit.to_string()],
            show,
        )
    };

    println!("bisect-opt: {} on {}", options.fixture, options.way.name());
    let reference = flux(&[], None);
    let all = opt(-1, None);
    let lines = rewrite_lines(&all.stderr);
    let again = opt(-1, None);
    if rewrite_lines(&again.stderr) != lines {
        return Err(
            "rewrite numbering is not deterministic: two runs with --opt-bisect-limit -1 differ"
                .to_string(),
        );
    }
    let count = lines.len() as u64;
    println!("{count} rewrite(s); the numbering is the same on two runs");

    let mut report = format!(
        "bisect-opt {} on {} ({count} rewrites)\n",
        options.fixture,
        options.way.name()
    );
    if same_behaviour(&reference, &all) {
        report.push_str("the opt way behaves as the plain way: nothing to bisect\n");
        return finish(&out, &report, runs, started).map(|()| true);
    }
    if !same_behaviour(&reference, &opt(0, None)) {
        report.push_str(
            "it still differs with every counted rewrite skipped (limit 0), so the cause is \
             outside them: the simplifier's extra rounds, LLVM -O2 (llvm_opt), or an \
             uncounted pass\n",
        );
        return finish(&out, &report, runs, started).map(|()| false);
    }

    let culprit = first_bad(count, |n| {
        let differs = !same_behaviour(&reference, &opt(i64::try_from(n).unwrap_or(i64::MAX), None));
        println!("  limit {n}: {}", if differs { "differs" } else { "same" });
        Ok(differs)
    })?;
    let line = &lines[usize::try_from(culprit - 1).unwrap_or(0)];
    let _ = writeln!(
        report,
        "first rewrite that changes the behaviour:\n  {line}"
    );
    if let Some(def) = def_of(line) {
        let n = i64::try_from(culprit).unwrap_or(i64::MAX);
        let without = shown(&opt(n - 1, Some(def)).stderr);
        let with = shown(&opt(n, Some(def)).stderr);
        let _ = writeln!(report, "\n`{def}` without it (-) and with it (+):");
        report.push_str(&line_diff(&without, &with));
    } else {
        report.push_str("an AST rewrite, before Core: no definition to show\n");
    }
    finish(&out, &report, runs, started).map(|()| true)
}

fn finish(out: &Path, report: &str, runs: usize, started: Instant) -> Result<(), String> {
    let report = format!(
        "{report}\n{runs} opt run(s) in {}\n",
        format_duration(started.elapsed())
    );
    let file = out.join("report.txt");
    fs::write(&file, &report).map_err(|e| format!("{}: {e}", file.display()))?;
    print!("{report}");
    println!("report: {}", store::display(&file));
    Ok(())
}

/// The `opt-bisect:` lines a `-1` run printed, in order.
fn rewrite_lines(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|line| line.starts_with("opt-bisect: "))
        .map(str::to_string)
        .collect()
}

/// Whether two runs behave the same: how they ended, their stdout, and
/// their stderr when they failed, without bisection's own lines.
fn same_behaviour(a: &Ran, b: &Ran) -> bool {
    let program_stderr = |ran: &Ran| -> String {
        ran.stderr
            .lines()
            .filter(|line| !line.starts_with("opt-bisect"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    a.finished == b.finished
        && a.exit == b.exit
        && a.stdout == b.stdout
        && (a.exit == 0 || program_stderr(a) == program_stderr(b))
}

/// The smallest `n` in `1..=bad` for which `differs(n)`, given that `0` does
/// not differ and `bad` does: a binary search, about log2(bad) calls.
fn first_bad(
    bad: u64,
    mut differs: impl FnMut(u64) -> Result<bool, String>,
) -> Result<u64, String> {
    let (mut good, mut bad) = (0, bad);
    while bad - good > 1 {
        let mid = good + (bad - good) / 2;
        if differs(mid)? {
            bad = mid;
        } else {
            good = mid;
        }
    }
    Ok(bad)
}

/// The definition an `opt-bisect:` line names:
/// `opt-bisect: 12 beta: <what> in <def> (<module>)`.
fn def_of(line: &str) -> Option<&str> {
    let before_module = &line[..line.rfind(" (")?];
    let def = &before_module[before_module.rfind(" in ")? + 4..];
    (!def.is_empty()).then_some(def)
}

/// The definitions `FLUX_OPT_BISECT_SHOW` printed, one after another.
fn shown(stderr: &str) -> String {
    let mut text = String::new();
    let mut inside = false;
    for line in stderr.lines() {
        if line.starts_with("opt-bisect-show: end") {
            inside = false;
        } else if let Some(heading) = line.strip_prefix("opt-bisect-show: ") {
            inside = true;
            let _ = writeln!(text, "== {heading}");
        } else if inside {
            let _ = writeln!(text, "{line}");
        }
    }
    text
}

/// A line diff of `a` and `b`: unchanged lines start with two spaces,
/// removed ones with `- `, added ones with `+ ` (longest common
/// subsequence; the texts are one definition each).
fn line_diff(a: &str, b: &str) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            let _ = writeln!(out, "  {}", a[i]);
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            let _ = writeln!(out, "- {}", a[i]);
            i += 1;
        } else {
            let _ = writeln!(out, "+ {}", b[j]);
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ran(exit: i32, stdout: &str, stderr: &str) -> Ran {
        Ran {
            finished: true,
            exit,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn the_search_finds_the_first_bad_limit() {
        for culprit in [1, 2, 37, 99, 100] {
            let mut calls = 0;
            let found = first_bad(100, |n| {
                calls += 1;
                Ok(n >= culprit)
            })
            .unwrap();
            assert_eq!(found, culprit);
            assert!(calls <= 7, "{calls} calls for 100 rewrites");
        }
        assert_eq!(first_bad(1, |_| unreachable!()).unwrap(), 1);
    }

    #[test]
    fn behaviour_ignores_bisection_lines_and_a_passing_runs_stderr() {
        let plain = ran(0, "1\n", "");
        assert!(same_behaviour(
            &plain,
            &ran(0, "1\n", "opt-bisect: 1 beta: x in f (m)\n")
        ));
        assert!(!same_behaviour(&plain, &ran(0, "2\n", "")));
        assert!(!same_behaviour(
            &ran(1, "", "error A"),
            &ran(1, "", "error B")
        ));
    }

    #[test]
    fn a_line_names_its_definition() {
        let line = "opt-bisect: 12 beta: apply a lambda in main (/x/Main.flx)";
        assert_eq!(def_of(line), Some("main"));
        assert_eq!(def_of("opt-bisect: 3 reuse: x in  (m)"), None);
        assert_eq!(
            def_of("opt-bisect: 4 desugar: drop a double `!` (/x/Main.flx)"),
            None
        );
    }

    #[test]
    fn shown_blocks_are_collected_and_diffed() {
        let stderr = "noise\nopt-bisect-show: core f\na\nb\nopt-bisect-show: end\n";
        assert_eq!(shown(stderr), "== core f\na\nb\n");
        assert_eq!(line_diff("a\nb\nc\n", "a\nx\nc\n"), "  a\n- b\n+ x\n  c\n");
    }

    #[test]
    fn ways_parse_by_name() {
        assert_eq!(Way::parse("llvm_opt"), Some(Way::LlvmOpt));
        assert_eq!(Way::parse("vm"), None);
    }
}
