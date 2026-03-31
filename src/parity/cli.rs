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
use super::report::{
    cargo_run_for_way, diagnose_mismatch, print_debug_first_failure, print_result,
    print_summary, DisplayFilter,
};
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
             Build with: CARGO_TARGET_DIR=target/parity_native cargo build --features core_to_llvm",
            config.llvm_binary.display()
        );
        std::process::exit(1);
    }

    print_run_context(&config, &binary_statuses);

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
    let mut saved_first_failure = false;

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
        let is_non_pass = !matches!(parity_result.verdict, Verdict::Pass);
        if is_non_pass
            && !saved_first_failure
            && let Some(dir) = &config.save_debug_dir
        {
            if let Err(err) = save_debug_bundle(dir, &parity_result) {
                eprintln!("[parity] failed to save debug bundle: {err}");
            } else {
                eprintln!(
                    "[parity] saved first failure bundle to {}",
                    dir.display()
                );
            }
            saved_first_failure = true;
        }
        let should_stop = config.debug_first_failure
            && is_non_pass;
        if should_stop {
            print_debug_first_failure(&parity_result);
            results.push(parity_result);
            break;
        }
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

    let mut artifacts = capture_artifacts(file, opts, false);
    let mut details = collect_mismatch_details(&run_results, &artifacts);

    if !opts.capture_core
        && !details.is_empty()
        && run_results.len() >= 2
        && artifacts.is_empty()
    {
        artifacts = capture_artifacts(file, opts, true);
        details = collect_mismatch_details(&run_results, &artifacts);
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

fn capture_artifacts(
    file: &Path,
    opts: &CheckOpts<'_>,
    force_core_on_mismatch: bool,
) -> Vec<(Way, DebugArtifacts)> {
    let capture_core = opts.capture_core || force_core_on_mismatch;
    let capture_aether = opts.capture_aether;
    if !capture_core && !capture_aether {
        return vec![];
    }

    opts.ways
        .iter()
        .map(|&way| {
            let mut arts = DebugArtifacts::default();
            if capture_core {
                let core = capture_dump_core(
                    opts.vm_binary, opts.llvm_binary, file, way, opts.extra_args, opts.timeout,
                );
                arts.dump_core = core.dump_core;
                arts.normalized_dump_core = core.normalized_dump_core;
            }
            if capture_aether {
                let aether = capture_dump_aether(
                    opts.vm_binary, opts.llvm_binary, file, way, opts.extra_args, opts.timeout,
                );
                arts.dump_aether = aether.dump_aether;
                arts.normalized_dump_aether = aether.normalized_dump_aether;
            }
            (way, arts)
        })
        .collect()
}

fn collect_mismatch_details(
    run_results: &[super::RunResult],
    artifacts: &[(Way, DebugArtifacts)],
) -> Vec<MismatchDetail> {
    let mut details = Vec::new();

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
    debug_first_failure: bool,
    save_debug_dir: Option<PathBuf>,
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
    let mut vm_binary = PathBuf::from(DEFAULT_VM_BINARY);
    let mut llvm_binary = PathBuf::from(DEFAULT_LLVM_BINARY);
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut extra_args = Vec::new();
    let mut capture_core = false;
    let mut capture_aether = false;
    let mut display_filter = DisplayFilter::All;
    let mut rebuild = false;
    let mut debug_first_failure = false;
    let mut save_debug_dir: Option<PathBuf> = None;

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
        debug_first_failure,
        save_debug_dir,
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
  --debug-first-failure  Stop after the first non-pass result and print extra debug detail
  --save-debug-dir <p>   Save the first non-pass result's artifacts under <p>
  --vm-binary <path>     Path to VM binary (default: {DEFAULT_VM_BINARY})
  --llvm-binary <path>   Path to native binary (default: {DEFAULT_LLVM_BINARY})
  --timeout <secs>       Timeout per file per way (default: {DEFAULT_TIMEOUT_SECS})
  --root <path>          Module root (forwarded to flux, can repeat)

Binaries are rebuilt automatically when missing or stale.
Use --rebuild to force a refresh:
  cargo run -- parity-check <file-or-dir> --rebuild"
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
    if config.capture_core || config.capture_aether {
        let mut captures = Vec::new();
        if config.capture_core {
            captures.push("core");
        }
        if config.capture_aether {
            captures.push("aether");
        }
        eprintln!("[parity] capture: {}", captures.join(","));
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
        summary.push_str(&format!(
            "way: {} exit={} code={}\n",
            run.way, run.exit_kind, run.exit_code
        ));
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
    }

    for (way, arts) in &result.artifacts {
        let prefix = way.to_string();
        if let Some(core) = &arts.dump_core {
            fs::write(fixture_dir.join(format!("{prefix}.core.txt")), core)
                .map_err(|e| format!("write core dump for {prefix}: {e}"))?;
        }
        if let Some(core) = &arts.normalized_dump_core {
            fs::write(fixture_dir.join(format!("{prefix}.core.normalized.txt")), core)
                .map_err(|e| format!("write normalized core dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.dump_aether {
            fs::write(fixture_dir.join(format!("{prefix}.aether.txt")), aether)
                .map_err(|e| format!("write aether dump for {prefix}: {e}"))?;
        }
        if let Some(aether) = &arts.normalized_dump_aether {
            fs::write(
                fixture_dir.join(format!("{prefix}.aether.normalized.txt")),
                aether,
            )
            .map_err(|e| format!("write normalized aether dump for {prefix}: {e}"))?;
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
        Verdict::Mismatch { details } => {
            match diagnose_mismatch(details) {
                Some(summary) => out.push_str(&format!(
                    "  \"diagnosis\": \"{}\",\n",
                    json_escape(summary)
                )),
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
        out.push_str("      \"artifacts\": {\n");
        out.push_str(&format!(
            "        \"core\": {},\n",
            artifact
                .and_then(|arts| arts.normalized_dump_core.as_ref())
                .is_some()
        ));
        out.push_str(&format!(
            "        \"aether\": {}\n",
            artifact
                .and_then(|arts| arts.normalized_dump_aether.as_ref())
                .is_some()
        ));
        out.push_str("      }\n");
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
