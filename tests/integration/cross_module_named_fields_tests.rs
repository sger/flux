//! Regression tests for named-field syntax on *imported* constructors
//!.
//!
//! Named-field syntax — `IoError { kind: .., message: .. }` as an expression
//! and `IoError { kind, message }` as a pattern — is desugared to positional
//! form by looking up the constructor's declared field order. That lookup ran
//! only over the current program's `data` declarations, so a constructor
//! declared in an *imported* module had no field order: the desugaring
//! silently produced a zero-field constructor and the arity check then
//! rejected it with a bogus "expects 3 argument(s) but got 0" (E082 for
//! construction, E085 for patterns).
//!
//! Module interfaces now carry `ctor_field_names`, so the field order survives
//! into importing modules — including across the `.flxi` cache, which is why
//! the warm-cache path is covered explicitly.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Write a two-file project (`<ModuleName>.flx` + `main.flx`) into a private
/// scratch directory and run it. A module must live in a file named after it,
/// so the module source cannot simply be inlined into `main.flx`.
fn run_project(case: &str, module_name: &str, module_src: &str, main_src: &str) -> (String, bool) {
    let scratch = Scratch::new(case);
    scratch.write(&format!("{module_name}.flx"), module_src);
    let main = scratch.write("main.flx", main_src);
    let dir = scratch.path();

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            "run",
            main.to_str().unwrap(),
            "--root",
            dir.to_str().unwrap(),
            "--no-cache",
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for {case}: {e}"));

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .replace("\r\n", "\n");
    (combined, output.status.success())
}

const ERRS_MODULE: &str = r#"
module Errs {
    public data IoErrorKind { NotFound, PermissionDenied, Other }

    public data IoError {
        IoError { kind: IoErrorKind, message: String, path: String }
    }

    public fn make() -> IoError {
        IoError { kind: NotFound, message: "m", path: "p" }
    }
}
"#;

#[test]
fn named_field_pattern_matches_an_imported_constructor() {
    let (out, success) = run_project(
        "xmod_named_pattern",
        "Errs",
        ERRS_MODULE,
        r#"
import Errs as Errs

fn main() -> Unit {
    match Errs.make() {
        IoError { kind, message, path } -> println("got " + path + " " + message),
    }
}
"#,
    );
    assert!(
        success,
        "named-field pattern on an imported ctor failed:\n{out}"
    );
    assert!(
        out.contains("got p m"),
        "expected `got p m`, output:\n{out}"
    );
}

#[test]
fn named_field_construction_of_an_imported_constructor() {
    let (out, success) = run_project(
        "xmod_named_construct",
        "Errs",
        ERRS_MODULE,
        r#"
import Errs as Errs

fn describe(e: IoError) -> String {
    match e {
        IoError { kind, message, path } -> "built " + path + " " + message,
    }
}

fn main() -> Unit {
    println(describe(IoError { kind: NotFound, message: "m2", path: "p2" }))
}
"#,
    );
    assert!(
        success,
        "named-field construction of an imported ctor failed:\n{out}"
    );
    assert!(
        out.contains("built p2 m2"),
        "expected `built p2 m2`:\n{out}"
    );
}

/// Field *order* is what the desugaring resolves, so writing the fields out of
/// declaration order must still bind each name to the right value. This is the
/// assertion that would catch a positional-vs-named mix-up.
#[test]
fn named_fields_may_be_written_out_of_declaration_order() {
    let (out, success) = run_project(
        "xmod_named_order",
        "Errs",
        ERRS_MODULE,
        r#"
import Errs as Errs

fn describe(e: IoError) -> String {
    match e {
        IoError { message, path, kind } -> "order " + path + "/" + message,
    }
}

fn main() -> Unit {
    println(describe(IoError { path: "P", kind: NotFound, message: "M" }))
}
"#,
    );
    assert!(success, "out-of-order named fields failed:\n{out}");
    assert!(
        out.contains("order P/M"),
        "fields bound to the wrong positions; expected `order P/M`:\n{out}"
    );
}

/// A locally declared constructor of the same name must keep its own field
/// order rather than inheriting the imported one.
#[test]
fn a_local_constructor_shadows_an_imported_one_of_the_same_name() {
    let (out, success) = run_project(
        "xmod_named_shadow",
        "Errs",
        ERRS_MODULE,
        r#"
import Errs as Errs

data IoError { IoError { path: String, message: String, kind: Int } }

fn describe(e: IoError) -> String {
    match e {
        IoError { path, message, kind } -> "local " + path + " " + to_string(kind),
    }
}

fn main() -> Unit {
    println(describe(IoError { path: "local", message: "mine", kind: 7 }))
}
"#,
    );
    assert!(success, "local ctor should shadow the imported one:\n{out}");
    assert!(
        out.contains("local local 7"),
        "expected `local local 7`:\n{out}"
    );
}

/// The field order has to survive the `.flxi` cache, not just a cold compile.
/// A previous cross-module constructor fix worked cold and failed warm, so the
/// second run here is the one that matters.
#[test]
fn field_order_survives_the_warm_module_cache() {
    let scratch = Scratch::new("xmod_named_warm");
    let dir = scratch.path().to_path_buf();
    scratch.write("Errs.flx", ERRS_MODULE);
    std::fs::write(
        dir.join("main.flx"),
        r#"
import Errs as Errs

fn main() -> Unit {
    match Errs.make() {
        IoError { kind, message, path } -> println("warm " + path + " " + message),
    }
}
"#,
    )
    .expect("write main");

    // Note: no `--no-cache`, so the first run populates the cache and the
    // second reads the interface back from disk. The cache is this test's own
    // (`--cache-dir`): sharing the repo-wide one let concurrent test binaries
    // corrupt each other's interfaces — see KI-010 in docs/known_issues.md.
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_flux"))
            .current_dir(workspace_root())
            .args([
                "run",
                dir.join("main.flx").to_str().unwrap(),
                "--root",
                dir.to_str().unwrap(),
            ])
            .args(scratch.cache_args())
            .output()
            .expect("failed to run flux");
        (
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            output.status.success(),
        )
    };

    let (cold, cold_ok) = run();
    assert!(cold_ok, "cold run failed:\n{cold}");
    assert!(cold.contains("warm p m"), "cold run output:\n{cold}");

    let (warm, warm_ok) = run();
    assert!(
        warm_ok,
        "warm run failed — ctor_field_names did not survive the interface cache:\n{warm}"
    );
    assert!(warm.contains("warm p m"), "warm run output:\n{warm}");
}
