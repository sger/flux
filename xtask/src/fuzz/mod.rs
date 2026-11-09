//! `cargo xtask fuzz` (proposal 0190 §6): generate well-typed Flux programs
//! and run them through `parity-check`, so the backends and the optimizer
//! are compared on programs nobody wrote as fixtures.
//!
//! A run's folder is `.verify/fuzz_<seed>/`, recreated each time:
//! - `cases/case_<i>.flx`: the programs that compile, which parity-check runs;
//! - `internal/`: programs the compiler failed on itself (a panic, a failed
//!   IR or Aether check, a timeout), each a compiler bug;
//! - `genfail/`: programs the compiler rejected with a diagnostic, each a
//!   generator bug;
//! - `mismatch/`: copies of the cases whose ways disagreed;
//! - `parity.log` and `summary.txt`.
//!
//! The copies sit outside `cases/` because parity-check sweeps a folder
//! recursively and would run them again.

mod generate;
mod reduce;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::outcome::{Steps, format_duration};
use crate::parity::{self, ParityCounts};
use crate::proc::run;
use crate::rewrite::{compile_errors, parallel};
use crate::store;
use crate::sweep::run_flux;
use crate::verify::{cargo, flux_binary};

pub use generate::Parts;
use generate::program_with;
pub use reduce::{ReduceOptions, run_reduce};

pub const DEFAULT_WAYS: &str = "vm,llvm,vm_opt,llvm_opt";

/// A generated program is small; one that takes this long to compile is a
/// finding in itself.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Lowercased stderr text only a failure of the compiler itself prints. A copy
/// of the front-end half of `CRASH_MARKERS` in `src/parity/runner.rs`, which
/// xtask can't import; E999 is matched by its title because FBIP's E999 is an
/// ordinary diagnostic.
const INTERNAL_MARKERS: &[&str] =
    &["panicked at", "internal compiler error", "core lint failure", "aether verification failed"];

pub struct Options {
    pub seed: u64,
    pub count: usize,
    pub ways: String,
    /// parity-check's jobs. The compile check uses all cores but two.
    pub jobs: usize,
    /// Write the cases and stop.
    pub gen_only: bool,
    /// The parts of the language beyond S8's subset (`--with`, 0190 S17).
    pub parts: Parts,
}

/// `fuzz_<seed>`, or `fuzz_<seed>_<parts>` with parts on, so that runs of
/// one seed with and without them keep their own cases.
fn run_dir(options: &Options) -> String {
    if options.parts == Parts::NONE {
        format!("fuzz_{}", options.seed)
    } else {
        format!("fuzz_{}_{}", options.seed, options.parts.names().replace(',', "_"))
    }
}

pub fn run_fuzz(root: &Path, options: &Options) -> Result<bool, String> {
    let out = store::store_root(root).join(run_dir(options));
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let cases = out.join("cases");
    let internal = out.join("internal");
    let genfail = out.join("genfail");
    let mismatch = out.join("mismatch");
    for dir in [&cases, &internal, &genfail, &mismatch] {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    println!("outputs: {}", store::display(&out));
    let started = Instant::now();
    let mut steps = Steps::new(if options.gen_only { 1 } else { 4 });

    steps.start(&format!("generate {} program(s) from seed {}", options.count, options.seed));
    let files: Vec<String> = (0..options.count).map(|i| format!("case_{i}.flx")).collect();
    for (i, file) in files.iter().enumerate() {
        write(&cases.join(file), &program_with(options.seed, i, &options.parts))?;
    }
    if options.gen_only {
        return Ok(true);
    }

    steps.start("build");
    let build = run(cargo().current_dir(root).args(["build", "--all-features", "--quiet"]))
        .map_err(|e| e.to_string())?;
    if build != 0 {
        return Err("cargo build failed".to_string());
    }
    let flux = flux_binary(root);

    steps.start("compile each program");
    let compiled = parallel(&files, crate::sweep::default_jobs(), |file| {
        run_flux(root, &flux, &["--dump-core"], &cases.join(file).to_string_lossy(), TIMEOUT)
    });
    let mut compiler_failures = Vec::new();
    let mut generator_failures = Vec::new();
    for (file, ran) in files.iter().zip(compiled) {
        let (to, list, why) = match judge(ran.finished, ran.exit, &ran.stderr) {
            Compile::Ok => continue,
            Compile::Internal(why) => (&internal, &mut compiler_failures, why),
            Compile::Rejected(why) => (&genfail, &mut generator_failures, why),
        };
        fs::rename(cases.join(file), to.join(file))
            .map_err(|e| format!("{}: {e}", to.join(file).display()))?;
        list.push(format!("{file}: {why}"));
    }
    let failed = compiler_failures.len() + generator_failures.len();
    println!("{} of {} compile", files.len() - failed, files.len());

    steps.start(&format!("parity on {}", options.ways));
    let mut counts = ParityCounts::default();
    if failed < files.len() {
        let shown = cases.strip_prefix(root).unwrap_or(&cases).to_string_lossy().into_owned();
        let mut log = fs::File::create(out.join("parity.log")).map_err(|e| e.to_string())?;
        let extra = ["--jobs".to_string(), options.jobs.to_string()];
        counts = parity::parity_with_env(
            root,
            &flux,
            &shown,
            &options.ways,
            false,
            &mut log,
            &[],
            &extra,
        )
        .map_err(|e| e.to_string())?;
    }
    for line in &counts.failures {
        if let Some(path) = failing_file(line) {
            let from = root.join(path);
            let name = from.file_name().ok_or(format!("no file name in {line}"))?;
            fs::copy(&from, mismatch.join(name)).map_err(|e| format!("{}: {e}", from.display()))?;
        }
    }

    let failures = Failures { compiler: &compiler_failures, generator: &generator_failures };
    let summary = summary(options, files.len(), &failures, &counts, started.elapsed());
    write(&out.join("summary.txt"), &summary)?;
    print!("{summary}");
    Ok(failed == 0 && counts.ok())
}

/// How compiling one program went.
#[derive(Debug, PartialEq)]
enum Compile {
    Ok,
    /// The compiler failed on its own: a bug in the compiler.
    Internal(String),
    /// The compiler reported the program wrong: a bug in the generator.
    Rejected(String),
}

fn judge(finished: bool, exit: i32, stderr: &str) -> Compile {
    let first_error = || {
        let line = stderr.lines().find(|l| l.contains("error[E") || l.contains("panicked at"));
        line.or_else(|| stderr.lines().find(|l| !l.trim().is_empty())).unwrap_or("").trim().into()
    };
    let lower = stderr.to_lowercase();
    if !finished {
        Compile::Internal("timed out compiling".into())
    } else if INTERNAL_MARKERS.iter().any(|marker| lower.contains(marker)) {
        Compile::Internal(first_error())
    } else if compile_errors(stderr) {
        Compile::Rejected(first_error())
    } else if exit != 0 {
        // No diagnostic explains the failure, so the compiler itself crashed.
        Compile::Internal(format!("exit {exit}: {}", first_error()))
    } else {
        Compile::Ok
    }
}

struct Failures<'a> {
    compiler: &'a [String],
    generator: &'a [String],
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The file a failing verdict line names: `MISMATCH <file>` or
/// `EXPECTED_OUTPUT_MISMATCH <file>`.
fn failing_file(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once(' ')?;
    rest.split_whitespace().next()
}

fn summary(
    options: &Options,
    generated: usize,
    failures: &Failures<'_>,
    counts: &ParityCounts,
    took: Duration,
) -> String {
    let mut text = String::new();
    let compiled = generated - failures.compiler.len() - failures.generator.len();
    let failing = counts.mismatches + counts.expected;
    let with = if options.parts == Parts::NONE {
        String::new()
    } else {
        format!(", with {}", options.parts.names())
    };
    let _ = writeln!(
        text,
        "fuzz: seed {}{with}, {} program(s), ways {}",
        options.seed, generated, options.ways
    );
    let _ = writeln!(text, "internal compiler errors: {}", failures.compiler.len());
    for failure in failures.compiler {
        let _ = writeln!(text, "  {failure}");
    }
    let _ = writeln!(text, "generator failures: {} (target 0)", failures.generator.len());
    for failure in failures.generator {
        let _ = writeln!(text, "  {failure}");
    }
    let _ = writeln!(
        text,
        "parity: {} of {compiled} agree, {} MISMATCH, {} EXPECTED_OUTPUT_MISMATCH, exit {}",
        compiled.saturating_sub(failing),
        counts.mismatches,
        counts.expected,
        counts.exit
    );
    for line in &counts.failures {
        let _ = writeln!(text, "  {line}");
    }
    let _ = writeln!(text, "took {}", format_duration(took));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_file_from_a_failing_verdict() {
        assert_eq!(
            failing_file("MISMATCH .verify/fuzz_1/cases/case_3.flx"),
            Some(".verify/fuzz_1/cases/case_3.flx")
        );
        assert_eq!(failing_file("EXPECTED_OUTPUT_MISMATCH a.flx (x)"), Some("a.flx"));
        assert_eq!(failing_file("MISMATCH"), None);
    }

    #[test]
    fn a_summary_lists_each_failure() {
        let options = Options {
            seed: 1,
            count: 3,
            ways: DEFAULT_WAYS.into(),
            jobs: 1,
            gen_only: false,
            parts: Parts::NONE,
        };
        let counts = ParityCounts {
            mismatches: 1,
            exit: 1,
            failures: vec!["MISMATCH c/case_2.flx".into()],
            ..ParityCounts::default()
        };
        let failures = Failures { compiler: &[], generator: &["case_0.flx: error[E001]".into()] };
        let text = summary(&options, 3, &failures, &counts, Duration::ZERO);
        assert!(text.contains("internal compiler errors: 0\n"));
        assert!(text.contains("generator failures: 1 (target 0)\n  case_0.flx: error[E001]\n"));
        assert!(text.contains("parity: 1 of 2 agree, 1 MISMATCH, 0 EXPECTED_OUTPUT_MISMATCH, exit 1\n  MISMATCH c/case_2.flx\n"));
    }

    #[test]
    fn an_internal_error_is_the_compilers_and_a_diagnostic_the_generators() {
        let aether = "error[E999]: Aether Verification Failed\n  --> case_8.flx:3:1\n";
        assert_eq!(
            judge(true, 1, aether),
            Compile::Internal("error[E999]: Aether Verification Failed".into())
        );
        let fbip = "error[E999]: FBIP NOT PROVABLE\n`f` is not yet provable\n";
        assert_eq!(
            judge(true, 1, fbip),
            Compile::Rejected("error[E999]: FBIP NOT PROVABLE".into())
        );
        let panic = "thread 'main' panicked at src/x.rs:1:1:\nboom\n";
        assert!(matches!(judge(true, 101, panic), Compile::Internal(_)));
        assert_eq!(judge(false, -1, ""), Compile::Internal("timed out compiling".into()));
        assert_eq!(
            judge(true, 1, "error[E001]: Duplicate Name\n"),
            Compile::Rejected("error[E001]: Duplicate Name".into())
        );
        assert_eq!(judge(true, 139, ""), Compile::Internal("exit 139: ".into()));
        assert_eq!(judge(true, 0, "warning[W001]: unused\n"), Compile::Ok);
    }
}
