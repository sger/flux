//! A payload destructured from an *imported* constructor must be forwardable
//! unchanged (KI-022).
//!
//! ```flux
//! match item {
//!     TString(text) -> Ok(text),   // error[E430] under `flux --test`
//!     _ -> Err("not a string"),
//! }
//! ```
//!
//! KI-014 gave `ModuleInterface` a `public_ctor_types` map so an imported
//! constructor infers as its ADT, but that map was populated only by
//! `preload_module_interface` — the *cached* `.flxi` path. A module compiled
//! fresh in the same run never seeded it, and the test runner compiles the
//! whole graph through one `Compiler` with no interface step at all. So under
//! `flux --test` an imported constructor's pattern bound its payload to an
//! unresolved variable, the enclosing `Ok(...)` application never got an
//! argument type, and strict types rejected the residue as E430.
//!
//! Every case here forwards the payload *unchanged* — `Ok(text)`, not
//! `Ok(text + "")`. Reconstructing it was the documented workaround precisely
//! because it supplies the constraint the pattern failed to, so a fixture that
//! reconstructs passes against the unfixed compiler and proves nothing.
//!
//! The three constructor shapes take different inference paths and are covered
//! separately: positional, generic (`Box<a>`), and named-field.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

/// The ADT whose constructors are imported. Deliberately in a module of its
/// own: the bug does not reproduce when the matched type is declared in the
/// same module as the match.
const VALUE_MODULE: &str = r#"module Value {
    public data Toml { TString(String), TInt(Int) }
    public data Box<a> { Wrap(a) }
    public data Pair { Both { left: String, right: Int } }
}
"#;

/// Every arm forwards its bound payload directly into `Ok`.
const READER_MODULE: &str = r#"import Flow.Result exposing (Result, Ok, Err)
import Value exposing (Toml, TString, TInt, Box, Wrap, Pair, Both)

module Reader {
    public fn as_string(item: Toml) -> Result<String, String> {
        match item {
            TString(text) -> Ok(text),
            _ -> Err("not a string"),
        }
    }

    public fn as_int(item: Toml) -> Result<Int, String> {
        match item {
            TInt(n) -> Ok(n),
            _ -> Err("not an int"),
        }
    }

    public fn unbox(b: Box<String>) -> Result<String, String> {
        match b {
            Wrap(v) -> Ok(v),
        }
    }

    public fn left_of(p: Pair) -> Result<String, String> {
        match p {
            Both { left: l, right: _ } -> Ok(l),
        }
    }
}
"#;

const ENTRY: &str = r#"import Flow.Result exposing (Result, Ok, Err)
import Value exposing (Toml, TString, TInt, Box, Wrap, Pair, Both)
import Reader

fn text_of(r: Result<String, String>) -> String {
    match r {
        Ok(s) -> s,
        Err(e) -> e,
    }
}

fn test_forwarded_payloads() with IO {
    println(text_of(Reader.as_string(TString("hi"))))
    match Reader.as_int(TInt(7)) {
        Ok(n) -> println(to_string(n)),
        Err(e) -> println(e),
    }
    println(text_of(Reader.unbox(Wrap("boxed"))))
    println(text_of(Reader.left_of(Both { left: "L", right: 1 })))
}
"#;

/// What the fixture prints, in order: one line per constructor shape.
const EXPECTED: &[&str] = &["hi", "7", "boxed", "L"];

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run the three-module program under `flux --test`.
///
/// `--test` is the trigger: strict types run for an imported module only in
/// test mode, so a plain `flux run` of the same sources compiles clean and is
/// not evidence either way.
fn run_under_test_mode() -> (String, bool) {
    let scratch = Scratch::new("ki022-imported-ctor-payload");
    scratch.write("Value.flx", VALUE_MODULE);
    scratch.write("Reader.flx", READER_MODULE);
    let entry = scratch.write("main.flx", ENTRY);
    let dir = scratch.path().to_string_lossy().into_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            "--test",
            entry.to_str().unwrap(),
            "--root",
            dir.as_str(),
            "--no-cache",
        ])
        .args(scratch.cache_args())
        .output()
        .expect("failed to run flux --test");

    // Both streams: E430 is rendered to stderr, so a stdout-only capture would
    // show the failure as unexplained missing output.
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    (format!("{stdout}\n{stderr}"), output.status.success())
}

#[test]
fn an_imported_constructors_payload_can_be_forwarded_unchanged() {
    let (output, success) = run_under_test_mode();
    assert!(
        !output.contains("E430"),
        "forwarding an imported constructor's payload must not trip strict \
         types:\n{output}"
    );
    assert!(success, "the fixture must run to completion:\n{output}");

    let printed: Vec<&str> = output
        .lines()
        .map(|line| line.trim().trim_matches('"'))
        .filter(|line| !line.is_empty())
        .collect();
    for expected in EXPECTED {
        assert!(
            printed.contains(expected),
            "expected `{expected}` in the output:\n{output}"
        );
    }
    assert!(
        output.contains("1 tests: 1 passed, 0 failed"),
        "the test function must pass:\n{output}"
    );
}
