//! Host target triple and data layout detection.
//!
//! Detects the host triple by querying `llvm-config` (preferred) or
//! falling back to compile-time `cfg` attributes.

use std::process::Command;

/// Detect the host target triple.
///
/// Uses compile-time `cfg` attributes to match the architecture of the
/// Flux binary itself.  This is more reliable than `llvm-config` which
/// may report a different architecture (e.g. a 32-bit LLVM build on a
/// 64-bit host).
pub fn host_triple() -> String {
    compile_time_triple()
}

/// Detect the data layout string for the host target.
///
/// Tries `llc --version` output (which prints the default data layout).
/// Returns `None` if detection fails — the generated `.ll` will omit
/// `target datalayout` and LLVM will infer it.
pub fn host_data_layout() -> Option<String> {
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        Some(
            "e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
                .into(),
        )
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        Some(
            "e-m:o-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
                .into(),
        )
    }
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        Some(
            "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
                .into(),
        )
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        Some("e-m:o-i64:64-i128:128-n32:64-S128-Fn32".into())
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        Some("e-m:e-i8:8:32-i16:16:32-i64:64-i128:128-n32:64-S128-Fn32".into())
    }
    #[cfg(not(any(
        all(
            target_arch = "x86_64",
            any(target_os = "windows", target_os = "macos", target_os = "linux")
        ),
        all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux"))
    )))]
    {
        None
    }
}

#[allow(dead_code)]
fn llvm_config_triple() -> Option<String> {
    let output = Command::new("llvm-config")
        .arg("--host-target")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if triple.is_empty() {
        None
    } else {
        Some(triple)
    }
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
