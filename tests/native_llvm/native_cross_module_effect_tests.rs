//! Native/VM parity: a `perform` in one module must unwind correctly through
//! callers compiled in another (KI-034).
//!
//! A yield unwinds by returning `FLUX_YIELD_SENTINEL` (the raw value `10`) up
//! the stack, so every caller must test `flux_is_yielding` after the call and
//! propagate rather than use the result. Which call sites get that check was
//! decided per module: `direct_async_func_ids` reasons over `LirFuncId`s,
//! which are module-local, and the only cross-module answer was a hardcoded
//! `Flow_Async_*` allowlist that no user-defined effect could match.
//!
//! So a caller in another module ran straight on with the sentinel in hand and
//! eventually dereferenced it as a pointer — SIGSEGV, or SIGBUS depending on
//! where the value landed. The VM was always correct, so this was also a
//! parity divergence.
//!
//! The fixture keeps the effect and its `perform` in `Effectful.flx` and the
//! handler in the entry module, because the bug reproduces *only* across that
//! boundary: the same code with both in one module always worked, which is why
//! a single-file fixture proves nothing here.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

static CROSS_MODULE_EFFECT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// The effect and the `perform`. Separate module on purpose — see the note
/// above.
const EFFECT_MODULE: &str = r#"module Effectful {
    effect MyFail {
        boom: String -> Unit
    }

    public fn abort_it(msg: String) -> String with MyFail {
        perform MyFail.boom(msg)
        ""
    }
}
"#;

/// Two frames between the `perform` and the handler, so the sentinel has to
/// propagate through more than one return. `trim` on the result is what
/// actually dereferenced the sentinel before the fix.
const ENTRY: &str = r#"import Flow.Result exposing (Result, Ok, Err)
import Effectful as Effectful

fn parse_it(s: String) -> String with MyFail {
    if len(s) < 100 { Effectful.abort_it("bad") } else { s }
}

fn from_line(line: String) -> Int with MyFail {
    let v = parse_it(trim(line))
    len(trim(v))
}

fn go(line: String) -> Result<Int, String> {
    Ok(from_line(line)) handle MyFail { boom(resume, m) -> Err(m) }
}

fn main() with IO {
    match go("not json") {
        Ok(n) -> println(to_string(n)),
        Err(e) -> println("err: " + e),
    }
}
"#;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run(native: bool) -> (String, bool) {
    let _guard = CROSS_MODULE_EFFECT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("cross-module effect test lock poisoned");

    let scratch = Scratch::new(if native {
        "native-cross-module-effect"
    } else {
        "vm-cross-module-effect"
    });
    scratch.write("Effectful.flx", EFFECT_MODULE);
    let entry = scratch.write("main.flx", ENTRY);
    let dir = scratch.path().to_string_lossy().into_owned();

    let mut args = vec![
        entry.to_string_lossy().into_owned(),
        "--root".to_string(),
        dir,
        "--no-cache".to_string(),
    ];
    args.extend(scratch.cache_args());
    if native {
        args.push("--native".to_string());
    }

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(args)
        .output()
        .expect("run cross-module effect fixture");

    // Both streams: a crash reports on stderr, so a stdout-only capture would
    // show the failure as unexplained empty output.
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    (format!("{stdout}\n{stderr}"), output.status.success())
}

#[test]
fn a_perform_in_another_module_unwinds_to_the_handler_on_both_backends() {
    let (vm_out, vm_ok) = run(false);
    assert!(vm_ok, "VM run failed:\n{vm_out}");
    assert!(
        vm_out.contains("bad"),
        "VM must reach the handler and surface the message:\n{vm_out}"
    );

    let (native_out, native_ok) = run(true);
    // Asserted before `native_ok` so the crash itself is the reported failure
    // rather than a bare non-zero exit.
    assert!(
        !native_out.contains("signal"),
        "the native program must not crash — the yield sentinel escaped into \
         straight-line code:\n{native_out}"
    );
    assert!(native_ok, "native run failed:\n{native_out}");
    assert!(
        native_out.contains("bad"),
        "native must reach the handler and surface the message:\n{native_out}"
    );
}
