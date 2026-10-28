//! Support census rows (proposal 0190 §16): one classified row per file and
//! way, written by `flux parity-check --census <file>`.
//!
//! A parity verdict says whether one file's ways agree. The census asks, per
//! way, whether that way supports the program: it ran and gave the right
//! output, failed as declared, rejected the program, crashed, or was not run.
//! `cargo xtask census` adds feature tags and known issues to these rows and
//! builds the support matrix from them.

use std::path::Path;
use std::time::Duration;

use super::fixture::{Expect, FixtureMeta};
use super::runner::error_codes;
use super::{ExitKind, ParityResult, RunResult, Way, stable_fingerprint};

/// How one way did on one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Succeeded with the right output: the expected block when the file
    /// declares one, otherwise the output most of the other ways agree on.
    Works,
    /// Succeeded with output the expected block, or most ways, disagree with;
    /// or failed differently from what the file declares.
    Wrong,
    /// Succeeded, but the ways split evenly on the output and the file
    /// declares none, so which side is right is unknown.
    Disagree,
    /// Failed as the file declares, with the declared error codes.
    ExpectedError,
    /// Rejected with a diagnostic a program expected to succeed.
    Rejected,
    /// The compiler crashed, a tool failed, or the run timed out.
    Crashed,
    /// A file without `main` that compiled: nothing to run.
    CompileOnly,
    /// Not run: skipped, outside the file's `// parity:` ways, or a
    /// `// skip-way:`. The row's note says which.
    NotRun,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Works => "works",
            Self::Wrong => "wrong",
            Self::Disagree => "disagree",
            Self::ExpectedError => "expected-error",
            Self::Rejected => "rejected",
            Self::Crashed => "crashed",
            Self::CompileOnly => "compile-only",
            Self::NotRun => "not-run",
        }
    }
}

/// One census row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub way: Way,
    pub exit_kind: Option<ExitKind>,
    pub class: Class,
    /// The run's error codes, as `E300`.
    pub codes: Vec<String>,
    /// A fingerprint of the normalized stdout of a successful run.
    pub stdout: Option<String>,
    pub elapsed: Option<Duration>,
    /// Why the row has its class, when that is not obvious: a skip reason
    /// (with its KI), the declared expectation, or the ways that disagree.
    pub note: String,
}

/// The census file's first line.
pub const HEADER: &str = "file\tway\texit\tclass\tcodes\tstdout\tms\tnote";

impl Row {
    pub fn to_tsv(&self) -> String {
        let exit = self.exit_kind.map(|kind| kind.to_string()).unwrap_or_default();
        let ms = self.elapsed.map(|elapsed| elapsed.as_millis().to_string()).unwrap_or_default();
        format!(
            "{}\t{}\t{exit}\t{}\t{}\t{}\t{ms}\t{}",
            self.file,
            self.way,
            self.class.as_str(),
            self.codes.join(","),
            self.stdout.as_deref().unwrap_or_default(),
            one_line(&self.note),
        )
    }
}

/// Text fit for one TSV field.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `way` is one the file's `// parity:` line allows. A line naming a
/// plain way allows its optimized and debug-heap ways too: `// parity: vm`
/// rules out the native ways, not the optimizer or the debug heap.
pub fn way_allowed(meta: &FixtureMeta, way: Way) -> bool {
    !meta.declared_ways
        || meta.ways.contains(&way)
        || meta.ways.contains(&way.non_opt())
        || meta.ways.contains(&way.non_heap())
}

/// What the census knows about one checked file.
pub struct Checked<'a> {
    pub result: &'a ParityResult,
    pub meta: &'a FixtureMeta,
    /// Every way the census asked for, including those the file rules out.
    pub ways: &'a [Way],
    pub has_main: bool,
}

/// The file's census rows, one per way in `checked.ways`.
pub fn rows(checked: &Checked<'_>) -> Vec<Row> {
    let file = checked.result.file.display().to_string();
    checked
        .ways
        .iter()
        .map(|&way| {
            let (class, run, note) = classify(checked, way);
            Row {
                file: file.clone(),
                way,
                exit_kind: run.map(|run| run.exit_kind),
                class,
                codes: run.map(|run| codes_of(&run.stderr)).unwrap_or_default(),
                stdout: run
                    .filter(|run| run.exit_kind == ExitKind::Success)
                    .map(|run| stable_fingerprint(&run.normalized_stdout)),
                elapsed: checked
                    .result
                    .elapsed
                    .iter()
                    .find(|(timed, _)| *timed == way)
                    .map(|&(_, elapsed)| elapsed),
                note,
            }
        })
        .collect()
}

/// Rows for a file the census lists but does not run, such as a benchmark.
pub fn unrun_rows(file: &Path, ways: &[Way], reason: &str) -> Vec<Row> {
    ways.iter()
        .map(|&way| Row {
            file: file.display().to_string(),
            way,
            exit_kind: None,
            class: Class::NotRun,
            codes: Vec::new(),
            stdout: None,
            elapsed: None,
            note: reason.to_string(),
        })
        .collect()
}

fn codes_of(stderr: &str) -> Vec<String> {
    let mut codes: Vec<String> = error_codes(stderr).map(|n| format!("E{n:03}")).collect();
    codes.dedup();
    codes
}

fn classify<'a>(checked: &Checked<'a>, way: Way) -> (Class, Option<&'a RunResult>, String) {
    let Checked { result, meta, .. } = checked;
    if !way_allowed(meta, way) {
        let declared = meta.ways.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        return (Class::NotRun, None, format!("parity: {declared}"));
    }
    if let Some(skip) = result.skipped_ways.iter().find(|skip| skip.way == way) {
        return (Class::NotRun, None, format!("skip-way: {}", skip.reason));
    }
    let Some(run) = result.results.iter().find(|run| run.way == way) else {
        let reason = match &result.verdict {
            super::Verdict::Skip { reason } => format!("skip: {reason}"),
            super::Verdict::ExpectedOutputMismatch { actual, .. } if result.results.is_empty() => {
                format!("metadata: {actual}")
            }
            _ => "not run".to_string(),
        };
        return (Class::NotRun, None, reason);
    };
    let (class, note) = classify_run(checked, run);
    (class, Some(run), note)
}

fn classify_run(checked: &Checked<'_>, run: &RunResult) -> (Class, String) {
    let meta = checked.meta;
    match run.exit_kind {
        ExitKind::Crash | ExitKind::Timeout | ExitKind::ToolFailure => {
            return (Class::Crashed, first_line(&run.stderr));
        }
        _ => {}
    }
    let expected_failure = match meta.expect {
        Expect::Any => return classify_by_agreement(checked, run),
        Expect::Success => None,
        Expect::CompileError => Some(ExitKind::CompileError),
        Expect::RuntimeError => Some(ExitKind::RuntimeError),
    };
    if let Some(expected) = expected_failure {
        let codes = codes_of(&run.stderr);
        let missing: Vec<&String> =
            meta.expected_errors.iter().filter(|code| !codes.contains(code)).collect();
        return if run.exit_kind == expected && missing.is_empty() {
            (Class::ExpectedError, String::new())
        } else if run.exit_kind == expected {
            let missing = missing.iter().map(|c| c.as_str()).collect::<Vec<_>>();
            (Class::Wrong, format!("missing {}", missing.join(", ")))
        } else {
            (Class::Wrong, format!("expected {expected}, got {}", run.exit_kind))
        };
    }
    match run.exit_kind {
        ExitKind::CompileError => (Class::Rejected, first_line(&run.stderr)),
        ExitKind::RuntimeError => (Class::Wrong, first_line(&run.stderr)),
        _ if !checked.has_main => (Class::CompileOnly, String::new()),
        _ => classify_output(checked, run),
    }
}

/// Under `expect: any`, whatever most ways do is right: the same exit kind,
/// and the same error codes, or the same stdout when it succeeds. Crashed
/// runs, already classed, do not vote.
fn classify_by_agreement(checked: &Checked<'_>, run: &RunResult) -> (Class, String) {
    let outcome = |r: &RunResult| -> (ExitKind, String) {
        if r.exit_kind == ExitKind::Success {
            (r.exit_kind, r.normalized_stdout.clone())
        } else {
            (r.exit_kind, codes_of(&r.stderr).join(","))
        }
    };
    let voters: Vec<(Way, (ExitKind, String))> = checked
        .result
        .results
        .iter()
        .filter(|r| {
            !matches!(r.exit_kind, ExitKind::Crash | ExitKind::Timeout | ExitKind::ToolFailure)
        })
        .map(|r| (r.way, outcome(r)))
        .collect();
    let mine = outcome(run);
    let votes = |o: &(ExitKind, String)| voters.iter().filter(|(_, v)| v == o).count();
    let best_other =
        voters.iter().filter(|(_, v)| *v != mine).map(|(_, v)| votes(v)).max().unwrap_or(0);
    let others = || {
        voters
            .iter()
            .filter(|(_, v)| *v != mine)
            .map(|(way, (kind, _))| format!("{way} {kind}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let agreed = match run.exit_kind {
        ExitKind::Success if checked.has_main => Class::Works,
        ExitKind::Success => Class::CompileOnly,
        _ => Class::ExpectedError,
    };
    match votes(&mine).cmp(&best_other) {
        std::cmp::Ordering::Greater => (agreed, String::new()),
        std::cmp::Ordering::Equal => (Class::Disagree, format!("differs from {}", others())),
        std::cmp::Ordering::Less => (Class::Wrong, format!("differs from {}", others())),
    }
}

/// A successful run's output against the expected block, or else against the
/// other successful runs.
fn classify_output(checked: &Checked<'_>, run: &RunResult) -> (Class, String) {
    if checked.meta.stdout_varies {
        return (Class::Works, "stdout varies: exit compared only".to_string());
    }
    if let Some(expected) = &checked.meta.expected_stdout {
        return if run.normalized_stdout.trim() == expected.trim() {
            (Class::Works, String::new())
        } else {
            (Class::Wrong, "differs from the expected stdout".to_string())
        };
    }
    let succeeded: Vec<&RunResult> = checked
        .result
        .results
        .iter()
        .filter(|other| other.exit_kind == ExitKind::Success)
        .collect();
    let agreeing =
        |stdout: &str| succeeded.iter().filter(|other| other.normalized_stdout == stdout).count();
    let mine = agreeing(&run.normalized_stdout);
    let best_other = succeeded
        .iter()
        .filter(|other| other.normalized_stdout != run.normalized_stdout)
        .map(|other| agreeing(&other.normalized_stdout))
        .max()
        .unwrap_or(0);
    let others = || {
        succeeded
            .iter()
            .filter(|other| other.normalized_stdout != run.normalized_stdout)
            .map(|other| other.way.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match mine.cmp(&best_other) {
        std::cmp::Ordering::Greater => (Class::Works, String::new()),
        std::cmp::Ordering::Equal => (Class::Disagree, format!("stdout differs from {}", others())),
        std::cmp::Ordering::Less => (Class::Wrong, format!("stdout differs from {}", others())),
    }
}

/// The line that best says why a run failed: the first diagnostic or panic,
/// else the first line that is not compiler progress.
fn first_line(stderr: &str) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|line| !line.is_empty());
    let is_progress = |line: &str| line.starts_with('[') && !line.starts_with("[E");
    lines()
        .find(|line| line.contains("error[E") || line.contains("panicked"))
        .or_else(|| lines().find(|line| line.contains("error")))
        .or_else(|| lines().find(|line| !is_progress(line)))
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::fixture::SkipWay;
    use crate::parity::{ParityResult, Verdict};
    use std::path::PathBuf;

    const WAYS: [Way; 4] = [Way::Vm, Way::Llvm, Way::VmOpt, Way::LlvmOpt];

    fn run(way: Way, exit_kind: ExitKind, stdout: &str, stderr: &str) -> RunResult {
        RunResult {
            way,
            exit_kind,
            exit_code: i32::from(exit_kind != ExitKind::Success),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            normalized_stdout: stdout.to_string(),
            normalized_stderr: stderr.to_string(),
            cache_observations: Vec::new(),
            leak: None,
        }
    }

    fn result(results: Vec<RunResult>) -> ParityResult {
        ParityResult {
            file: PathBuf::from("examples/x.flx"),
            elapsed: results.iter().map(|r| (r.way, Duration::from_millis(5))).collect(),
            results,
            artifacts: Vec::new(),
            verdict: Verdict::Pass,
            skipped_ways: Vec::new(),
            unexpected_passes: Vec::new(),
        }
    }

    fn classes(result: &ParityResult, meta: &FixtureMeta, has_main: bool) -> Vec<Class> {
        rows(&Checked { result, meta, ways: &WAYS, has_main })
            .into_iter()
            .map(|row| row.class)
            .collect()
    }

    #[test]
    fn the_majority_output_works_and_the_odd_one_out_is_wrong() {
        let r = result(vec![
            run(Way::Vm, ExitKind::Success, "1", ""),
            run(Way::Llvm, ExitKind::Success, "1", ""),
            run(Way::VmOpt, ExitKind::Success, "1", ""),
            run(Way::LlvmOpt, ExitKind::Success, "2", ""),
        ]);
        let meta = FixtureMeta::default();
        use Class::*;
        assert_eq!(classes(&r, &meta, true), [Works, Works, Works, Wrong]);
    }

    #[test]
    fn an_even_split_without_expected_output_is_a_disagreement() {
        let r = result(vec![
            run(Way::Vm, ExitKind::Success, "1", ""),
            run(Way::Llvm, ExitKind::Success, "2", ""),
            run(Way::VmOpt, ExitKind::Success, "1", ""),
            run(Way::LlvmOpt, ExitKind::Success, "2", ""),
        ]);
        let meta = FixtureMeta::default();
        assert!(classes(&r, &meta, true).iter().all(|c| *c == Class::Disagree));
    }

    #[test]
    fn the_expected_block_decides_when_there_is_one() {
        let r = result(vec![
            run(Way::Vm, ExitKind::Success, "1", ""),
            run(Way::Llvm, ExitKind::Success, "2", ""),
            run(Way::VmOpt, ExitKind::Success, "1", ""),
            run(Way::LlvmOpt, ExitKind::Success, "2", ""),
        ]);
        let meta = FixtureMeta { expected_stdout: Some("2".to_string()), ..Default::default() };
        use Class::*;
        assert_eq!(classes(&r, &meta, true), [Wrong, Works, Wrong, Works]);
    }

    #[test]
    fn failures_are_rejected_crashed_or_wrong() {
        let r = result(vec![
            run(Way::Vm, ExitKind::Success, "", ""),
            run(Way::Llvm, ExitKind::CompileError, "", "error[E300]: no"),
            run(Way::VmOpt, ExitKind::Crash, "", "thread 'main' panicked at x"),
            run(Way::LlvmOpt, ExitKind::RuntimeError, "", "error[E1009]: panic"),
        ]);
        let meta = FixtureMeta::default();
        use Class::*;
        let rows = rows(&Checked { result: &r, meta: &meta, ways: &WAYS, has_main: true });
        let got: Vec<Class> = rows.iter().map(|row| row.class).collect();
        assert_eq!(got, [Works, Rejected, Crashed, Wrong]);
        assert_eq!(rows[1].codes, ["E300"]);
        assert!(rows[2].note.contains("panicked"));
    }

    #[test]
    fn a_declared_failure_is_expected_only_with_its_codes() {
        let r = result(vec![
            run(Way::Vm, ExitKind::CompileError, "", "error[E300]: a"),
            run(Way::Llvm, ExitKind::CompileError, "", "error[E301]: b"),
            run(Way::VmOpt, ExitKind::Success, "", ""),
            run(Way::LlvmOpt, ExitKind::RuntimeError, "", "error[E1009]: c"),
        ]);
        let meta = FixtureMeta {
            expect: Expect::CompileError,
            expected_errors: vec!["E300".to_string()],
            ..Default::default()
        };
        use Class::*;
        assert_eq!(classes(&r, &meta, true), [ExpectedError, Wrong, Wrong, Wrong]);
    }

    #[test]
    fn under_expect_any_the_ways_must_agree() {
        let r = result(vec![
            run(Way::Vm, ExitKind::RuntimeError, "", "error[E1000]: arity"),
            run(Way::Llvm, ExitKind::Success, "", ""),
            run(Way::VmOpt, ExitKind::RuntimeError, "", "error[E1000]: arity"),
            run(Way::LlvmOpt, ExitKind::RuntimeError, "", "error[E1000]: arity"),
        ]);
        let meta = FixtureMeta { expect: Expect::Any, ..Default::default() };
        use Class::*;
        assert_eq!(classes(&r, &meta, true), [ExpectedError, Wrong, ExpectedError, ExpectedError]);
        let compiles = result(vec![
            run(Way::Vm, ExitKind::Success, "", ""),
            run(Way::Llvm, ExitKind::Success, "", ""),
        ]);
        assert_eq!(classes(&compiles, &meta, false)[..2], [CompileOnly, CompileOnly]);
    }

    #[test]
    fn a_file_without_main_is_compile_only() {
        let r = result(vec![run(Way::Vm, ExitKind::Success, "", "")]);
        let meta = FixtureMeta::default();
        assert_eq!(classes(&r, &meta, false)[0], Class::CompileOnly);
    }

    #[test]
    fn unrun_ways_say_why() {
        let mut r = result(vec![run(Way::Vm, ExitKind::Success, "1", "")]);
        r.skipped_ways = vec![SkipWay { way: Way::VmOpt, reason: "KI-104 dictionary".to_string() }];
        let meta = FixtureMeta { ways: vec![Way::Vm], declared_ways: true, ..Default::default() };
        let rows = rows(&Checked { result: &r, meta: &meta, ways: &WAYS, has_main: true });
        assert_eq!(rows[0].class, Class::Works);
        assert_eq!(rows[1].note, "parity: vm");
        assert!(rows[2].note.starts_with("skip-way: KI-104"));
        assert_eq!(rows[3].class, Class::NotRun);
    }

    #[test]
    fn a_row_is_one_tsv_line() {
        let r =
            result(vec![run(Way::Vm, ExitKind::CompileError, "", "error[E300]: a\n\tsecond line")]);
        let meta = FixtureMeta::default();
        let row = &rows(&Checked { result: &r, meta: &meta, ways: &[Way::Vm], has_main: true })[0];
        let line = row.to_tsv();
        assert_eq!(line.split('\t').count(), HEADER.split('\t').count());
        assert!(!line.contains('\n'));
        assert!(line.starts_with("examples/x.flx\tvm\tCompileError\trejected\tE300\t\t5\t"));
    }

    #[test]
    fn the_note_skips_compiler_progress() {
        let stderr = "[ 1 of 18] Compiling Add\n[cfg→vm] Running\nerror[E300]: mismatch\n• 1 error • x.flx\n";
        assert_eq!(first_line(stderr), "error[E300]: mismatch");
        assert_eq!(first_line("[ 1 of 2] x\nsegfault\n"), "segfault");
    }
}
