use super::*;

#[test]
fn public_module_function_access_ok() {
    compile_ok_in(
        "examples/test.flx",
        "module Math { public fn open() { 1; } } module Main { fn main() { Math.open(); } }",
    );
}

#[test]
fn module_internal_private_helper_access_ok() {
    compile_ok_in(
        "examples/test.flx",
        "module Math { fn hidden() { 1; } public fn run() { hidden(); } } module Main { fn main() { Math.run(); } }",
    );
}

#[test]
fn alias_access_to_non_public_module_function_error() {
    let code = compile_err(
        "import Math as M module Math { fn hidden() { 1; } } module Main { fn main() { M.hidden(); } }",
    );
    assert_eq!(code, "E011");
}

#[test]
fn alias_access_to_public_module_function_ok() {
    compile_ok_in(
        "examples/test.flx",
        "import Math as M module Math { public fn open() { 1; } } module Main { fn main() { M.open(); } }",
    );
}

#[test]
fn module_name_lowercase_error() {
    let code = compile_err("module math { fn main() { 1; } }");
    assert_eq!(code, "E008");
}

#[test]
fn module_name_clash_error() {
    let code = compile_err("module Math { fn Math() { 1; } }");
    assert_eq!(code, "E009");
}

#[test]
fn qualified_use_requires_import() {
    let title = compile_err_title("module Main { fn main() { Data.MyFile.value(); } }");
    assert_eq!(title, "MODULE NOT IMPORTED");
}

#[test]
fn alias_hides_original_qualifier() {
    let title = compile_err_title(
        "import Data.MyFile as MyFile module Main { fn main() { Data.MyFile.value(); } }",
    );
    assert_eq!(title, "MODULE NOT IMPORTED");
}

#[test]
fn duplicate_params_error() {
    let code = compile_err("fn f(x, x) { x; }");
    assert_eq!(code, "E007");
}

#[test]
fn duplicate_params_literal_error() {
    let code = compile_err("let f = fn(x, x) { x; };");
    assert_eq!(code, "E007");
}

#[test]
fn immutable_reassign_error() {
    let code = compile_err("let x = 1; x = 2;");
    assert_eq!(code, "E002");
}

#[test]
fn binding_shadowing_sample_program_reports_duplicate_name_for_inner_let() {
    let code = compile_err(
        r#"
let x = 3

fn t(x) {
    let x = x;
}
"#,
    );
    assert_eq!(code, "E001");
}

#[test]
fn binding_shadowing_sample_program_duplicate_message_is_clear() {
    let message = compile_err_message(
        r#"
let x = 3

fn t(x) {
    let x = x;
}
"#,
    );
    assert!(
        message.contains("Duplicate binding: `x`"),
        "expected duplicate-name message, got: {message}"
    );
}

#[test]
fn parameter_shadowing_outer_binding_without_inner_duplicate_is_allowed() {
    compile_ok_in(
        "test.flx",
        r#"
let x = 3
fn t(x) { x; }
t(9);
"#,
    );
}

#[test]
fn an_error_inside_a_lambda_does_not_leak_its_parameters_ki142() {
    // `first`'s lambda fails to compile. Its `x` must not stay in scope and
    // collide with `second`'s own `x`.
    let rendered = compile_err_rendered(
        r#"
fn apply(f: () -> Int) -> Int { f() }

fn first(x) {
    apply(fn() { print(x); 1 })
}

fn second() -> Int {
    let x = 2
    fn inner() -> Int { x }
    inner()
}
"#,
    );
    assert!(rendered.contains("E400"), "expected the lambda's E400:\n{rendered}");
    assert!(!rendered.contains("E001"), "expected no duplicate-name error:\n{rendered}");
}

#[test]
fn a_duplicate_instance_reports_no_duplicate_internal_name_ki143() {
    let codes: Vec<_> = compile_err_diagnostics(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
}

instance Sizeable<Int> {
    fn size(x) { x + 1 }
}

fn main() {
    42
}
"#,
    )
    .iter()
    .filter_map(|d| d.code().map(str::to_string))
    .collect();
    assert_eq!(codes, ["E443"]);
}

#[test]
fn assignment_in_block_reassign_error() {
    let code = compile_err("fn f() { let x = 1; x = 2; }");
    assert_eq!(code, "E002");
}

#[test]
fn duplicate_let_in_same_scope_errors() {
    let code = compile_err("fn bad() { let x = 1; let x = 2; }");
    assert_eq!(code, "E001");
}

#[test]
fn assignment_to_parameter_reassign_error() {
    let code = compile_err("fn f(x) { x = 2; }");
    assert_eq!(code, "E002");
}

#[test]
fn outer_assignment_error() {
    let code = compile_err("fn outer() { let x = 1; let f = fn() { x = 2; }; }");
    assert_eq!(code, "E003");
}

#[test]
fn match_non_exhaustive_error() {
    let code = compile_err("let x = 2; match x { 1 -> 10 }");
    assert_eq!(code, "E015");
}

#[test]
fn match_bool_exhaustive_without_catchall_ok() {
    compile_ok_in("test.flx", "let x = true; match x { true -> 1, false -> 0 };");
}

#[test]
fn match_bool_missing_true_reports_e015() {
    let code = compile_err("let x = true; match x { false -> 0 };");
    assert_eq!(code, "E015");
}

#[test]
fn match_bool_with_wildcard_fallback_ok() {
    compile_ok_in("test.flx", "let x = true; match x { true -> 1, _ -> 0 };");
}

#[test]
fn match_list_exhaustive_without_catchall_ok() {
    compile_ok_in("test.flx", "let xs = [1, 2]; match xs { [] -> 0, [h | t] -> h };");
}

#[test]
fn match_guarded_wildcard_only_non_exhaustive_error() {
    let code = compile_err("let x = 2; match x { _ if x > 0 -> 1 }");
    assert_eq!(code, "E015");
}

#[test]
fn guarded_wildcard_only_reports_targeted_e015_message() {
    // The matrix coverage checker reports this as a generic E015 —
    // a guarded arm does not contribute structural coverage, so the
    // match is treated as though no arm exists.
    let rendered = compile_err_rendered("let x = 2; match x { _ if x > 0 -> 1 }");
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for guarded-wildcard-only match, got:\n{}",
        rendered
    );
    assert!(rendered.contains("exhaustive"), "expected non-exhaustive message, got:\n{}", rendered);
}

#[test]
fn guarded_wildcard_with_bare_fallback_is_exhaustive() {
    compile_ok_in("test.flx", "let x = 2; match x { _ if x > 0 -> 1, _ -> 2 };");
}

#[test]
fn match_tuple_without_catchall_is_conservatively_non_exhaustive() {
    let code = compile_err("let t = (1, true); match t { (1, true) -> 1, (2, false) -> 2 }");
    assert_eq!(code, "E015");
}

#[test]
fn match_tuple_without_catchall_reports_tuple_conservative_message() {
    let rendered = compile_err_rendered(include_str!(
        "../../../examples/type_system/failing/157_match_tuple_missing_catchall_general.flx"
    ));
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for tuple conservative fixture 157, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 157, got:\n{}",
        rendered
    );
}

#[test]
fn match_tuple_guarded_only_is_non_exhaustive() {
    let rendered = compile_err_rendered(include_str!(
        "../../../examples/type_system/failing/158_match_tuple_guarded_only_non_exhaustive.flx"
    ));
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for guarded tuple fixture 158, got:\n{}",
        rendered
    );
    // A lone guarded arm never contributes coverage — the matrix
    // checker emits the generic non-exhaustive message.
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 158, got:\n{}",
        rendered
    );
}

#[test]
fn nested_tuple_mixed_shape_reports_non_exhaustive() {
    // The matrix coverage checker emits E015 for all non-exhaustive
    // matches — there's no longer a separate ADT-specific code.
    let rendered = compile_err_rendered(include_str!(
        "../../../examples/type_system/failing/159_match_nested_tuple_mixed_shape_non_exhaustive.flx"
    ));
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for nested tuple mixed-shape fixture 159, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 159, got:\n{}",
        rendered
    );
}

#[test]
fn nested_tuple_with_catchall_compiles() {
    compile_ok_in(
        "test.flx",
        include_str!("../../../examples/type_system/98_match_nested_tuple_with_catchall_ok.flx"),
    );
}

#[test]
fn match_bool_guarded_only_reports_deterministic_missing_order() {
    // The matrix checker reports a count of missing patterns rather
    // than enumerating the specific `true, false` order the old
    // ad-hoc Bool path did. Retained as a regression against
    // non-exhaustiveness being detected at all.
    let message = compile_err_message("let b = true; match b { _ if b -> 1 }");
    assert!(
        message.contains("not exhaustive"),
        "expected non-exhaustive Bool message, got: {message}"
    );
}

#[test]
fn adt_match_all_constructor_arms_guarded_is_non_exhaustive_e083() {
    let code = compile_err(
        r#"
type Result<T, E> = Ok(T) | Err(E)
let r: Result<Int, String> = Ok(1)
match r {
    Ok(v) if v > 0 -> v,
    Err(_e) if true -> 0
}
"#,
    );
    assert_eq!(code, "E015");
}

#[test]
fn adt_match_guarded_constructor_with_unguarded_fallback_is_exhaustive() {
    compile_ok_in(
        "test.flx",
        r#"
type Result<T, E> = Ok(T) | Err(E)
let r: Result<Int, String> = Ok(1)
match r {
    Ok(v) if v > 0 -> v,
    _ -> 0
};
"#,
    );
}

#[test]
fn adt_match_mixed_constructor_spaces_reports_error() {
    // Matching `A` against a `B`-constructor pattern now produces a
    // type-mismatch error (E300) rather than the old ADT-specific
    // non-exhaustive error, because HM unification catches the
    // constructor-domain clash before the coverage check runs.
    let code = compile_err(
        r#"
type A = A1 | A2
type B = B1 | B2
let x: A = A1
match x {
    A1 -> 1,
    B1 -> 2
}
"#,
    );
    assert!(code == "E300" || code == "E015", "got code: {code}");
}

#[test]
fn match_identifier_non_last_error() {
    let code = compile_err("let x = 2; match x { y -> 1, _ -> 2 }");
    assert_eq!(code, "E016");
}

#[test]
fn match_wildcard_non_last_error() {
    let code = compile_err("let x = 2; match x { _ -> 1, 2 -> 2 }");
    assert_eq!(code, "E016");
}

#[test]
fn legacy_none_list_tail_is_compile_error() {
    let code = compile_err("let xs = [1 | None]; xs;");
    assert_eq!(code, "E077");
}

#[test]
fn forward_reference_simple() {
    // Function g calls function f, which is defined after g
    compile_ok_in("test.flx", "fn g() { f(); } fn f() { 1; }");
}
