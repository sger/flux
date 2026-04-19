//! Subprocess execution for parity ways.
//!
//! Each way invokes a pre-built flux binary with the appropriate flags
//! and captures stdout, stderr, and exit code.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bytecode::bytecode_cache::{BytecodeCache, hash_bytes, hash_cache_key};
use crate::shared::cache_paths;

use super::normalize::{
    normalize, normalize_aether_dump, normalize_cfg_dump, normalize_core_dump, normalize_lir_dump,
    normalize_repr_dump,
};
use super::{
    BackendId, CacheFileKind, CacheFileState, CacheObservation, DebugArtifacts, ExitKind,
    RunResult, SurfaceArtifact, Way,
};

/// Default timeout per way per fixture.
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// Outcome of the `--compile` pre-pass on a single fixture+backend.
pub struct CompileOutcome {
    pub success: bool,
    pub exit_code: i32,
    pub stderr: String,
}

/// Compile a fixture (populate caches) without collecting output for parity.
///
/// Used by `--compile`: runs the fixture with cache enabled so that downstream
/// parity ways can reuse the cached artifacts. Returns success/failure so the
/// caller can bail before the parity phase if any fixture fails to compile.
///
/// Note: this still executes the program (Flux does not currently expose a
/// `--check-only` mode). The exit code tells us whether compilation + run
/// succeeded; cache artifacts are the side effect we want.
pub fn compile_fixture(
    vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> CompileOutcome {
    let compile_timeout = std::cmp::max(timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS * 4));
    let cache_dir = parity_cache_dir(file, way.base_way());
    // Clear cache before each fixture to avoid cross-fixture contamination
    // (native caches share `target/flux/native/` and can produce incompatible
    // exports across fixtures with different module usage shapes).
    clear_cache_files(file, extra_args, &cache_dir);
    let (bin, mut args) = build_way_args(vm_binary, llvm_binary, way.base_way());
    args.push("--cache-dir".to_string());
    args.push(cache_dir.to_string_lossy().into_owned());
    // Do NOT pass --no-cache: we want cache artifacts populated.
    args.push(file.to_string_lossy().into_owned());
    args.extend_from_slice(extra_args);

    if !bin.exists() {
        return CompileOutcome {
            success: false,
            exit_code: -1,
            stderr: format!("binary not found: {}", bin.display()),
        };
    }

    match spawn_with_timeout(bin, &args, compile_timeout) {
        SpawnResult::Completed {
            exit_code, stderr, ..
        } => CompileOutcome {
            success: exit_code == 0,
            exit_code,
            stderr,
        },
        SpawnResult::Timeout => CompileOutcome {
            success: false,
            exit_code: -1,
            stderr: format!("timed out after {}s", compile_timeout.as_secs()),
        },
        SpawnResult::SpawnError(err) => CompileOutcome {
            success: false,
            exit_code: -1,
            stderr: err,
        },
    }
}

/// Run a single fixture under a single way.
///
/// The caller must provide paths to pre-built binaries:
/// - `vm_binary`: the flux binary built without native features
/// - `llvm_binary`: the flux binary built with `--features llvm`
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

    let cache_dir = parity_cache_dir(file, way.base_way());
    let (binary, mut args) = build_way_args(vm_binary, llvm_binary, way);

    // Always disable cache for fresh parity checks
    args.push("--no-cache".to_string());
    args.push("--cache-dir".to_string());
    args.push(cache_dir.to_string_lossy().into_owned());
    args.push(file.to_string_lossy().into_owned());
    args.extend_from_slice(extra_args);

    if !binary.exists() {
        return make_tool_failure(way, &format!("binary not found: {}", binary.display()));
    }

    // Clear stale bytecode cache
    clear_cache_files(file, extra_args, &cache_dir);

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
    let cache_dir = parity_cache_dir(file, base_way);
    let (binary, mut warm_args) = build_way_args(vm_binary, llvm_binary, base_way);

    if !binary.exists() {
        return make_tool_failure(way, &format!("binary not found: {}", binary.display()));
    }

    // Step 1: Clear all cache files
    clear_cache_files(file, extra_args, &cache_dir);

    // Step 2: Warming run (with cache enabled, so it writes cache files)
    warm_args.push("--cache-dir".to_string());
    warm_args.push(cache_dir.to_string_lossy().into_owned());
    warm_args.push(file.to_string_lossy().into_owned());
    warm_args.extend_from_slice(extra_args);
    let _ = spawn_with_timeout(binary, &warm_args, timeout);

    // Step 3: Observe cache files created by the warming run
    let cache_after_warm = observe_cache_files(file, extra_args, &cache_dir);

    // Step 4: Cached run (with cache enabled, so it reads cache files)
    let (_, mut cached_args) = build_way_args(vm_binary, llvm_binary, base_way);
    cached_args.push("--cache-dir".to_string());
    cached_args.push(cache_dir.to_string_lossy().into_owned());
    cached_args.push(file.to_string_lossy().into_owned());
    cached_args.extend_from_slice(extra_args);

    let mut result = execute_and_collect(binary, &cached_args, way, timeout);

    // Step 5: Observe cache files after the cached run
    let cache_after_cached = observe_cache_files(file, extra_args, &cache_dir);

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
    clear_cache_files(file, extra_args, &cache_dir);

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
fn clear_cache_files(file: &Path, extra_args: &[String], cache_dir: &Path) {
    let layout = cache_paths::resolve_cache_layout(file, Some(cache_dir));
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
fn observe_cache_files(file: &Path, extra_args: &[String], cache_dir: &Path) -> Vec<CacheObservation> {
    let mut obs = Vec::new();
    let (bytecode_key, _, _) = cache_keys_for_fixture(file, extra_args);
    let layout = cache_paths::resolve_cache_layout(file, Some(cache_dir));
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

fn parity_cache_dir(file: &Path, way: Way) -> PathBuf {
    let project_root = cache_paths::find_project_root(file)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let backend = match way {
        Way::Vm | Way::VmCached | Way::VmStrict => "vm",
        Way::Llvm | Way::LlvmCached | Way::LlvmStrict => "llvm",
    };
    project_root
        .join("target")
        .join("parity-cache")
        .join(backend)
        .join(cache_paths::artifact_stem(file))
}

fn cache_keys_for_fixture(file: &Path, extra_args: &[String]) -> ([u8; 32], [u8; 32], [u8; 32]) {
    let source = std::fs::read(file).unwrap_or_default();
    let source_hash = hash_bytes(&source);
    let roots_hash = hash_bytes(roots_marker(file, extra_args).as_bytes());
    let strict_hash = hash_bytes(b"strict=0\n");
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
        if line.contains("llvm compilation failed") || line.contains("unsupported CoreToLlvm") {
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
                core: SurfaceArtifact::from_raw(stdout, normalized),
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
                aether: SurfaceArtifact::from_raw(stdout, normalized),
                ..Default::default()
            }
        }
        _ => DebugArtifacts::default(),
    }
}

/// Capture `--dump-repr` output for a fixture under a given way.
///
/// This dump is intentionally backend-agnostic: both binaries should report
/// the same runtime representation contract even though they enforce it via
/// different implementations.
pub fn capture_dump_repr(
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
        "--dump-repr".to_string(),
        "--no-cache".to_string(),
        file.to_string_lossy().into_owned(),
    ];
    if matches!(way, Way::Llvm | Way::LlvmCached | Way::LlvmStrict) {
        args.push("--native".to_string());
    }
    if way.is_strict() {
        args.push("--strict".to_string());
    }
    args.extend_from_slice(extra_args);

    let result = spawn_with_timeout(binary, &args, timeout);

    match result {
        SpawnResult::Completed { stdout, .. } => {
            let normalized = normalize_repr_dump(&stdout);
            DebugArtifacts {
                repr: SurfaceArtifact::from_raw(stdout, normalized),
                ..Default::default()
            }
        }
        _ => DebugArtifacts::default(),
    }
}

pub fn capture_dump_cfg(
    vm_binary: &Path,
    _llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> DebugArtifacts {
    if way.backend_id() != BackendId::Vm || !vm_binary.exists() {
        return DebugArtifacts::default();
    }
    let mut args = vec![
        "--dump-cfg".to_string(),
        "--no-cache".to_string(),
        file.to_string_lossy().into_owned(),
    ];
    if way.is_strict() {
        args.push("--strict".to_string());
    }
    args.extend_from_slice(extra_args);

    match spawn_with_timeout(vm_binary, &args, timeout) {
        SpawnResult::Completed { stdout, .. } => DebugArtifacts {
            backend_ir: vec![(
                BackendId::Vm,
                SurfaceArtifact::from_raw(stdout.clone(), normalize_cfg_dump(&stdout)),
            )],
            ..Default::default()
        },
        _ => DebugArtifacts::default(),
    }
}

pub fn capture_dump_lir(
    _vm_binary: &Path,
    llvm_binary: &Path,
    file: &Path,
    way: Way,
    extra_args: &[String],
    timeout: Duration,
) -> DebugArtifacts {
    if way.backend_id() != BackendId::Llvm || !llvm_binary.exists() {
        return DebugArtifacts::default();
    }
    let mut args = vec![
        "--dump-lir".to_string(),
        "--native".to_string(),
        "--no-cache".to_string(),
        file.to_string_lossy().into_owned(),
    ];
    if way.is_strict() {
        args.push("--strict".to_string());
    }
    args.extend_from_slice(extra_args);

    match spawn_with_timeout(llvm_binary, &args, timeout) {
        SpawnResult::Completed { stdout, .. } => DebugArtifacts {
            backend_ir: vec![(
                BackendId::Llvm,
                SurfaceArtifact::from_raw(stdout.clone(), normalize_lir_dump(&stdout)),
            )],
            ..Default::default()
        },
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

    let has_runtime_error_code = stderr.lines().any(|line| {
        let Some(start) = line.find("error[E") else {
            return false;
        };
        let rest = &line[start + 7..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits
            .parse::<u32>()
            .map(|n| (1000..2000).contains(&n))
            .unwrap_or(false)
    });
    if has_runtime_error_code {
        return ExitKind::RuntimeError;
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
