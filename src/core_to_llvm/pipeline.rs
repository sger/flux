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
    let output = Command::new(&llc)
        .arg(bc_path)
        .arg("-o")
        .arg(obj_path)
        .arg("--filetype=obj")
        .output()?;
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

    run_linker(
        &obj_path,
        &exe_path,
        config.runtime_lib_dir.as_deref(),
    )?;

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

/// Locate a tool on PATH.
fn find_tool(name: &'static str) -> Result<PathBuf, PipelineError> {
    which(name).ok_or_else(|| PipelineError::ToolNotFound {
        tool: name,
        detail: format!("`{name}` not found on PATH. Install LLVM to use --core-to-llvm."),
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
