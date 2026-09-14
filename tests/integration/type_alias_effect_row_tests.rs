//! An effect row that only appears after a type alias expands
//! ([KI-094](../../docs/known_issues.md#ki-094)).
//!
//! `tests/parity/type_alias_transparent.flx` covers the same ground, but a
//! parity fixture compares the two backends against *each other*: when both
//! fail identically the outputs match and the fixture is reported as passing.
//! That is [KI-062](../../docs/known_issues.md#ki-062), and it is why this
//! fixture sat skipped for so long without anyone noticing the feature had
//! never run. These tests assert the program's actual output instead.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn run(program: &Path) -> String {
    let output = Command::new(Path::new(env!("CARGO_BIN_EXE_flux")))
        .arg(program)
        .arg("--no-cache")
        .output()
        .expect("run flux");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_source(label: &str, source: &str) -> String {
    let scratch = Scratch::new(label);
    run(&scratch.write("main.flx", source))
}

/// The parity fixture, executed. `10 + 20 + 22 + 10 + 5`.
#[test]
fn the_transparent_alias_fixture_runs_and_prints_its_sum() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/parity/type_alias_transparent.flx");
    let out = run(&fixture);
    assert!(
        out.contains("67"),
        "expected the fixture to print 67, got:\n{out}"
    );
}

/// An alias whose *body* carries a row: the row only exists in the AST once the
/// type alias has been substituted, so it is decomposed after that pass, not
/// before. Expanding effect aliases first left `Async` as a single atom while
/// the argument's own contract had it decomposed into four.
#[test]
fn an_alias_body_carrying_an_effect_row_runs() {
    let out = run_source(
        "ki094-alias-body-row",
        "import Flow.Async exposing (..)\n\
         \n\
         alias Stream<a> = () -> Option<a> with Async\n\
         \n\
         fn ten() -> Option<Int> with Async { Some(10) }\n\
         \n\
         fn consume(s: Stream<Int>) -> Int with Async {\n\
         \x20   match s() { Some(v) -> v, None -> 0 }\n\
         }\n\
         \n\
         fn body() -> Int with Async { consume(ten) }\n\
         \n\
         fn main() with IO { print(run_async(body)) }\n",
    );
    assert!(out.contains("10"), "expected 10, got:\n{out}");
}

/// The same row written directly on a parameter, with no alias in sight. A
/// `FnContract` outlives the AST expansion pass, so its captured annotations
/// have to be expanded eagerly too — otherwise the row solver compares an
/// undecomposed `Async` against a decomposed one and reports them disjoint.
#[test]
fn an_effect_row_written_on_a_parameter_runs() {
    let out = run_source(
        "ki094-parameter-row",
        "import Flow.Async exposing (..)\n\
         \n\
         fn five(n: Int) -> Int with Async { n }\n\
         \n\
         fn consume(s: (Int) -> Int with Async) -> Int with Async { s(5) }\n\
         \n\
         fn body() -> Int with Async { consume(five) }\n\
         \n\
         fn main() with IO { print(run_async(body)) }\n",
    );
    assert!(out.contains('5'), "expected 5, got:\n{out}");
}
