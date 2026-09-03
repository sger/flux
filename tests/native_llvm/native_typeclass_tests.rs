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

fn run_module_scoped_contextual_instance_fixture(native: bool) -> (String, String, bool) {
    let _guard = TYPECLASS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("typeclass test lock poisoned");
    let scratch = Scratch::new(if native {
        "native-module-scoped-contextual-instance"
    } else {
        "vm-module-scoped-contextual-instance"
    });
    scratch.write(
        "M.flx",
        r#"
module M {
    public class MyEq<a> { fn my_eq(x: a, y: a) -> Bool }

    public instance MyEq<Int> { fn my_eq(x, y) { x == y } }

    public instance MyEq<a> => MyEq<List<a>> {
        fn my_eq(xs, ys) {
            match xs {
                [h1 | t1] -> match ys { [h2 | t2] -> my_eq(h1, h2) && my_eq(t1, t2), _ -> false },
                _ -> match ys { [h | t] -> false, _ -> true }
            }
        }
    }
}
"#,
    );
    let entry = scratch.write(
        "main.flx",
        r#"
import M

fn main() with IO {
    print(M.my_eq([1], [1]))
    print(M.my_eq([1, 2], [1, 2]))
    print(M.my_eq([1, 2], [1, 3]))
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
        .expect("run module-scoped contextual instance fixture");

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn module_scoped_contextual_instance_recurses_on_vm_and_llvm() {
    let (vm_stdout, vm_stderr, vm_ok) = run_module_scoped_contextual_instance_fixture(false);
    let (native_stdout, native_stderr, native_ok) =
        run_module_scoped_contextual_instance_fixture(true);

    assert!(
        vm_ok,
        "module-scoped contextual instance fixture failed on VM:\nstdout:\n{vm_stdout}\nstderr:\n{vm_stderr}"
    );
    assert!(
        native_ok,
        "module-scoped contextual instance fixture failed natively:\nstdout:\n{native_stdout}\nstderr:\n{native_stderr}"
    );
    for (backend, stdout) in [("VM", vm_stdout), ("native", native_stdout)] {
        let values: Vec<_> = stdout
            .lines()
            .filter(|line| matches!(*line, "true" | "false"))
            .collect();
        assert_eq!(
            values,
            vec!["true", "true", "false"],
            "module-scoped contextual instance {backend} output was:\n{stdout}"
        );
    }
}

/// Two modules declaring the same class name with the same instance head.
///
/// Uses single-segment module names deliberately: the generated method is
/// emitted inside its module *and* as a file-scope forwarding alias, and the
/// alias is qualified with the entry file's stem. For `module Alpha` in
/// `Alpha.flx` that stem equals the module prefix, so both claimed one symbol
/// and LLVM rejected the redefinition. A dotted module (`Data.Enc` in
/// `Data/Enc.flx`) cannot reproduce it — the prefix and the stem differ.
fn run_same_class_name_fixture(native: bool) -> (String, String, bool) {
    let _guard = TYPECLASS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("typeclass test lock poisoned");
    let scratch = Scratch::new(if native {
        "native-same-class-name"
    } else {
        "vm-same-class-name"
    });
    for (module, answer) in [("Alpha", "alpha"), ("Beta", "beta")] {
        scratch.write(
            &format!("{module}.flx"),
            &format!(
                r#"
module {module} {{
    public class Render<a> {{
        fn render(value: a) -> String
    }}

    public instance Render<Int> {{
        fn render(value) {{ "{answer}" }}
    }}
}}
"#
            ),
        );
    }
    let entry = scratch.write(
        "main.flx",
        r#"
import Alpha
import Beta

fn main() with IO {
    print(Alpha.render(1))
    print(Beta.render(1))
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
        .expect("run same-class-name fixture");

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn same_class_name_in_two_modules_stays_distinct_on_vm_and_llvm() {
    let (vm_stdout, vm_stderr, vm_ok) = run_same_class_name_fixture(false);
    let (native_stdout, native_stderr, native_ok) = run_same_class_name_fixture(true);

    assert!(
        vm_ok,
        "same-class-name fixture failed on VM:\nstdout:\n{vm_stdout}\nstderr:\n{vm_stderr}"
    );
    assert!(
        native_ok,
        "same-class-name fixture failed natively:\nstdout:\n{native_stdout}\nstderr:\n{native_stderr}"
    );
    // Each module must answer for itself; a collapsed symbol prints one twice.
    for stdout in [&vm_stdout, &native_stdout] {
        assert!(
            stdout.contains("alpha"),
            "expected Alpha's answer:\n{stdout}"
        );
        assert!(stdout.contains("beta"), "expected Beta's answer:\n{stdout}");
    }
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

fn run_same_named_fixture(native: bool) -> (String, String, bool) {
    let _guard = TYPECLASS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("typeclass test lock poisoned");
    let scratch = Scratch::new(if native {
        "native-qualified-class-id"
    } else {
        "vm-qualified-class-id"
    });
    scratch.write(
        "Mod/A.flx",
        r#"
module Mod.A {
    public class Foo<a> {
        fn render(x: a) -> String
    }

    public instance Foo<Int> {
        fn render(x) { "A" }
    }
}
"#,
    );
    scratch.write(
        "Mod/B.flx",
        r#"
module Mod.B {
    public class Foo<a> {
        fn render(x: a) -> String
    }

    public instance Foo<Int> {
        fn render(x) { "B" }
    }
}
"#,
    );
    let entry = scratch.write(
        "main.flx",
        r#"
import Mod.A as Alpha
import Mod.B as Beta

fn main() with IO {
    print(Alpha.render(0))
    print(Beta.render(0))
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
        .expect("run same-named class-id fixture");

    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn same_named_module_classes_dispatch_on_vm_and_llvm() {
    let (vm_stdout, vm_stderr, vm_ok) = run_same_named_fixture(false);
    let (native_stdout, native_stderr, native_ok) = run_same_named_fixture(true);

    assert!(
        vm_ok,
        "same-named class fixture failed on VM:\nstdout:\n{vm_stdout}\nstderr:\n{vm_stderr}"
    );
    assert!(
        native_ok,
        "same-named class fixture failed natively:\nstdout:\n{native_stdout}\nstderr:\n{native_stderr}"
    );
    assert!(
        vm_stdout.contains("A") && vm_stdout.contains("B"),
        "VM output was:\n{vm_stdout}"
    );
    assert!(
        native_stdout.contains("A") && native_stdout.contains("B"),
        "native output was:\n{native_stdout}"
    );
}
