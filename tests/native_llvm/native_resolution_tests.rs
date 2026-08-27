//! Native/VM parity regressions for qualified imported-member resolution.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

static NATIVE_RESOLUTION_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, native: bool) -> (String, String, bool) {
    let _guard = NATIVE_RESOLUTION_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native resolution test lock poisoned");

    let scratch = Scratch::new(if native {
        "native-qualified-resolution"
    } else {
        "vm-qualified-resolution"
    });
    let path = scratch.write("fixture.flx", source);
    let mut args = vec![
        path.to_string_lossy().into_owned(),
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
        .expect("run qualified-resolution fixture");

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn qualified_imported_member_beats_same_named_local_on_both_backends() {
    let source = r#"
import Flow.Json as Json

fn as_string(found: Json, key: String) -> String {
    match Json.as_string(found, key) {
        JsonErr(_) -> "err",
        JsonOk(text) -> text,
    }
}

fn main() with IO {
    println(as_string(Json.string("hi"), "k"))
}
"#;

    let (vm_stdout, vm_stderr, vm_ok) = run_source(source, false);
    let (native_stdout, native_stderr, native_ok) = run_source(source, true);

    assert!(
        vm_ok,
        "qualified-resolution fixture failed on the VM:\nstdout:\n{vm_stdout}\nstderr:\n{vm_stderr}"
    );
    assert!(
        native_ok,
        "qualified-resolution fixture failed natively:\nstdout:\n{native_stdout}\nstderr:\n{native_stderr}"
    );
    assert!(vm_stdout.contains("hi"), "VM output was:\n{vm_stdout}");
    assert!(
        native_stdout.contains("hi"),
        "native output was:\n{native_stdout}"
    );
}
