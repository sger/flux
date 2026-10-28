//! CLI entry point for `flux parity-check`.
//!
//! Usage:
//!   flux parity-check <file-or-dir> [--root <path> ...] [options]
//!
//! Options:
//!   --ways <w1,w2,...>     Ways to compare (default: vm,llvm)
//!   --vm-binary <path>     Path to VM flux binary
//!   --llvm-binary <path>   Path to native flux binary
//!   --timeout <secs>       Timeout per file per way (default: 20)
//!   --root <path>          Module root (forwarded to flux)
//!   --jobs <n>             Files checked at once (default: 1)
//!   --census <file.tsv>    Write one classified row per file and way
//!   --allow <resource>     Run fixtures that `// requires:` it (network)
//!   --bless                Write what agreeing ways printed into the expected blocks

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::census;
use super::fixture::{
    Expect, ExpectedCount, FixtureMeta, SkipWay, annotations_inside_tokens, converted_source,
    parse_fixture_meta, parse_fixture_meta_allowing, with_annotations,
};
use super::known_issues;
use super::normalize::normalize_stderr;
use super::report::{
    DisplayFilter, cargo_run_for_way, diagnose_mismatch, print_debug_first_failure, print_result,
    print_summary,
};
use super::runner::{
    DEFAULT_TIMEOUT_SECS, capture_dump_aether, capture_dump_cfg, capture_dump_core,
    capture_dump_lir, capture_dump_repr, capture_metrics, compile_fixture, is_native_skip, run_way,
};
use super::{
    BackendId, DebugArtifacts, ExitKind, MismatchDetail, ParityResult, SurfaceKind, Verdict, Way,
    backend_spec,
};

fn default_vm_binary() -> PathBuf {
    default_parity_binary("target/parity_vm/debug")
}

fn default_llvm_binary() -> PathBuf {
    default_parity_binary("target/parity_native/debug")
}

fn default_parity_binary(debug_dir: &str) -> PathBuf {
    let mut path = PathBuf::from(debug_dir);
    path.push(format!("flux{}", std::env::consts::EXE_SUFFIX));
    path
}

/// Entry point called from `main.rs`.
pub fn run_parity_check(args: &[String]) {
    let config = match parse_args(args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("Error: {msg}");
            eprintln!();
            print_usage();
            std::process::exit(1);
        }
    };

    let binary_statuses = ensure_parity_binaries(&config);

    // Validate binaries exist
    if !config.vm_binary.exists() {
        eprintln!(
            "Error: VM binary not found at {}\n\
             Build with: CARGO_TARGET_DIR=target/parity_vm cargo build",
            config.vm_binary.display()
        );
        std::process::exit(1);
    }
    if !config.llvm_binary.exists() {
        eprintln!(
            "Error: LLVM binary not found at {}\n\
             Build with: CARGO_TARGET_DIR=target/parity_native cargo build --features llvm",
            config.llvm_binary.display()
        );
        std::process::exit(1);
    }

    print_run_context(&config, &binary_statuses);

    // Collect .flx files
    let census_mode = config.census.is_some();
    let files: Vec<PathBuf> =
        config.paths.iter().flat_map(|path| collect_fixtures(path, census_mode)).collect();
    if files.is_empty() {
        let paths = config
            .paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("Error: no .flx files found in {paths}");
        std::process::exit(1);
    }
    let jobs = if config.debug_first_failure { 1 } else { config.jobs };
    eprintln!("[parity] fixtures: {} (jobs: {jobs})", files.len());

    // `--compile` pre-pass: compile every fixture (populating caches) before
    // starting the parity loop. Bail on the first compilation failure.
    if config.compile_first && !run_compile_phase(&files, &config) {
        std::process::exit(1);
    }

    let mut census_out = config.census.as_deref().map(|path| {
        CensusWriter::create(path).unwrap_or_else(|err| {
            eprintln!("Error: cannot write {}: {err}", path.display());
            std::process::exit(1);
        })
    });
    let mut results = Vec::new();
    let mut saved_first_failure = false;

    run_in_order(
        &files,
        jobs,
        |file| check_one(file, &config),
        |checked| {
            let mut parity_result = checked.result;
            if let Some(out) = census_out.as_mut() {
                out.write(&checked.rows);
            }
            if config.annotate {
                annotate(&mut parity_result, &config);
            }
            if config.bless {
                // The verdict was judged against the old annotations; judge
                // again so a stale stderr block can be blessed in the same run.
                if bless_annotations(&parity_result, &config.allow) {
                    parity_result = check_one(&parity_result.file, &config).result;
                }
                bless(&mut parity_result, &config.allow);
            }
            print_result(&parity_result, config.display_filter, config.explain);
            let is_non_pass = !matches!(parity_result.verdict, Verdict::Pass);
            if is_non_pass
                && !saved_first_failure
                && let Some(dir) = &config.save_debug_dir
            {
                if let Err(err) = save_debug_bundle(dir, &parity_result) {
                    eprintln!("[parity] failed to save debug bundle: {err}");
                } else {
                    eprintln!("[parity] saved first failure bundle to {}", dir.display());
                }
                saved_first_failure = true;
            }
            let should_stop = config.debug_first_failure && is_non_pass;
            if should_stop {
                print_debug_first_failure(&parity_result);
            }
            results.push(parity_result);
            !should_stop
        },
    );

    print_summary(&results);
    if let (Some(out), Some(path)) = (census_out, &config.census) {
        let rows = out.finish();
        eprintln!("[parity] census: {rows} row(s) in {}", path.display());
        // A census records; it never fails on what it finds.
        return;
    }

    // Exit with appropriate code
    let has_mismatch = results.iter().any(|r| {
        matches!(r.verdict, Verdict::Mismatch { .. } | Verdict::ExpectedOutputMismatch { .. })
    });
    let has_pass = results.iter().any(|r| matches!(r.verdict, Verdict::Pass));

    if has_mismatch || !has_pass {
        std::process::exit(1);
    }
}

fn cached_equivalent_ways(ways: &[Way]) -> Vec<Way> {
    ways.iter()
        .copied()
        .map(|way| match way {
            Way::Vm => Way::VmCached,
            Way::Llvm => Way::LlvmCached,
            other => other,
        })
        .collect()
}

/// The fixtures `flux parity-check <dir>` would check, in the same order.
pub fn fixtures_in(dir: &Path) -> Vec<PathBuf> {
    collect_fixtures(dir, false)
}

/// Checks one fixture as `flux parity-check <file>` would, with the ways its
/// header names, running every way on `flux`. Unlike the command it never
/// rebuilds a binary, so a test can pass the `flux` cargo just built.
pub fn check_fixture(file: &Path, flux: &Path) -> ParityResult {
    let flux = flux.display().to_string();
    let args = [
        file.display().to_string(),
        "--vm-binary".to_string(),
        flux.clone(),
        "--llvm-binary".to_string(),
        flux,
    ];
    let config = parse_args(&args).expect("parity-check arguments");
    check_one(file, &config).result
}

/// An expected-output block in a fixture, between its begin and end comments.
#[derive(Clone, Copy)]
enum Block {
    Stdout,
    Stderr,
}

impl Block {
    fn begins_at(self, comment: &str) -> bool {
        match self {
            Self::Stdout => {
                comment == "parity-expected-stdout-begin" || comment == "parity-oracle-stdout-begin"
            }
            Self::Stderr => comment == "parity-expected-stderr-begin",
        }
    }

    fn ends_at(self, comment: &str) -> bool {
        match self {
            Self::Stdout => {
                comment == "parity-expected-stdout-end" || comment == "parity-oracle-stdout-end"
            }
            Self::Stderr => comment == "parity-expected-stderr-end",
        }
    }

    fn markers(self) -> (&'static str, &'static str) {
        match self {
            Self::Stdout => ("parity-expected-stdout-begin", "parity-expected-stdout-end"),
            Self::Stderr => ("parity-expected-stderr-begin", "parity-expected-stderr-end"),
        }
    }
}

/// `--bless`: when every way ran and they agree, writes what they printed into
/// the fixture, so later runs check it. A fixture that expects success gets its
/// stdout block written, or added after its header comments; one that expects
/// an error only has an existing stderr block replaced, since most pin codes
/// alone. Anything else that failed stays failed. A blessed result passes.
fn bless(result: &mut ParityResult, allow: &[String]) {
    let meta = parse_fixture_meta_allowing(&result.file, allow);
    let runs = &result.results;
    let Some(first) = runs.first() else {
        return;
    };
    let stdout_agrees =
        runs.iter().all(|run| run.normalized_stdout.trim() == first.normalized_stdout.trim());
    let stderr_agrees =
        runs.iter().all(|run| run.normalized_stderr.trim() == first.normalized_stderr.trim());
    let stdout_is_the_only_problem = match &result.verdict {
        Verdict::Pass => true,
        Verdict::ExpectedOutputMismatch { expected, .. } => {
            meta.expected_stdout.as_deref().map(str::trim) == Some(expected.as_str())
        }
        _ => false,
    };
    let stderr_is_the_only_problem = match &result.verdict {
        Verdict::Mismatch { details } => {
            details.iter().all(|detail| matches!(detail, MismatchDetail::ExpectedStderr { .. }))
        }
        _ => false,
    };
    let (block, body, add) = match meta.expect {
        Expect::Success
            if !meta.stdout_varies
                && stdout_is_the_only_problem
                && runs.iter().all(|run| run.exit_kind == ExitKind::Success)
                && stdout_agrees =>
        {
            (Block::Stdout, first.normalized_stdout.trim(), true)
        }
        Expect::CompileError | Expect::RuntimeError
            if meta.expected_stderr.is_some() && stderr_is_the_only_problem && stderr_agrees =>
        {
            (Block::Stderr, first.normalized_stderr.trim(), false)
        }
        _ => return,
    };
    let file = result.file.clone();
    let Ok(source) = fs::read_to_string(&file) else {
        return;
    };
    let Some(blessed) = with_expected_block(&source, block, body, add) else {
        return;
    };
    if blessed != source {
        if let Err(err) = fs::write(&file, &blessed) {
            eprintln!("[parity] cannot bless {}: {err}", file.display());
            return;
        }
        eprintln!("[parity] blessed {}", file.display());
    }
    result.verdict = Verdict::Pass;
}

/// Rewrites an annotated fixture's `//~` annotations to match what its ways
/// report. Only when the annotations are the problem, every non-strict way
/// reports the same errors, and each is located in the fixture. Returns
/// whether the file changed.
fn bless_annotations(result: &ParityResult, allow: &[String]) -> bool {
    let meta = parse_fixture_meta_allowing(&result.file, allow);
    if meta.expected_locations.is_empty() {
        return false;
    }
    let Verdict::Mismatch { details } = &result.verdict else {
        return false;
    };
    let annotations_differ =
        details.iter().any(|d| matches!(d, MismatchDetail::DiagnosticLocations { .. }));
    let only_errors_differ = details.iter().all(|d| {
        matches!(
            d,
            MismatchDetail::DiagnosticLocations { .. } | MismatchDetail::ExpectedStderr { .. }
        )
    });
    if !annotations_differ || !only_errors_differ {
        return false;
    }
    let file = &result.file;
    let pairs = match reported_pairs(result) {
        Ok(pairs) => pairs,
        Err(reason) => {
            eprintln!("[parity] cannot bless {}: {reason}", file.display());
            return false;
        }
    };
    let Ok(source) = fs::read_to_string(file) else {
        return false;
    };
    let (blessed, unplaced) = with_annotations(&source, &pairs);
    for problem in &unplaced {
        eprintln!("[parity] {}: {problem}", file.display());
    }
    if let Some(line) = annotations_inside_tokens(&blessed).first() {
        eprintln!(
            "[parity] cannot bless {}: the annotation on line {line} would be read as part of the program",
            file.display()
        );
        return false;
    }
    if blessed == source {
        return false;
    }
    if let Err(err) = fs::write(file, &blessed) {
        eprintln!("[parity] cannot bless {}: {err}", file.display());
        return false;
    }
    eprintln!("[parity] blessed the annotations of {}", file.display());
    true
}

/// The (line, code) pairs the non-strict ways report, sorted, when they agree
/// and every error is located in the fixture. A fixture that runs only strict
/// ways (`// flags: --strict`) is read from those.
fn reported_pairs(result: &ParityResult) -> Result<Vec<(usize, String)>, String> {
    let all_strict = result.results.iter().all(|run| run.way.is_strict());
    let mut agreed: Option<Vec<(usize, String)>> = None;
    for run in result.results.iter().filter(|run| all_strict || !run.way.is_strict()) {
        let mut pairs = Vec::new();
        for error in diagnostic_locations(&run.normalized_stderr) {
            match (&error.path, error.line) {
                (Some(path), Some(line)) if same_file(path, &result.file) => {
                    pairs.push((line, error.code))
                }
                _ => {
                    return Err(format!("{} is not located in the fixture", error.describe(false)));
                }
            }
        }
        pairs.sort();
        match &agreed {
            Some(first) if *first != pairs => {
                return Err("the ways report different errors".to_string());
            }
            Some(_) => {}
            None => agreed = Some(pairs),
        }
    }
    agreed.ok_or_else(|| "no way ran".to_string())
}

/// `--annotate`: moves a passing fixture from `// expect-error:` lines to `//~`
/// annotations, and drops its expected-stderr block when a snapshot pins
/// exactly the same text. The file is left as it was, with the reason printed, unless every
/// error is located in the fixture, the codes are exactly the listed ones, and
/// the converted fixture passes too.
fn annotate(result: &mut ParityResult, config: &Config) {
    let file = result.file.clone();
    let meta = parse_fixture_meta_allowing(&file, &config.allow);
    if meta.expected_errors.is_empty()
        || !matches!(meta.expect, Expect::CompileError | Expect::RuntimeError)
    {
        return;
    }
    let keep = |reason: &str| eprintln!("[annotate] kept {}: {reason}", file.display());
    if !matches!(result.verdict, Verdict::Pass) {
        return keep("it does not pass today");
    }
    let pairs = match reported_pairs(result) {
        Ok(pairs) => pairs,
        Err(reason) => return keep(&reason),
    };
    let reported = sorted_codes(pairs.iter().map(|(_, code)| code.clone()));
    let listed = sorted_codes(meta.expected_errors.iter().cloned());
    if reported != listed {
        return keep(&format!("it reports {reported:?}, its header lists {listed:?}"));
    }
    let Ok(source) = fs::read_to_string(&file) else {
        return keep("it cannot be read");
    };
    let drop_block =
        meta.expected_stderr.as_deref().is_some_and(|block| snapshot_pins(&file, block));
    let converted = match converted_source(&source, &pairs, drop_block) {
        Ok(converted) => converted,
        Err(reason) => return keep(&reason),
    };
    if let Some(line) = annotations_inside_tokens(&converted).first() {
        return keep(&format!(
            "the annotation on line {line} would be read as part of the program"
        ));
    }
    if let Err(err) = fs::write(&file, &converted) {
        return keep(&err.to_string());
    }
    let mut rerun = check_one(&file, config).result;
    // A kept block still quotes the old line numbers. When it sits above the
    // code and its length changes, blessing it moves the code again, so it may
    // take a second pass to settle.
    if !drop_block && meta.expected_stderr.is_some() {
        for _ in 0..3 {
            if matches!(rerun.verdict, Verdict::Pass) {
                break;
            }
            bless(&mut rerun, &config.allow);
            rerun = check_one(&file, config).result;
        }
    }
    if !matches!(rerun.verdict, Verdict::Pass) {
        let _ = fs::write(&file, &source);
        return keep("the converted fixture does not pass; restored");
    }
    eprintln!("[annotate] converted {}", file.display());
    *result = rerun;
}

/// Whether an examples snapshot of this fixture (found by the `Fixture:` line
/// each one starts with) shows exactly the diagnostics in `block`. The CLI's
/// `• N errors • path` summary line is not part of a snapshot. A snapshot
/// harness can miss checks the CLI runs, so existing is not enough.
fn snapshot_pins(file: &Path, block: &str) -> bool {
    static SNAPSHOTS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let snapshots = SNAPSHOTS.get_or_init(|| {
        let mut found = Vec::new();
        let mut dirs = vec![PathBuf::from("tests/snapshots")];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for path in entries.flatten().map(|entry| entry.path()) {
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "snap")
                    && let Ok(text) = fs::read_to_string(&path)
                    && let Some(fixture) = text.lines().find_map(|l| l.strip_prefix("Fixture: "))
                {
                    let diagnostics = text
                        .split_once("== diagnostics ==\n")
                        .and_then(|(_, rest)| rest.split_once("\n== stdout =="))
                        .map(|(diagnostics, _)| diagnostics.to_string())
                        .unwrap_or_default();
                    found.push((fixture.to_string(), diagnostics));
                }
            }
        }
        found
    });
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| !line.starts_with("• "))
            .map(|line| line.trim_end().to_string())
            .collect::<Vec<_>>()
    };
    let block = lines(block.trim());
    snapshots.iter().any(|(fixture, diagnostics)| {
        same_file(fixture, file) && lines(diagnostics.trim()) == block
    })
}

/// `source` with `block` holding `body`. An existing block is replaced; with
/// `add`, a missing one goes after the header comments. `None` when there is
/// no block and `add` is false, or a block never ends.
fn with_expected_block(source: &str, block: Block, body: &str, add: bool) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    fn comment(line: &str) -> Option<&str> {
        line.trim().strip_prefix("//").map(str::trim)
    }
    // The parser strips `//` and one space, so a blank line is a bare `//`.
    let body = body
        .lines()
        .map(|line| if line.is_empty() { "//".to_string() } else { format!("// {line}") });
    let mut out: Vec<String> = Vec::new();
    if let Some(begin) =
        lines.iter().position(|line| comment(line).is_some_and(|c| block.begins_at(c)))
    {
        let end = begin
            + 1
            + lines[begin + 1..]
                .iter()
                .position(|line| comment(line).is_some_and(|c| block.ends_at(c)))?;
        out.extend(lines[..=begin].iter().map(|line| line.to_string()));
        out.extend(body);
        out.extend(lines[end..].iter().map(|line| line.to_string()));
    } else if add {
        let header = lines.iter().take_while(|line| line.trim_start().starts_with("//")).count();
        let (begin, end) = block.markers();
        out.extend(lines[..header].iter().map(|line| line.to_string()));
        out.push(format!("// {begin}"));
        out.extend(body);
        out.push(format!("// {end}"));
        out.extend(lines[header..].iter().map(|line| line.to_string()));
    } else {
        return None;
    }
    let mut text = out.join("\n");
    if source.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

/// One checked file: its parity result and, under `--census`, its rows.
struct CheckedFile {
    result: ParityResult,
    rows: Vec<census::Row>,
}

/// The ways a census runs when `--ways` is not given.
const CENSUS_WAYS: [Way; 4] = [Way::Vm, Way::Llvm, Way::VmOpt, Way::LlvmOpt];

/// Checks one file as `config` asks: the ways come from `--ways` or the
/// file's header, and a census limits them to those the header allows.
fn check_one(file: &Path, config: &Config) -> CheckedFile {
    let census_ways: &[Way] = config.ways.as_deref().unwrap_or(&CENSUS_WAYS);
    let mut fixture_meta = parse_fixture_meta_allowing(file, &config.allow);
    if let Some(open) = &config.open_kis {
        let stale = known_issues::stale_kis(&fixture_meta, open);
        fixture_meta.meta_errors.extend(stale);
    }
    let unrun_reason = if is_bench_or_profile(file) {
        Some("benchmark or profile: not run by the census".to_string())
    } else if is_prelude_module(file) {
        Some("standard library: compiled into every program, not alone".to_string())
    } else {
        fixture_meta.census_skip.as_ref().map(|reason| format!("census-skip: {reason}"))
    };
    if config.census.is_some()
        && let Some(reason) = unrun_reason
    {
        return CheckedFile {
            result: ParityResult {
                file: file.to_path_buf(),
                results: Vec::new(),
                artifacts: Vec::new(),
                verdict: Verdict::Skip { reason: reason.clone() },
                skipped_ways: Vec::new(),
                elapsed: Vec::new(),
                unexpected_passes: Vec::new(),
            },
            rows: census::unrun_rows(file, census_ways, &reason),
        };
    }
    let mut extra_args = merged_fixture_args(&config.extra_args, &fixture_meta.extra_args);
    let has_main = declares_entry_point(file);
    // A module without `main`, compiled on its own, needs the root its dotted
    // name is relative to, or it fails with a module path mismatch. One whose
    // header declares a failure (often that very mismatch, which its snapshot
    // pins) runs as declared.
    if config.census.is_some()
        && !has_main
        && matches!(fixture_meta.expect, Expect::Success | Expect::Any)
        && let Some(root) = module_root(file)
    {
        extra_args.push("--root".to_string());
        extra_args.push(root.display().to_string());
    }
    let effective_ways = ways_for(config, &fixture_meta, census_ways);
    let result = check_file(
        file,
        &CheckOpts {
            ways: &effective_ways,
            vm_binary: &config.vm_binary,
            llvm_binary: &config.llvm_binary,
            extra_args: &extra_args,
            timeout: config.timeout,
            capture_core: config.capture_core,
            capture_aether: config.capture_aether,
            capture_repr: config.capture_repr,
            capture_cfg: config.capture_cfg,
            capture_lir: config.capture_lir,
            compare_surfaces_only: config.compare_surfaces_only,
            explain: config.explain,
            expected_stdout: fixture_meta.expected_stdout.as_deref(),
            expected_stderr: fixture_meta.expected_stderr.as_deref(),
            expected_errors: &fixture_meta.expected_errors,
            expected_locations: &fixture_meta.expected_locations,
            fixture: file,
            expect: fixture_meta.expect,
            skip: fixture_meta.skip.as_deref(),
            skip_ways: &fixture_meta.skip_ways,
            meta_errors: &fixture_meta.meta_errors,
            census: config.census.is_some(),
            bless: config.bless,
            stdout_varies: fixture_meta.stdout_varies,
            check_skips: config.check_skips,
            expected_counts: &fixture_meta.expected_counts,
        },
    );
    let rows = if config.census.is_some() {
        census::rows(&census::Checked {
            result: &result,
            meta: &fixture_meta,
            ways: census_ways,
            has_main,
        })
    } else {
        Vec::new()
    };
    CheckedFile { result, rows }
}

/// The folder a file's `module A.B.C` name is relative to, when the name has
/// more than one part and the file sits at `<root>/A/B/C.flx`.
fn module_root(file: &Path) -> Option<PathBuf> {
    let source = fs::read_to_string(file).ok()?;
    let name = source.lines().find_map(|line| {
        let line = line.trim_start();
        let line = line.strip_prefix("public ").unwrap_or(line);
        line.strip_prefix("module ")
    })?;
    let name: String =
        name.chars().take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_').collect();
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() < 2 || file.file_stem()?.to_str()? != *parts.last()? {
        return None;
    }
    let mut root = file.parent()?;
    for part in parts[..parts.len() - 1].iter().rev() {
        if root.file_name()?.to_str()? != *part {
            return None;
        }
        root = root.parent()?;
    }
    Some(root.to_path_buf())
}

/// The ways to run on one file.
fn ways_for(config: &Config, meta: &FixtureMeta, census_ways: &[Way]) -> Vec<Way> {
    if config.census.is_some() {
        census_ways.iter().copied().filter(|&way| census::way_allowed(meta, way)).collect()
    } else if let Some(ways) = &config.ways {
        ways.clone()
    } else if config.compile_first && !matches!(meta.expect, Expect::CompileError) {
        // --compile: run against warmed caches instead of fresh ways.
        cached_equivalent_ways(&meta.ways)
    } else {
        meta.ways.clone()
    }
}

/// Runs `work` on every item, `jobs` at a time, and hands each output to
/// `done` in item order, as soon as it and every output before it are ready.
/// `done` returns `false` to stop: no new item starts after that.
fn run_in_order<T, R>(
    items: &[T],
    jobs: usize,
    work: impl Fn(&T) -> R + Sync,
    mut done: impl FnMut(R) -> bool,
) where
    T: Sync,
    R: Send,
{
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    if jobs <= 1 {
        for item in items {
            if !done(work(item)) {
                return;
            }
        }
        return;
    }
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(items.len()) {
            let sender = sender.clone();
            let (next, stop, work) = (&next, &stop, &work);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    if sender.send((index, work(item))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut ready = std::collections::BTreeMap::new();
        let mut wanted = 0;
        for (index, output) in receiver {
            ready.insert(index, output);
            while let Some(output) = ready.remove(&wanted) {
                wanted += 1;
                if !done(output) {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    });
}

/// The census file, written row by row in file order.
struct CensusWriter {
    out: std::io::BufWriter<fs::File>,
    rows: usize,
}

impl CensusWriter {
    fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut out = std::io::BufWriter::new(fs::File::create(path)?);
        std::io::Write::write_all(&mut out, format!("{}\n", census::HEADER).as_bytes())?;
        Ok(Self { out, rows: 0 })
    }

    fn write(&mut self, rows: &[census::Row]) {
        for row in rows {
            if let Err(err) =
                std::io::Write::write_all(&mut self.out, format!("{}\n", row.to_tsv()).as_bytes())
            {
                eprintln!("[parity] census write failed: {err}");
                return;
            }
            self.rows += 1;
        }
    }

    /// Flushes the file and returns how many rows it holds.
    fn finish(mut self) -> usize {
        if let Err(err) = std::io::Write::flush(&mut self.out) {
            eprintln!("[parity] census write failed: {err}");
        }
        self.rows
    }
}

struct CheckOpts<'a> {
    ways: &'a [Way],
    vm_binary: &'a Path,
    llvm_binary: &'a Path,
    extra_args: &'a [String],
    timeout: Duration,
    capture_core: bool,
    capture_aether: bool,
    capture_repr: bool,
    capture_cfg: bool,
    capture_lir: bool,
    compare_surfaces_only: bool,
    explain: bool,
    expected_stdout: Option<&'a str>,
    expected_stderr: Option<&'a str>,
    expected_errors: &'a [String],
    /// `//~` annotations as (line, code); when present they replace
    /// `expected_errors`.
    expected_locations: &'a [(usize, String)],
    /// The fixture file, to tell its own error locations from a module's.
    fixture: &'a Path,
    /// Declared fixture expectation: `success`, `compile_error`, or
    /// `runtime_error`. Controls stdout-comparison semantics.
    expect: Expect,
    /// Declared reason to skip this fixture while still reporting it.
    skip: Option<&'a str>,
    /// Ways the fixture is known to diverge on (`// skip-way:`).
    skip_ways: &'a [SkipWay],
    /// Header lines that could not be understood.
    meta_errors: &'a [String],
    /// A census run: classify every way, so a single remaining way is still
    /// run, and skip the dump captures a mismatch otherwise triggers.
    census: bool,
    /// `--bless`: run a fixture even when one way is left and it pins no
    /// output yet, so the run can write its first block.
    bless: bool,
    /// `// stdout: varies`: compare the ways' exits, not what they print.
    stdout_varies: bool,
    /// `--check-skips`: also run the skipped ways, outside the verdict.
    check_skips: bool,
    /// `// expect-reuse:` and `// expect-allocs:`, checked once the ways pass.
    expected_counts: &'a [ExpectedCount],
}

impl CheckOpts<'_> {
    /// Whether the fixture says what its run must produce, so that a single
    /// way still has something to be checked against.
    fn declares_expectation(&self) -> bool {
        self.expected_stdout.is_some()
            || self.expected_stderr.is_some()
            || !self.expected_errors.is_empty()
            || !self.expected_locations.is_empty()
            || !matches!(self.expect, Expect::Success)
    }

    /// Whether `ways` ways leave nothing to check: one way alone needs a
    /// declared expectation to be checked against, unless a census records
    /// it or `--bless` is about to write that expectation.
    fn too_few_to_compare(&self, ways: usize) -> bool {
        ways < 2 && !self.declares_expectation() && !self.census && !self.bless
    }
}

/// Splits `ways` into those to run and the `skip_ways` that remove one of
/// them. A skip naming a way that was not requested is not reported.
fn apply_skip_ways(ways: &[Way], skip_ways: &[SkipWay]) -> (Vec<Way>, Vec<SkipWay>) {
    // A heap way is the plain build on the debug heap: what makes the plain
    // way fail fails it too, so its skip carries over with the same KI.
    let carried = skip_ways.iter().flat_map(|skip| {
        ways.iter()
            .filter(move |way| way.is_heap() && way.non_heap() == skip.way)
            .map(move |&way| SkipWay { way, reason: skip.reason.clone() })
    });
    let skipped: Vec<SkipWay> =
        skip_ways.iter().filter(|skip| ways.contains(&skip.way)).cloned().chain(carried).collect();
    let run =
        ways.iter().copied().filter(|way| !skipped.iter().any(|skip| skip.way == *way)).collect();
    (run, skipped)
}

/// Run all requested ways on a single file and compare.
fn check_file(file: &Path, opts: &CheckOpts<'_>) -> ParityResult {
    // A verdict reached before any way runs.
    let unrun = |verdict, skipped_ways| ParityResult {
        file: file.to_path_buf(),
        results: vec![],
        artifacts: vec![],
        verdict,
        skipped_ways,
        elapsed: Vec::new(),
        unexpected_passes: Vec::new(),
    };
    // Before the skip: a skip whose reason cites a closed KI is itself an
    // error (`--known-issues`).
    if !opts.meta_errors.is_empty() {
        let verdict = Verdict::ExpectedOutputMismatch {
            expected: "valid fixture metadata".to_string(),
            actual: opts.meta_errors.join("\n"),
        };
        return unrun(verdict, vec![]);
    }
    if let Some(reason) = opts.skip {
        let reason = reason.to_string();
        return unrun(Verdict::Skip { reason }, vec![]);
    }

    let (ways, skipped_ways) = apply_skip_ways(opts.ways, opts.skip_ways);
    if !skipped_ways.is_empty() && (ways.is_empty() || opts.too_few_to_compare(ways.len())) {
        let reason = format!("{} way(s) left after skip-way", ways.len());
        let mut result = unrun(Verdict::Skip { reason }, Vec::new());
        // The fixture stays skipped, but a skipped way that would now pass
        // with the ways left is still worth reporting.
        if opts.check_skips {
            let mut run_results: Vec<_> = ways
                .iter()
                .map(|&way| {
                    run_way(
                        opts.vm_binary,
                        opts.llvm_binary,
                        file,
                        way,
                        opts.extra_args,
                        opts.timeout,
                    )
                })
                .collect();
            if opts.stdout_varies {
                for run in &mut run_results {
                    run.normalized_stdout.clear();
                }
            }
            result.unexpected_passes =
                skipped_ways_that_pass(file, opts, &mut run_results, &skipped_ways);
        }
        result.skipped_ways = skipped_ways;
        return result;
    }

    let mut run_results = Vec::new();
    let mut elapsed = Vec::new();

    for &way in &ways {
        let started = std::time::Instant::now();
        let result =
            run_way(opts.vm_binary, opts.llvm_binary, file, way, opts.extra_args, opts.timeout);
        elapsed.push((way, started.elapsed()));
        run_results.push(result);
    }
    if opts.stdout_varies {
        for result in &mut run_results {
            result.normalized_stdout.clear();
        }
    }

    // Check for native skip
    for result in &run_results {
        if let Some(reason) = is_native_skip(result) {
            return ParityResult {
                file: file.to_path_buf(),
                results: run_results,
                artifacts: vec![],
                verdict: Verdict::Skip { reason },
                skipped_ways,
                elapsed,
                unexpected_passes: Vec::new(),
            };
        }
    }

    let mut artifacts = capture_artifacts(file, opts, &ways, false);
    let mut details =
        collect_mismatch_details(&run_results, &artifacts, opts.compare_surfaces_only, opts.expect);

    if !opts.capture_core
        && !opts.census
        && !details.is_empty()
        && run_results.len() >= 2
        && artifacts.is_empty()
        && !opts.compare_surfaces_only
    {
        artifacts = capture_artifacts(file, opts, &ways, true);
        details = collect_mismatch_details(
            &run_results,
            &artifacts,
            opts.compare_surfaces_only,
            opts.expect,
        );
    }

    let mut verdict = judge(&run_results, details, opts);
    if matches!(verdict, Verdict::Pass) && !opts.expected_counts.is_empty() && !opts.census {
        verdict = judge_counts(
            opts.expected_counts,
            capture_metrics(opts.vm_binary, file, opts.extra_args, opts.timeout),
        );
    }
    let unexpected_passes = if opts.check_skips && matches!(verdict, Verdict::Pass) {
        skipped_ways_that_pass(file, opts, &mut run_results, &skipped_ways)
    } else {
        Vec::new()
    };

    ParityResult {
        file: file.to_path_buf(),
        results: run_results,
        artifacts,
        verdict,
        skipped_ways,
        elapsed,
        unexpected_passes,
    }
}

/// A fixture's stated counts against what `--metrics` reported: `Pass`, or
/// the first count that does not hold.
fn judge_counts(
    expected: &[ExpectedCount],
    metrics: Result<std::collections::BTreeMap<String, u64>, String>,
) -> Verdict {
    let metrics = match metrics {
        Ok(metrics) => metrics,
        Err(error) => {
            return Verdict::ExpectedOutputMismatch {
                expected: "flux --metrics to report counts".to_string(),
                actual: error,
            };
        }
    };
    for count in expected {
        let seen = metrics.get(count.key).copied();
        if !seen.is_some_and(|seen| count.holds(seen)) {
            return Verdict::ExpectedOutputMismatch {
                expected: count.to_string(),
                actual: match seen {
                    Some(seen) => format!("{} is {seen}", count.key),
                    None => format!("--metrics did not report {}", count.key),
                },
            };
        }
    }
    Verdict::Pass
}

/// `--check-skips`: runs each skipped way and judges it alongside the ways
/// that ran, as if its skip were gone. Those that would pass are returned.
/// `run_results` is left as it was.
fn skipped_ways_that_pass(
    file: &Path,
    opts: &CheckOpts<'_>,
    run_results: &mut Vec<super::RunResult>,
    skipped_ways: &[SkipWay],
) -> Vec<SkipWay> {
    let mut passing = Vec::new();
    for skip in skipped_ways {
        let mut result = run_way(
            opts.vm_binary,
            opts.llvm_binary,
            file,
            skip.way,
            opts.extra_args,
            opts.timeout,
        );
        if opts.stdout_varies {
            result.normalized_stdout.clear();
        }
        run_results.push(result);
        let details =
            collect_mismatch_details(run_results, &[], opts.compare_surfaces_only, opts.expect);
        if matches!(judge(run_results, details, opts), Verdict::Pass) {
            passing.push(skip.clone());
        }
        run_results.pop();
    }
    passing
}

/// The verdict over `run_results`, given the `details` their comparison
/// already found: the cache, strict and opt checks, then what the fixture
/// declares about its exit and output.
fn judge(
    run_results: &[super::RunResult],
    mut details: Vec<MismatchDetail>,
    opts: &CheckOpts<'_>,
) -> Verdict {
    // Cache parity: compare each cached way against its fresh counterpart
    for cached_result in run_results.iter().filter(|r| r.way.is_cached()) {
        let fresh_way = cached_result.way.base_way();
        if let Some(fresh_result) = run_results.iter().find(|r| r.way == fresh_way) {
            if fresh_result.exit_kind != cached_result.exit_kind {
                details.push(MismatchDetail::CacheMismatch {
                    fresh_way,
                    cached_way: cached_result.way,
                    field: "exit_kind".to_string(),
                    fresh: fresh_result.exit_kind.to_string(),
                    cached: cached_result.exit_kind.to_string(),
                });
            }
            if fresh_result.normalized_stdout != cached_result.normalized_stdout {
                details.push(MismatchDetail::CacheMismatch {
                    fresh_way,
                    cached_way: cached_result.way,
                    field: "stdout".to_string(),
                    fresh: fresh_result.normalized_stdout.clone(),
                    cached: cached_result.normalized_stdout.clone(),
                });
            }
            if fresh_result.normalized_stderr != cached_result.normalized_stderr
                && (fresh_result.exit_kind != ExitKind::Success
                    || cached_result.exit_kind != ExitKind::Success)
            {
                details.push(MismatchDetail::CacheMismatch {
                    fresh_way,
                    cached_way: cached_result.way,
                    field: "stderr".to_string(),
                    fresh: fresh_result.normalized_stderr.clone(),
                    cached: cached_result.normalized_stderr.clone(),
                });
            }
        }
    }

    // Strict-mode parity: compare each strict way against its normal counterpart.
    // Allowed difference: strict mode may reject programs that normal mode accepts
    // (CompileError in strict vs Success in normal). But when both succeed, output
    // must match. And if normal fails, strict must fail the same way.
    for strict_result in run_results.iter().filter(|r| r.way.is_strict()) {
        let normal_way = strict_result.way.non_strict();
        if let Some(normal_result) = run_results.iter().find(|r| r.way == normal_way) {
            // Allowed: normal=Success, strict=CompileError (strict caught more)
            let is_allowed = allowed_strict_compile_error(strict_result, run_results);

            if !is_allowed {
                if normal_result.exit_kind != strict_result.exit_kind {
                    details.push(MismatchDetail::StrictModeMismatch {
                        normal_way,
                        strict_way: strict_result.way,
                        field: "exit_kind".to_string(),
                        normal: normal_result.exit_kind.to_string(),
                        strict: strict_result.exit_kind.to_string(),
                    });
                }

                // Only compare stdout when both succeed
                if normal_result.exit_kind == ExitKind::Success
                    && strict_result.exit_kind == ExitKind::Success
                    && normal_result.normalized_stdout != strict_result.normalized_stdout
                {
                    details.push(MismatchDetail::StrictModeMismatch {
                        normal_way,
                        strict_way: strict_result.way,
                        field: "stdout".to_string(),
                        normal: normal_result.normalized_stdout.clone(),
                        strict: strict_result.normalized_stdout.clone(),
                    });
                }
            }
        }
    }

    details.extend(collect_opt_mode_details(run_results));
    details.extend(collect_heap_mode_details(run_results));

    if !details.is_empty() {
        Verdict::Mismatch { details }
    } else if matches!(opts.expect, Expect::CompileError | Expect::RuntimeError) {
        // Fixture declares it should fail. Require all runs to fail in the
        // expected phase and assert diagnostic code parity without pinning the
        // full rendered diagnostic text.
        let expected_kind = expected_exit_kind(opts.expect);
        let all_expected_kind =
            !run_results.is_empty() && run_results.iter().all(|r| r.exit_kind == expected_kind);
        if all_expected_kind {
            let mut expected_failure_details = if opts.expected_locations.is_empty() {
                collect_diagnostic_code_details(run_results, opts.expected_errors)
            } else {
                collect_diagnostic_location_details(
                    run_results,
                    opts.expected_locations,
                    opts.fixture,
                )
            };
            expected_failure_details
                .extend(collect_expected_stderr_details(run_results, opts.expected_stderr));
            if expected_failure_details.is_empty() {
                Verdict::Pass
            } else {
                Verdict::Mismatch { details: expected_failure_details }
            }
        } else {
            let actual = run_results
                .iter()
                .map(|r| format!("{}: {}", r.way, r.exit_kind))
                .collect::<Vec<_>>()
                .join(", ");
            Verdict::ExpectedOutputMismatch { expected: expected_kind.to_string(), actual }
        }
    } else if opts.expect != Expect::Any
        && let Some(failed) = first_unsuccessful_run(run_results)
    {
        // Fixture declares `expect: success`, so every way must actually have
        // run. Comparing the backends against each other is not enough: when
        // both fail the same way their outputs match and `details` is empty, so
        // the fixture would pass without the program ever running — the one
        // thing a parity fixture exists to prove. See
        // `docs/known_issues.md#ki-062`.
        Verdict::ExpectedOutputMismatch {
            expected: ExitKind::Success.to_string(),
            actual: format!("{}: {}", failed.way, failed.exit_kind),
        }
    } else if let Some(expected_stdout) = opts.expected_stdout.filter(|_| !opts.stdout_varies) {
        let expected = expected_stdout.trim();
        let actual = run_results
            .first()
            .map(|run| run.normalized_stdout.trim().to_string())
            .unwrap_or_default();
        if actual != expected {
            Verdict::ExpectedOutputMismatch { expected: expected.to_string(), actual }
        } else {
            Verdict::Pass
        }
    } else {
        Verdict::Pass
    }
}

/// Optimizer parity: each opt way against its unoptimized counterpart. The
/// optimizer must not change what a program does.
fn collect_opt_mode_details(run_results: &[super::RunResult]) -> Vec<MismatchDetail> {
    collect_paired_details(run_results, Way::is_opt, Way::non_opt, |field, unopt, opt| {
        MismatchDetail::OptModeMismatch {
            unopt_way: unopt.0,
            opt_way: opt.0,
            field: field.to_string(),
            unopt: unopt.1,
            opt: opt.1,
        }
    })
}

/// Debug-heap parity (0190 S15): `llvm_heap` against `llvm`. The debug heap
/// must not change what a program does, and a use after free it stops at is
/// a fault even when the plain build fails too. Leaks are counted apart.
fn collect_heap_mode_details(run_results: &[super::RunResult]) -> Vec<MismatchDetail> {
    let mut details =
        collect_paired_details(run_results, Way::is_heap, Way::non_heap, |field, plain, heap| {
            MismatchDetail::HeapModeMismatch {
                plain_way: plain.0,
                heap_way: heap.0,
                field: field.to_string(),
                plain: plain.1,
                heap: heap.1,
            }
        });
    for run in run_results.iter().filter(|r| r.way.is_heap()) {
        if let Some(line) = run.stderr.lines().find(|line| line.contains("use after free:")) {
            details
                .push(MismatchDetail::HeapFault { way: run.way, message: line.trim().to_string() });
        }
        // A freed cell that is only read never reaches a dup or a drop, so the
        // debug heap cannot stop it: the program prints the poison and may
        // still exit 0, as the Perceus audit's M2 did.
        if let Some(line) = run
            .stdout
            .lines()
            .find(|line| POISON_RENDERINGS.iter().any(|poison| line.contains(poison)))
        {
            details.push(MismatchDetail::HeapFault {
                way: run.way,
                message: format!("printed a freed value: {}", line.trim()),
            });
        }
    }
    details
}

/// How the debug heap's poison word, `0xDEADDEADDEADDEAD` (`FLUX_POISON` in
/// `runtime/c/rc.c`), prints. A freed block keeps its header and loses its
/// payload: a constructor's tag is the word's low 32 bits, a field read from
/// it is odd and prints as an Int, and a boxed Float prints the word as a
/// double.
const POISON_RENDERINGS: &[&str] =
    &["tag=-559030611", "-1200509093985980586", "-1.19355048209882e+148"];

/// Each run whose way `is_variant` against the run of `base_of(way)`, which
/// it must behave like: the exit kind always agrees, stdout when both
/// succeed, and stderr when either fails. A variant whose base did not run
/// is not compared. `mismatch` gets the field, then (way, text) of the base
/// and of the variant.
fn collect_paired_details(
    run_results: &[super::RunResult],
    is_variant: fn(Way) -> bool,
    base_of: fn(Way) -> Way,
    mismatch: impl Fn(&str, (Way, String), (Way, String)) -> MismatchDetail,
) -> Vec<MismatchDetail> {
    let mut details = Vec::new();
    for variant in run_results.iter().filter(|r| is_variant(r.way)) {
        let base_way = base_of(variant.way);
        let Some(base) = run_results.iter().find(|r| r.way == base_way) else {
            continue;
        };
        let mut differ = |field: &str, base_text: String, variant_text: String| {
            details.push(mismatch(field, (base_way, base_text), (variant.way, variant_text)));
        };
        let both_succeeded =
            base.exit_kind == ExitKind::Success && variant.exit_kind == ExitKind::Success;
        if base.exit_kind != variant.exit_kind {
            differ("exit_kind", base.exit_kind.to_string(), variant.exit_kind.to_string());
        }
        if both_succeeded && base.normalized_stdout != variant.normalized_stdout {
            differ("stdout", base.normalized_stdout.clone(), variant.normalized_stdout.clone());
        }
        if !both_succeeded && base.normalized_stderr != variant.normalized_stderr {
            differ("stderr", base.normalized_stderr.clone(), variant.normalized_stderr.clone());
        }
    }
    details
}

fn capture_artifacts(
    file: &Path,
    opts: &CheckOpts<'_>,
    ways: &[Way],
    force_core_on_mismatch: bool,
) -> Vec<(Way, DebugArtifacts)> {
    let capture_core = opts.capture_core || force_core_on_mismatch;
    let capture_aether = opts.capture_aether;
    let capture_repr = opts.capture_repr;
    let capture_cfg = opts.capture_cfg;
    let capture_lir = opts.capture_lir;
    if !capture_core && !capture_aether && !capture_repr && !capture_cfg && !capture_lir {
        return vec![];
    }

    ways.iter()
        .map(|&way| {
            let mut arts = DebugArtifacts::default();
            if capture_core {
                if opts.explain {
                    eprintln!("[parity] capture {}: core", way);
                }
                let core = capture_dump_core(
                    opts.vm_binary,
                    opts.llvm_binary,
                    file,
                    way,
                    opts.extra_args,
                    opts.timeout,
                );
                arts.core = core.core;
            }
            if capture_aether {
                if opts.explain {
                    eprintln!("[parity] capture {}: aether", way);
                }
                let aether = capture_dump_aether(
                    opts.vm_binary,
                    opts.llvm_binary,
                    file,
                    way,
                    opts.extra_args,
                    opts.timeout,
                );
                arts.aether = aether.aether;
            }
            if capture_repr {
                if opts.explain {
                    eprintln!("[parity] capture {}: repr", way);
                }
                let repr = capture_dump_repr(
                    opts.vm_binary,
                    opts.llvm_binary,
                    file,
                    way,
                    opts.extra_args,
                    opts.timeout,
                );
                arts.repr = repr.repr;
            }
            if capture_cfg {
                if opts.explain && way.backend_id() == BackendId::Vm {
                    eprintln!("[parity] capture {}: vm:cfg", way);
                }
                let cfg = capture_dump_cfg(
                    opts.vm_binary,
                    opts.llvm_binary,
                    file,
                    way,
                    opts.extra_args,
                    opts.timeout,
                );
                arts.backend_ir.extend(cfg.backend_ir);
            }
            if capture_lir {
                if opts.explain && way.backend_id() == BackendId::Llvm {
                    eprintln!("[parity] capture {}: llvm:lir", way);
                }
                let lir = capture_dump_lir(
                    opts.vm_binary,
                    opts.llvm_binary,
                    file,
                    way,
                    opts.extra_args,
                    opts.timeout,
                );
                arts.backend_ir.extend(lir.backend_ir);
            }
            (way, arts)
        })
        .collect()
}

/// The first way whose run did not succeed, if any.
///
/// Only meaningful for a fixture declaring `expect: success` — a fixture that
/// declares a failure is judged against its own expected phase instead.
///
/// A strict way that rejects a program its non-strict counterpart accepted does
/// not count: strict mode exists to reject more, and the same allowance is made
/// when comparing the ways against each other. `expect: success` is a claim
/// about the program running, not about it surviving strict mode.
fn first_unsuccessful_run(run_results: &[super::RunResult]) -> Option<&super::RunResult> {
    run_results.iter().find(|run| {
        run.exit_kind != ExitKind::Success && !allowed_strict_compile_error(run, run_results)
    })
}

fn expected_exit_kind(expect: Expect) -> ExitKind {
    match expect {
        // `any` accepts every exit kind; its ways are held to agree instead.
        Expect::Success | Expect::Any => ExitKind::Success,
        Expect::CompileError => ExitKind::CompileError,
        Expect::RuntimeError => ExitKind::RuntimeError,
    }
}

fn collect_diagnostic_code_details(
    run_results: &[super::RunResult],
    expected_errors: &[String],
) -> Vec<MismatchDetail> {
    let has_expected = !expected_errors.is_empty();
    let baseline_expected = if has_expected {
        Vec::new()
    } else {
        run_results.first().map(|run| diagnostic_codes(&run.normalized_stderr)).unwrap_or_default()
    };

    run_results
        .iter()
        .filter_map(|run| {
            let expected = if has_expected {
                sorted_codes(expected_errors.iter().cloned())
            } else {
                baseline_expected.clone()
            };
            let actual = diagnostic_codes(&run.normalized_stderr);
            // Strict mode exists to reject more, so a strict way may add
            // codes (such as E425) as long as it reports every expected one.
            let agrees = if run.way.is_strict() {
                includes_codes(&actual, &expected)
            } else {
                actual == expected
            };
            if agrees {
                None
            } else {
                Some(MismatchDetail::DiagnosticCodes {
                    way: run.way,
                    expected: expected.clone(),
                    actual,
                })
            }
        })
        .collect()
}

/// For an annotated fixture: each way must report exactly the annotated
/// (line, code) pairs, or, on a strict way, at least them.
fn collect_diagnostic_location_details(
    run_results: &[super::RunResult],
    expected: &[(usize, String)],
    fixture: &Path,
) -> Vec<MismatchDetail> {
    run_results
        .iter()
        .filter_map(|run| {
            let mut missing: Vec<(usize, String)> = expected.to_vec();
            let mut unexpected = Vec::new();
            for error in diagnostic_locations(&run.normalized_stderr) {
                let in_fixture = error.path.as_deref().is_some_and(|path| same_file(path, fixture));
                let found = in_fixture
                    .then(|| {
                        missing.iter().position(|(l, c)| Some(*l) == error.line && *c == error.code)
                    })
                    .flatten();
                match found {
                    Some(index) => {
                        missing.remove(index);
                    }
                    None => unexpected.push(error.describe(in_fixture)),
                }
            }
            if run.way.is_strict() {
                unexpected.clear();
            }
            if missing.is_empty() && unexpected.is_empty() {
                return None;
            }
            missing.sort();
            Some(MismatchDetail::DiagnosticLocations {
                way: run.way,
                missing: missing
                    .iter()
                    .map(|(line, code)| format!("line {line}: {code}"))
                    .collect(),
                unexpected,
            })
        })
        .collect()
}

/// An error as rendered: its code and, when it has one, where it points.
#[derive(Debug, PartialEq, Eq)]
struct RenderedError {
    code: String,
    path: Option<String>,
    line: Option<usize>,
}

impl RenderedError {
    fn describe(&self, in_fixture: bool) -> String {
        match (&self.path, self.line) {
            (Some(_), Some(line)) if in_fixture => format!("line {line}: {}", self.code),
            (Some(path), Some(line)) => format!("{path}:{line}: {}", self.code),
            _ => format!("no location: {}", self.code),
        }
    }
}

/// Each `error[E…]` in rendered stderr with the location under it. Only the
/// first `  path:line:col` after a header is the error's own: hints and
/// related notes print further locations in the same shape.
fn diagnostic_locations(stderr: &str) -> Vec<RenderedError> {
    let mut errors: Vec<RenderedError> = Vec::new();
    let mut awaiting_location = false;
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("error[") {
            if let Some(code) = rest.split(']').next().filter(|c| c.starts_with('E')) {
                errors.push(RenderedError { code: code.to_string(), path: None, line: None });
                awaiting_location = true;
            }
            continue;
        }
        if !awaiting_location {
            continue;
        }
        if let Some((path, line)) = rendered_location(line) {
            let error = errors.last_mut().expect("a header came first");
            error.path = Some(path.to_string());
            error.line = Some(line);
            awaiting_location = false;
        }
    }
    errors
}

/// `  path:line:col`, as the renderer prints an error's location.
fn rendered_location(line: &str) -> Option<(&str, usize)> {
    let rest = line.strip_prefix("  ")?;
    if rest.starts_with(' ') {
        return None;
    }
    let (rest, col) = rest.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    col.parse::<usize>().ok()?;
    Some((path, line.parse().ok()?)).filter(|(path, _)| !path.is_empty())
}

/// Whether a rendered path names `fixture`. Rendering makes paths relative to
/// the working directory, so one need only be a suffix of the other.
fn same_file(rendered: &str, fixture: &Path) -> bool {
    let rendered = Path::new(rendered);
    fixture.ends_with(rendered) || rendered.ends_with(fixture)
}

/// Whether `actual` holds every code in `expected`, each as many times as
/// `expected` lists it. Both are sorted.
fn includes_codes(actual: &[String], expected: &[String]) -> bool {
    let mut actual = actual.iter().peekable();
    expected.iter().all(|code| {
        while actual.next_if(|candidate| *candidate < code).is_some() {}
        actual.next_if(|candidate| *candidate == code).is_some()
    })
}

fn diagnostic_codes(stderr: &str) -> Vec<String> {
    let mut codes = Vec::new();
    let mut rest = stderr;
    while let Some(start) = rest.find("error[E") {
        let after_prefix = &rest[start + "error[".len()..];
        let code: String = after_prefix.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let code_len = code.len();
        if !code.is_empty() {
            codes.push(code);
        }
        rest = &after_prefix[code_len..];
    }
    sorted_codes(codes)
}

fn sorted_codes<I>(codes: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut codes = codes.into_iter().collect::<Vec<_>>();
    codes.sort();
    codes
}

fn collect_expected_stderr_details(
    run_results: &[super::RunResult],
    expected_stderr: Option<&str>,
) -> Vec<MismatchDetail> {
    let Some(expected_stderr) = expected_stderr else {
        return Vec::new();
    };
    let expected = normalize_stderr(expected_stderr);
    let expected = expected.trim();
    run_results
        .iter()
        .filter_map(|run| {
            let actual = run.normalized_stderr.trim();
            if actual == expected {
                None
            } else {
                Some(MismatchDetail::ExpectedStderr {
                    way: run.way,
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                })
            }
        })
        .collect()
}

fn collect_mismatch_details(
    run_results: &[super::RunResult],
    artifacts: &[(Way, DebugArtifacts)],
    compare_surfaces_only: bool,
    expect: Expect,
) -> Vec<MismatchDetail> {
    // For error fixtures, stdout/stderr diverge by construction — each backend
    // formats its own diagnostic. The parity gate is "both backends failed,"
    // which is verified later in the Verdict assembly.  Suppress shared/backend
    // runtime divergence details here so the fixture doesn't register as a
    // MISMATCH just because diagnostics read differently.
    let suppress_runtime_comparison = matches!(expect, Expect::CompileError | Expect::RuntimeError);
    let mut details = Vec::new();

    if artifacts.len() >= 2 {
        let (base_way, ref base_arts) = artifacts[0];
        for &(other_way, ref other_arts) in &artifacts[1..] {
            let pair = (&base_arts.core.normalized, &other_arts.core.normalized);
            if let (Some(base_core), Some(other_core)) = pair
                && base_core != other_core
            {
                details.push(MismatchDetail::CoreMismatch {
                    left_way: base_way,
                    left: base_core.clone(),
                    right_way: other_way,
                    right: other_core.clone(),
                });
            }

            let aether_pair = (&base_arts.aether.normalized, &other_arts.aether.normalized);
            if let (Some(base_aether), Some(other_aether)) = aether_pair
                && base_aether != other_aether
            {
                details.push(MismatchDetail::AetherMismatch {
                    left_way: base_way,
                    left: base_aether.clone(),
                    right_way: other_way,
                    right: other_aether.clone(),
                });
            }

            let repr_pair = (&base_arts.repr.normalized, &other_arts.repr.normalized);
            if let (Some(base_repr), Some(other_repr)) = repr_pair
                && base_repr != other_repr
            {
                details.push(MismatchDetail::RepresentationMismatch {
                    left_way: base_way,
                    left: base_repr.clone(),
                    right_way: other_way,
                    right: other_repr.clone(),
                });
            }
        }
    }

    if !compare_surfaces_only && !suppress_runtime_comparison && run_results.len() >= 2 {
        // Strict mode is allowed to reject a program that normal mode
        // accepts. `check_file`'s strict-mode parity pass checks that rule
        // against each way's own non-strict counterpart, and
        // `collect_strict_cross_backend_details` keeps the strict ways in
        // agreement with each other; do not feed an allowed strict rejection
        // into the backend comparison here, where it would look like backend
        // IR/runtime divergence — and never let one serve as the baseline,
        // or every other way would go uncompared.
        let base = run_results
            .iter()
            .find(|r| !allowed_strict_compile_error(r, run_results))
            .unwrap_or(&run_results[0]);
        let has_shared_mismatch = details.iter().any(|d| {
            matches!(
                d,
                MismatchDetail::CoreMismatch { .. }
                    | MismatchDetail::AetherMismatch { .. }
                    | MismatchDetail::RepresentationMismatch { .. }
            )
        });
        for other in run_results.iter().filter(|r| !std::ptr::eq(*r, base)) {
            if allowed_strict_compile_error(other, run_results) {
                continue;
            }

            let stdout_differs = !super::debug::outputs_match(base, other);
            let runtime_diverged = base.exit_kind != other.exit_kind
                || stdout_differs
                || ((base.exit_kind != ExitKind::Success || other.exit_kind != ExitKind::Success)
                    && base.normalized_stderr != other.normalized_stderr);
            if runtime_diverged && !has_shared_mismatch {
                let backend = other.way.backend_id();
                details.push(MismatchDetail::BackendIrMismatch {
                    baseline_way: base.way,
                    backend,
                    surface: backend_spec(backend).ir_surface.to_string(),
                    summary: format!(
                        "shared layers match; inspect {}.ir({}) next",
                        backend,
                        backend_spec(backend).ir_surface
                    ),
                });
                details.push(MismatchDetail::BackendRuntimeMismatch {
                    baseline_way: base.way,
                    backend,
                    summary: format!("{backend} diverged from baseline {}", base.way),
                });
            }
            if base.exit_kind != other.exit_kind {
                details.push(MismatchDetail::ExitKind {
                    left_way: base.way,
                    left: base.exit_kind,
                    right_way: other.way,
                    right: other.exit_kind,
                });
            }

            if stdout_differs {
                details.push(MismatchDetail::Stdout {
                    left_way: base.way,
                    left: base.normalized_stdout.clone(),
                    right_way: other.way,
                    right: other.normalized_stdout.clone(),
                });
            }

            // A diagnostic demo's messages are pinned by snapshots; each
            // backend words its own, so only its exit and stdout are compared.
            if expect != Expect::Any
                && (base.exit_kind != ExitKind::Success || other.exit_kind != ExitKind::Success)
                && base.normalized_stderr != other.normalized_stderr
            {
                details.push(MismatchDetail::Stderr {
                    left_way: base.way,
                    left: base.normalized_stderr.clone(),
                    right_way: other.way,
                    right: other.normalized_stderr.clone(),
                });
            }
        }

        details.extend(collect_strict_cross_backend_details(run_results));
    }

    details
}

/// Strict-vs-strict cross-backend parity. The generic comparison loop skips
/// allowed strict rejections and the strict-mode pass in `check_file` only
/// pairs each strict way with its own non-strict counterpart, so this is the
/// one place the two backends' strict analyses are held to agree with each
/// other: both must reject (or not) the same programs, with the same
/// diagnostic codes.
fn collect_strict_cross_backend_details(run_results: &[super::RunResult]) -> Vec<MismatchDetail> {
    let vm_strict = run_results.iter().find(|r| r.way == Way::VmStrict);
    let llvm_strict = run_results.iter().find(|r| r.way == Way::LlvmStrict);
    let (Some(vm_strict), Some(llvm_strict)) = (vm_strict, llvm_strict) else {
        return Vec::new();
    };

    // When neither side is an allowed rejection, both took part in the
    // generic comparison loop and any divergence is already reported there;
    // re-checking here would only duplicate details.
    if !allowed_strict_compile_error(vm_strict, run_results)
        && !allowed_strict_compile_error(llvm_strict, run_results)
    {
        return Vec::new();
    }

    let mut details = Vec::new();
    if vm_strict.exit_kind != llvm_strict.exit_kind {
        details.push(MismatchDetail::ExitKind {
            left_way: vm_strict.way,
            left: vm_strict.exit_kind,
            right_way: llvm_strict.way,
            right: llvm_strict.exit_kind,
        });
    } else if vm_strict.exit_kind == ExitKind::CompileError {
        let expected = diagnostic_codes(&vm_strict.normalized_stderr);
        let actual = diagnostic_codes(&llvm_strict.normalized_stderr);
        if expected != actual {
            details.push(MismatchDetail::DiagnosticCodes {
                way: llvm_strict.way,
                expected,
                actual,
            });
        }
    }
    details
}

fn allowed_strict_compile_error(
    result: &super::RunResult,
    run_results: &[super::RunResult],
) -> bool {
    result.way.is_strict()
        && result.exit_kind == ExitKind::CompileError
        && run_results.iter().any(|candidate| {
            candidate.way == result.way.non_strict() && candidate.exit_kind == ExitKind::Success
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bless_replaces_an_existing_block() {
        let source = "// parity: vm, llvm\n// parity-expected-stdout-begin\n// old\n// parity-expected-stdout-end\n\nfn main() {}\n";
        let blessed = with_expected_block(source, Block::Stdout, "1\n\n  2", true).unwrap();
        assert_eq!(
            blessed,
            "// parity: vm, llvm\n// parity-expected-stdout-begin\n// 1\n//\n//   2\n// parity-expected-stdout-end\n\nfn main() {}\n"
        );
    }

    #[test]
    fn bless_adds_a_missing_stdout_block_after_the_header() {
        let source = "// parity: vm, llvm\n// expect: success\n\nfn main() {}\n";
        let blessed = with_expected_block(source, Block::Stdout, "42", true).unwrap();
        assert_eq!(
            blessed,
            "// parity: vm, llvm\n// expect: success\n// parity-expected-stdout-begin\n// 42\n// parity-expected-stdout-end\n\nfn main() {}\n"
        );
    }

    #[test]
    fn bless_runs_a_fixture_skip_way_left_with_one_way() {
        let opts = plain_opts(&[Way::Vm]);
        assert!(opts.too_few_to_compare(1), "nothing to check one way against");
        let blessing = CheckOpts { bless: true, ..plain_opts(&[Way::Vm]) };
        assert!(!blessing.too_few_to_compare(1), "bless writes the expectation");
    }

    #[test]
    fn bless_adds_no_stderr_block() {
        let source = "// expect: compile_error\nfn main() {}\n";
        assert_eq!(with_expected_block(source, Block::Stderr, "error", false), None);
    }

    #[test]
    fn a_blessed_block_reads_back_as_written() {
        let dir = std::env::temp_dir().join(format!("flux-bless-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("roundtrip.flx");
        let body = "first\n\n  indented\nlast";
        let source =
            with_expected_block("// expect: success\nfn main() {}\n", Block::Stdout, body, true);
        fs::write(&file, source.unwrap()).unwrap();
        let meta = parse_fixture_meta_allowing(&file, &[]);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(meta.expected_stdout.as_deref(), Some(body));
    }

    #[test]
    fn parallel_checks_report_in_file_order() {
        let items: Vec<u64> = (0..40).collect();
        let mut seen = Vec::new();
        // Later items finish first, so arrival order is not item order.
        run_in_order(
            &items,
            4,
            |&n| {
                std::thread::sleep(Duration::from_millis(40 - n));
                n
            },
            |n| {
                seen.push(n);
                true
            },
        );
        assert_eq!(seen, items);
    }

    #[test]
    fn stopping_early_reports_nothing_after_the_stop() {
        let items: Vec<u32> = (0..20).collect();
        let mut seen = Vec::new();
        run_in_order(
            &items,
            3,
            |&n| n,
            |n| {
                seen.push(n);
                n < 5
            },
        );
        assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_standard_library_is_not_compiled_alone() {
        assert!(is_prelude_module(Path::new("lib/Flow/List.flx")));
        assert!(is_prelude_module(Path::new("./lib/Flow/Http/Server.flx")));
        assert!(!is_prelude_module(Path::new("lib/Flume/Cli.flx")));
        assert!(!is_prelude_module(Path::new("examples/lib/Flow/x.flx")));
    }

    #[test]
    fn a_dotted_module_is_rooted_above_its_folders() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let file = root.join("examples/runtime_errors/RuntimeErrors/BoundaryUnknown.flx");
        assert_eq!(module_root(&file), Some(root.join("examples/runtime_errors")));
        // A one-part name is already relative to its own folder.
        let file = root.join("examples/runtime_errors/RuntimeErrors/ExposingRuntimeTarget.flx");
        assert_eq!(module_root(&file), None);
    }

    #[test]
    fn a_census_keeps_the_ways_a_parity_line_allows() {
        let config =
            parse_args(&["x.flx".to_string(), "--census".to_string(), "c.tsv".to_string()])
                .expect("valid arguments");
        let vm_only =
            FixtureMeta { ways: vec![Way::Vm], declared_ways: true, ..Default::default() };
        assert_eq!(ways_for(&config, &vm_only, &CENSUS_WAYS), [Way::Vm, Way::VmOpt]);
        assert_eq!(ways_for(&config, &FixtureMeta::default(), &CENSUS_WAYS), CENSUS_WAYS);
    }

    /// A support module — no `main` — is not a fixture, and a real fixture is.
    /// Both are checked against the tree so the rule cannot drift from it.
    #[test]
    fn a_support_module_is_not_swept_as_a_fixture() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
        assert!(
            should_skip_fixture(&root.join("ImportedCtor/Shapes.flx")),
            "a module with no entry point must not be swept as a fixture"
        );
        assert!(
            !should_skip_fixture(&root.join("imported_constructor_types.flx")),
            "the fixture that imports it must still be swept"
        );
    }

    #[test]
    fn every_swept_fixture_declares_an_entry_point() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
        let orphans: Vec<_> = collect_fixtures(&root, false)
            .into_iter()
            .filter(|path| !declares_entry_point(path))
            .collect();
        assert!(orphans.is_empty(), "swept fixtures without a `main`: {orphans:?}");
    }

    #[test]
    fn an_unreadable_path_is_reported_rather_than_dropped() {
        assert!(declares_entry_point(Path::new("does/not/exist.flx")));
    }

    fn result(
        way: Way,
        exit_kind: ExitKind,
        stdout: &str,
        stderr: &str,
    ) -> super::super::RunResult {
        super::super::RunResult {
            way,
            exit_kind,
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            normalized_stdout: stdout.to_string(),
            normalized_stderr: stderr.to_string(),
            cache_observations: vec![],
            leak: None,
        }
    }

    fn plain_opts(ways: &[Way]) -> CheckOpts<'_> {
        CheckOpts {
            ways,
            vm_binary: Path::new("flux"),
            llvm_binary: Path::new("flux"),
            extra_args: &[],
            timeout: Duration::from_secs(1),
            capture_core: false,
            capture_aether: false,
            capture_repr: false,
            capture_cfg: false,
            capture_lir: false,
            compare_surfaces_only: false,
            explain: false,
            expected_stdout: None,
            expected_stderr: None,
            expected_errors: &[],
            expected_locations: &[],
            fixture: Path::new("fixture.flx"),
            expect: Expect::Success,
            skip: None,
            skip_ways: &[],
            meta_errors: &[],
            census: false,
            bless: false,
            stdout_varies: false,
            check_skips: true,
            expected_counts: &[],
        }
    }

    /// `--check-skips` judges a skipped way's run with the others: one that
    /// agrees passes, one that still diverges does not.
    #[test]
    fn a_skipped_way_passes_only_when_it_agrees() {
        let ways = [Way::Vm, Way::Llvm, Way::LlvmOpt];
        let opts = plain_opts(&ways);
        let judged = |skipped: super::super::RunResult| {
            let runs = vec![
                result(Way::Vm, ExitKind::Success, "42", ""),
                result(Way::Llvm, ExitKind::Success, "42", ""),
                skipped,
            ];
            let details = collect_mismatch_details(&runs, &[], false, Expect::Success);
            judge(&runs, details, &opts)
        };
        assert!(matches!(judged(result(Way::LlvmOpt, ExitKind::Success, "42", "")), Verdict::Pass));
        assert!(!matches!(judged(result(Way::LlvmOpt, ExitKind::Success, "0", "")), Verdict::Pass));
        assert!(!matches!(
            judged(result(Way::LlvmOpt, ExitKind::Crash, "", "signal")),
            Verdict::Pass
        ));
    }

    #[test]
    fn check_skips_and_known_issues_flags_parse() {
        let known = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/known_issues.md");
        let config = parse_args(&[
            "x.flx".to_string(),
            "--check-skips".to_string(),
            "--known-issues".to_string(),
            known.display().to_string(),
        ])
        .expect("valid arguments");
        assert!(config.check_skips);
        let open = config.open_kis.expect("read the file");
        assert!(open.contains("KI-103"), "KI-103 is open today");
        assert!(parse_args(&["x.flx".to_string(), "--known-issues".to_string()]).is_err());
    }

    #[test]
    fn allowed_strict_compile_errors_do_not_look_like_backend_divergence() {
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "output", ""),
            result(Way::Llvm, ExitKind::Success, "output", ""),
            result(Way::VmStrict, ExitKind::CompileError, "", "E425"),
            result(Way::LlvmStrict, ExitKind::CompileError, "", "E425"),
        ];

        let details = collect_mismatch_details(&run_results, &[], false, Expect::Success);

        assert!(details.is_empty(), "unexpected parity details: {details:?}");
    }

    /// The 5-way guide sweep caught this: `expect: success` must not fail a
    /// fixture because strict mode rejected what normal mode ran, since the
    /// harness already treats that difference as allowed.
    #[test]
    fn a_strict_only_compile_error_does_not_fail_an_expect_success_fixture() {
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "output", ""),
            result(Way::VmStrict, ExitKind::CompileError, "", "E425"),
        ];

        assert!(first_unsuccessful_run(&run_results).is_none());
    }

    /// But a strict failure with no successful counterpart is still a failure,
    /// and so is any non-strict one.
    #[test]
    fn a_run_that_simply_failed_still_fails_an_expect_success_fixture() {
        let normal_failed = vec![result(Way::Vm, ExitKind::CompileError, "", "E004")];
        assert!(first_unsuccessful_run(&normal_failed).is_some());

        let strict_alone = vec![result(Way::VmStrict, ExitKind::CompileError, "", "E425")];
        assert!(first_unsuccessful_run(&strict_alone).is_some());
    }

    #[test]
    fn strict_successes_are_still_compared_with_normal_output() {
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "normal", ""),
            result(Way::VmStrict, ExitKind::Success, "strict", ""),
        ];

        let details = collect_mismatch_details(&run_results, &[], false, Expect::Success);

        assert!(details.iter().any(|detail| matches!(
            detail,
            MismatchDetail::Stdout { left_way: Way::Vm, right_way: Way::VmStrict, .. }
        )));
    }

    #[test]
    fn strict_way_first_does_not_disable_backend_comparison() {
        // An allowed strict rejection at index 0 must not become the baseline
        // and silence the vm/llvm comparison.
        let run_results = vec![
            result(Way::VmStrict, ExitKind::CompileError, "", "E425"),
            result(Way::Vm, ExitKind::Success, "8", ""),
            result(Way::Llvm, ExitKind::Success, "7", ""),
        ];

        let details = collect_mismatch_details(&run_results, &[], false, Expect::Success);

        assert!(
            details.iter().any(|detail| matches!(
                detail,
                MismatchDetail::Stdout { left_way: Way::Vm, right_way: Way::Llvm, .. }
            )),
            "vm/llvm stdout divergence went unreported: {details:?}"
        );
    }

    #[test]
    fn strict_rejection_on_one_backend_only_is_a_mismatch() {
        // One backend's strict analysis catching a violation the other's
        // misses is exactly what strict parity exists to detect.
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "output", ""),
            result(Way::Llvm, ExitKind::Success, "output", ""),
            result(Way::VmStrict, ExitKind::CompileError, "", "error[E425]: strict"),
            result(Way::LlvmStrict, ExitKind::Success, "output", ""),
        ];

        let details = collect_mismatch_details(&run_results, &[], false, Expect::Success);

        assert!(
            details.iter().any(|detail| matches!(
                detail,
                MismatchDetail::ExitKind {
                    left_way: Way::VmStrict,
                    right_way: Way::LlvmStrict,
                    ..
                }
            )),
            "strict exit-kind asymmetry went unreported: {details:?}"
        );
    }

    #[test]
    fn a_strict_way_may_add_codes_but_not_drop_one() {
        let expected = vec!["E300".to_string()];
        let run_results = vec![
            result(Way::Vm, ExitKind::CompileError, "", "error[E300]: a"),
            result(
                Way::VmStrict,
                ExitKind::CompileError,
                "",
                "error[E300]: a\nerror[E425]: strict",
            ),
        ];
        assert!(collect_diagnostic_code_details(&run_results, &expected).is_empty());

        // An extra code is still a mismatch on a non-strict way, and a strict
        // way that lost the expected code is one too.
        let run_results = vec![
            result(Way::Vm, ExitKind::CompileError, "", "error[E300]: a\nerror[E425]: b"),
            result(Way::VmStrict, ExitKind::CompileError, "", "error[E425]: strict"),
        ];
        let ways: Vec<Way> = collect_diagnostic_code_details(&run_results, &expected)
            .iter()
            .map(|detail| match detail {
                MismatchDetail::DiagnosticCodes { way, .. } => *way,
                other => panic!("unexpected detail {other:?}"),
            })
            .collect();
        assert_eq!(ways, [Way::Vm, Way::VmStrict]);
    }

    /// Rendered stderr for an error at `line` of `path`, with a hint that
    /// points elsewhere, as the renderer prints one.
    fn rendered(code: &str, path: &str, line: usize) -> String {
        format!(
            "error[{code}]: Title\n\nMessage.\n\n  {path}:{line}:5\n   |\n{line} | x\n\nHint:\n  see here\n  lib/Other.flx:3:1\n"
        )
    }

    #[test]
    fn an_errors_location_is_the_first_one_under_its_header() {
        let stderr = format!(
            "• 2 errors • examples/a.flx\n{}\n{}error[E999]: no location\n\nMessage.\n",
            rendered("E300", "examples/a.flx", 38),
            rendered("E1000", "examples/Mod.flx", 32),
        );
        let found = diagnostic_locations(&stderr);
        let summary: Vec<(String, Option<String>, Option<usize>)> =
            found.into_iter().map(|e| (e.code, e.path, e.line)).collect();
        assert_eq!(
            summary,
            [
                ("E300".to_string(), Some("examples/a.flx".to_string()), Some(38)),
                ("E1000".to_string(), Some("examples/Mod.flx".to_string()), Some(32)),
                ("E999".to_string(), None, None),
            ]
        );
    }

    fn location_details(
        runs: &[super::super::RunResult],
        expected: &[(usize, &str)],
    ) -> Vec<(Way, Vec<String>, Vec<String>)> {
        let expected: Vec<(usize, String)> =
            expected.iter().map(|(line, code)| (*line, code.to_string())).collect();
        collect_diagnostic_location_details(runs, &expected, Path::new("/repo/examples/a.flx"))
            .into_iter()
            .map(|detail| match detail {
                MismatchDetail::DiagnosticLocations { way, missing, unexpected } => {
                    (way, missing, unexpected)
                }
                other => panic!("unexpected detail {other:?}"),
            })
            .collect()
    }

    #[test]
    fn annotated_errors_match_by_line_and_code() {
        let both = format!(
            "{}{}",
            rendered("E300", "examples/a.flx", 38),
            rendered("E300", "examples/a.flx", 40)
        );
        let runs = vec![result(Way::Vm, ExitKind::CompileError, "", &both)];
        assert!(location_details(&runs, &[(38, "E300"), (40, "E300")]).is_empty());

        // The right code on the wrong line is one missing and one unexpected.
        let details = location_details(&runs, &[(38, "E300"), (41, "E300")]);
        assert_eq!(
            details,
            [(Way::Vm, vec!["line 41: E300".to_string()], vec!["line 40: E300".to_string()])]
        );
    }

    #[test]
    fn an_error_in_another_file_is_unexpected_and_named() {
        let stderr = rendered("E1000", "examples/Mod.flx", 32);
        let runs = vec![result(Way::Vm, ExitKind::RuntimeError, "", &stderr)];
        let details = location_details(&runs, &[(5, "E1000")]);
        assert_eq!(
            details,
            [(
                Way::Vm,
                vec!["line 5: E1000".to_string()],
                vec!["examples/Mod.flx:32: E1000".to_string()]
            )]
        );
    }

    #[test]
    fn a_strict_way_may_add_located_errors_but_not_drop_one() {
        let extra = format!(
            "{}{}",
            rendered("E300", "examples/a.flx", 38),
            rendered("E425", "examples/a.flx", 12)
        );
        let runs = vec![
            result(Way::VmStrict, ExitKind::CompileError, "", &extra),
            result(Way::Vm, ExitKind::CompileError, "", &extra),
            result(
                Way::LlvmStrict,
                ExitKind::CompileError,
                "",
                &rendered("E425", "examples/a.flx", 12),
            ),
        ];
        let details = location_details(&runs, &[(38, "E300")]);
        assert_eq!(
            details,
            [
                (Way::Vm, vec![], vec!["line 12: E425".to_string()]),
                (Way::LlvmStrict, vec!["line 38: E300".to_string()], vec![]),
            ]
        );
    }

    fn parity_result(runs: Vec<super::super::RunResult>) -> ParityResult {
        ParityResult {
            file: PathBuf::from("/repo/examples/a.flx"),
            results: runs,
            artifacts: vec![],
            verdict: Verdict::Pass,
            skipped_ways: vec![],
            elapsed: vec![],
            unexpected_passes: vec![],
        }
    }

    #[test]
    fn errors_convert_only_when_the_ways_agree_and_each_is_in_the_fixture() {
        let here = rendered("E300", "examples/a.flx", 7);
        let agreeing = parity_result(vec![
            result(Way::Vm, ExitKind::CompileError, "", &here),
            result(Way::Llvm, ExitKind::CompileError, "", &here),
            result(
                Way::VmStrict,
                ExitKind::CompileError,
                "",
                &rendered("E425", "examples/a.flx", 2),
            ),
        ]);
        assert_eq!(reported_pairs(&agreeing), Ok(vec![(7, "E300".to_string())]));

        let disagreeing = parity_result(vec![
            result(Way::Vm, ExitKind::CompileError, "", &here),
            result(Way::Llvm, ExitKind::CompileError, "", &rendered("E300", "examples/a.flx", 8)),
        ]);
        assert_eq!(
            reported_pairs(&disagreeing),
            Err("the ways report different errors".to_string())
        );

        let elsewhere = parity_result(vec![result(
            Way::Vm,
            ExitKind::RuntimeError,
            "",
            &rendered("E1000", "examples/Mod.flx", 3),
        )]);
        assert_eq!(
            reported_pairs(&elsewhere),
            Err("examples/Mod.flx:3: E1000 is not located in the fixture".to_string())
        );
    }

    #[test]
    fn includes_codes_counts_repeats() {
        let codes = |list: &[&str]| list.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        assert!(includes_codes(&codes(&["E300", "E300", "E425"]), &codes(&["E300", "E300"])));
        assert!(!includes_codes(&codes(&["E300", "E425"]), &codes(&["E300", "E300"])));
        assert!(includes_codes(&codes(&["E100"]), &[]));
    }

    #[test]
    fn skip_way_removes_only_requested_ways_and_reports_them() {
        let skip = |way| SkipWay { way, reason: "KI-1".to_string() };
        let (run, skipped) = apply_skip_ways(
            &[Way::Vm, Way::VmStrict, Way::Llvm],
            &[skip(Way::Llvm), skip(Way::LlvmStrict)],
        );
        assert_eq!(run, [Way::Vm, Way::VmStrict]);
        // `llvm_strict` was not requested, so its skip says nothing.
        assert_eq!(skipped, [skip(Way::Llvm)]);
    }

    #[test]
    fn a_skip_of_llvm_also_skips_the_heap_way() {
        let skip = |way| SkipWay { way, reason: "KI-1".to_string() };
        let (run, skipped) = apply_skip_ways(&[Way::Llvm, Way::LlvmHeap], &[skip(Way::Llvm)]);
        assert!(run.is_empty());
        assert_eq!(skipped, [skip(Way::Llvm), skip(Way::LlvmHeap)]);
        // Without llvm requested, its skip still covers the heap way.
        let (run, skipped) = apply_skip_ways(&[Way::Vm, Way::LlvmHeap], &[skip(Way::Llvm)]);
        assert_eq!(run, [Way::Vm]);
        assert_eq!(skipped, [skip(Way::LlvmHeap)]);
    }

    #[test]
    fn an_opt_way_must_print_what_its_plain_way_prints() {
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "1\n2", ""),
            result(Way::VmOpt, ExitKind::Success, "1\n3", ""),
            result(Way::Llvm, ExitKind::Success, "1\n2", ""),
            result(Way::LlvmOpt, ExitKind::Success, "1\n2", ""),
        ];
        let labels: Vec<String> =
            collect_opt_mode_details(&run_results).iter().map(mismatch_detail_label).collect();
        assert_eq!(labels, ["opt stdout: vm vs vm_opt"]);
    }

    #[test]
    fn an_opt_way_must_fail_the_way_its_plain_way_fails() {
        let run_results = vec![
            result(Way::Llvm, ExitKind::RuntimeError, "", "error[E1000]: x"),
            result(Way::LlvmOpt, ExitKind::Success, "42", ""),
        ];
        let labels: Vec<String> =
            collect_opt_mode_details(&run_results).iter().map(mismatch_detail_label).collect();
        assert_eq!(labels, ["opt exit_kind: llvm vs llvm_opt", "opt stderr: llvm vs llvm_opt"]);
    }

    #[test]
    fn an_opt_way_without_its_plain_way_is_not_compared() {
        let run_results = vec![result(Way::VmOpt, ExitKind::Success, "1", "")];
        assert!(collect_opt_mode_details(&run_results).is_empty());
    }

    fn counts(lines: &[&str]) -> Vec<ExpectedCount> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("flux_counts_{}_{n}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("f.flx");
        let header: String = lines.iter().map(|line| format!("// {line}\n")).collect();
        fs::write(&path, format!("{header}fn main() {{ }}\n")).unwrap();
        let meta = parse_fixture_meta(&path);
        let _ = fs::remove_dir_all(&dir);
        meta.expected_counts
    }

    fn metrics(pairs: &[(&str, u64)]) -> std::collections::BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn stated_counts_that_hold_pass() {
        let expected = counts(&["expect-reuse: >= 1", "expect-allocs: 1"]);
        let seen = metrics(&[("aether_reuses", 3), ("aether_allocs", 1)]);
        assert!(matches!(judge_counts(&expected, Ok(seen)), Verdict::Pass));
    }

    #[test]
    fn a_lost_reuse_misses_its_stated_count() {
        let expected = counts(&["expect-reuse: >= 1"]);
        let verdict = judge_counts(&expected, Ok(metrics(&[("aether_reuses", 0)])));
        let Verdict::ExpectedOutputMismatch { expected, actual } = verdict else {
            panic!("expected a mismatch");
        };
        assert_eq!(expected, "// expect-reuse: >= 1");
        assert_eq!(actual, "aether_reuses is 0");
    }

    #[test]
    fn missing_metrics_miss_every_stated_count() {
        let expected = counts(&["expect-allocs: <= 2"]);
        let verdict = judge_counts(&expected, Ok(metrics(&[])));
        assert!(matches!(
            verdict,
            Verdict::ExpectedOutputMismatch { actual, .. } if actual == "--metrics did not report aether_allocs"
        ));
        let verdict = judge_counts(&expected, Err("error[E1]: x".to_string()));
        assert!(matches!(
            verdict,
            Verdict::ExpectedOutputMismatch { actual, .. } if actual == "error[E1]: x"
        ));
    }

    fn heap_labels(run_results: &[super::super::RunResult]) -> Vec<String> {
        collect_heap_mode_details(run_results).iter().map(mismatch_detail_label).collect()
    }

    #[test]
    fn the_heap_way_must_print_what_plain_llvm_prints() {
        let same = vec![
            result(Way::Llvm, ExitKind::Success, "6", ""),
            result(Way::LlvmHeap, ExitKind::Success, "6", ""),
        ];
        assert!(heap_labels(&same).is_empty());
        let different = vec![
            result(Way::Llvm, ExitKind::Success, "6", ""),
            result(Way::LlvmHeap, ExitKind::Success, "7", ""),
        ];
        assert_eq!(heap_labels(&different), ["heap stdout: llvm vs llvm_heap"]);
    }

    #[test]
    fn a_use_after_free_is_a_heap_fault_even_when_llvm_fails_too() {
        let fault = "panic: use after free: copying a String that was already freed \
                     (allocated at f (a.flx:1:1), freed at g (a.flx:2:1))";
        let run_results = vec![
            result(Way::Llvm, ExitKind::RuntimeError, "", "panic: boom"),
            result(Way::LlvmHeap, ExitKind::RuntimeError, "", &format!("{fault}\nStack trace:")),
        ];
        let details = collect_heap_mode_details(&run_results);
        assert_eq!(
            details.iter().map(mismatch_detail_label).collect::<Vec<_>>(),
            ["heap stderr: llvm vs llvm_heap", "heap_fault: llvm_heap"]
        );
        assert!(
            matches!(&details[1], MismatchDetail::HeapFault { message, .. } if message == fault)
        );
    }

    #[test]
    fn the_heap_way_without_plain_llvm_still_reports_a_fault() {
        let run_results = vec![result(
            Way::LlvmHeap,
            ExitKind::RuntimeError,
            "",
            "panic: use after free: dropping a tuple that was already freed",
        )];
        assert_eq!(heap_labels(&run_results), ["heap_fault: llvm_heap"]);
    }

    #[test]
    fn a_freed_constructor_in_heap_stdout_is_a_heap_fault() {
        let run_results =
            vec![result(Way::LlvmHeap, ExitKind::Success, "Some(<adt tag=-559030611>())\n", "")];
        let details = collect_heap_mode_details(&run_results);
        assert_eq!(
            details.iter().map(mismatch_detail_label).collect::<Vec<_>>(),
            ["heap_fault: llvm_heap"]
        );
        assert!(matches!(
            &details[0],
            MismatchDetail::HeapFault { message, .. } if message.contains("Some(<adt tag=-559030611>())")
        ));
    }

    #[test]
    fn a_freed_int_in_heap_stdout_is_a_heap_fault() {
        let run_results = vec![
            result(Way::Llvm, ExitKind::Success, "[1, 2, 3]\n", ""),
            result(Way::LlvmHeap, ExitKind::Success, "[-1200509093985980586, 2, 3]\n", ""),
        ];
        assert_eq!(
            heap_labels(&run_results),
            ["heap stdout: llvm vs llvm_heap", "heap_fault: llvm_heap"]
        );
    }

    #[test]
    fn strict_rejections_must_agree_on_diagnostic_codes() {
        let run_results = vec![
            result(Way::Vm, ExitKind::Success, "output", ""),
            result(Way::Llvm, ExitKind::Success, "output", ""),
            result(Way::VmStrict, ExitKind::CompileError, "", "error[E425]: strict"),
            result(Way::LlvmStrict, ExitKind::CompileError, "", "error[E999]: strict"),
        ];

        let details = collect_mismatch_details(&run_results, &[], false, Expect::Success);

        assert!(
            details.iter().any(|detail| matches!(
                detail,
                MismatchDetail::DiagnosticCodes { way: Way::LlvmStrict, .. }
            )),
            "divergent strict diagnostic codes went unreported: {details:?}"
        );
    }
}

// ── Fixture collection ─────────────────────────────────────────────────────

/// `--compile` phase: compile every fixture through its declared parity
/// backends, populating their caches. Returns true if all requested fixture
/// backends compiled successfully; false if any failed (reported inline).
/// Callers should abort the parity phase on false.
fn run_compile_phase(files: &[PathBuf], config: &Config) -> bool {
    eprintln!(
        "[parity] --compile: compiling {} fixture(s) with fixture-declared backends",
        files.len()
    );
    let mut all_ok = true;
    for file in files {
        let meta = parse_fixture_meta(file);
        if let Some(reason) = &meta.skip {
            eprintln!("\x1b[0;33mCOMPILE SKIP\x1b[0m {} — {}", file.display(), reason);
            continue;
        }
        let extra_args = merged_fixture_args(&config.extra_args, &meta.extra_args);
        let expects_failure = matches!(meta.expect, Expect::CompileError | Expect::RuntimeError);
        let (ways, _) = apply_skip_ways(&meta.ways, &meta.skip_ways);
        let mut compile_ways = ways.into_iter().map(Way::base_way).collect::<Vec<_>>();
        compile_ways.sort_by_key(|way| Way::ALL.iter().position(|w| w == way));
        compile_ways.dedup();
        let mut expected_failure_stderr: Vec<(Way, String)> = Vec::new();
        for way in compile_ways {
            let label = way.to_string();
            if matches!(meta.expect, Expect::CompileError) {
                let result = run_way(
                    &config.vm_binary,
                    &config.llvm_binary,
                    file,
                    way,
                    &extra_args,
                    config.timeout,
                );
                if result.exit_kind != ExitKind::CompileError {
                    eprintln!(
                        "\x1b[0;31mCOMPILE UNEXPECTED {}\x1b[0m {} ({}) — fixture declares `expect: compile_error`",
                        if result.exit_kind == ExitKind::Success { "OK" } else { "ERR" },
                        file.display(),
                        label.as_str(),
                    );
                    all_ok = false;
                } else {
                    eprintln!(
                        "\x1b[0;33mCOMPILE EXPECTED FAIL\x1b[0m {} ({}) — matches `expect: compile_error`",
                        file.display(),
                        label.as_str(),
                    );
                    if !meta.expected_codes().is_empty() {
                        let expected = sorted_codes(meta.expected_codes());
                        let actual = diagnostic_codes(&result.normalized_stderr);
                        if actual != expected {
                            eprintln!(
                                "\x1b[0;31mCOMPILE DIAGNOSTIC MISMATCH\x1b[0m {} ({}) expected {:?}, actual {:?}",
                                file.display(),
                                label.as_str(),
                                expected,
                                actual
                            );
                            all_ok = false;
                        }
                    }
                    expected_failure_stderr.push((way, result.normalized_stderr));
                }
                continue;
            }
            let outcome = compile_fixture(
                &config.vm_binary,
                &config.llvm_binary,
                file,
                way,
                &extra_args,
                config.timeout,
            );
            if expects_failure {
                // Fixture is declared to fail; a failing compile is EXPECTED.
                if outcome.success {
                    eprintln!(
                        "\x1b[0;31mCOMPILE UNEXPECTED OK\x1b[0m {} ({}) — fixture declares `expect: {}` but compiled successfully",
                        file.display(),
                        label.as_str(),
                        meta.expect.name()
                    );
                    all_ok = false;
                } else {
                    eprintln!(
                        "\x1b[0;33mCOMPILE EXPECTED FAIL\x1b[0m {} ({}) — matches `expect: {}`",
                        file.display(),
                        label.as_str(),
                        meta.expect.name()
                    );
                    let normalized_stderr = normalize_stderr(&outcome.stderr);
                    if !meta.expected_codes().is_empty() {
                        let expected = sorted_codes(meta.expected_codes());
                        let actual = diagnostic_codes(&normalized_stderr);
                        if actual != expected {
                            eprintln!(
                                "\x1b[0;31mCOMPILE DIAGNOSTIC MISMATCH\x1b[0m {} ({}) expected {:?}, actual {:?}",
                                file.display(),
                                label.as_str(),
                                expected,
                                actual
                            );
                            all_ok = false;
                        }
                    }
                    expected_failure_stderr.push((way, normalized_stderr));
                }
                continue;
            }
            if outcome.success {
                eprintln!("\x1b[0;32mCOMPILE OK\x1b[0m   {} ({})", file.display(), label.as_str());
                continue;
            }
            eprintln!(
                "\x1b[0;31mCOMPILE FAIL\x1b[0m {} ({}) exit={}",
                file.display(),
                label.as_str(),
                outcome.exit_code
            );
            if !outcome.stderr.trim().is_empty() {
                // Show only the last ~15 lines of stderr for focused errors.
                let tail: Vec<&str> = outcome
                    .stderr
                    .lines()
                    .rev()
                    .take(15)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                for line in tail {
                    eprintln!("  {line}");
                }
            }
            all_ok = false;
        }
        if expects_failure {
            if let Some((baseline_way, baseline_stderr)) = expected_failure_stderr.first() {
                for (way, stderr) in expected_failure_stderr.iter().skip(1) {
                    if stderr != baseline_stderr {
                        eprintln!(
                            "\x1b[0;31mCOMPILE STDERR MISMATCH\x1b[0m {} ({} vs {})",
                            file.display(),
                            baseline_way,
                            way
                        );
                        all_ok = false;
                    }
                }
            }
            if let Some(expected_stderr) = meta.expected_stderr.as_deref() {
                let expected = normalize_stderr(expected_stderr);
                let expected = expected.trim();
                for (way, stderr) in &expected_failure_stderr {
                    let actual = stderr.trim();
                    if actual != expected {
                        eprintln!(
                            "\x1b[0;31mCOMPILE EXPECTED STDERR MISMATCH\x1b[0m {} ({})",
                            file.display(),
                            way
                        );
                        print_first_line_diff(expected, actual);
                        all_ok = false;
                    }
                }
            }
        }
    }
    if !all_ok {
        eprintln!("[parity] --compile: stopping; fixtures above failed to compile");
    }
    all_ok
}

fn print_first_line_diff(expected: &str, actual: &str) {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let max_len = expected_lines.len().max(actual_lines.len());
    for idx in 0..max_len {
        let expected_line = expected_lines.get(idx).copied().unwrap_or("<missing>");
        let actual_line = actual_lines.get(idx).copied().unwrap_or("<missing>");
        if expected_line != actual_line {
            eprintln!("  first diff at line {}:", idx + 1);
            eprintln!("    expected: {expected_line}");
            eprintln!("    actual:   {actual_line}");
            return;
        }
    }
    eprintln!("  stderr differs only by trailing whitespace");
}

fn merged_fixture_args(global_args: &[String], fixture_args: &[String]) -> Vec<String> {
    let mut merged = Vec::with_capacity(global_args.len() + fixture_args.len());
    merged.extend_from_slice(global_args);
    merged.extend_from_slice(fixture_args);
    merged
}

/// The `.flx` files at `path`. A directory is searched recursively; without
/// `include_all`, files without `main` and benchmarks are left out.
fn collect_fixtures(path: &Path, include_all: bool) -> Vec<PathBuf> {
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("flx") {
            return vec![path.to_path_buf()];
        }
        return vec![];
    }

    if path.is_dir() {
        let mut files = Vec::new();
        collect_fixtures_recursive(path, include_all, &mut files);
        files.sort();
        return files;
    }

    vec![]
}

fn collect_fixtures_recursive(path: &Path, include_all: bool, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures_recursive(&path, include_all, files);
        } else if path.extension().and_then(|e| e.to_str()) == Some("flx")
            && (include_all || !should_skip_fixture(&path))
        {
            files.push(path);
        }
    }
}

/// Whether a `.flx` file found by a directory sweep is not a fixture.
///
/// Two kinds are skipped:
///
/// - benchmark and profile files, which produce timing-oriented output and may
///   use workloads too heavy for a parity sweep;
/// - support modules — a file with no entry point cannot be run, so it is
///   something an actual fixture imports (`ImportedCtor/Shapes.flx` is imported
///   by `imported_constructor_types.flx`). Sweeping one in used to be invisible
///   because a fixture that failed to compile still passed; see
///   `docs/known_issues.md#ki-062`.
///
/// Only applies to a directory sweep. A file named explicitly on the command
/// line is always run, so a support module can still be inspected on purpose.
fn should_skip_fixture(path: &Path) -> bool {
    is_bench_or_profile(path) || !declares_entry_point(path)
}

/// A `lib/Flow/` module: the prelude imports it into every program, so
/// compiled on its own it imports itself.
fn is_prelude_module(path: &Path) -> bool {
    let mut parts = path.components().filter(|part| !matches!(part, std::path::Component::CurDir));
    matches!(
        (parts.next(), parts.next()),
        (Some(lib), Some(flow)) if lib.as_os_str() == "lib" && flow.as_os_str() == "Flow"
    )
}

fn is_bench_or_profile(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.contains("_bench") || stem.contains("_profile"))
}

/// Whether a file declares a `main` a parity run could execute.
///
/// Unreadable is treated as "yes" so the sweep reports the read failure rather
/// than silently dropping a fixture.
fn declares_entry_point(path: &Path) -> bool {
    let Ok(source) = std::fs::read_to_string(path) else {
        return true;
    };
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        let trimmed = trimmed.strip_prefix("public ").unwrap_or(trimmed);
        trimmed
            .strip_prefix("fn main")
            .is_some_and(|rest| rest.starts_with('(') || rest.starts_with(' '))
    })
}

// ── Argument parsing ───────────────────────────────────────────────────────

struct Config {
    paths: Vec<PathBuf>,
    /// Files checked at once.
    jobs: usize,
    /// Where `--census` writes its rows.
    census: Option<PathBuf>,
    /// `// requires:` resources this run may use.
    allow: Vec<String>,
    /// `--known-issues`: the open KI ids; a skip citing any other is a
    /// metadata error.
    open_kis: Option<BTreeSet<String>>,
    /// `--check-skips`: run the skipped ways too and report those that pass.
    check_skips: bool,
    /// `--bless`: write what agreeing ways printed into the expected blocks.
    bless: bool,
    /// `--annotate`: move fixtures from `// expect-error:` lines to `//~`.
    annotate: bool,
    /// `None` means "use per-fixture metadata, falling back to [Vm, Llvm]".
    ways: Option<Vec<Way>>,
    vm_binary: PathBuf,
    llvm_binary: PathBuf,
    timeout: Duration,
    extra_args: Vec<String>,
    /// When true, capture `--dump-core` per way and compare Core IR.
    capture_core: bool,
    /// When true, capture `--dump-aether=debug` per way and compare ownership.
    capture_aether: bool,
    /// When true, capture `--dump-repr` per way and compare backend contracts.
    capture_repr: bool,
    /// When true, capture `--dump-cfg` for VM ways.
    capture_cfg: bool,
    /// When true, capture `--dump-lir` for LLVM ways.
    capture_lir: bool,
    /// When true, print the multi-layer debug ladder.
    explain: bool,
    /// When true, compare debug surfaces only.
    compare_surfaces_only: bool,
    /// Optional explicit surface filter.
    surfaces: Option<Vec<SurfaceKind>>,
    /// Filter which results to display.
    display_filter: DisplayFilter,
    /// When true, rebuild parity binaries before running checks.
    rebuild: bool,
    debug_first_failure: bool,
    save_debug_dir: Option<PathBuf>,
    /// When true, compile all fixtures first (populating caches) and stop on
    /// any compilation failure. Then run the parity phase against cached
    /// artifacts (using cached ways instead of fresh `--no-cache` ways).
    compile_first: bool,
}

#[derive(Debug)]
struct BinaryStatus {
    kind: &'static str,
    path: PathBuf,
    status: &'static str,
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut jobs = 1;
    let mut census: Option<PathBuf> = None;
    let mut allow = Vec::new();
    let mut open_kis: Option<BTreeSet<String>> = None;
    let mut check_skips = false;
    let mut bless = false;
    let mut annotate = false;
    let mut ways: Option<Vec<Way>> = None;
    let mut vm_binary = default_vm_binary();
    let mut llvm_binary = default_llvm_binary();
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut extra_args = Vec::new();
    let mut capture_core = false;
    let mut capture_aether = false;
    let mut capture_repr = false;
    let mut capture_cfg = false;
    let mut capture_lir = false;
    let mut explain = false;
    let mut compare_surfaces_only = false;
    let mut surfaces: Option<Vec<SurfaceKind>> = None;
    let mut display_filter = DisplayFilter::All;
    let mut rebuild = false;
    let mut debug_first_failure = false;
    let mut save_debug_dir: Option<PathBuf> = None;
    let mut compile_first = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ways" => {
                i += 1;
                if i >= args.len() {
                    return Err("--ways requires a value".to_string());
                }
                let parsed: Result<Vec<Way>, String> = args[i]
                    .split(',')
                    .map(|s| Way::parse(s.trim()).ok_or_else(|| format!("unknown way: {s}")))
                    .collect();
                ways = Some(parsed?);
            }
            "--vm-binary" => {
                i += 1;
                if i >= args.len() {
                    return Err("--vm-binary requires a path".to_string());
                }
                vm_binary = PathBuf::from(&args[i]);
            }
            "--llvm-binary" => {
                i += 1;
                if i >= args.len() {
                    return Err("--llvm-binary requires a path".to_string());
                }
                llvm_binary = PathBuf::from(&args[i]);
            }
            "--timeout" => {
                i += 1;
                if i >= args.len() {
                    return Err("--timeout requires a value".to_string());
                }
                timeout_secs =
                    args[i].parse().map_err(|_| format!("invalid timeout: {}", args[i]))?;
            }
            "--jobs" => {
                i += 1;
                jobs = args
                    .get(i)
                    .and_then(|value| value.parse().ok())
                    .filter(|&n: &usize| n > 0)
                    .ok_or("--jobs requires a positive number")?;
            }
            "--census" => {
                i += 1;
                let path = args.get(i).ok_or("--census requires a file path")?;
                census = Some(PathBuf::from(path));
            }
            "--allow" => {
                i += 1;
                let resource = args.get(i).ok_or("--allow requires a resource (network)")?;
                if resource != "network" {
                    return Err(format!("unknown --allow resource: {resource} (use network)"));
                }
                allow.push(resource.clone());
            }
            "--known-issues" => {
                i += 1;
                let path = args.get(i).ok_or("--known-issues requires a file path")?;
                let text = fs::read_to_string(path)
                    .map_err(|err| format!("cannot read --known-issues {path}: {err}"))?;
                open_kis = Some(known_issues::open_ids(&text));
            }
            "--check-skips" => {
                check_skips = true;
            }
            "--bless" => {
                bless = true;
            }
            "--annotate" => {
                annotate = true;
            }
            "--capture-core" => {
                capture_core = true;
            }
            "--capture-aether" => {
                capture_aether = true;
            }
            "--capture-repr" => {
                capture_repr = true;
            }
            "--capture-cfg" => {
                capture_cfg = true;
            }
            "--capture-lir" => {
                capture_lir = true;
            }
            "--explain" => {
                explain = true;
            }
            "--compare-surfaces" => {
                compare_surfaces_only = true;
            }
            "--surfaces" => {
                i += 1;
                if i >= args.len() {
                    return Err("--surfaces requires a value".to_string());
                }
                surfaces = Some(parse_surface_list(&args[i])?);
            }
            "--rebuild" => {
                rebuild = true;
            }
            "--compile" => {
                compile_first = true;
            }
            "--debug-first-failure" => {
                debug_first_failure = true;
            }
            "--save-debug-dir" => {
                i += 1;
                if i >= args.len() {
                    return Err("--save-debug-dir requires a path".to_string());
                }
                save_debug_dir = Some(PathBuf::from(&args[i]));
            }
            "--show" => {
                i += 1;
                if i >= args.len() {
                    return Err("--show requires a value (pass, fail, all)".to_string());
                }
                display_filter = match args[i].as_str() {
                    "pass" => DisplayFilter::PassOnly,
                    "fail" | "failed" => DisplayFilter::FailOnly,
                    "all" => DisplayFilter::All,
                    other => {
                        return Err(format!(
                            "unknown --show value: {other} (use pass, fail, or all)"
                        ));
                    }
                };
            }
            "--root" => {
                extra_args.push("--root".to_string());
                i += 1;
                if i >= args.len() {
                    return Err("--root requires a path".to_string());
                }
                extra_args.push(args[i].clone());
            }
            arg if !arg.starts_with('-') => {
                paths.push(PathBuf::from(arg));
            }
            other => {
                return Err(format!("unknown option: {other}"));
            }
        }
        i += 1;
    }

    if paths.is_empty() {
        return Err("missing file or directory argument".to_string());
    }

    if explain {
        capture_core = true;
        capture_aether = true;
        capture_repr = true;
        capture_cfg = true;
        capture_lir = true;
    }
    if compare_surfaces_only && surfaces.is_none() {
        surfaces = Some(vec![
            SurfaceKind::Core,
            SurfaceKind::Aether,
            SurfaceKind::Repr,
            SurfaceKind::BackendIr(BackendId::Vm),
            SurfaceKind::BackendIr(BackendId::Llvm),
        ]);
    }
    if let Some(requested) = &surfaces {
        capture_core |= requested.iter().any(|surface| matches!(surface, SurfaceKind::Core));
        capture_aether |= requested.iter().any(|surface| matches!(surface, SurfaceKind::Aether));
        capture_repr |= requested.iter().any(|surface| matches!(surface, SurfaceKind::Repr));
        capture_cfg |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::BackendIr(BackendId::Vm)));
        capture_lir |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::BackendIr(BackendId::Llvm)));
    }

    Ok(Config {
        paths,
        jobs,
        census,
        allow,
        open_kis,
        check_skips,
        bless,
        annotate,
        ways,
        vm_binary,
        llvm_binary,
        timeout: Duration::from_secs(timeout_secs),
        extra_args,
        capture_core,
        capture_aether,
        capture_repr,
        capture_cfg,
        capture_lir,
        explain,
        compare_surfaces_only,
        surfaces,
        display_filter,
        rebuild,
        debug_first_failure,
        save_debug_dir,
        compile_first,
    })
}

fn print_usage() {
    eprintln!(
        "\
Usage:
  flux parity-check <file-or-dir>... [options]

Options:
  --ways <w1,w2,...>     Ways to compare (default: vm,llvm)
                         Available: vm, llvm, vm_cached, llvm_cached, vm_strict, llvm_strict,
                         llvm_g (native with -g); vm_opt, llvm_opt (with --optimize);
                         llvm_heap (native with --leak-check: a use after free
                         fails, blocks live at exit print LEAK lines);
                         vm_debug, llvm_debug (scripted debug sessions)
  --show <filter>        Show only: pass, fail, or all (default: all)
  --capture-core         Capture --dump-core per way and compare Core IR
  --capture-aether       Capture --dump-aether=debug per way and compare ownership
  --capture-repr         Capture --dump-repr per way and compare backend contracts
  --capture-cfg          Capture --dump-cfg for VM ways
  --capture-lir          Capture --dump-lir for LLVM ways
  --explain              Run parity, then explain the mismatch with shared/backend IR captures
  --rebuild              Force rebuild of parity VM/native binaries before running checks
  --compile              Two-phase run: compile all fixtures first (stop on compile failure),
                         then parity against cached artifacts (faster than fresh-compile per run)
  --debug-first-failure  Stop after the first non-pass result and print extra debug detail
  --save-debug-dir <p>   Save the first non-pass result's artifacts under <p>
  --vm-binary <path>     Path to VM binary (default: {})
  --llvm-binary <path>   Path to native binary (default: {})
  --timeout <secs>       Timeout per file per way (default: {DEFAULT_TIMEOUT_SECS})
  --root <path>          Module root (forwarded to flux, can repeat)
  --jobs <n>             Check n files at once; results still print in file order (default: 1)
  --census <file.tsv>    Support census: run every .flx file, including those without main,
                         on vm,llvm,vm_opt,llvm_opt (or --ways) within each file's `// parity:`
                         line, and write one classified row per file and way. Never fails.
  --allow network        Run fixtures marked `// requires: network`
  --check-skips          Also run the ways `// skip-way:` removes, outside the verdict, and
                         print UNEXPECTED_PASS for each that would pass (its KI may be fixed)
  --known-issues <file>  docs/known_issues.md: a skip reason citing a KI that is not open
                         there is a metadata error
  --bless                When every way ran and agrees, write what they printed into the
                         fixture: its stdout block (added if missing) for a fixture that
                         expects success, or its existing stderr block for one that expects
                         an error
  --annotate             Move each passing fixture from `// expect-error:` lines to `//~`
                         annotations on the lines its errors are reported on, dropping its
                         stderr block when a snapshot pins the same text (proposal 0191)

Binaries are rebuilt automatically when missing or stale.
Use --rebuild to force a refresh:
  cargo run -- parity-check <file-or-dir> --rebuild",
        default_vm_binary().display(),
        default_llvm_binary().display()
    );
}

fn ensure_parity_binaries(config: &Config) -> Vec<BinaryStatus> {
    let newest_source = newest_parity_source_mtime().unwrap_or(SystemTime::UNIX_EPOCH);
    let need_vm = config.rebuild || binary_is_stale(&config.vm_binary, newest_source);
    let need_llvm = config.rebuild || binary_is_stale(&config.llvm_binary, newest_source);
    let mut statuses = Vec::new();

    if !need_vm && !need_llvm {
        statuses.push(BinaryStatus { kind: "vm", path: config.vm_binary.clone(), status: "fresh" });
        statuses.push(BinaryStatus {
            kind: "llvm",
            path: config.llvm_binary.clone(),
            status: "fresh",
        });
        return statuses;
    }

    let vm_target_dir = parity_target_dir(&config.vm_binary, "target/parity_vm");
    let llvm_target_dir = parity_target_dir(&config.llvm_binary, "target/parity_native");

    if need_vm {
        let reason = if config.rebuild {
            "forced rebuild"
        } else if config.vm_binary.exists() {
            "stale -> rebuilt"
        } else {
            "missing -> rebuilt"
        };
        eprintln!("[parity] rebuilding VM binary in {}", vm_target_dir.display());
        run_cargo_build(&vm_target_dir, false);
        statuses.push(BinaryStatus { kind: "vm", path: config.vm_binary.clone(), status: reason });
    } else {
        statuses.push(BinaryStatus { kind: "vm", path: config.vm_binary.clone(), status: "fresh" });
    }

    if need_llvm {
        let reason = if config.rebuild {
            "forced rebuild"
        } else if config.llvm_binary.exists() {
            "stale -> rebuilt"
        } else {
            "missing -> rebuilt"
        };
        eprintln!("[parity] rebuilding native binary in {}", llvm_target_dir.display());
        run_cargo_build(&llvm_target_dir, true);
        statuses.push(BinaryStatus {
            kind: "llvm",
            path: config.llvm_binary.clone(),
            status: reason,
        });
    } else {
        statuses.push(BinaryStatus {
            kind: "llvm",
            path: config.llvm_binary.clone(),
            status: "fresh",
        });
    }

    statuses
}

fn parity_target_dir(binary: &Path, fallback: &str) -> PathBuf {
    binary
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

fn run_cargo_build(target_dir: &Path, enable_llvm: bool) {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if enable_llvm {
        cmd.args(["--features", "llvm"]);
    }
    cmd.env("CARGO_TARGET_DIR", target_dir);

    let status = cmd
        .status()
        .unwrap_or_else(|err| panic!("failed to spawn cargo build for parity binaries: {err}"));
    if !status.success() {
        panic!("parity binary rebuild failed with exit status {status}");
    }
}

fn binary_is_stale(binary: &Path, newest_source: SystemTime) -> bool {
    let Ok(meta) = fs::metadata(binary) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    modified < newest_source
}

fn newest_parity_source_mtime() -> Option<SystemTime> {
    let mut newest = None;
    for root in [
        Path::new("Cargo.toml"),
        Path::new("Cargo.lock"),
        Path::new("build.rs"),
        Path::new("src"),
        Path::new("runtime/c"),
        Path::new("lib"),
    ] {
        newest = max_system_time(newest, newest_path_mtime(root));
    }
    newest
}

fn newest_path_mtime(path: &Path) -> Option<SystemTime> {
    let meta = fs::metadata(path).ok()?;
    if meta.is_file() {
        return meta.modified().ok();
    }
    if !meta.is_dir() {
        return None;
    }

    let mut newest = meta.modified().ok();
    let entries = fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        newest = max_system_time(newest, newest_path_mtime(&entry.path()));
    }
    newest
}

fn max_system_time(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(x), Some(y)) => Some(std::cmp::max(x, y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn print_run_context(config: &Config, statuses: &[BinaryStatus]) {
    let targets =
        config.paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" ");
    eprintln!("[parity] target: {targets}");
    match &config.ways {
        Some(ways) => {
            let way_list = ways.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(",");
            eprintln!("[parity] ways: {way_list}");
        }
        None => eprintln!("[parity] ways: fixture metadata/default"),
    }
    if config.capture_core
        || config.capture_aether
        || config.capture_repr
        || config.capture_cfg
        || config.capture_lir
    {
        let mut captures = Vec::new();
        if config.capture_core {
            captures.push("core");
        }
        if config.capture_aether {
            captures.push("aether");
        }
        if config.capture_repr {
            captures.push("repr");
        }
        if config.capture_cfg {
            captures.push("vm:cfg");
        }
        if config.capture_lir {
            captures.push("llvm:lir");
        }
        eprintln!("[parity] capture: {}", captures.join(","));
    }
    if config.explain {
        eprintln!("[parity] explain: enabled");
    }
    if config.compare_surfaces_only {
        eprintln!("[parity] compare-surfaces: enabled");
    }
    if let Some(surfaces) = &config.surfaces {
        let text = surfaces.iter().map(|surface| surface.label()).collect::<Vec<_>>().join(",");
        eprintln!("[parity] surfaces: {text}");
    }
    for status in statuses {
        eprintln!("[parity] binary {}: {} ({})", status.kind, status.path.display(), status.status);
    }
}

fn save_debug_bundle(dir: &Path, result: &ParityResult) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create dir {}: {e}", dir.display()))?;

    let fixture_dir = dir.join(sanitize_path_for_fs(&result.file));
    fs::create_dir_all(&fixture_dir)
        .map_err(|e| format!("create fixture dir {}: {e}", fixture_dir.display()))?;

    fs::write(fixture_dir.join("metadata.json"), build_metadata_json(result))
        .map_err(|e| format!("write metadata.json: {e}"))?;
    fs::write(fixture_dir.join("commands.txt"), build_commands_txt(result))
        .map_err(|e| format!("write commands.txt: {e}"))?;
    fs::write(fixture_dir.join("diagnosis.txt"), build_diagnosis_txt(result))
        .map_err(|e| format!("write diagnosis.txt: {e}"))?;

    let mut summary = String::new();
    summary.push_str(&format!("file: {}\n", result.file.display()));
    summary.push_str(&format!("verdict: {:?}\n", result.verdict));
    for run in &result.results {
        summary
            .push_str(&format!("way: {} exit={} code={}\n", run.way, run.exit_kind, run.exit_code));
    }
    fs::write(fixture_dir.join("summary.txt"), summary)
        .map_err(|e| format!("write summary: {e}"))?;

    for run in &result.results {
        let prefix = run.way.to_string();
        fs::write(fixture_dir.join(format!("{prefix}.stdout.txt")), &run.stdout)
            .map_err(|e| format!("write stdout for {prefix}: {e}"))?;
        fs::write(fixture_dir.join(format!("{prefix}.stderr.txt")), &run.stderr)
            .map_err(|e| format!("write stderr for {prefix}: {e}"))?;
        fs::write(
            fixture_dir.join(format!("{prefix}.stdout.normalized.txt")),
            &run.normalized_stdout,
        )
        .map_err(|e| format!("write normalized stdout for {prefix}: {e}"))?;
        fs::write(
            fixture_dir.join(format!("{prefix}.stderr.normalized.txt")),
            &run.normalized_stderr,
        )
        .map_err(|e| format!("write normalized stderr for {prefix}: {e}"))?;
        let mut cache_txt = String::new();
        if run.cache_observations.is_empty() {
            cache_txt.push_str("cache artifacts: none\n");
        } else {
            for obs in &run.cache_observations {
                cache_txt.push_str(&format!(
                    "{} [{}] {}\n",
                    obs.kind,
                    match obs.state {
                        super::CacheFileState::Created => "created",
                        super::CacheFileState::Existed => "reused",
                    },
                    obs.path.display()
                ));
            }
        }
        fs::write(fixture_dir.join(format!("{prefix}.cache.txt")), cache_txt)
            .map_err(|e| format!("write cache state for {prefix}: {e}"))?;
    }

    for (way, arts) in &result.artifacts {
        let prefix = way.to_string();
        if let Some(core) = &arts.core.raw {
            fs::write(fixture_dir.join(format!("{prefix}.core.txt")), core)
                .map_err(|e| format!("write core dump for {prefix}: {e}"))?;
        }
        if let Some(core) = &arts.core.normalized {
            fs::write(fixture_dir.join(format!("{prefix}.core.normalized.txt")), core)
                .map_err(|e| format!("write normalized core dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.aether.raw {
            fs::write(fixture_dir.join(format!("{prefix}.aether.txt")), aether)
                .map_err(|e| format!("write aether dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.aether.normalized {
            fs::write(fixture_dir.join(format!("{prefix}.aether.normalized.txt")), aether)
                .map_err(|e| format!("write normalized aether dump for {prefix}: {e}"))?;
        }
        if let Some(repr) = &arts.repr.raw {
            fs::write(fixture_dir.join(format!("{prefix}.repr.txt")), repr)
                .map_err(|e| format!("write repr dump for {prefix}: {e}"))?;
        }
        if let Some(repr) = &arts.repr.normalized {
            fs::write(fixture_dir.join(format!("{prefix}.repr.normalized.txt")), repr)
                .map_err(|e| format!("write normalized repr dump for {prefix}: {e}"))?;
        }
        for (backend, artifact) in &arts.backend_ir {
            let surface = backend_spec(*backend).ir_surface;
            if let Some(raw) = &artifact.raw {
                fs::write(fixture_dir.join(format!("{prefix}.{backend}.{surface}.txt")), raw)
                    .map_err(|e| format!("write backend ir for {prefix}: {e}"))?;
            }
            if let Some(normalized) = &artifact.normalized {
                fs::write(
                    fixture_dir.join(format!("{prefix}.{backend}.{surface}.normalized.txt")),
                    normalized,
                )
                .map_err(|e| format!("write normalized backend ir for {prefix}: {e}"))?;
            }
        }
    }

    Ok(())
}

fn sanitize_path_for_fs(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | ' ' => '_',
            other => other,
        })
        .collect()
}

fn build_commands_txt(result: &ParityResult) -> String {
    let file = result.file.to_string_lossy();
    let mut out = String::new();
    for run in &result.results {
        out.push_str(&format!("{}: {}\n", run.way, cargo_run_for_way(run.way, &file)));
    }
    out
}

fn build_diagnosis_txt(result: &ParityResult) -> String {
    let mut out = String::new();
    match &result.verdict {
        Verdict::Pass => out.push_str("pass\n"),
        Verdict::Skip { reason } => {
            out.push_str("skip\n");
            out.push_str(&format!("reason: {reason}\n"));
        }
        Verdict::ExpectedOutputMismatch { expected, actual } => {
            out.push_str("expected_output_mismatch\n");
            out.push_str(
                "diagnosis: backends agree, but the output disagrees with the fixture expected output\n",
            );
            out.push_str(&format!("expected:\n{expected}\n"));
            out.push_str(&format!("actual:\n{actual}\n"));
        }
        Verdict::Mismatch { details } => {
            out.push_str("mismatch\n");
            if let Some(summary) = diagnose_mismatch(details) {
                out.push_str(&format!("diagnosis: {summary}\n"));
            }
            out.push_str(&format!("detail_count: {}\n", details.len()));
            for detail in details {
                out.push_str(&format!("- {}\n", mismatch_detail_label(detail)));
            }
        }
    }
    out
}

fn build_metadata_json(result: &ParityResult) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"file\": \"{}\",\n",
        json_escape(&result.file.display().to_string())
    ));
    out.push_str(&format!("  \"verdict\": \"{}\",\n", verdict_label(&result.verdict)));
    match &result.verdict {
        Verdict::ExpectedOutputMismatch { expected, actual } => {
            out.push_str(
                "  \"diagnosis\": \"backends agree, but the output disagrees with the fixture expected output\",\n",
            );
            out.push_str(&format!("  \"expected_stdout\": \"{}\",\n", json_escape(expected)));
            out.push_str(&format!("  \"actual_stdout\": \"{}\",\n", json_escape(actual)));
            out.push_str("  \"mismatch_details\": [],\n");
        }
        Verdict::Mismatch { details } => {
            match diagnose_mismatch(details) {
                Some(summary) => {
                    out.push_str(&format!("  \"diagnosis\": \"{}\",\n", json_escape(summary)))
                }
                None => out.push_str("  \"diagnosis\": null,\n"),
            }
            out.push_str("  \"mismatch_details\": [\n");
            for (idx, detail) in details.iter().enumerate() {
                out.push_str(&format!("    \"{}\"", json_escape(&mismatch_detail_label(detail))));
                if idx + 1 != details.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str("  ],\n");
        }
        _ => out.push_str("  \"diagnosis\": null,\n"),
    }
    out.push_str("  \"results\": [\n");
    for (idx, run) in result.results.iter().enumerate() {
        let artifact =
            result.artifacts.iter().find(|(way, _)| *way == run.way).map(|(_, arts)| arts);
        out.push_str("    {\n");
        out.push_str(&format!("      \"way\": \"{}\",\n", run.way));
        out.push_str(&format!("      \"exit_kind\": \"{}\",\n", run.exit_kind));
        out.push_str(&format!("      \"exit_code\": {},\n", run.exit_code));
        out.push_str(&format!(
            "      \"command\": \"{}\",\n",
            json_escape(&cargo_run_for_way(run.way, &result.file.to_string_lossy()))
        ));
        out.push_str(&format!("      \"stdout_bytes\": {},\n", run.stdout.len()));
        out.push_str(&format!("      \"stderr_bytes\": {},\n", run.stderr.len()));
        out.push_str(&format!(
            "      \"normalized_stdout_bytes\": {},\n",
            run.normalized_stdout.len()
        ));
        out.push_str(&format!(
            "      \"normalized_stderr_bytes\": {},\n",
            run.normalized_stderr.len()
        ));
        out.push_str(&format!("      \"cache_observations\": {},\n", run.cache_observations.len()));
        out.push_str("      \"artifacts\": {\n");
        out.push_str(&format!(
            "        \"core\": {},\n",
            artifact.and_then(|arts| arts.core.normalized.as_ref()).is_some()
        ));
        out.push_str(&format!(
            "        \"core_fingerprint\": {},\n",
            artifact
                .and_then(|arts| arts.core.fingerprint.as_ref())
                .map(|fp| format!("\"{}\"", json_escape(fp)))
                .unwrap_or_else(|| "null".to_string())
        ));
        out.push_str(&format!(
            "        \"aether\": {},\n",
            artifact.and_then(|arts| arts.aether.normalized.as_ref()).is_some()
        ));
        out.push_str(&format!(
            "        \"aether_fingerprint\": {},\n",
            artifact
                .and_then(|arts| arts.aether.fingerprint.as_ref())
                .map(|fp| format!("\"{}\"", json_escape(fp)))
                .unwrap_or_else(|| "null".to_string())
        ));
        out.push_str(&format!(
            "        \"repr\": {},\n",
            artifact.and_then(|arts| arts.repr.normalized.as_ref()).is_some()
        ));
        out.push_str(&format!(
            "        \"repr_fingerprint\": {},\n",
            artifact
                .and_then(|arts| arts.repr.fingerprint.as_ref())
                .map(|fp| format!("\"{}\"", json_escape(fp)))
                .unwrap_or_else(|| "null".to_string())
        ));
        out.push_str(&format!(
            "        \"backend_ir\": {}\n",
            artifact.map(|arts| arts.backend_ir.len()).unwrap_or_default()
        ));
        out.push_str("      },\n");
        out.push_str("      \"cache_observations_detail\": [\n");
        for (cache_idx, obs) in run.cache_observations.iter().enumerate() {
            out.push_str(&format!(
                "        {{\"kind\":\"{}\",\"state\":\"{}\",\"path\":\"{}\"}}",
                obs.kind,
                match obs.state {
                    super::CacheFileState::Created => "created",
                    super::CacheFileState::Existed => "reused",
                },
                json_escape(&obs.path.display().to_string())
            ));
            if cache_idx + 1 != run.cache_observations.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("      ]\n");
        out.push_str("    }");
        if idx + 1 != result.results.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn verdict_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Mismatch { .. } => "mismatch",
        Verdict::ExpectedOutputMismatch { .. } => "expected_output_mismatch",
        Verdict::Skip { .. } => "skip",
    }
}

fn mismatch_detail_label(detail: &MismatchDetail) -> String {
    match detail {
        MismatchDetail::ExitKind { left_way, right_way, .. } => {
            format!("exit_kind: {left_way} vs {right_way}")
        }
        MismatchDetail::Stdout { left_way, right_way, .. } => {
            format!("stdout: {left_way} vs {right_way}")
        }
        MismatchDetail::Stderr { left_way, right_way, .. } => {
            format!("stderr: {left_way} vs {right_way}")
        }
        MismatchDetail::DiagnosticCodes { way, .. } => format!("diagnostic_codes: {way}"),
        MismatchDetail::DiagnosticLocations { way, .. } => format!("diagnostic_locations: {way}"),
        MismatchDetail::ExpectedStderr { way, .. } => format!("expected_stderr: {way}"),
        MismatchDetail::CoreMismatch { left_way, right_way, .. } => {
            format!("core: {left_way} vs {right_way}")
        }
        MismatchDetail::AetherMismatch { left_way, right_way, .. } => {
            format!("aether: {left_way} vs {right_way}")
        }
        MismatchDetail::RepresentationMismatch { left_way, right_way, .. } => {
            format!("repr: {left_way} vs {right_way}")
        }
        MismatchDetail::BackendIrMismatch { backend, surface, .. } => {
            format!("backend_ir: {backend}:{surface}")
        }
        MismatchDetail::BackendRuntimeMismatch { backend, .. } => {
            format!("backend_runtime: {backend}")
        }
        MismatchDetail::CacheMismatch { fresh_way, cached_way, field, .. } => {
            format!("cache {field}: {fresh_way} vs {cached_way}")
        }
        MismatchDetail::StrictModeMismatch { normal_way, strict_way, field, .. } => {
            format!("strict {field}: {normal_way} vs {strict_way}")
        }
        MismatchDetail::OptModeMismatch { unopt_way, opt_way, field, .. } => {
            format!("opt {field}: {unopt_way} vs {opt_way}")
        }
        MismatchDetail::HeapModeMismatch { plain_way, heap_way, field, .. } => {
            format!("heap {field}: {plain_way} vs {heap_way}")
        }
        MismatchDetail::HeapFault { way, .. } => format!("heap_fault: {way}"),
    }
}

fn parse_surface_list(raw: &str) -> Result<Vec<SurfaceKind>, String> {
    raw.split(',')
        .map(|item| match item.trim() {
            "core" => Ok(SurfaceKind::Core),
            "aether" => Ok(SurfaceKind::Aether),
            "repr" => Ok(SurfaceKind::Repr),
            "vm:cfg" => Ok(SurfaceKind::BackendIr(BackendId::Vm)),
            "llvm:lir" => Ok(SurfaceKind::BackendIr(BackendId::Llvm)),
            other => Err(format!("unknown surface: {other}")),
        })
        .collect()
}

fn json_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped
}
