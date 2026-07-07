//! Native cross-module async yield-check regression tests (KI-1).
//!
//! A user-defined `async` function defined in one module and called from
//! another must emit a `flux_is_yielding` check at its native call site. Before
//! the fix, cross-module async-ness was decided by a hardcoded `Flow.*`
//! allowlist (`is_direct_async_extern_symbol`), so a user-defined async import
//! was treated as non-suspending: when the callee suspended, the caller
//! dereferenced the yield sentinel → SIGSEGV. The VM was unaffected.
//!
//! The fix makes async-ness data-driven from the callee's known effect row
//! (`ImportedNativeSymbol::is_async` → `LirProgram::async_extern_symbols`).

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Write `files` (name → source) into a fresh scratch directory and return the
/// path to the entry file (`main.flx`).
fn write_module_fixture(tag: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!(
            "flux-native-async-crossmod-{}-{}",
            std::process::id(),
            tag,
        ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    for (name, source) in files {
        std::fs::write(dir.join(name), source).expect("write fixture");
    }
    (dir.join("main.flx"), dir)
}

fn run(entry: &Path, native: bool) -> (String, String, bool) {
    let mut args = vec![
        entry.to_str().unwrap().to_string(),
        "--no-cache".to_string(),
    ];
    if native {
        args.push("--native".to_string());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(&args)
        .output()
        .expect("run flux");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    (stdout, stderr, output.status.success())
}

/// The minimal KI-1 repro: a generic user-defined async combinator in `Lib`
/// called from `main`, whose callback suspends via `yield_now`.
#[test]
fn native_cross_module_user_async_gets_yield_check() {
    let lib = r#"module Lib {
    public fn step<e>(f: () -> Int with Suspend, Fork, GetContext, AsyncFail | e)
        -> Int with Suspend, Fork, GetContext, AsyncFail | e { let r = f(); r }
}
"#;
    let main = r#"import Flow.Async exposing (..)
import Lib exposing (..)
fn body() -> Int with Async { step(fn() with Async { yield_now(); 7 }) }
fn main() with IO { print(run_async(body)) }
"#;
    let (entry, dir) = write_module_fixture("step", &[("Lib.flx", lib), ("main.flx", main)]);

    let (vm_out, vm_err, vm_ok) = run(&entry, false);
    assert!(
        vm_ok,
        "VM run must succeed:\nstdout:\n{vm_out}\nstderr:\n{vm_err}"
    );
    assert_eq!(vm_out.trim(), "7", "VM output");

    let (nat_out, nat_err, nat_ok) = run(&entry, true);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        nat_ok,
        "native run must succeed (KI-1: cross-module async call missed its \
         yield check → SIGSEGV):\nstdout:\n{nat_out}\nstderr:\n{nat_err}"
    );
    assert_eq!(nat_out.trim(), "7", "native output must match VM");
}

/// A cross-module async function that itself suspends (via `sleep`) rather than
/// through a callback — exercises the same yield-check gap on a non-generic,
/// non-higher-order shape.
#[test]
fn native_cross_module_direct_async_gets_yield_check() {
    let lib = r#"import Flow.Async exposing (..)
module Lib {
    public fn napped(n: Int) -> Int with Async { sleep(n); n + 1 }
}
"#;
    let main = r#"import Flow.Async exposing (..)
import Lib exposing (..)
fn body() -> Int with Async { napped(5) }
fn main() with IO { print(run_async(body)) }
"#;
    let (entry, dir) = write_module_fixture("napped", &[("Lib.flx", lib), ("main.flx", main)]);

    let (vm_out, _vm_err, vm_ok) = run(&entry, false);
    assert!(vm_ok, "VM run must succeed");
    assert_eq!(vm_out.trim(), "6", "VM output");

    let (nat_out, nat_err, nat_ok) = run(&entry, true);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        nat_ok,
        "native run must succeed:\nstdout:\n{nat_out}\nstderr:\n{nat_err}"
    );
    assert_eq!(nat_out.trim(), "6", "native output must match VM");
}
