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

use super::fixture::parse_fixture_meta;
use super::report::{print_result, print_summary, DisplayFilter};
use super::runner::{
    capture_dump_aether, capture_dump_core, is_native_skip, run_way, DEFAULT_TIMEOUT_SECS,
};
use super::{DebugArtifacts, ExitKind, MismatchDetail, ParityResult, Verdict, Way};

const DEFAULT_VM_BINARY: &str = "target/parity_vm/debug/flux";
const DEFAULT_LLVM_BINARY: &str = "target/parity_native/debug/flux";

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

    ensure_parity_binaries(&config);

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
             Build with: CARGO_TARGET_DIR=target/parity_native cargo build --features core_to_llvm",
            config.llvm_binary.display()
        );
        std::process::exit(1);
    }

    // Collect .flx files
    let files = collect_fixtures(&config.path);
    if files.is_empty() {
        eprintln!(
            "Error: no .flx files found in {}",
            config.path.display()
        );
        std::process::exit(1);
    }

    // Run parity checks
    let default_ways = vec![Way::Vm, Way::Llvm];
    let mut results = Vec::new();

    for file in &files {
        // Use CLI-specified ways, or fall back to per-fixture metadata
        let ways = config.ways.as_deref().unwrap_or_else(|| {
            // Leak is avoided by using default_ways for non-metadata case
            &default_ways
        });
        let fixture_meta = parse_fixture_meta(file);
        let effective_ways = if config.ways.is_some() {
            ways
        } else {
            &fixture_meta.ways
        };

        let parity_result = check_file(
            file,
            &CheckOpts {
                ways: effective_ways,
                vm_binary: &config.vm_binary,
                llvm_binary: &config.llvm_binary,
                extra_args: &config.extra_args,
                timeout: config.timeout,
                capture_core: config.capture_core,
                capture_aether: config.capture_aether,
            },
        );
        print_result(&parity_result, config.display_filter);
        results.push(parity_result);
    }

    print_summary(&results);

    // Exit with appropriate code
    let has_mismatch = results
        .iter()
        .any(|r| matches!(r.verdict, Verdict::Mismatch { .. }));
    let has_pass = results
        .iter()
        .any(|r| matches!(r.verdict, Verdict::Pass));

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
}

/// Run all requested ways on a single file and compare.
fn check_file(file: &Path, opts: &CheckOpts<'_>) -> ParityResult {
    let mut run_results = Vec::new();

    for &way in opts.ways {
        let result = run_way(
            opts.vm_binary, opts.llvm_binary, file, way, opts.extra_args, opts.timeout,
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

    // Optionally capture debug artifacts (Core IR and/or Aether)
    let artifacts: Vec<(Way, DebugArtifacts)> =
        if opts.capture_core || opts.capture_aether {
            opts.ways
                .iter()
                .map(|&way| {
                    let mut arts = DebugArtifacts::default();
                    if opts.capture_core {
                        let core = capture_dump_core(
                            opts.vm_binary, opts.llvm_binary, file, way,
                            opts.extra_args, opts.timeout,
                        );
                        arts.dump_core = core.dump_core;
                        arts.normalized_dump_core = core.normalized_dump_core;
                    }
                    if opts.capture_aether {
                        let aether = capture_dump_aether(
                            opts.vm_binary, opts.llvm_binary, file, way,
                            opts.extra_args, opts.timeout,
                        );
                        arts.dump_aether = aether.dump_aether;
                        arts.normalized_dump_aether = aether.normalized_dump_aether;
                    }
                (way, arts)
            })
            .collect()
    } else {
        vec![]
    };

    // Compare all ways pairwise against the first
    let mut details = Vec::new();

    // Core IR comparison (if captured) — checked first for classification
    if artifacts.len() >= 2 {
        let (base_way, ref base_arts) = artifacts[0];
        for &(other_way, ref other_arts) in &artifacts[1..] {
            let pair = (
                &base_arts.normalized_dump_core,
                &other_arts.normalized_dump_core,
            );
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

            // Aether comparison — checked after Core
            let aether_pair = (
                &base_arts.normalized_dump_aether,
                &other_arts.normalized_dump_aether,
            );
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
        }
    }

    if run_results.len() >= 2 {
        let base = &run_results[0];
        for other in &run_results[1..] {
            // Compare exit kind
            if base.exit_kind != other.exit_kind {
                details.push(MismatchDetail::ExitKind {
                    left_way: base.way,
                    left: base.exit_kind,
                    right_way: other.way,
                    right: other.exit_kind,
                });
            }

            // Compare normalized stdout
            if base.normalized_stdout != other.normalized_stdout {
                details.push(MismatchDetail::Stdout {
                    left_way: base.way,
                    left: base.normalized_stdout.clone(),
                    right_way: other.way,
                    right: other.normalized_stdout.clone(),
                });
            }

            // Compare normalized stderr (only if both had errors)
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

    let verdict = if details.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Mismatch { details }
    };

    ParityResult {
        file: file.to_path_buf(),
        results: run_results,
        artifacts,
        verdict,
    }
}

// ── Fixture collection ─────────────────────────────────────────────────────

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
            .collect();
        files.sort();
        return files;
    }

    vec![]
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
    /// Filter which results to display.
    display_filter: DisplayFilter,
    /// When true, rebuild parity binaries before running checks.
    rebuild: bool,
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut path: Option<PathBuf> = None;
    let mut ways: Option<Vec<Way>> = None;
    let mut vm_binary = PathBuf::from(DEFAULT_VM_BINARY);
    let mut llvm_binary = PathBuf::from(DEFAULT_LLVM_BINARY);
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut extra_args = Vec::new();
    let mut capture_core = false;
    let mut capture_aether = false;
    let mut display_filter = DisplayFilter::All;
    let mut rebuild = false;

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
            "--rebuild" => {
                rebuild = true;
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
                    other => return Err(format!("unknown --show value: {other} (use pass, fail, or all)")),
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

    Ok(Config {
        path,
        ways,
        vm_binary,
        llvm_binary,
        timeout: Duration::from_secs(timeout_secs),
        extra_args,
        capture_core,
        capture_aether,
        display_filter,
        rebuild,
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
  --rebuild              Force rebuild of parity VM/native binaries before running checks
  --vm-binary <path>     Path to VM binary (default: {DEFAULT_VM_BINARY})
  --llvm-binary <path>   Path to native binary (default: {DEFAULT_LLVM_BINARY})
  --timeout <secs>       Timeout per file per way (default: {DEFAULT_TIMEOUT_SECS})
  --root <path>          Module root (forwarded to flux, can repeat)

Binaries are rebuilt automatically when missing or stale.
Use --rebuild to force a refresh:
  cargo run -- parity-check <file-or-dir> --rebuild"
    );
}

fn ensure_parity_binaries(config: &Config) {
    let newest_source = newest_parity_source_mtime().unwrap_or(SystemTime::UNIX_EPOCH);
    let need_vm = config.rebuild || binary_is_stale(&config.vm_binary, newest_source);
    let need_llvm = config.rebuild || binary_is_stale(&config.llvm_binary, newest_source);

    if !need_vm && !need_llvm {
        return;
    }

    let vm_target_dir = parity_target_dir(&config.vm_binary, "target/parity_vm");
    let llvm_target_dir = parity_target_dir(&config.llvm_binary, "target/parity_native");

    if need_vm {
        eprintln!(
            "[parity] rebuilding VM binary in {}",
            vm_target_dir.display()
        );
        run_cargo_build(&vm_target_dir, false);
    }

    if need_llvm {
        eprintln!(
            "[parity] rebuilding native binary in {}",
            llvm_target_dir.display()
        );
        run_cargo_build(&llvm_target_dir, true);
    }
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
        cmd.args(["--features", "core_to_llvm"]);
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
