//! A class method applied to a value written with named-field syntax must
//! reach its instance (KI-012).
//!
//! `Red { on: true }` is rewritten into an ordinary constructor call by
//! `desugar_named_fields`, which runs *after* inference. That pass used to
//! stamp the synthesized `Expression::Call` with `ExprId::UNSET`, discarding
//! the id under which inference had recorded the constructed value's type.
//! Compile-time dispatch looks the first argument's type up by id, found
//! nothing, and fell through to the stub whose body is
//! `panic("No instance ...")`.
//!
//! The symptom read as "only the first instance of a class dispatches", but
//! neither resolved at compile time — positional constructors were unaffected,
//! which is what made one arm appear to work. Both spellings are covered here
//! so a future change cannot fix one and regress the other.

use std::path::Path;
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(name: &str, source: &str) -> (String, String, bool) {
    // Own scratch dir per run: a literal filename in one shared directory let
    // concurrent test binaries overwrite each other (KI-010).
    let scratch = Scratch::new(name.trim_end_matches(".flx"));
    let file = scratch.write(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

/// The regression itself: two instances, values built with named fields.
///
/// Two ADTs rather than one because the original report was that the *second*
/// instance failed; a single-instance test would have passed throughout.
#[test]
fn named_field_constructors_dispatch_to_their_instances() {
    let (stdout, stderr, success) = run_source(
        "named_field_dispatch.flx",
        r#"
data Colour { Red { on: Bool }, Blue { on: Bool } }
data Tag { Tag { name: String } }

class Describe<a> { fn describe(value: a) -> String }

instance Describe<Colour> {
    fn describe(value) {
        match value { Red { on: _ } -> "red", Blue { on: _ } -> "blue" }
    }
}

instance Describe<Tag> {
    fn describe(value) { match value { Tag { name } -> name } }
}

fn main() with IO {
    println(describe(Red { on: true }))
    println(describe(Blue { on: false }))
    println(describe(Tag { name: "tagged" }))
}
"#,
    );

    assert!(
        success,
        "named-field constructors must dispatch to their instance:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("red") && stdout.contains("blue") && stdout.contains("tagged"),
        "every instance should have been selected, got:\n{stdout}{stderr}"
    );
}

/// The spelling that already worked, kept so a fix cannot trade one for the
/// other.
#[test]
fn positional_constructors_dispatch_to_their_instances() {
    let (stdout, stderr, success) = run_source(
        "positional_dispatch.flx",
        r#"
data Shape { Circle(Int), Square(Int) }
data Label { Label(String) }

class Describe<a> { fn describe(value: a) -> String }

instance Describe<Shape> {
    fn describe(value) {
        match value { Circle(_) -> "circle", Square(_) -> "square" }
    }
}

instance Describe<Label> {
    fn describe(value) { match value { Label(text) -> text } }
}

fn main() with IO {
    println(describe(Circle(1)))
    println(describe(Label("labelled")))
}
"#,
    );

    assert!(
        success,
        "positional constructors must dispatch to their instance:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("circle") && stdout.contains("labelled"),
        "every instance should have been selected, got:\n{stdout}{stderr}"
    );
}

/// A named-field value bound to a `let` first still dispatches.
///
/// This spelling did not fail under the original bug — a bound value reaches
/// dispatch through its binding rather than through the desugared call's own
/// id. It is here as a guard on the shape most real code takes, not as a
/// second witness to KI-012.
#[test]
fn a_named_field_value_dispatches_after_being_bound() {
    let (stdout, stderr, success) = run_source(
        "named_field_bound_dispatch.flx",
        r#"
data Point { Point { x: Int, y: Int } }

class Describe<a> { fn describe(value: a) -> String }

instance Describe<Point> {
    fn describe(value) {
        match value { Point { x, y } -> to_string(x) + "," + to_string(y) }
    }
}

fn main() with IO {
    let here = Point { x: 1, y: 2 }
    println(describe(here))
}
"#,
    );

    assert!(
        success,
        "a bound named-field value must still dispatch:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("1,2"),
        "expected the instance's rendering, got:\n{stdout}{stderr}"
    );
}
