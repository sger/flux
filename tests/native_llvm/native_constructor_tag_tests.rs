//! Constructor tags must agree across separately compiled modules.
//!
//! Native builds give each module its own `Compiler`. Tags were numbered from
//! whatever that module happened to preload, so the same constructor could get
//! a different tag in each object file: `Flow.Result`'s `Ok` was 5 while
//! `Flow.Fs` built its `Ok` values with 14. Matching inline still worked
//! (payload extraction ignores the tag), but passing the value to a function
//! in another module — `Result.is_ok(...)` — took the wrong branch.
//!
//! These tests run on the native backend only; the VM does not use tags.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run one source file on both backends and return `(vm_stdout, native_stdout)`.
fn run_both(name: &str, source: &str) -> (String, String) {
    let dir: PathBuf = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join(name);
    std::fs::write(&file, source).expect("write fixture");

    let run = |native: bool| {
        let mut args = vec!["run", file.to_str().unwrap(), "--no-cache"];
        if native {
            args.push("--native");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args(&args)
            .output()
            .expect("run flux");
        String::from_utf8_lossy(&output.stdout)
            .replace("\r\n", "\n")
            .lines()
            .filter(|l| l.starts_with('"'))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let vm = run(false);
    let native = run(true);
    let _ = std::fs::remove_file(&file);
    (vm, native)
}

/// `Result.is_ok` lives in `Flow.Result`; the value comes from a primop called
/// in `Flow.Fs`. Both objects must agree on `Ok`'s tag.
#[test]
fn a_result_from_one_module_dispatches_correctly_in_another() {
    let (vm, native) = run_both(
        "ctor_tag_cross_module.flx",
        r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn main() -> Unit with IO {
    println("read=" + to_string(Result.is_ok(Fs.read_file("Cargo.toml"))))
    println("missing=" + to_string(Result.is_ok(Fs.read_file("/nonexistent/x"))))
}
"#,
    );
    assert_eq!(
        vm, native,
        "VM and native disagree:\nvm:\n{vm}\nnative:\n{native}"
    );
    assert!(vm.contains("read=true"), "expected read=true:\n{vm}");
    assert!(
        vm.contains("missing=false"),
        "expected missing=false:\n{vm}"
    );
}

/// `Result<Unit, _>` is the shape the filesystem mutations return. Its `Ok`
/// carries the unit value, so a wrong tag shows up as a spurious `Err`.
#[test]
fn a_unit_result_dispatches_correctly_across_modules() {
    let _ = std::fs::remove_dir_all(workspace_root().join("target/test-scratch/ctor_tag_unit"));
    let (vm, native) = run_both(
        "ctor_tag_unit_result.flx",
        r#"
import Flow.Fs as Fs
import Flow.Result as Result

fn main() -> Unit with IO {
    let d = "target/test-scratch/ctor_tag_unit"
    println("mkdir=" + to_string(Result.is_ok(Fs.create_dir_all(d))))
    println("write=" + to_string(Result.is_ok(Fs.write_file(d + "/f.txt", "x"))))
    println("rm=" + to_string(Result.is_ok(Fs.remove_dir_all(d))))
    println("missing=" + to_string(Result.is_err(Fs.remove_file("/nonexistent/f"))))
}
"#,
    );
    let _ = std::fs::remove_dir_all(workspace_root().join("target/test-scratch/ctor_tag_unit"));
    assert_eq!(
        vm, native,
        "VM and native disagree:\nvm:\n{vm}\nnative:\n{native}"
    );
    for expected in ["mkdir=true", "write=true", "rm=true", "missing=true"] {
        assert!(vm.contains(expected), "missing {expected}:\n{vm}");
    }
}

/// Two modules each declaring their own constructors must not collide:
/// `Flow.IoError`'s `NotFound` and `Flow.Result`'s `Ok` were both tag 5.
#[test]
fn constructors_from_different_modules_get_distinct_tags() {
    let (vm, native) = run_both(
        "ctor_tag_distinct.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io
import Flow.Result as Result

fn main() -> Unit with IO {
    let r = Fs.read_file("/nonexistent/x")
    println("is_err=" + to_string(Result.is_err(r)))
    match r {
        Ok(_) -> println("kind=UNEXPECTED"),
        Err(e) -> println("kind=" + Io.kind_name(Io.error_kind(e))),
    }
    println("not_found=" + to_string(Io.is_not_found(Io.io_error(NotFound, "m", "p"))))
}
"#,
    );
    assert_eq!(
        vm, native,
        "VM and native disagree:\nvm:\n{vm}\nnative:\n{native}"
    );
    assert!(vm.contains("is_err=true"), "{vm}");
    assert!(vm.contains("kind=NotFound"), "{vm}");
    assert!(vm.contains("not_found=true"), "{vm}");
}
