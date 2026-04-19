//! CLI entry point for `flux parity-check`.
//!
//! Usage:
//!   flux parity-check <file-or-dir> [--root <path> ...] [options]
//!
//! Options:
//!   --ways <w1,w2,...>     Ways to compare (default: vm,llvm)
//!   --vm-binary <path>     Path to VM flux binary
//!   --llvm-binary <path>   Path to native flux binary
//!   --timeout <secs>       Timeout per file per way (default: 15)
//!   --root <path>          Module root (forwarded to flux)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::fixture::{Expect, parse_fixture_meta};
use super::report::{
    DisplayFilter, cargo_run_for_way, diagnose_mismatch, print_debug_first_failure, print_result,
    print_summary,
};
use super::runner::{
    DEFAULT_TIMEOUT_SECS, capture_dump_aether, capture_dump_cfg, capture_dump_core,
    capture_dump_lir, capture_dump_repr, compile_fixture, is_native_skip, run_way,
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
    let files = collect_fixtures(&config.path);
    if files.is_empty() {
        eprintln!("Error: no .flx files found in {}", config.path.display());
        std::process::exit(1);
    }

    // `--compile` pre-pass: compile every fixture (populating caches) before
    // starting the parity loop. Bail on the first compilation failure.
    if config.compile_first && !run_compile_phase(&files, &config) {
        std::process::exit(1);
    }

    // Run parity checks
    let default_ways = vec![Way::Vm, Way::Llvm];
    let compile_ways = vec![Way::VmCached, Way::LlvmCached];
    let mut results = Vec::new();
    let mut saved_first_failure = false;

    for file in &files {
        // Use CLI-specified ways, or fall back to per-fixture metadata
        let ways = config.ways.as_deref().unwrap_or_else(|| {
            // Leak is avoided by using default_ways for non-metadata case
            &default_ways
        });
        let fixture_meta = parse_fixture_meta(file);
        let extra_args = merged_fixture_args(&config.extra_args, &fixture_meta.extra_args);
        let effective_ways = if config.ways.is_some() {
            ways
        } else if config.compile_first {
            // --compile: run against warmed caches instead of fresh ways.
            &compile_ways
        } else {
            &fixture_meta.ways
        };

        let parity_result = check_file(
            file,
            &CheckOpts {
                ways: effective_ways,
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
                expect: fixture_meta.expect,
            },
        );
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
            results.push(parity_result);
            break;
        }
        results.push(parity_result);
    }

    print_summary(&results);

    // Exit with appropriate code
    let has_mismatch = results.iter().any(|r| {
        matches!(
            r.verdict,
            Verdict::Mismatch { .. } | Verdict::ExpectedOutputMismatch { .. }
        )
    });
    let has_pass = results.iter().any(|r| matches!(r.verdict, Verdict::Pass));

    if has_mismatch || !has_pass {
        std::process::exit(1);
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
    /// Declared fixture expectation: `success`, `compile_error`, or
    /// `runtime_error`. Controls stdout-comparison semantics.
    expect: Expect,
}

/// Run all requested ways on a single file and compare.
fn check_file(file: &Path, opts: &CheckOpts<'_>) -> ParityResult {
    let mut run_results = Vec::new();

    for &way in opts.ways {
        let result = run_way(
            opts.vm_binary,
            opts.llvm_binary,
            file,
            way,
            opts.extra_args,
            opts.timeout,
        );
        run_results.push(result);
    }

    // Check for native skip
    for result in &run_results {
        if let Some(reason) = is_native_skip(result) {
            return ParityResult {
                file: file.to_path_buf(),
                results: run_results,
                artifacts: vec![],
                verdict: Verdict::Skip { reason },
            };
        }
    }

    let mut artifacts = capture_artifacts(file, opts, false);
    let mut details =
        collect_mismatch_details(&run_results, &artifacts, opts.compare_surfaces_only);

    if !opts.capture_core
        && !details.is_empty()
        && run_results.len() >= 2
        && artifacts.is_empty()
        && !opts.compare_surfaces_only
    {
        artifacts = capture_artifacts(file, opts, true);
        details = collect_mismatch_details(&run_results, &artifacts, opts.compare_surfaces_only);
    }

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
            let is_allowed = normal_result.exit_kind == ExitKind::Success
                && strict_result.exit_kind == ExitKind::CompileError;

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

    let verdict = if !details.is_empty() {
        Verdict::Mismatch { details }
    } else if matches!(opts.expect, Expect::CompileError | Expect::RuntimeError) {
        // Fixture declares it should fail. Require all runs to exit non-zero
        // and skip the expected_stdout comparison — stderr carries the real
        // signal for error fixtures and is too volatile to pin to a block.
        let all_failed = !run_results.is_empty() && run_results.iter().all(|r| r.exit_code != 0);
        if all_failed {
            Verdict::Pass
        } else {
            Verdict::ExpectedOutputMismatch {
                expected: format!(
                    "{} (all backends should exit non-zero)",
                    match opts.expect {
                        Expect::CompileError => "compile_error",
                        Expect::RuntimeError => "runtime_error",
                        Expect::Success => "success",
                    }
                ),
                actual: run_results
                    .iter()
                    .map(|r| format!("{}: exit={}", r.way, r.exit_code))
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        }
    } else if let Some(expected_stdout) = opts.expected_stdout {
        let expected = expected_stdout.trim();
        let actual = run_results
            .first()
            .map(|run| run.normalized_stdout.trim().to_string())
            .unwrap_or_default();
        if actual != expected {
            Verdict::ExpectedOutputMismatch {
                expected: expected.to_string(),
                actual,
            }
        } else {
            Verdict::Pass
        }
    } else {
        Verdict::Pass
    };

    ParityResult {
        file: file.to_path_buf(),
        results: run_results,
        artifacts,
        verdict,
    }
}

fn capture_artifacts(
    file: &Path,
    opts: &CheckOpts<'_>,
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

    opts.ways
        .iter()
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

fn collect_mismatch_details(
    run_results: &[super::RunResult],
    artifacts: &[(Way, DebugArtifacts)],
    compare_surfaces_only: bool,
) -> Vec<MismatchDetail> {
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

    if !compare_surfaces_only && run_results.len() >= 2 {
        let base = &run_results[0];
        let has_shared_mismatch = details.iter().any(|d| {
            matches!(
                d,
                MismatchDetail::CoreMismatch { .. }
                    | MismatchDetail::AetherMismatch { .. }
                    | MismatchDetail::RepresentationMismatch { .. }
            )
        });
        for other in &run_results[1..] {
            let runtime_diverged = base.exit_kind != other.exit_kind
                || base.normalized_stdout != other.normalized_stdout
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

            if base.normalized_stdout != other.normalized_stdout {
                details.push(MismatchDetail::Stdout {
                    left_way: base.way,
                    left: base.normalized_stdout.clone(),
                    right_way: other.way,
                    right: other.normalized_stdout.clone(),
                });
            }

            if (base.exit_kind != ExitKind::Success || other.exit_kind != ExitKind::Success)
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
    }

    details
}

// ── Fixture collection ─────────────────────────────────────────────────────

/// `--compile` phase: compile every fixture via both backends, populating
/// their caches. Returns true if all fixtures compiled successfully; false if
/// any failed (reported inline). Callers should abort the parity phase on
/// false.
fn run_compile_phase(files: &[PathBuf], config: &Config) -> bool {
    eprintln!(
        "[parity] --compile: compiling {} fixture(s) with VM and LLVM backends",
        files.len()
    );
    let mut all_ok = true;
    for file in files {
        let meta = parse_fixture_meta(file);
        let extra_args = merged_fixture_args(&config.extra_args, &meta.extra_args);
        let expects_failure = matches!(meta.expect, Expect::CompileError | Expect::RuntimeError);
        for (way, label) in [(Way::Vm, "vm"), (Way::Llvm, "llvm")] {
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
                        label,
                        match meta.expect {
                            Expect::CompileError => "compile_error",
                            Expect::RuntimeError => "runtime_error",
                            Expect::Success => "success",
                        }
                    );
                    all_ok = false;
                } else {
                    eprintln!(
                        "\x1b[0;33mCOMPILE EXPECTED FAIL\x1b[0m {} ({}) — matches `expect: {}`",
                        file.display(),
                        label,
                        match meta.expect {
                            Expect::CompileError => "compile_error",
                            Expect::RuntimeError => "runtime_error",
                            Expect::Success => "success",
                        }
                    );
                }
                continue;
            }
            if outcome.success {
                eprintln!(
                    "\x1b[0;32mCOMPILE OK\x1b[0m   {} ({})",
                    file.display(),
                    label
                );
                continue;
            }
            // Distinguish native-unsupported (skip-able) from real compile failure.
            let is_llvm_skip = matches!(way, Way::Llvm)
                && (outcome.stderr.contains("llvm compilation failed")
                    || outcome.stderr.contains("unsupported CoreToLlvm"));
            if is_llvm_skip {
                eprintln!(
                    "\x1b[0;33mCOMPILE SKIP\x1b[0m {} ({}) — native unsupported",
                    file.display(),
                    label
                );
                continue;
            }
            eprintln!(
                "\x1b[0;31mCOMPILE FAIL\x1b[0m {} ({}) exit={}",
                file.display(),
                label,
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
    }
    if !all_ok {
        eprintln!("[parity] --compile: stopping; fixtures above failed to compile");
    }
    all_ok
}

fn merged_fixture_args(global_args: &[String], fixture_args: &[String]) -> Vec<String> {
    let mut merged = Vec::with_capacity(global_args.len() + fixture_args.len());
    merged.extend_from_slice(global_args);
    merged.extend_from_slice(fixture_args);
    merged
}

fn collect_fixtures(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("flx") {
            return vec![path.to_path_buf()];
        }
        return vec![];
    }

    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("flx"))
            .filter(|p| !should_skip_fixture(p))
            .collect();
        files.sort();
        return files;
    }

    vec![]
}

/// Skip benchmark/profile files — they produce timing-oriented output and may
/// intentionally use workloads that are too heavy for parity sweeps.
fn should_skip_fixture(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.contains("_bench") || stem.contains("_profile"))
}

// ── Argument parsing ───────────────────────────────────────────────────────

struct Config {
    path: PathBuf,
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
    let mut path: Option<PathBuf> = None;
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
                timeout_secs = args[i]
                    .parse()
                    .map_err(|_| format!("invalid timeout: {}", args[i]))?;
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
                if path.is_some() {
                    return Err(format!("unexpected argument: {arg}"));
                }
                path = Some(PathBuf::from(arg));
            }
            other => {
                return Err(format!("unknown option: {other}"));
            }
        }
        i += 1;
    }

    let path = path.ok_or("missing file or directory argument")?;

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
        capture_core |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::Core));
        capture_aether |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::Aether));
        capture_repr |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::Repr));
        capture_cfg |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::BackendIr(BackendId::Vm)));
        capture_lir |= requested
            .iter()
            .any(|surface| matches!(surface, SurfaceKind::BackendIr(BackendId::Llvm)));
    }

    Ok(Config {
        path,
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
  flux parity-check <file-or-dir> [options]

Options:
  --ways <w1,w2,...>     Ways to compare (default: vm,llvm)
                         Available: vm, llvm, vm_cached, llvm_cached, vm_strict, llvm_strict
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
        statuses.push(BinaryStatus {
            kind: "vm",
            path: config.vm_binary.clone(),
            status: "fresh",
        });
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
        eprintln!(
            "[parity] rebuilding VM binary in {}",
            vm_target_dir.display()
        );
        run_cargo_build(&vm_target_dir, false);
        statuses.push(BinaryStatus {
            kind: "vm",
            path: config.vm_binary.clone(),
            status: reason,
        });
    } else {
        statuses.push(BinaryStatus {
            kind: "vm",
            path: config.vm_binary.clone(),
            status: "fresh",
        });
    }

    if need_llvm {
        let reason = if config.rebuild {
            "forced rebuild"
        } else if config.llvm_binary.exists() {
            "stale -> rebuilt"
        } else {
            "missing -> rebuilt"
        };
        eprintln!(
            "[parity] rebuilding native binary in {}",
            llvm_target_dir.display()
        );
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
    eprintln!("[parity] target: {}", config.path.display());
    match &config.ways {
        Some(ways) => {
            let way_list = ways
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
                .join(",");
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
        let text = surfaces
            .iter()
            .map(|surface| surface.label())
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("[parity] surfaces: {text}");
    }
    for status in statuses {
        eprintln!(
            "[parity] binary {}: {} ({})",
            status.kind,
            status.path.display(),
            status.status
        );
    }
}

fn save_debug_bundle(dir: &Path, result: &ParityResult) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create dir {}: {e}", dir.display()))?;

    let fixture_dir = dir.join(sanitize_path_for_fs(&result.file));
    fs::create_dir_all(&fixture_dir)
        .map_err(|e| format!("create fixture dir {}: {e}", fixture_dir.display()))?;

    fs::write(
        fixture_dir.join("metadata.json"),
        build_metadata_json(result),
    )
    .map_err(|e| format!("write metadata.json: {e}"))?;
    fs::write(fixture_dir.join("commands.txt"), build_commands_txt(result))
        .map_err(|e| format!("write commands.txt: {e}"))?;
    fs::write(
        fixture_dir.join("diagnosis.txt"),
        build_diagnosis_txt(result),
    )
    .map_err(|e| format!("write diagnosis.txt: {e}"))?;

    let mut summary = String::new();
    summary.push_str(&format!("file: {}\n", result.file.display()));
    summary.push_str(&format!("verdict: {:?}\n", result.verdict));
    for run in &result.results {
        summary.push_str(&format!(
            "way: {} exit={} code={}\n",
            run.way, run.exit_kind, run.exit_code
        ));
    }
    fs::write(fixture_dir.join("summary.txt"), summary)
        .map_err(|e| format!("write summary: {e}"))?;

    for run in &result.results {
        let prefix = run.way.to_string();
        fs::write(
            fixture_dir.join(format!("{prefix}.stdout.txt")),
            &run.stdout,
        )
        .map_err(|e| format!("write stdout for {prefix}: {e}"))?;
        fs::write(
            fixture_dir.join(format!("{prefix}.stderr.txt")),
            &run.stderr,
        )
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
            fs::write(
                fixture_dir.join(format!("{prefix}.core.normalized.txt")),
                core,
            )
            .map_err(|e| format!("write normalized core dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.aether.raw {
            fs::write(fixture_dir.join(format!("{prefix}.aether.txt")), aether)
                .map_err(|e| format!("write aether dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.aether.normalized {
            fs::write(
                fixture_dir.join(format!("{prefix}.aether.normalized.txt")),
                aether,
            )
            .map_err(|e| format!("write normalized aether dump for {prefix}: {e}"))?;
        }
        if let Some(repr) = &arts.repr.raw {
            fs::write(fixture_dir.join(format!("{prefix}.repr.txt")), repr)
                .map_err(|e| format!("write repr dump for {prefix}: {e}"))?;
        }
        if let Some(repr) = &arts.repr.normalized {
            fs::write(
                fixture_dir.join(format!("{prefix}.repr.normalized.txt")),
                repr,
            )
            .map_err(|e| format!("write normalized repr dump for {prefix}: {e}"))?;
        }
        for (backend, artifact) in &arts.backend_ir {
            let surface = backend_spec(*backend).ir_surface;
            if let Some(raw) = &artifact.raw {
                fs::write(
                    fixture_dir.join(format!("{prefix}.{backend}.{surface}.txt")),
                    raw,
                )
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
        out.push_str(&format!(
            "{}: {}\n",
            run.way,
            cargo_run_for_way(run.way, &file)
        ));
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
    out.push_str(&format!(
        "  \"verdict\": \"{}\",\n",
        verdict_label(&result.verdict)
    ));
    match &result.verdict {
        Verdict::ExpectedOutputMismatch { expected, actual } => {
            out.push_str(
                "  \"diagnosis\": \"backends agree, but the output disagrees with the fixture expected output\",\n",
            );
            out.push_str(&format!(
                "  \"expected_stdout\": \"{}\",\n",
                json_escape(expected)
            ));
            out.push_str(&format!(
                "  \"actual_stdout\": \"{}\",\n",
                json_escape(actual)
            ));
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
                out.push_str(&format!(
                    "    \"{}\"",
                    json_escape(&mismatch_detail_label(detail))
                ));
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
        let artifact = result
            .artifacts
            .iter()
            .find(|(way, _)| *way == run.way)
            .map(|(_, arts)| arts);
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
        out.push_str(&format!(
            "      \"cache_observations\": {},\n",
            run.cache_observations.len()
        ));
        out.push_str("      \"artifacts\": {\n");
        out.push_str(&format!(
            "        \"core\": {},\n",
            artifact
                .and_then(|arts| arts.core.normalized.as_ref())
                .is_some()
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
            artifact
                .and_then(|arts| arts.aether.normalized.as_ref())
                .is_some()
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
            artifact
                .and_then(|arts| arts.repr.normalized.as_ref())
                .is_some()
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
            artifact
                .map(|arts| arts.backend_ir.len())
                .unwrap_or_default()
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
        MismatchDetail::ExitKind {
            left_way,
            right_way,
            ..
        } => format!("exit_kind: {left_way} vs {right_way}"),
        MismatchDetail::Stdout {
            left_way,
            right_way,
            ..
        } => format!("stdout: {left_way} vs {right_way}"),
        MismatchDetail::Stderr {
            left_way,
            right_way,
            ..
        } => format!("stderr: {left_way} vs {right_way}"),
        MismatchDetail::CoreMismatch {
            left_way,
            right_way,
            ..
        } => format!("core: {left_way} vs {right_way}"),
        MismatchDetail::AetherMismatch {
            left_way,
            right_way,
            ..
        } => format!("aether: {left_way} vs {right_way}"),
        MismatchDetail::RepresentationMismatch {
            left_way,
            right_way,
            ..
        } => format!("repr: {left_way} vs {right_way}"),
        MismatchDetail::BackendIrMismatch {
            backend, surface, ..
        } => format!("backend_ir: {backend}:{surface}"),
        MismatchDetail::BackendRuntimeMismatch { backend, .. } => {
            format!("backend_runtime: {backend}")
        }
        MismatchDetail::CacheMismatch {
            fresh_way,
            cached_way,
            field,
            ..
        } => format!("cache {field}: {fresh_way} vs {cached_way}"),
        MismatchDetail::StrictModeMismatch {
            normal_way,
            strict_way,
            field,
            ..
        } => format!("strict {field}: {normal_way} vs {strict_way}"),
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
