//! `Sendable<T>` type-class tests (proposal 0174 Phase 1a-v).
//!
//! `Sendable` is a marker class that authorizes a value to cross a worker-
//! thread boundary (`Channel.send`, `Task.spawn`). Phase 1a-v's deliverable
//! is the type-class machinery: built-in instances for primitives, structural
//! instances for tuples and persistent collections, and absence-of-instance
//! reported at compile time.
//!
//! These tests exercise the constraint solver directly through Compiler.

use flux::{
    compiler::Compiler,
    syntax::{lexer::Lexer, parser::Parser},
};

fn compile_source(src: &str) -> Result<(), Vec<String>> {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return Err(parser
            .errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>());
    }
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, false, false).map_err(
        |errs: Vec<flux::diagnostics::Diagnostic>| {
            errs.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>()
        },
    )?;
    Ok(())
}

#[test]
fn sendable_int_is_satisfied() {
    compile_source(
        r#"
fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    ferry(42)
}
"#,
    )
    .expect("Sendable<Int> must be a built-in instance");
}

#[test]
fn sendable_primitives_are_all_satisfied() {
    // One call site per primitive — guards against a typo in the
    // `register_builtins` block dropping any instance.
    compile_source(
        r#"
fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    let _ = ferry(1);
    let _ = ferry(1.0);
    let _ = ferry("hi");
    let _ = ferry(true);
    let _ = ferry(())
}
"#,
    )
    .expect("Sendable instances for Int/Float/String/Bool/Unit must all hold");
}

#[test]
fn sendable_tuple_of_sendable_is_structural() {
    compile_source(
        r#"
fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    ferry((1, "hi", true))
}
"#,
    )
    .expect("Sendable<(Int, String, Bool)> should derive structurally");
}

#[test]
fn sendable_collections_of_sendable_are_structural() {
    // Option/List/Array/Either/Map all auto-derive `Sendable` when their
    // element types satisfy it.
    compile_source(
        r#"
fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    let _ = ferry([1, 2, 3]);
    let _ = ferry(Some(7));
    let _ = ferry((Some(1), [true, false]))
}
"#,
    )
    .expect("Sendable should derive over Option/List and nested combinations");
}

#[test]
fn sendable_user_adt_with_only_sendable_fields_is_auto_derived() {
    // Proposal 0174 D4: monomorphic ADT whose every field is Sendable
    // gets a synthesized `Sendable<Foo>` instance.
    compile_source(
        r#"
data Point { Point(Int, Int) }

fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    ferry(Point(1, 2))
}
"#,
    )
    .expect("Sendable<Point> must be auto-derived for an ADT of two Ints");
}

#[test]
fn sendable_parameterized_adt_uses_contextual_bound() {
    // Proposal 0174 D4: `data Box<a> { Box(a) }` synthesizes
    // `<a: Sendable> => Sendable<Box<a>>`. So `Box<Int>` works…
    compile_source(
        r#"
data Box<a> { Box(a) }

fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    ferry(Box(42))
}
"#,
    )
    .expect("Sendable<Box<Int>> must derive via the contextual bound");
}

#[test]
fn sendable_parameterized_adt_rejects_non_sendable_arg() {
    // …and `Box<Int -> Int>` correctly fails because the contextual
    // `Sendable<a>` bound can't be discharged for a function type.
    let result = compile_source(
        r#"
data Box<a> { Box(a) }

fn ferry<a: Sendable>(x: a) -> a { x }

fn id_int(x: Int) -> Int { x }

fn main() {
    let b = Box(id_int);
    ferry(b)
}
"#,
    );
    let errs =
        result.expect_err("Sendable<Box<Int -> Int>> must fail — function types are not Sendable");
    let joined = errs.join("\n");
    assert!(
        joined.contains("Sendable") || joined.contains("instance"),
        "expected a Sendable-related diagnostic, got:\n{joined}"
    );
}

#[test]
fn sendable_user_adt_with_function_field_is_not_derived() {
    // The positive-only rule: an ADT that can directly hold a closure
    // must not get a synthesized Sendable instance.
    let result = compile_source(
        r#"
data WithFn { WithFn(Int, () -> Int) }

fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    let f = fn() { 42 };
    ferry(WithFn(1, f))
}
"#,
    );
    let errs = result.expect_err(
        "Sendable<WithFn> must not be auto-derived — the ADT contains a function field",
    );
    let joined = errs.join("\n");
    assert!(
        joined.contains("Sendable") || joined.contains("instance"),
        "expected a Sendable-related diagnostic, got:\n{joined}"
    );
}

#[test]
fn sendable_function_type_has_no_instance() {
    // Closures aren't sendable — the proposal's "absence means not sendable"
    // rule. Compilation must fail with a no-instance diagnostic.
    let result = compile_source(
        r#"
fn ferry<a: Sendable>(x: a) -> a { x }

fn main() {
    let f = fn(x) { x + 1 };
    ferry(f)
}
"#,
    );
    let errs = result
        .expect_err("Sendable on a closure must not be derivable — closures are not auto-sendable");
    let joined = errs.join("\n");
    assert!(
        joined.contains("Sendable") || joined.contains("instance"),
        "expected a Sendable-related diagnostic, got:\n{joined}"
    );
}
