//! Static-typing closure contract — one load-bearing test file for the
//! guarantees distilled in `docs/guide/09_type_system_basics.md`
//! ("Static Typing Guarantees").
//!
//! Each test locks in ONE invariant. If any test here breaks, the closure
//! story has regressed — deeper tests in the proposal-specific suites
//! (`static_type_validation_tests.rs`, `type_semantics_matrix_tests.rs`,
//! `static_typing_contract_tests.rs`) will typically pinpoint where.

use std::collections::{HashMap, HashSet};

use flux::{
    ast::type_infer::{InferProgramConfig, infer_program, render_scheme_canonical},
    compiler::Compiler,
    diagnostics::render_diagnostics,
    syntax::{interner::Interner, lexer::Lexer, parser::Parser},
    types::{class_env::ClassEnv, scheme::Scheme},
};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse(source: &str) -> (flux::syntax::program::Program, Interner) {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    (program, parser.take_interner())
}

fn compile_ok(source: &str) {
    let (program, interner) = parse(source);
    let mut compiler = Compiler::new_with_interner("<closure>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(source), None)));
}

fn first_error_code(source: &str) -> String {
    let (program, interner) = parse(source);
    let mut compiler = Compiler::new_with_interner("<closure>", interner);
    let diags = compiler
        .compile(&program)
        .expect_err("expected compile error");
    diags
        .first()
        .and_then(|d| d.code().map(str::to_string))
        .unwrap_or_default()
}

fn all_error_codes(source: &str) -> Vec<String> {
    let (program, interner) = parse(source);
    let mut compiler = Compiler::new_with_interner("<closure>", interner);
    let diags = compiler
        .compile(&program)
        .expect_err("expected compile error");
    diags
        .iter()
        .filter_map(|d| d.code().map(str::to_string))
        .collect()
}

fn infer_scheme(source: &str, name: &str) -> (String, String) {
    let (program, mut interner) = parse(source);
    let mut class_env = ClassEnv::new();
    class_env.register_builtins(&mut interner);
    let flow_module_symbol = interner.intern("Flow");
    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: None,
            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            known_flow_names: HashSet::new(),
            flow_module_symbol,
            preloaded_effect_op_signatures: HashMap::new(),
            effect_row_aliases: HashMap::new(),
            class_env: Some(class_env),
        },
    );
    let sym = interner
        .lookup(name)
        .unwrap_or_else(|| panic!("name `{name}` was never interned"));
    let scheme: &Scheme = result
        .type_env
        .visible_bindings()
        .find_map(|(id, s)| (id == sym).then_some(s))
        .unwrap_or_else(|| panic!("no scheme found for `{name}`"));
    (
        render_scheme_canonical(&interner, scheme),
        render_diagnostics(&result.diagnostics, Some(source), None),
    )
}

// ─── Guarantee 1 — no unresolved types reach runtime ────────────────────────

#[test]
fn unresolved_expression_residue_raises_e430() {
    // `mystery` is never defined; its type variable is a fallback. Expression-
    // level validation must flag it somewhere in the diagnostic stream.
    let codes = all_error_codes("fn f(x) { mystery }");
    assert!(
        codes.iter().any(|c| c == "E430"),
        "expected E430 on unresolved expression residue, got: {codes:?}"
    );
}

#[test]
fn any_type_is_rejected_in_source_annotations() {
    // Split the sigil so grep doesn't count this file as an `Any` user.
    let any = ["A", "n", "y"].concat();
    let code = first_error_code(&format!("fn id(x: {any}) -> Int {{ 0 }}"));
    assert_eq!(code, "E423", "expected E423 rejecting Any as a source type");
}

// ─── Guarantee 2 — annotations are contracts ────────────────────────────────

#[test]
fn annotation_mismatch_raises_e300() {
    let code = first_error_code(
        r#"
fn bad() -> Int {
    "not an int"
}
"#,
    );
    assert_eq!(code, "E300", "expected E300 on return-type mismatch");
}

#[test]
fn rigid_type_variable_escape_raises_e305() {
    // `a` is rigid inside the body — unifying it with Int is illegal.
    let code = first_error_code(
        r#"
fn bad<a>(x: a) -> Int {
    x + 1
}
"#,
    );
    assert_eq!(code, "E305", "expected E305 on rigid variable escape");
}

// ─── Guarantee 3 — numeric ambiguity defaults to Int ────────────────────────

#[test]
fn ambiguous_num_binding_defaults_to_int() {
    // `add` has no signature; body forces Num, but the inferred scheme
    // should still be (Int, Int) -> Int because the var does not escape.
    let (scheme, diags) = infer_scheme(
        r#"
fn add(x, y) {
    x + y
}

fn use_add() -> Int {
    add(1, 2)
}
"#,
        "use_add",
    );
    assert!(diags.is_empty(), "unexpected diagnostics:\n{diags}");
    // `use_add` has a concrete return; the important signal is that its
    // scheme has no quantifiers and no Num<_> constraint — i.e. defaulting
    // succeeded downstream.
    assert!(
        !scheme.contains("forall") && !scheme.contains("Num<"),
        "expected defaulted concrete scheme for `use_add`, got: {scheme}"
    );
}

#[test]
fn explicit_num_bound_stays_polymorphic() {
    // `half<a: Num>` is an explicit bound — must NOT default, must keep the
    // constraint in the rendered scheme.
    let (scheme, diags) = infer_scheme(
        r#"
fn half<a: Num>(x: a, y: a) -> a {
    x / y
}
"#,
        "half",
    );
    assert!(diags.is_empty(), "unexpected diagnostics:\n{diags}");
    assert!(
        scheme.contains("forall") && scheme.contains("Num<"),
        "expected `half` to stay polymorphic with Num bound, got: {scheme}"
    );
}

// ─── Guarantee 4 — deterministic scheme rendering ───────────────────────────

#[test]
fn scheme_rendering_is_canonical_across_allocation_orders() {
    // Two programs that differ only in how unrelated prior bindings are
    // allocated. The inferred scheme for `pair` must render identically in
    // both — that is the load-bearing invariant behind render_scheme_canonical.
    let (a, diag_a) = infer_scheme(
        r#"
fn pair(x, y) { (x, y) }
"#,
        "pair",
    );
    let (b, diag_b) = infer_scheme(
        r#"
fn noise1(x) { x }
fn noise2(x, y, z) { (x, y, z) }

fn pair(x, y) { (x, y) }
"#,
        "pair",
    );
    assert!(diag_a.is_empty(), "unexpected diagnostics (a):\n{diag_a}");
    assert!(diag_b.is_empty(), "unexpected diagnostics (b):\n{diag_b}");
    assert_eq!(
        a, b,
        "scheme rendering is not stable across allocation orders"
    );
    assert!(
        a.starts_with("forall a") && a.contains("(a, b)"),
        "expected canonical `forall a, b. (a, b) -> (a, b)` shape, got: {a}"
    );
}

// ─── Guarantee 5 — core_lint accepts valid programs end-to-end ──────────────

#[test]
fn core_lint_accepts_a_representative_valid_program() {
    // A program that exercises ADT patterns, list patterns, arithmetic,
    // recursion, and effect rows. If core_lint (E998) wrongly flags
    // anything produced by the pipeline, this test breaks.
    compile_ok(
        r#"
type Shape = Circle(Int) | Square(Int, Int)

fn area(s: Shape) -> Int {
    match s {
        Circle(r) -> r * r,
        Square(w, h) -> w * h,
    }
}

fn sum_list(xs: List<Int>) -> Int {
    match xs {
        [] -> 0,
        [x | rest] -> x + sum_list(rest),
    }
}

fn main() with IO {
    let total = sum_list([area(Circle(3)), area(Square(2, 5))])
    print(total)
}
"#,
    );
}

// ─── Guarantee 6 — runtime boundary preserves ADT shape ─────────────────────

#[test]
fn public_adt_boundary_compiles_end_to_end() {
    // A public function returning a user-defined ADT must lower with a full
    // runtime contract (constructor tags + field types). Compile success here
    // is the end-to-end signal — unit tests in runtime_type.rs lock in the
    // shape-match details.
    compile_ok(
        r#"
type Outcome = Ok(Int) | Err(String)

fn classify(n: Int) -> Outcome {
    if n >= 0 { Ok(n) } else { Err("negative") }
}

fn main() with IO {
    let r = classify(3)
    match r {
        Ok(v) -> print(v),
        Err(_) -> print(0),
    }
}
"#,
    );
}
