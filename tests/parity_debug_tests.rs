//! Debug parity (proposal 0188 S6): each fixture in `tests/parity_debug/`
//! scripts a debugging session in its `// dbg:` lines. The session runs in
//! the VM debugger and, on a `-g` native build, in lldb; the two reduced
//! transcripts must agree (`flux::parity::debug::outputs_match`).
//!
//! `flux parity-check tests/parity_debug --ways vm_debug,llvm_debug` runs
//! the same check. A machine without lldb skips with a note; CI installs it.

#![cfg(all(feature = "llvm", unix))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use flux::parity::debug::{NO_LLDB, outputs_match, run_debug_way};
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
    let failures: Vec<String> = std::thread::scope(|threads| {
        let runs: Vec<_> = files
            .iter()
            .map(|file| {
                threads.spawn(move || {
                    let vm = run_debug_way(flux, flux, file, Way::VmDebug, &[], timeout);
                    let native = run_debug_way(flux, flux, file, Way::LlvmDebug, &[], timeout);
                    (file, vm, native)
                })
            })
            .collect();
        runs.into_iter()
            .filter_map(|run| {
                let (file, vm, native) = run.join().expect("a fixture's thread");
                if native.stderr.contains(NO_LLDB) {
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
            .collect()
    });
    assert!(
        failures.is_empty(),
        "the debuggers disagree:\n\n{}",
        failures.join("\n\n")
    );
}
