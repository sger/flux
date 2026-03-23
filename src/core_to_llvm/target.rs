//! Host target triple and data layout detection.
//!
//! Detects the host triple by querying `llvm-config` (preferred) or
//! falling back to compile-time `cfg` attributes.

use std::process::Command;

/// Detect the host target triple.
///
/// Tries `llvm-config --host-target` first.  Falls back to a
/// compile-time guess based on `cfg!(target_arch/os/env)`.
pub fn host_triple() -> String {
    if let Some(triple) = llvm_config_triple() {
        return triple;
    }
    compile_time_triple()
}

/// Detect the data layout string for the host target.
///
/// Tries `llc --version` output (which prints the default data layout).
/// Returns `None` if detection fails — the generated `.ll` will omit
/// `target datalayout` and LLVM will infer it.
pub fn host_data_layout() -> Option<String> {
    // Common default for x86_64.
    #[cfg(target_arch = "x86_64")]
    {
        Some("e-m:o-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128".into())
    }
    #[cfg(target_arch = "aarch64")]
    {
        Some("e-m:o-i64:64-i128:128-n32:64-S128-Fn32".into())
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

fn llvm_config_triple() -> Option<String> {
    let output = Command::new("llvm-config")
        .arg("--host-target")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if triple.is_empty() { None } else { Some(triple) }
}

fn compile_time_triple() -> String {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "i386"
    } else {
        "unknown"
    };

    let os = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "linux") {
        "unknown-linux-gnu"
    } else if cfg!(target_os = "windows") {
        "pc-windows-msvc"
    } else {
        "unknown-unknown"
    };

    format!("{arch}-{os}")
}
