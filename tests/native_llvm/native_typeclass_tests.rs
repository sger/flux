//! Native/VM regression coverage for module-qualified generated typeclass methods.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

static TYPECLASS_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_fixture(native: bool) -> (String, String, bool) {
    let _guard = TYPECLASS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("typeclass test lock poisoned");
    let scratch = Scratch::new(if native {
        "native-dotted-typeclass"
    } else {
        "vm-dotted-typeclass"
    });
    scratch.write(
        "Data/Enc.flx",
        r#"
module Data.Enc {
    public class Encodable<a> {
        fn enc(x: a) -> String
    }

    fn render(x: Int) -> String { to_string(x) }

    public instance Encodable<Int> {
        fn enc(x) { render(x) }
    }
}
"#,
    );
    let entry = scratch.write(
        "main.flx",
        r#"
import Data.Enc exposing (enc)

fn main() with IO {
    print(enc(42))
}
"#,
    );

    let mut args = vec![
        entry.to_string_lossy().into_owned(),
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
        .expect("run dotted typeclass fixture");

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn dotted_module_instance_dispatches_on_vm_and_llvm() {
    let (vm_stdout, vm_stderr, vm_ok) = run_fixture(false);
    let (native_stdout, native_stderr, native_ok) = run_fixture(true);

    assert!(
        vm_ok,
        "dotted typeclass fixture failed on VM:\nstdout:\n{vm_stdout}\nstderr:\n{vm_stderr}"
    );
    assert!(
        native_ok,
        "dotted typeclass fixture failed natively:\nstdout:\n{native_stdout}\nstderr:\n{native_stderr}"
    );
    assert!(vm_stdout.contains("42"), "VM output was:\n{vm_stdout}");
    assert!(
        native_stdout.contains("42"),
        "native output was:\n{native_stdout}"
    );
}
