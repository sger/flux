//! Native backend pipeline entrypoints and runtime-support helpers.

#![cfg_attr(not(feature = "llvm"), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};

#[cfg(feature = "llvm")]
use crate::{
    llvm::{
        pipeline::{compile_ir_to_object, ensure_runtime_lib},
        render_module, target,
    },
    lir::{LirProgram, emit_llvm::emit_llvm_module_with_options},
};
use crate::{
    diagnostics::Diagnostic,
    shared::cache_paths::CacheLayout,
    syntax::{interner::Interner, module_graph::ModuleGraph},
};

#[cfg(feature = "llvm")]
mod parallel;

/// Returns runtime library candidate directories in lookup order.
fn runtime_lib_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..5 {
            if let Some(ref d) = dir {
                candidates.push(d.join("runtime").join("c"));
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }
    candidates.push(PathBuf::from("runtime/c"));
    candidates
}

/// Locates a built native runtime library directory if one is available.
pub(crate) fn locate_runtime_lib_dir() -> Option<std::path::PathBuf> {
    let candidates = runtime_lib_candidates();
    for candidate in &candidates {
        if candidate.join("flux_rt.h").exists() {
            #[cfg(feature = "native")]
            if let Err(e) = ensure_runtime_lib(candidate) {
                eprintln!("Warning: failed to build C runtime: {e}");
            }
            let lib_exists = if cfg!(windows) {
                candidate.join("flux_rt.lib").exists()
            } else {
                candidate.join("libflux_rt.a").exists()
            };
            if lib_exists {
                return Some(candidate.clone());
            }
        }
    }
    None
}

#[cfg(feature = "llvm")]
/// Creates a unique temporary directory path for uncached native artifacts.
fn native_temp_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("flux_native_{}_{}", std::process::id(), stamp))
}

#[cfg(feature = "llvm")]
/// Builds or reuses the shared native support object used by native linking.
pub(crate) fn compile_native_support_object(
    cache_layout: &CacheLayout,
    no_cache: bool,
    enable_optimize: bool,
) -> Result<PathBuf, String> {
    let object_path = if no_cache {
        let dir = native_temp_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(if cfg!(windows) {
            "flux_support.obj"
        } else {
            "flux_support.o"
        })
    } else {
        let dir = cache_layout.native_dir();
        let _ = std::fs::create_dir_all(&dir);
        dir.join(if enable_optimize {
            if cfg!(windows) {
                "flux_support_O2.obj"
            } else {
                "flux_support_O2.o"
            }
        } else if cfg!(windows) {
            "flux_support_O0.obj"
        } else {
            "flux_support_O0.o"
        })
    };

    if !no_cache && object_path.exists() {
        return Ok(object_path);
    }

    let lir = LirProgram::new();
    let mut llvm_module = emit_llvm_module_with_options(&lir, true, false);
    llvm_module.target_triple = Some(target::host_triple());
    llvm_module.data_layout = target::host_data_layout();
    let ll_text = render_module(&llvm_module);
    compile_ir_to_object(&ll_text, &object_path, if enable_optimize { 2 } else { 0 })
        .map_err(|err| format!("native support object compilation failed: {err}"))?;
    Ok(object_path)
}

#[cfg(feature = "llvm")]
/// Grouped inputs for native parallel module compilation.
pub(crate) struct NativeParallelCompileRequest<'a> {
    pub(crate) graph: &'a ModuleGraph,
    pub(crate) cache_layout: &'a CacheLayout,
    pub(crate) no_cache: bool,
    pub(crate) strict_mode: bool,
    pub(crate) strict_types: bool,
    pub(crate) enable_optimize: bool,
    pub(crate) enable_analyze: bool,
    pub(crate) verbose: bool,
    pub(crate) base_interner: &'a Interner,
}

#[cfg(feature = "llvm")]
/// Compiles a module graph into per-module native objects in dependency order.
pub(crate) fn compile_native_modules_parallel(
    request: NativeParallelCompileRequest<'_>,
    all_diagnostics: &mut Vec<Diagnostic>,
) -> Result<(Vec<PathBuf>, bool), String> {
    parallel::compile_native_modules_parallel(request, all_diagnostics)
}

#[cfg(test)]
mod tests {
    use super::runtime_lib_candidates;

    #[test]
    fn runtime_lib_candidates_include_repo_relative_fallback() {
        let candidates = runtime_lib_candidates();

        assert!(
            candidates
                .iter()
                .any(|candidate| candidate == std::path::Path::new("runtime/c"))
        );
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn native_temp_dir_uses_flux_native_prefix() {
        let dir = super::native_temp_dir();
        let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");

        assert!(name.starts_with("flux_native_"));
    }
}
