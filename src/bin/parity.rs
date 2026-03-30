//! flux-parity — Backend parity checker for Flux.
//!
//! Builds both VM and native (core_to_llvm) backends in isolated target
//! directories, then runs `.flx` fixtures through both and compares
//! stdout, stderr, and exit code.
//!
//! Usage:
//!   cargo run --bin flux-parity -- [OPTIONS] <path>
//!
//!   <path>           Single .flx file or directory of .flx files
//!
//! Options:
//!   --check-core     Also compare --dump-core output between backends
//!   --timeout <N>    Per-test timeout in seconds (default: 15)
//!   --jobs <N>       Parallel test workers (default: CPU count)
//!   --root <DIR>     Extra module root passed through to flux (repeatable)

use std::collections::VecDeque;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ── ANSI colors ──────────────────────────────────────────────────────────

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

// ── Data types ───────────────────────────────────────────────────────────

struct Args {
    target: PathBuf,
    check_core: bool,
    timeout: Duration,
    roots: Vec<String>,
    jobs: usize,
}

#[derive(Clone)]
struct RunOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
}

enum Verdict {
    Pass,
    Skip { reason: String },
    Mismatch { vm: RunOutput, native: RunOutput },
    BackendMismatchIdenticalCore { vm: RunOutput, native: RunOutput },
    CoreMismatch { vm_core: String, native_core: String },
    DumpCoreFailed { error: String },
    Fail { error: String },
}

struct TestResult {
    name: String,
    file: String,
    verdict: Verdict,
    duration: Duration,
}

struct Summary {
    total: usize,
    pass: usize,
    skip: usize,
    mismatch: usize,
    fail: usize,
}

/// Paths to the two pre-built flux binaries.
#[derive(Clone)]
struct BuildConfig {
    vm_binary: PathBuf,
    native_binary: PathBuf,
}

// ── Argument parsing ─────────────────────────────────────────────────────

fn parse_args() -> Result<Args, String> {
    let mut args_iter = env::args().skip(1);
    let mut target: Option<PathBuf> = None;
    let mut check_core = false;
    let mut timeout_secs: u64 = 15;
    let mut roots: Vec<String> = Vec::new();
    let mut jobs: Option<usize> = None;

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--check-core" => check_core = true,
            "--timeout" => {
                let val = args_iter
                    .next()
                    .ok_or("--timeout requires a value")?;
                timeout_secs = val
                    .parse()
                    .map_err(|_| format!("invalid timeout: {val}"))?;
            }
            "--jobs" | "-j" => {
                let val = args_iter
                    .next()
                    .ok_or("--jobs requires a value")?;
                jobs = Some(
                    val.parse()
                        .map_err(|_| format!("invalid jobs: {val}"))?,
                );
            }
            "--root" => {
                let val = args_iter
                    .next()
                    .ok_or("--root requires a value")?;
                roots.push(val);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {other}"));
            }
            _ => {
                if target.is_some() {
                    return Err("only one target path allowed".into());
                }
                target = Some(PathBuf::from(arg));
            }
        }
    }

    let target = target.ok_or("missing target path (file or directory)")?;
    let default_jobs = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    Ok(Args {
        target,
        check_core,
        timeout: Duration::from_secs(timeout_secs),
        roots,
        jobs: jobs.unwrap_or(default_jobs),
    })
}

fn print_usage() {
    eprintln!(
        "Usage: flux-parity [OPTIONS] <path>\n\n\
         <path>           Single .flx file or directory of .flx files\n\n\
         Options:\n  \
           --check-core     Also compare --dump-core output between backends\n  \
           --timeout <N>    Per-test timeout in seconds (default: 15)\n  \
           --jobs <N>       Parallel test workers (default: CPU count)\n  \
           --root <DIR>     Extra module root passed through to flux (repeatable)\n  \
           --help           Show this help"
    );
}

// ── Build phase ──────────────────────────────────────────────────────────

fn build_binaries() -> Result<BuildConfig, String> {
    let vm_dir = PathBuf::from("target/parity_vm");
    let native_dir = PathBuf::from("target/parity_native");

    eprint!("{CYAN}Building VM binary...{RESET}");
    io::stderr().flush().ok();
    build_one(&vm_dir, None)?;
    eprintln!(" {GREEN}done{RESET}");

    eprint!("{CYAN}Building native binary...{RESET}");
    io::stderr().flush().ok();
    build_one(&native_dir, Some("core_to_llvm"))?;
    eprintln!(" {GREEN}done{RESET}");

    let exe = env::consts::EXE_SUFFIX;
    let vm_binary = vm_dir.join(format!("debug/flux{exe}"));
    let native_binary = native_dir.join(format!("debug/flux{exe}"));

    if !vm_binary.exists() {
        return Err(format!("VM binary not found at {}", vm_binary.display()));
    }
    if !native_binary.exists() {
        return Err(format!(
            "native binary not found at {}",
            native_binary.display()
        ));
    }

    Ok(BuildConfig {
        vm_binary,
        native_binary,
    })
}

fn build_one(target_dir: &Path, features: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");
    if let Some(f) = features {
        cmd.arg("--features").arg(f);
    }
    cmd.env("CARGO_TARGET_DIR", target_dir);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());

    let output = cmd
        .output()
        .map_err(|e| format!("failed to spawn cargo: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "cargo build failed (target_dir={}):\n{stderr}",
            target_dir.display()
        ));
    }
    Ok(())
}

// ── Fixture collection ───────────────────────────────────────────────────

fn collect_fixtures(target: &Path) -> Result<Vec<PathBuf>, String> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    if !target.is_dir() {
        return Err(format!(
            "target `{}` is not a file or directory",
            target.display()
        ));
    }

    let mut files = Vec::new();
    collect_flx_recursive(target, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "no .flx files found under `{}`",
            target.display()
        ));
    }
    Ok(files)
}

fn collect_flx_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory `{}`: {e}", dir.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("cannot read entry in `{}`: {e}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_flx_recursive(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("flx") {
            out.push(path);
        }
    }
    Ok(())
}

// ── Backend execution ────────────────────────────────────────────────────

fn run_backend(
    binary: &Path,
    file: &Path,
    extra_flags: &[&str],
    roots: &[String],
    timeout: Duration,
) -> RunOutput {
    // Delete stale bytecode cache
    let fxc = file.with_extension("fxc");
    let _ = fs::remove_file(&fxc);

    let mut cmd = Command::new(binary);
    cmd.arg(file);
    for flag in extra_flags {
        cmd.arg(flag);
    }
    cmd.arg("--no-cache");
    for root in roots {
        cmd.arg("--root").arg(root);
    }
    cmd.env("NO_COLOR", "1");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    run_with_timeout(cmd, timeout)
}

fn run_with_timeout(mut cmd: Command, timeout: Duration) -> RunOutput {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return RunOutput {
                stdout: String::new(),
                stderr: format!("failed to spawn: {e}"),
                exit_code: None,
                timed_out: false,
            };
        }
    };

    // Read stdout/stderr in separate threads to avoid deadlock on full pipe
    // buffers. Take the handles before spawning readers.
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut r) = stdout_handle {
            let _ = r.read_to_string(&mut buf);
        }
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut r) = stderr_handle {
            let _ = r.read_to_string(&mut buf);
        }
        buf
    });

    // Poll for completion with timeout
    let start = Instant::now();
    let poll_interval = Duration::from_millis(50);
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                status = Some(s);
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let stdout = stdout_thread.join().unwrap_or_default();
                    let stderr = stderr_thread.join().unwrap_or_default();
                    return RunOutput {
                        stdout,
                        stderr,
                        exit_code: None,
                        timed_out: true,
                    };
                }
                thread::sleep(poll_interval);
            }
            Err(_) => break,
        }
    }

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();

    RunOutput {
        stdout,
        stderr,
        exit_code: status.and_then(|s| s.code()),
        timed_out: false,
    }
}

// ── Output filtering ─────────────────────────────────────────────────────

/// Strip backend-specific banner lines that differ between VM and native
/// but do not represent semantic differences.
fn filter_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            !line.starts_with("[cfg")
                && !line.starts_with("[lir")
                && !line.starts_with("[llvm]")
                && !line.starts_with("[native]")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Skip detection ───────────────────────────────────────────────────────

fn is_skip(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        if line.contains("core_to_llvm compilation failed")
            || line.contains("unsupported CoreToLlvm")
        {
            // Extract the short reason
            let reason = line
                .rsplit_once(": ")
                .map(|(_, r)| r.to_string())
                .unwrap_or_else(|| line.to_string());
            return Some(reason);
        }
    }
    None
}

// ── Comparison ───────────────────────────────────────────────────────────

fn compare_run(vm: &RunOutput, native: &RunOutput) -> bool {
    let vm_stdout = filter_output(&vm.stdout);
    let native_stdout = filter_output(&native.stdout);
    vm.exit_code == native.exit_code
        && vm_stdout == native_stdout
        && vm.timed_out == native.timed_out
}

// ── Core dump comparison ─────────────────────────────────────────────────

fn compare_core(
    config: &BuildConfig,
    file: &Path,
    roots: &[String],
    timeout: Duration,
) -> Option<Verdict> {
    let vm_core = run_backend(
        &config.vm_binary,
        file,
        &["--dump-core"],
        roots,
        timeout,
    );
    let native_core = run_backend(
        &config.native_binary,
        file,
        &["--native", "--dump-core"],
        roots,
        timeout,
    );

    let vm_ok = vm_core.exit_code == Some(0) && !vm_core.timed_out;
    let native_ok = native_core.exit_code == Some(0) && !native_core.timed_out;

    if !vm_ok || !native_ok {
        let mut error = String::new();
        if !vm_ok {
            let _ = write!(error, "VM dump-core failed");
        }
        if !native_ok {
            if !error.is_empty() {
                error.push_str("; ");
            }
            let _ = write!(error, "native dump-core failed");
        }
        return Some(Verdict::DumpCoreFailed { error });
    }

    if vm_core.stdout != native_core.stdout {
        return Some(Verdict::CoreMismatch {
            vm_core: vm_core.stdout,
            native_core: native_core.stdout,
        });
    }

    None // Core matches
}

// ── Single fixture execution ─────────────────────────────────────────────

fn run_fixture(file: &Path, config: &BuildConfig, args: &Args) -> TestResult {
    let start = Instant::now();
    let name = file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let file_display = file.to_string_lossy().replace('\\', "/");

    // Run VM and native in parallel
    let vm_binary = config.vm_binary.clone();
    let native_binary = config.native_binary.clone();
    let file_vm = file.to_path_buf();
    let file_native = file.to_path_buf();
    let roots_vm = args.roots.clone();
    let roots_native = args.roots.clone();
    let timeout = args.timeout;

    let vm_handle = thread::spawn(move || {
        run_backend(&vm_binary, &file_vm, &[], &roots_vm, timeout)
    });
    let native_handle = thread::spawn(move || {
        run_backend(&native_binary, &file_native, &["--native"], &roots_native, timeout)
    });

    let vm = vm_handle.join().unwrap();
    let native = native_handle.join().unwrap();

    // Check for skip
    if let Some(reason) = is_skip(&native.stderr) {
        return TestResult {
            name,
            file: file_display.clone(),
            verdict: Verdict::Skip { reason },
            duration: start.elapsed(),
        };
    }

    // Check for timeout
    if vm.timed_out || native.timed_out {
        let which = match (vm.timed_out, native.timed_out) {
            (true, true) => "both backends",
            (true, false) => "VM",
            (false, true) => "native",
            _ => unreachable!(),
        };
        return TestResult {
            name,
            file: file_display.clone(),
            verdict: Verdict::Fail {
                error: format!("timeout ({which})"),
            },
            duration: start.elapsed(),
        };
    }

    // Compare execution
    let exec_match = compare_run(&vm, &native);

    if args.check_core {
        if let Some(core_verdict) = compare_core(config, file, &args.roots, args.timeout) {
            // Core-level issue found
            return TestResult {
                name,
                file: file_display.clone(),
                verdict: core_verdict,
                duration: start.elapsed(),
            };
        }
        // Core matches — if execution also mismatches, it's a backend issue
        if !exec_match {
            return TestResult {
                name,
                file: file_display.clone(),
                verdict: Verdict::BackendMismatchIdenticalCore { vm, native },
                duration: start.elapsed(),
            };
        }
    } else if !exec_match {
        return TestResult {
            name,
            file: file_display.clone(),
            verdict: Verdict::Mismatch { vm, native },
            duration: start.elapsed(),
        };
    }

    TestResult {
        name,
        file: file_display,
        verdict: Verdict::Pass,
        duration: start.elapsed(),
    }
}

// ── Parallel execution ───────────────────────────────────────────────────

fn run_fixtures_parallel(
    fixtures: Vec<PathBuf>,
    config: &BuildConfig,
    args: &Args,
) -> Vec<TestResult> {
    let total = fixtures.len();
    let work: Arc<Mutex<VecDeque<(usize, PathBuf)>>> =
        Arc::new(Mutex::new(fixtures.into_iter().enumerate().collect()));
    let (tx, rx) = std::sync::mpsc::channel::<(usize, TestResult)>();

    let num_workers = args.jobs.min(total).max(1);
    let mut handles = Vec::with_capacity(num_workers);

    for _ in 0..num_workers {
        let work = Arc::clone(&work);
        let tx = tx.clone();
        let config = config.clone();
        let roots = args.roots.clone();
        let check_core = args.check_core;
        let timeout = args.timeout;

        handles.push(thread::spawn(move || {
            let worker_args = Args {
                target: PathBuf::new(), // unused by run_fixture
                check_core,
                timeout,
                roots,
                jobs: 1,
            };
            loop {
                let item = work.lock().unwrap().pop_front();
                match item {
                    Some((idx, file)) => {
                        let result = run_fixture(&file, &config, &worker_args);
                        if tx.send((idx, result)).is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }));
    }
    drop(tx);

    // Collect results and print in arrival order
    let mut results: Vec<Option<TestResult>> = (0..total).map(|_| None).collect();
    for (idx, result) in rx {
        print_result(&result, idx + 1, total);
        results[idx] = Some(result);
    }

    for h in handles {
        let _ = h.join();
    }

    results.into_iter().map(|r| r.unwrap()).collect()
}

// ── Reporting ────────────────────────────────────────────────────────────

fn print_result(result: &TestResult, num: usize, total: usize) {
    let counter = format!("[{num}/{total}]");
    let ms = result.duration.as_millis();
    match &result.verdict {
        Verdict::Pass => {
            eprintln!(
                "{counter} {GREEN}PASS{RESET}  {} {CYAN}({ms}ms){RESET}",
                result.name
            );
        }
        Verdict::Skip { reason } => {
            eprintln!(
                "{counter} {YELLOW}SKIP{RESET}  {} ({reason})",
                result.name
            );
        }
        Verdict::Mismatch { vm, native } => {
            eprintln!(
                "{counter} {RED}MISMATCH{RESET}  {} {CYAN}({ms}ms){RESET}",
                result.name
            );
            print_repro_commands(&result.file);
            print_diff(&vm.stdout, &native.stdout, "stdout");
            if vm.exit_code != native.exit_code {
                eprintln!(
                    "  exit: vm={:?} native={:?}",
                    vm.exit_code, native.exit_code
                );
            }
        }
        Verdict::BackendMismatchIdenticalCore { vm, native } => {
            eprintln!(
                "{counter} {RED}{BOLD}BACKEND MISMATCH (identical core){RESET}  {} {CYAN}({ms}ms){RESET}",
                result.name
            );
            print_repro_commands(&result.file);
            print_diff(&vm.stdout, &native.stdout, "stdout");
            if vm.exit_code != native.exit_code {
                eprintln!(
                    "  exit: vm={:?} native={:?}",
                    vm.exit_code, native.exit_code
                );
            }
        }
        Verdict::CoreMismatch {
            vm_core,
            native_core,
        } => {
            eprintln!(
                "{counter} {RED}{BOLD}CORE MISMATCH{RESET}  {} {CYAN}({ms}ms){RESET}",
                result.name
            );
            print_repro_commands(&result.file);
            print_diff(vm_core, native_core, "core");
        }
        Verdict::DumpCoreFailed { error } => {
            eprintln!(
                "{counter} {RED}FAIL{RESET}  {} ({error})",
                result.name
            );
        }
        Verdict::Fail { error } => {
            eprintln!(
                "{counter} {RED}FAIL{RESET}  {} ({error})",
                result.name
            );
        }
    }
}

fn print_repro_commands(file: &str) {
    eprintln!("  {CYAN}vm:{RESET}     cargo run -- {file} --no-cache");
    eprintln!(
        "  {CYAN}native:{RESET} cargo run --features core_to_llvm -- {file} --native --no-cache"
    );
}

fn print_diff(a: &str, b: &str, label: &str) {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    let max = a_lines.len().max(b_lines.len());
    let mut diffs = 0;
    let max_shown = 8;

    for i in 0..max {
        let al = a_lines.get(i).copied().unwrap_or("");
        let bl = b_lines.get(i).copied().unwrap_or("");
        if al != bl {
            if diffs == 0 {
                eprintln!("  {label} diff:");
            }
            if diffs < max_shown {
                eprintln!("    {RED}vm:     {al}{RESET}");
                eprintln!("    {GREEN}native: {bl}{RESET}");
            }
            diffs += 1;
        }
    }
    if diffs > max_shown {
        eprintln!("    ... and {} more differing lines", diffs - max_shown);
    }
}

fn summarize(results: &[TestResult]) -> Summary {
    let mut s = Summary {
        total: results.len(),
        pass: 0,
        skip: 0,
        mismatch: 0,
        fail: 0,
    };
    for r in results {
        match &r.verdict {
            Verdict::Pass => s.pass += 1,
            Verdict::Skip { .. } => s.skip += 1,
            Verdict::Mismatch { .. } | Verdict::BackendMismatchIdenticalCore { .. } => {
                s.mismatch += 1;
            }
            Verdict::CoreMismatch { .. } => s.mismatch += 1,
            Verdict::DumpCoreFailed { .. } | Verdict::Fail { .. } => s.fail += 1,
        }
    }
    s
}

fn print_summary(s: &Summary) {
    eprintln!();
    eprintln!("{BOLD}=== Parity Results ==={RESET}");
    eprintln!("Total:    {}", s.total);
    eprintln!("Pass:     {GREEN}{}{RESET}", s.pass);
    eprintln!("Mismatch: {RED}{}{RESET}", s.mismatch);
    eprintln!("Skip:     {YELLOW}{}{RESET}", s.skip);
    eprintln!("Fail:     {RED}{}{RESET}", s.fail);

    if s.mismatch == 0 && s.fail == 0 && s.pass > 0 {
        eprintln!("\n{GREEN}All compiled examples match!{RESET}");
    }
}

// ── Windows ANSI support ─────────────────────────────────────────────────

#[cfg(windows)]
fn enable_ansi() {
    use std::os::windows::io::AsRawHandle;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        let handle = io::stderr().as_raw_handle();
        let mut mode: u32 = 0;
        // GetConsoleMode + SetConsoleMode via raw FFI
        unsafe extern "system" {
            fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
        }
        if GetConsoleMode(handle, &mut mode) != 0 {
            let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

#[cfg(not(windows))]
fn enable_ansi() {}

// ── Entry point ──────────────────────────────────────────────────────────

fn main() {
    enable_ansi();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{RED}error:{RESET} {e}");
            print_usage();
            std::process::exit(2);
        }
    };

    let config = match build_binaries() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{RED}build failed:{RESET} {e}");
            std::process::exit(1);
        }
    };

    let fixtures = match collect_fixtures(&args.target) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{RED}error:{RESET} {e}");
            std::process::exit(2);
        }
    };

    let count = fixtures.len();
    eprintln!(
        "\n{BOLD}Running {count} fixture(s) with {} worker(s){RESET}\n",
        args.jobs.min(count).max(1)
    );

    let results = if count == 1 {
        // Single file — no thread pool overhead
        let result = run_fixture(&fixtures[0], &config, &args);
        print_result(&result, 1, 1);
        vec![result]
    } else {
        run_fixtures_parallel(fixtures, &config, &args)
    };

    let summary = summarize(&results);
    print_summary(&summary);

    if summary.mismatch > 0 || summary.fail > 0 {
        std::process::exit(1);
    }
}
