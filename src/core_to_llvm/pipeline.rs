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
        .arg("--filetype=obj")
        .arg(format!("--mtriple={}", super::target::host_triple()));
    // macOS Mach-O is position-independent by default; Linux and Windows
    // require PIC for PIE executables (the default linker mode).
    if !cfg!(target_os = "macos") {
        cmd.arg("--relocation-model=pic");
    }
    let output = cmd.output()?;
    check_output("llc", &output)
}

/// Link `.o`/`.obj` + runtime library → executable.
///
/// On Unix: uses `cc` (system linker).
/// On Windows: tries `clang` first, then falls back to MSVC `link.exe`.
fn run_linker(
    obj_path: &Path,
    exe_path: &Path,
    runtime_lib_dir: Option<&Path>,
) -> Result<(), PipelineError> {
    let toolchain = detect_c_toolchain()?;

    match &toolchain {
        CToolchain::Msvc { .. } => {
            let exe_path_with_ext = if exe_path.extension().is_none() {
                exe_path.with_extension("exe")
            } else {
                exe_path.to_path_buf()
            };
            let mut cmd = Command::new("link");
            cmd.args(["/nologo", "/subsystem:console"])
                .arg(format!("/OUT:{}", exe_path_with_ext.display()))
                .arg(obj_path);
            // Set large stack size for deeply recursive programs.
            cmd.arg("/STACK:67108864"); // 64 MB stack
            if let Some(dir) = runtime_lib_dir {
                cmd.arg(format!("/LIBPATH:{}", dir.display()));
                cmd.arg("flux_rt.lib");
            }
            // Link the C runtime and kernel libraries.
            cmd.args(["libcmt.lib", "kernel32.lib"]);
            let output = cmd.output()?;
            check_output("link", &output)
        }
        CToolchain::Gcc { cc, .. } => {
            let exe_path_final = if cfg!(windows) && exe_path.extension().is_none() {
                exe_path.with_extension("exe")
            } else {
                exe_path.to_path_buf()
            };
            let mut cmd = Command::new(cc);
            cmd.arg(obj_path).arg("-o").arg(&exe_path_final);
            if let Some(dir) = runtime_lib_dir {
                cmd.arg(format!("-L{}", dir.display()));
                cmd.arg("-lflux_rt");
            }
            if cfg!(windows) {
                // Windows: set subsystem and stack size via lld-link.
                cmd.args(["-Wl,/subsystem:console", "-Wl,/STACK:67108864"]);
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
    }
}

/// Compile LLVM IR to a native binary.
///
/// Steps: `.ll` → `opt` → `.bc` → `llc` → `.o` → `cc` → executable.
pub fn compile_to_binary(config: &PipelineConfig) -> Result<PipelineResult, PipelineError> {
    // Use target/native/ inside the project (if available) to avoid Windows
    // Application Control policies that block unsigned executables from temp dirs.
    let base_dir = std::env::current_dir()
        .ok()
        .map(|d| d.join("target").join("native"))
        .filter(|d| d.parent().is_some_and(|p| p.exists()))
        .unwrap_or_else(|| std::env::temp_dir().join("flux_core_to_llvm"));
    // Clean up stale builds from previous runs to avoid WDAC caching issues.
    if base_dir.exists() {
        let _ = std::fs::remove_dir_all(&base_dir);
    }
    let dir = base_dir.join(format!("flux_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let ll_path = dir.join("program.ll");
    let bc_path = dir.join("program.bc");
    let obj_path = dir.join(if cfg!(windows) {
        "program.obj"
    } else {
        "program.o"
    });

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

/// Detected C toolchain on the current system.
enum CToolchain {
    /// GCC/Clang-compatible: `cc`/`clang` + `ar`/`llvm-ar` (Unix flags).
    Gcc { cc: String, ar: String },
    /// MSVC: `cl.exe` + `lib.exe` (MSVC flags, Developer Command Prompt).
    Msvc { cc: String, lib_tool: String },
}

/// Detect the available C toolchain.
///
/// Priority:
/// 1. `CC`/`AR` environment variables (user override).
/// 2. On Windows: `clang` + `llvm-ar` (works from any terminal).
/// 3. On Windows: `cl.exe` + `lib.exe` (requires Developer Command Prompt).
/// 4. On Unix: `cc` + `ar` (always available).
fn detect_c_toolchain() -> Result<CToolchain, PipelineError> {
    // 1. User override via environment variables.
    if let Ok(cc) = std::env::var("CC") {
        let is_msvc = cc == "cl" || cc == "cl.exe";
        if is_msvc {
            let lib_tool = std::env::var("AR").unwrap_or_else(|_| "lib".into());
            return Ok(CToolchain::Msvc { cc, lib_tool });
        }
        let ar = std::env::var("AR").unwrap_or_else(|_| "ar".into());
        return Ok(CToolchain::Gcc { cc, ar });
    }

    if cfg!(windows) {
        // 2. Try clang + llvm-ar (winget LLVM, any terminal).
        if which("clang").is_some() {
            let ar = if which("llvm-ar").is_some() {
                "llvm-ar".into()
            } else {
                "ar".into()
            };
            return Ok(CToolchain::Gcc {
                cc: "clang".into(),
                ar,
            });
        }
        // 3. Try MSVC cl.exe (Developer Command Prompt).
        if which("cl").is_some() {
            return Ok(CToolchain::Msvc {
                cc: "cl".into(),
                lib_tool: "lib".into(),
            });
        }
        return Err(PipelineError::ToolNotFound {
            tool: "cc",
            detail: "No C compiler found. Either install LLVM (`winget install LLVM.LLVM`) \
                     or run from a Visual Studio Developer Command Prompt."
                .into(),
        });
    }

    // 4. Unix: cc + ar.
    Ok(CToolchain::Gcc {
        cc: "cc".into(),
        ar: "ar".into(),
    })
}

/// Build the Flux C runtime as a static library if it doesn't exist.
///
/// This mirrors what `make` does in `runtime/c/`, but runs automatically
/// so users never need to build the C runtime manually.
///
/// On Unix: produces `libflux_rt.a` using `cc` + `ar`.
/// On Windows: produces `flux_rt.lib` using `clang` + `llvm-ar` or `cl.exe` + `lib.exe`.
pub fn ensure_runtime_lib(runtime_c_dir: &Path) -> Result<(), PipelineError> {
    let toolchain = detect_c_toolchain()?;

    let lib_name = match &toolchain {
        CToolchain::Msvc { .. } => "flux_rt.lib",
        CToolchain::Gcc { .. } if cfg!(windows) => "flux_rt.lib",
        CToolchain::Gcc { .. } => "libflux_rt.a",
    };
    let lib_path = runtime_c_dir.join(lib_name);

    if lib_path.exists() {
        // Check if any .c or .h file is newer than the library
        let lib_mtime = std::fs::metadata(&lib_path).and_then(|m| m.modified()).ok();
        let sources_newer = lib_mtime.is_none_or(|lib_t| {
            let mut newer = false;
            for ext in &["c", "h"] {
                if let Ok(entries) = std::fs::read_dir(runtime_c_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some(ext)
                            && let Ok(src_t) = std::fs::metadata(&p).and_then(|m| m.modified())
                            && src_t > lib_t
                        {
                            newer = true;
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

    eprintln!("[c2l] Building C runtime ({lib_name})...");

    let c_files = [
        "rc.c",
        "flux_rt.c",
        "string.c",
        "hamt.c",
        "effects.c",
        "array.c",
    ];
    let mut obj_files = Vec::new();

    match &toolchain {
        CToolchain::Msvc { cc, lib_tool } => {
            for c_file in &c_files {
                let src = runtime_c_dir.join(c_file);
                if !src.exists() {
                    continue;
                }
                let obj = runtime_c_dir.join(c_file.replace(".c", ".obj"));
                let output = Command::new(cc)
                    .args(["/nologo", "/c", "/O2", "/W3"])
                    .arg(format!("/Fo{}", obj.display()))
                    .arg(&src)
                    .arg(format!("/I{}", runtime_c_dir.display()))
                    .output()?;
                check_output("cl (runtime)", &output)?;
                obj_files.push(obj);
            }

            let mut cmd = Command::new(lib_tool);
            cmd.args(["/nologo"])
                .arg(format!("/OUT:{}", lib_path.display()));
            for obj in &obj_files {
                cmd.arg(obj);
            }
            let output = cmd.output()?;
            check_output("lib (runtime)", &output)?;
        }
        CToolchain::Gcc { cc, ar } => {
            let obj_ext = if cfg!(windows) { ".obj" } else { ".o" };
            for c_file in &c_files {
                let src = runtime_c_dir.join(c_file);
                if !src.exists() {
                    continue;
                }
                let obj = runtime_c_dir.join(c_file.replace(".c", obj_ext));
                let output = Command::new(cc)
                    .args(["-std=c11", "-Wall", "-O2", "-g", "-c"])
                    .arg("-o")
                    .arg(&obj)
                    .arg(&src)
                    .arg(format!("-I{}", runtime_c_dir.display()))
                    .output()?;
                check_output("cc (runtime)", &output)?;
                obj_files.push(obj);
            }

            let mut cmd = Command::new(ar);
            cmd.args(["rcs"]).arg(&lib_path);
            for obj in &obj_files {
                cmd.arg(obj);
            }
            let output = cmd.output()?;
            check_output("ar", &output)?;
        }
    }

    // Clean up object files
    for obj in &obj_files {
        let _ = std::fs::remove_file(obj);
    }

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
        if cfg!(windows) {
            let candidate_exe = PathBuf::from(dir).join(format!("{name}.exe"));
            if candidate_exe.is_file() {
                return Ok(candidate_exe);
            }
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
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            names.iter().find_map(|n| {
                let candidate = dir.join(n);
                candidate.is_file().then_some(candidate)
            })
        })
    })
}

/// Check subprocess output and return an error if it failed.
fn check_output(tool: &'static str, output: &std::process::Output) -> Result<(), PipelineError> {
    if output.status.success() {
        Ok(())
    } else {
        // MSVC tools (cl, link, lib) write diagnostics to stdout, not stderr.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = if stderr.trim().is_empty() {
            stdout.into_owned()
        } else {
            stderr.into_owned()
        };
        Err(PipelineError::ToolFailed {
            tool,
            exit_code: output.status.code(),
            stderr: combined,
        })
    }
}
