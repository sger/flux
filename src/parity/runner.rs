//! Subprocess execution for parity ways.
//!
//! Each way invokes a pre-built flux binary with the appropriate flags
//! and captures stdout, stderr, and exit code.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bytecode::bytecode_cache::{BytecodeCache, hash_bytes, hash_cache_key};
use crate::cache_paths;

use super::normalize::{normalize, normalize_aether_dump, normalize_core_dump};
use super::{
    CacheFileKind, CacheFileState, CacheObservation, DebugArtifacts, ExitKind, RunResult, Way,
};

/// Default timeout per way per fixture.
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// Run a single fixture under a single way.
///
/// The caller must provide paths to pre-built binaries:
/// - `vm_binary`: the flux binary built without native features
/// - `llvm_binary`: the flux binary built with `--features core_to_llvm`
///
/// For cached ways (`vm_cached`, `llvm_cached`): clears cache, runs once to
/// warm it, then runs again with cache enabled and returns the cached run.
pub fn run_way(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> RunResult {
    if way.is_cached() {
        return run_cached_way(vm_binary, llvm_binary, file, way, extra_args, timeout);
    }

    let (binary, mut args) = build_way_args(vm_binary, llvm_binary, way);

    // Always disable cache for fresh parity checks
    args.push("--no-cache".to_string());
    args.push(file.to_string_lossy().into_owned());
    args.extend_from_slice(extra_args);

    if !binary.exists() {
        return make_tool_failure(way, &format!("binary not found: {}", binary.display()));
    }

    // Clear stale bytecode cache
    clear_cache_files(file, extra_args);

    execute_and_collect(binary, &args, way, timeout)
}

/// Run a cached way: warm the cache, then execute with cache enabled.
fn run_cached_way(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> RunResult {
    let base_way = way.base_way();
    let (binary, mut warm_args) = build_way_args(vm_binary, llvm_binary, base_way);

    if !binary.exists() {
        return make_tool_failure(way, &format!("binary not found: {}", binary.display()));
    }

    // Step 1: Clear all cache files
    clear_cache_files(file, extra_args);

    // Step 2: Warming run (with cache enabled, so it writes cache files)
    warm_args.push(file.to_string_lossy().into_owned());
    warm_args.extend_from_slice(extra_args);
    let _ = spawn_with_timeout(binary, &warm_args, timeout);

    // Step 3: Observe cache files created by the warming run
    let cache_after_warm = observe_cache_files(file, extra_args);

    // Step 4: Cached run (with cache enabled, so it reads cache files)
    let (_, mut cached_args) = build_way_args(vm_binary, llvm_binary, base_way);
    cached_args.push(file.to_string_lossy().into_owned());
    cached_args.extend_from_slice(extra_args);

    let mut result = execute_and_collect(binary, &cached_args, way, timeout);

    // Step 5: Observe cache files after the cached run
    let cache_after_cached = observe_cache_files(file, extra_args);

    // Merge observations: warming creates, cached run should find them
    let mut observations = Vec::new();
    for obs in cache_after_warm {
        observations.push(CacheObservation {
            path: obs.path,
            kind: obs.kind,
            state: CacheFileState::Created,
        });
    }
    for obs in cache_after_cached {
        // Only report as Existed if it was also in the warm set
        if observations.iter().any(|o| o.path == obs.path) {
            // Already recorded as Created — this confirms cache hit
        } else {
            observations.push(obs);
        }
    }
    result.cache_observations = observations;

    // Step 6: Clean up cache files
    clear_cache_files(file, extra_args);

    result
}

fn build_way_args<'a>(
    vm_binary: &'a Path,
    llvm_binary: &'a Path,
    way: Way,
) -> (&'a Path, Vec<String>) {
    let (binary, mut args) = match way {
        Way::Vm | Way::VmCached | Way::VmStrict => (vm_binary, vec![]),
        Way::Llvm | Way::LlvmCached | Way::LlvmStrict => {
            (llvm_binary, vec!["--native".to_string()])
        }
    };
    if way.is_strict() {
        args.push("--strict".to_string());
    }
    (binary, args)
}

fn make_tool_failure(way: Way, msg: &str) -> RunResult {
    RunResult {
        way,
        exit_kind: ExitKind::ToolFailure,
        exit_code: -1,
        stdout: String::new(),
        stderr: msg.to_string(),
        normalized_stdout: String::new(),
        normalized_stderr: msg.to_string(),
        cache_observations: vec![],
    }
}

fn execute_and_collect(binary: &Path, args: &[String], way: Way, timeout: Duration) -> RunResult {
    let result = spawn_with_timeout(binary, args, timeout);

    match result {
        SpawnResult::Completed {
            exit_code,
            stdout,
            stderr,
        } => {
            let exit_kind = classify_exit(exit_code, &stderr);
            let normalized_stdout = normalize(&stdout);
            let normalized_stderr = normalize(&stderr);
            RunResult {
                way,
                exit_kind,
                exit_code,
                stdout,
                stderr,
                normalized_stdout,
                normalized_stderr,
                cache_observations: vec![],
            }
        }
        SpawnResult::Timeout => RunResult {
            way,
            exit_kind: ExitKind::Timeout,
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("timed out after {}s", timeout.as_secs()),
            normalized_stdout: String::new(),
            normalized_stderr: format!("timed out after {}s", timeout.as_secs()),
            cache_observations: vec![],
        },
        SpawnResult::SpawnError(err) => RunResult {
            way,
            exit_kind: ExitKind::ToolFailure,
            exit_code: -1,
            stdout: String::new(),
            stderr: err.clone(),
            normalized_stdout: String::new(),
            normalized_stderr: err,
            cache_observations: vec![],
        },
    }
}

// ── Cache file management ──────────────────────────────────────────────────

/// Clear all known cache files for a fixture.
fn clear_cache_files(file: &Path, extra_args: &[String]) {
    let layout = cache_paths::resolve_cache_layout(file, None);
    for dir in [
        layout.interfaces_dir(),
        layout.vm_dir(),
        layout.native_dir(),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
    let (bytecode_key, _, _) = cache_keys_for_fixture(file, extra_args);
    let bytecode_cache = BytecodeCache::new(layout.root());
    let bytecode_path = bytecode_cache.cache_path(file, &bytecode_key);
    if bytecode_path.exists() {
        let _ = std::fs::remove_file(bytecode_path);
    }
}

/// Observe which cache files exist for a fixture.
fn observe_cache_files(file: &Path, extra_args: &[String]) -> Vec<CacheObservation> {
    let mut obs = Vec::new();
    let (bytecode_key, _, _) = cache_keys_for_fixture(file, extra_args);
    let layout = cache_paths::resolve_cache_layout(file, None);
    let bytecode_cache = BytecodeCache::new(layout.root());

    let fxc = bytecode_cache.cache_path(file, &bytecode_key);
    if fxc.exists() {
        obs.push(CacheObservation {
            path: fxc,
            kind: CacheFileKind::Bytecode,
            state: CacheFileState::Existed,
        });
    }

    for dir in [
        layout.interfaces_dir(),
        layout.vm_dir(),
        layout.native_dir(),
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(kind) = classify_cache_observation(&path) else {
                    continue;
                };
                obs.push(CacheObservation {
                    path,
                    kind,
                    state: CacheFileState::Existed,
                });
            }
        }
    }

    obs.sort_by(|left, right| left.path.cmp(&right.path));
    obs
}

fn cache_keys_for_fixture(file: &Path, extra_args: &[String]) -> ([u8; 32], [u8; 32], [u8; 32]) {
    let source = std::fs::read(file).unwrap_or_default();
    let source_hash = hash_bytes(&source);
    let roots_hash = hash_bytes(roots_marker(file, extra_args).as_bytes());
    let strict_hash = hash_bytes(b"strict=0");
    let bytecode_key = hash_cache_key(&hash_cache_key(&source_hash, &roots_hash), &strict_hash);
    let module_key = hash_cache_key(&source_hash, &strict_hash);
    let native_key = hash_cache_key(&source_hash, &strict_hash);
    (bytecode_key, module_key, native_key)
}

fn roots_marker(file: &Path, extra_args: &[String]) -> String {
    let mut roots = Vec::new();
    if let Some(parent) = file.parent() {
        roots.push(parent.to_path_buf());
    }

    for default_root in ["src", "lib"] {
        let path = Path::new(default_root);
        if path.exists() {
            roots.push(path.to_path_buf());
        }
    }

    let mut i = 0;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        if arg == "--root" {
            if let Some(value) = extra_args.get(i + 1) {
                roots.push(Path::new(value).to_path_buf());
                i += 2;
                continue;
            }
        } else if let Some(value) = arg.strip_prefix("--root=") {
            roots.push(Path::new(value).to_path_buf());
        }
        i += 1;
    }

    roots
        .into_iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

fn classify_cache_observation(path: &Path) -> Option<CacheFileKind> {
    let file_name = path.file_name()?.to_string_lossy();
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("flxi") => Some(CacheFileKind::Interface),
        Some("fxm") => Some(CacheFileKind::Module),
        Some("fno") => Some(CacheFileKind::NativeMetadata),
        Some("o") | Some("obj") if file_name.starts_with("flux_support_") => {
            Some(CacheFileKind::NativeSupport)
        }
        Some("o") | Some("obj") => Some(CacheFileKind::NativeObject),
        _ => None,
    }
}

/// Check if native compilation was unsupported (should be treated as skip).
pub fn is_native_skip(result: &RunResult) -> Option<String> {
    if !matches!(result.way, Way::Llvm | Way::LlvmCached | Way::LlvmStrict) {
        return None;
    }
    for line in result.stderr.lines() {
        if line.contains("core_to_llvm compilation failed")
            || line.contains("unsupported CoreToLlvm")
        {
            let reason = line
                .rsplit_once(": ")
                .map(|(_, r)| r.to_string())
                .unwrap_or_else(|| line.to_string());
            return Some(reason);
        }
    }
    None
}

/// Capture `--dump-core` output for a fixture under a given way.
///
/// Returns `DebugArtifacts` with the Core IR dump. Both VM and LLVM binaries
/// share the same frontend, so in theory the dump should be identical — but
/// capturing per-way lets us detect if binary differences affect lowering.
pub fn capture_dump_core(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> DebugArtifacts {
    let binary = match way {
        Way::Vm | Way::VmCached | Way::VmStrict => vm_binary,
        Way::Llvm | Way::LlvmCached | Way::LlvmStrict => llvm_binary,
    };

    if !binary.exists() {
        return DebugArtifacts::default();
    }

    let mut args = vec![
        "--dump-core".to_string(),
        "--no-cache".to_string(),
        file.to_string_lossy().into_owned(),
    ];
    args.extend_from_slice(extra_args);

    let result = spawn_with_timeout(binary, &args, timeout);

    match result {
        SpawnResult::Completed { stdout, .. } => {
            let normalized = normalize_core_dump(&stdout);
            DebugArtifacts {
                dump_core: Some(stdout),
                normalized_dump_core: Some(normalized),
                ..Default::default()
            }
        }
        _ => DebugArtifacts::default(),
    }
}

/// Capture `--dump-aether=debug` output for a fixture under a given way.
///
/// Returns `DebugArtifacts` with the Aether debug report containing
/// per-function borrow signatures, call modes, and dup/drop/reuse details.
pub fn capture_dump_aether(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> DebugArtifacts {
    let binary = match way {
        Way::Vm | Way::VmCached | Way::VmStrict => vm_binary,
        Way::Llvm | Way::LlvmCached | Way::LlvmStrict => llvm_binary,
    };

    if !binary.exists() {
        return DebugArtifacts::default();
    }

    let mut args = vec![
        "--dump-aether=debug".to_string(),
        "--no-cache".to_string(),
        file.to_string_lossy().into_owned(),
    ];
    args.extend_from_slice(extra_args);

    let result = spawn_with_timeout(binary, &args, timeout);

    match result {
        SpawnResult::Completed { stdout, .. } => {
            let normalized = normalize_aether_dump(&stdout);
            DebugArtifacts {
                dump_aether: Some(stdout),
                normalized_dump_aether: Some(normalized),
                ..Default::default()
            }
        }
        _ => DebugArtifacts::default(),
    }
}

// ── Subprocess management ──────────────────────────────────────────────────

enum SpawnResult {
    Completed {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    Timeout,
    SpawnError(String),
}

fn spawn_with_timeout(binary: &Path, args: &[String], timeout: Duration) -> SpawnResult {
    let mut child = match Command::new(binary)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return SpawnResult::SpawnError(format!("failed to spawn: {e}")),
    };

    // Use a thread to implement timeout since std::process has no built-in timeout.
    let timeout_ms = timeout.as_millis() as u64;
    let start = std::time::Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        let mut buf = String::new();
                        std::io::Read::read_to_string(&mut s, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                return SpawnResult::Completed {
                    exit_code: status.code().unwrap_or(-1),
                    stdout,
                    stderr,
                };
            }
            Ok(None) => {
                if start.elapsed().as_millis() as u64 > timeout_ms {
                    let _ = child.kill();
                    let _ = child.wait();
                    return SpawnResult::Timeout;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return SpawnResult::SpawnError(format!("wait error: {e}")),
        }
    }
}

// ── Exit classification ────────────────────────────────────────────────────

fn classify_exit(code: i32, stderr: &str) -> ExitKind {
    if code == 0 {
        return ExitKind::Success;
    }

    // Check for compile-time errors (diagnostics with error codes like E001, E300, etc.)
    let has_compile_error = stderr.lines().any(|line| {
        // Flux diagnostic format: "error[E###]:" or lines containing "error:" from the compiler
        (line.contains("error[E") && line.contains("]:"))
            || line.contains("parse error")
            || line.contains("type error")
    });

    if has_compile_error {
        return ExitKind::CompileError;
    }

    // Check for runtime errors
    let has_runtime_error = stderr.lines().any(|line| {
        line.contains("runtime error")
            || line.contains("stack overflow")
            || line.contains("division by zero")
            || line.contains("panic")
    });

    if has_runtime_error {
        return ExitKind::RuntimeError;
    }

    // Default: treat non-zero exit as runtime error
    ExitKind::RuntimeError
}
