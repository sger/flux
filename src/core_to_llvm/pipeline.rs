//! Pipeline orchestration: `.ll` → `opt` → `llc` → `cc` → native binary.
//!
//! Invokes external LLVM tools as subprocesses.  The pipeline produces
//! self-contained binaries that link against `libflux_rt.a` (the C runtime
//! from `runtime/c/`).

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Errors from external tool invocation.
#[derive(Debug)]
pub enum PipelineError {
    /// An external tool was not found on PATH.
    ToolNotFound { tool: &'static str, detail: String },
    /// An external tool exited with a non-zero status.
    ToolFailed {
        tool: &'static str,
        exit_code: Option<i32>,
        stderr: String,
    },
    /// I/O error (e.g., writing the `.ll` file).
    Io(std::io::Error),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::ToolNotFound { tool, detail } => {
                write!(f, "{tool} not found: {detail}")
            }
            PipelineError::ToolFailed {
                tool,
                exit_code,
                stderr,
            } => {
                write!(
                    f,
                    "{tool} failed (exit {}): {}",
                    exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".into()),
                    stderr.trim()
                )
            }
            PipelineError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<std::io::Error> for PipelineError {
    fn from(e: std::io::Error) -> Self {
        PipelineError::Io(e)
    }
}

/// Configuration for the pipeline.
pub struct PipelineConfig {
    /// LLVM IR text (the `.ll` content).
    pub ll_text: String,
    /// Optimization level (0–3).
    pub opt_level: u32,
    /// Output path for the final binary (if `emit_binary` is true).
    pub output_path: Option<PathBuf>,
    /// Path to the Flux C runtime library directory.
    pub runtime_lib_dir: Option<PathBuf>,
}

/// Result of running the pipeline.
pub enum PipelineResult {
    /// LLVM IR was written to a file.
    EmittedLlvm { path: PathBuf },
    /// A native binary was produced.
    EmittedBinary { path: PathBuf },
    /// The program was executed in-place (compiled + ran + cleaned up).
    Executed { exit_code: i32 },
}

/// Write LLVM IR to a `.ll` file.
pub fn emit_llvm_ir(ll_text: &str, output: &Path) -> Result<(), PipelineError> {
    let mut file = std::fs::File::create(output)?;
    file.write_all(ll_text.as_bytes())?;
    Ok(())
}

/// Run `opt` to optimize a `.ll` file, producing `.bc`.
fn run_opt(ll_path: &Path, bc_path: &Path, opt_level: u32) -> Result<(), PipelineError> {
    let opt = find_tool("opt")?;
    let output = Command::new(&opt)
        .arg(ll_path)
        .arg("-o")
        .arg(bc_path)
        .arg(format!("-passes=default<O{opt_level}>"))
        .output()?;
    check_output("opt", &output)
}

/// Run `llc` to compile `.bc` → `.o`.
fn run_llc(bc_path: &Path, obj_path: &Path) -> Result<(), PipelineError> {
    let llc = find_tool("llc")?;
    let mut cmd = Command::new(&llc);
    cmd.arg(bc_path)
        .arg("-o")
        .arg(obj_path)
        .arg("--filetype=obj");
    // macOS Mach-O is position-independent by default; Linux and Windows
    // require PIC for PIE executables (the default linker mode).
    if !cfg!(target_os = "macos") {
        cmd.arg("--relocation-model=pic");
    }
    let output = cmd.output()?;
    check_output("llc", &output)
}

/// Run `cc` (system linker) to link `.o` + `libflux_rt.a` → executable.
fn run_linker(
    obj_path: &Path,
    exe_path: &Path,
    runtime_lib_dir: Option<&Path>,
) -> Result<(), PipelineError> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let mut cmd = Command::new(&cc);
    cmd.arg(obj_path).arg("-o").arg(exe_path);
    if let Some(dir) = runtime_lib_dir {
        cmd.arg(format!("-L{}", dir.display()));
        cmd.arg("-lflux_rt");
    }
    // Set large stack size for deeply recursive programs.
    #[cfg(target_os = "macos")]
    cmd.args(["-Wl,-stack_size,0x4000000"]); // 64 MB stack
    // Link math library on Linux.
    #[cfg(target_os = "linux")]
    cmd.arg("-lm");
    let output = cmd.output()?;
    check_output("cc", &output)
}

/// Compile LLVM IR to a native binary.
///
/// Steps: `.ll` → `opt` → `.bc` → `llc` → `.o` → `cc` → executable.
pub fn compile_to_binary(config: &PipelineConfig) -> Result<PipelineResult, PipelineError> {
    let dir = std::env::temp_dir().join(format!("flux_core_to_llvm_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let ll_path = dir.join("program.ll");
    let bc_path = dir.join("program.bc");
    let obj_path = dir.join("program.o");

    emit_llvm_ir(&config.ll_text, &ll_path)?;

    if config.opt_level > 0 {
        run_opt(&ll_path, &bc_path, config.opt_level)?;
        run_llc(&bc_path, &obj_path)?;
    } else {
        // Skip opt for -O0: compile .ll directly with llc.
        run_llc(&ll_path, &obj_path)?;
    }

    let exe_path = config
        .output_path
        .clone()
        .unwrap_or_else(|| dir.join("program"));

    run_linker(&obj_path, &exe_path, config.runtime_lib_dir.as_deref())?;

    // Clean up intermediates (keep the output binary).
    let _ = std::fs::remove_file(&ll_path);
    let _ = std::fs::remove_file(&bc_path);
    let _ = std::fs::remove_file(&obj_path);

    Ok(PipelineResult::EmittedBinary { path: exe_path })
}

/// Compile LLVM IR and execute the resulting binary immediately.
pub fn compile_and_run(config: &PipelineConfig) -> Result<PipelineResult, PipelineError> {
    let result = compile_to_binary(config)?;
    let PipelineResult::EmittedBinary { ref path } = result else {
        unreachable!()
    };

    let output = Command::new(path).status()?;
    let exit_code = output.code().unwrap_or(1);

    // Clean up the temporary binary.
    let _ = std::fs::remove_file(path);
    // Clean up the temp directory.
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }

    Ok(PipelineResult::Executed { exit_code })
}

/// Build `libflux_rt.a` from the C source files if it doesn't exist.
///
/// This mirrors what `make` does in `runtime/c/`, but runs automatically
/// so users never need to build the C runtime manually.
pub fn ensure_runtime_lib(runtime_c_dir: &Path) -> Result<(), PipelineError> {
    let lib_path = runtime_c_dir.join("libflux_rt.a");
    if lib_path.exists() {
        // Check if any .c or .h file is newer than the .a
        let lib_mtime = std::fs::metadata(&lib_path).and_then(|m| m.modified()).ok();
        let sources_newer = lib_mtime.map_or(true, |lib_t| {
            let mut newer = false;
            for ext in &["c", "h"] {
                if let Ok(entries) = std::fs::read_dir(runtime_c_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                            if let Ok(src_t) = std::fs::metadata(&p).and_then(|m| m.modified()) {
                                if src_t > lib_t {
                                    newer = true;
                                }
                            }
                        }
                    }
                }
            }
            newer
        });
        if !sources_newer {
            return Ok(());
        }
    }

    eprintln!("[c2l] Building C runtime (libflux_rt.a)...");

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let c_files = [
        "gc.c",
        "flux_rt.c",
        "string.c",
        "hamt.c",
        "effects.c",
        "array.c",
    ];
    let mut obj_files = Vec::new();

    for c_file in &c_files {
        let src = runtime_c_dir.join(c_file);
        if !src.exists() {
            continue;
        }
        let obj = runtime_c_dir.join(c_file.replace(".c", ".o"));
        let output = Command::new(&cc)
            .args(["-std=c11", "-Wall", "-O2", "-g", "-c"])
            .arg("-o")
            .arg(&obj)
            .arg(&src)
            .arg(format!("-I{}", runtime_c_dir.display()))
            .output()?;
        check_output("cc (runtime)", &output)?;
        obj_files.push(obj);
    }

    // Create static library
    let ar = std::env::var("AR").unwrap_or_else(|_| "ar".into());
    let mut cmd = Command::new(&ar);
    cmd.args(["rcs"]).arg(&lib_path);
    for obj in &obj_files {
        cmd.arg(obj);
    }
    let output = cmd.output()?;
    check_output("ar", &output)?;

    eprintln!("[c2l] Built {}", lib_path.display());
    Ok(())
}

/// Locate an LLVM tool, searching:
/// 1. `<tool>` on PATH
/// 2. Versioned names on PATH: `<tool>-18`, `<tool>-17`, `<tool>-16`
/// 3. Well-known install directories (Linux, macOS Homebrew, Windows)
fn find_tool(name: &'static str) -> Result<PathBuf, PipelineError> {
    // 1. Exact name on PATH
    if let Some(p) = which(name) {
        return Ok(p);
    }

    // 3. Versioned names on PATH (prefer newest)
    for ver in &["18", "17", "16"] {
        let versioned = format!("{name}-{ver}");
        if let Some(p) = which(&versioned) {
            return Ok(p);
        }
    }

    // 4. Well-known install directories
    let well_known: &[&str] = &[
        // Linux (apt.llvm.org packages)
        "/usr/lib/llvm-18/bin",
        "/usr/lib/llvm-17/bin",
        "/usr/lib/llvm-16/bin",
        // macOS Homebrew (Apple Silicon + Intel)
        "/opt/homebrew/opt/llvm@18/bin",
        "/opt/homebrew/opt/llvm/bin",
        "/usr/local/opt/llvm@18/bin",
        "/usr/local/opt/llvm/bin",
        // Windows (typical LLVM install)
        "C:\\Program Files\\LLVM\\bin",
    ];
    for dir in well_known {
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(PipelineError::ToolNotFound {
        tool: name,
        detail: format!(
            "`{name}` not found on PATH or in well-known LLVM directories. \
             Install LLVM (e.g., `apt install llvm-18` or `brew install llvm@18`)."
        ),
    })
}

/// Simple `which`-style lookup.
fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

/// Check subprocess output and return an error if it failed.
fn check_output(tool: &'static str, output: &std::process::Output) -> Result<(), PipelineError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(PipelineError::ToolFailed {
            tool,
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
