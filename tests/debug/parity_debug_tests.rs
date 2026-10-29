//! Debug parity (proposal 0188 S6): each fixture in `tests/parity_debug/`
//! scripts a debugging session in its `// dbg:` lines. The session runs in
//! the VM debugger and, on a `-g` native build, in lldb; the two reduced
//! transcripts must agree (`flux::parity::debug::outputs_match`).
//!
//! `flux parity-check tests/parity_debug --ways vm_debug,llvm_debug` runs
//! the same check. A machine without lldb skips with a note; CI installs it.

#![cfg(all(feature = "llvm", unix))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use flux::parity::debug::{NATIVE_DEBUG_SKIPPED, outputs_match, run_debug_way};
use flux::parity::{ExitKind, Way};

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity_debug");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("tests/parity_debug")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "flx"))
        .collect();
    files.sort();
    files
}

#[test]
fn the_vm_and_native_debuggers_agree() {
    let flux = Path::new(env!("CARGO_BIN_EXE_flux"));
    let timeout = Duration::from_secs(60);
    let files = fixtures();
    assert!(!files.is_empty(), "no debug parity fixtures");
    // Each fixture is a full native build, and building them all at once
    // starves a small runner past the build timeout.
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(files.len());
    let next = AtomicUsize::new(0);
    let mut runs: Vec<_> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                threads.spawn(|| {
                    let mut done = Vec::new();
                    while let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let vm = run_debug_way(flux, flux, file, Way::VmDebug, &[], timeout);
                        let native = run_debug_way(flux, flux, file, Way::LlvmDebug, &[], timeout);
                        done.push((file, vm, native));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().expect("a fixture worker")).collect()
    });
    runs.sort_by_key(|(file, _, _)| *file);
    let failures: Vec<String> = runs
        .into_iter()
        .filter_map(|(file, vm, native)| {
            if native.stderr.contains(NATIVE_DEBUG_SKIPPED) {
                eprintln!("note: {}; skipping {}", native.stderr, file.display());
                return None;
            }
            let agree = vm.exit_kind == ExitKind::Success
                && native.exit_kind == ExitKind::Success
                && outputs_match(&vm, &native);
            (!agree).then(|| {
                format!(
                    "{}:\n--- vm_debug ({})\n{}{}\n--- llvm_debug ({})\n{}{}",
                    file.display(),
                    vm.exit_kind,
                    vm.normalized_stdout,
                    vm.stderr,
                    native.exit_kind,
                    native.normalized_stdout,
                    native.stderr
                )
            })
        })
        .collect();
    assert!(failures.is_empty(), "the debuggers disagree:\n\n{}", failures.join("\n\n"));
}
