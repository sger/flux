use flux::bytecode::compiler::Compiler;
use flux::diagnostics::{Diagnostic, LabelStyle, render_diagnostics};
use flux::syntax::{lexer::Lexer, parser::Parser};

fn compile_ok_in(file_path: &str, input: &str) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    compiler.compile(&program).expect("expected compile ok");
}

fn compile_err(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn compile_err_in(file_path: &str, input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn compile_err_strict(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.set_strict_mode(true);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn compile_ok_with_warnings_in(file_path: &str, input: &str, strict_mode: bool) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    compiler.set_strict_mode(strict_mode);
    compiler.compile(&program).expect("expected compile ok");
    compiler.take_warnings()
}

fn compile_err_strict_rendered(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.set_strict_mode(true);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    render_diagnostics(&err, None, None)
}

fn compile_err_title(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.title().to_string())
        .unwrap_or_default()
}

fn compile_err_message(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .and_then(|d| d.message().map(ToOwned::to_owned))
        .unwrap_or_default()
}

fn compile_err_rendered(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    render_diagnostics(&err, None, None)
}

fn compile_err_diagnostics(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .expect_err("expected compile error")
}

fn compile_rendered_or_empty(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    match compiler.compile(&program) {
        Ok(()) => String::new(),
        Err(err) => render_diagnostics(&err, None, None),
    }
}

#[test]
fn import_top_level_ok() {
    compile_ok_in(
        "examples/test.flx",
        "import Math module Main { fn main() { 1; } }",
    );
}

#[test]
fn import_in_function_error() {
    let code = compile_err("module Main { fn main() { import Math } }");
    assert_eq!(code, "E017");
}

#[test]
fn import_name_collision_error() {
    let code = compile_err_in("examples/test.flx", "let Math = 1; import Math");
    assert_eq!(code, "E029");
}

#[test]
fn private_member_access_error() {
    let code = compile_err(
        "module Math { fn _private() { 1; } } module Main { fn main() { Math._private(); } }",
    );
    assert_eq!(code, "E011");
}

#[test]
fn non_public_module_function_access_error() {
    let code = compile_err(
        "module Math { fn hidden() { 1; } } module Main { fn main() { Math.hidden(); } }",
    );
    assert_eq!(code, "E011");
}

#[test]
fn module_adt_constructor_access_strict_uses_e086() {
    let code = compile_err_strict(
        "module M { type MaybeInt = SomeInt(Int) | NoneInt } fn main() { M.SomeInt(1); }",
    );
    assert_eq!(code, "E086");
}

#[test]
fn module_adt_constructor_access_non_strict_emits_w201_warning() {
    let warnings = compile_ok_with_warnings_in(
        "examples/test.flx",
        "module M { type MaybeInt = SomeInt(Int) | NoneInt } fn main() { M.SomeInt(1); }",
        false,
    );
    assert!(
        warnings.iter().any(|d| d.code() == Some("W201")),
        "expected W201 warning, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.code(), d.title().to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn constructor_pattern_arity_mismatch_uses_e085() {
    let code = compile_err(
        "type BoxI = BoxI(Int) | EmptyI fn main() -> Unit { let _x = match BoxI(1) { BoxI(a, b) -> a, EmptyI -> 0 } }",
    );
    assert_eq!(code, "E085");
}

#[test]
fn constructor_call_arity_mismatch_uses_diagnostic_not_panic() {
    let rendered = compile_err_rendered(
        r#"
type BoxI = BoxI(Int) | EmptyI
fn main() -> Unit {
    let _x = BoxI(1, 2)
}
"#,
    );
    assert!(
        rendered.contains("error[E082]"),
        "expected E082 constructor call arity diagnostic, got:\n{}",
        rendered
    );
}

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
    compile_ok_in(
        "test.flx",
        "let x = true; match x { true -> 1, false -> 0 };",
    );
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
    compile_ok_in(
        "test.flx",
        "let xs = [1, 2]; match xs { [] -> 0, [h | t] -> h };",
    );
}

#[test]
fn match_guarded_wildcard_only_non_exhaustive_error() {
    let code = compile_err("let x = 2; match x { _ if x > 0 -> 1 }");
    assert_eq!(code, "E015");
}

#[test]
fn guarded_wildcard_only_reports_targeted_e015_message() {
    let rendered = compile_err_rendered("let x = 2; match x { _ if x > 0 -> 1 }");
    assert!(
        rendered.contains("guarded wildcard"),
        "expected targeted guarded wildcard message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("guard may fail"),
        "expected guarded wildcard explanation, got:\n{}",
        rendered
    );
}

#[test]
fn guarded_wildcard_with_bare_fallback_is_exhaustive() {
    compile_ok_in(
        "test.flx",
        "let x = 2; match x { _ if x > 0 -> 1, _ -> 2 };",
    );
}

#[test]
fn match_tuple_without_catchall_is_conservatively_non_exhaustive() {
    let code = compile_err("let t = (1, true); match t { (1, true) -> 1, (2, false) -> 2 }");
    assert_eq!(code, "E015");
}

#[test]
fn match_tuple_without_catchall_reports_tuple_conservative_message() {
    let rendered = compile_err_rendered(include_str!(
        "../examples/type_system/failing/157_match_tuple_missing_catchall_general.flx"
    ));
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for tuple conservative fixture 157, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("tuple domains is conservatively non-exhaustive"),
        "expected tuple-conservative message for fixture 157, got:\n{}",
        rendered
    );
}

#[test]
fn match_tuple_guarded_only_is_non_exhaustive() {
    let rendered = compile_err_rendered(include_str!(
        "../examples/type_system/failing/158_match_tuple_guarded_only_non_exhaustive.flx"
    ));
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for guarded tuple fixture 158, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("tuple domains is conservatively non-exhaustive"),
        "expected tuple-conservative guarded-arm message for fixture 158, got:\n{}",
        rendered
    );
}

#[test]
fn nested_tuple_mixed_shape_reports_non_exhaustive() {
    let rendered = compile_err_rendered(include_str!(
        "../examples/type_system/failing/159_match_nested_tuple_mixed_shape_non_exhaustive.flx"
    ));
    assert!(
        rendered.contains("error[E083]"),
        "expected E083 for nested tuple mixed-shape fixture 159, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("nested tuple patterns"),
        "expected nested tuple mixed-shape message for fixture 159, got:\n{}",
        rendered
    );
}

#[test]
fn nested_tuple_with_catchall_compiles() {
    compile_ok_in(
        "test.flx",
        include_str!("../examples/type_system/160_match_nested_tuple_with_catchall_ok.flx"),
    );
}

#[test]
fn match_bool_guarded_only_reports_deterministic_missing_order() {
    let message = compile_err_message("let b = true; match b { _ if b -> 1 }");
    assert!(
        message.contains("true, false"),
        "expected deterministic Bool missing-order `true, false`, got: {message}"
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
    assert_eq!(code, "E083");
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
fn adt_match_mixed_constructor_spaces_reports_e083() {
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
    assert_eq!(code, "E083");
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

#[test]
fn strict_public_function_typed_contract_is_unsupported() {
    let code = compile_err_strict(
        r#"
public fn apply(f: (Int) -> Bool, x: Int) -> Bool {
    f(x)
}
fn main() -> Unit {
    apply(\(n: Int) -> n > 0, 1)
}
"#,
    );
    assert_eq!(code, "E424");
}

#[test]
fn strict_unresolved_generic_boundary_reports_error() {
    let code = compile_err_strict(
        r#"
public fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    id(1)
}
"#,
    );
    assert_eq!(code, "E425");
}

#[test]
fn hm_polymorphic_reuse_across_concrete_types_ok() {
    compile_ok_in(
        "test.flx",
        r#"
fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    let n = id(1)
    let s = id("ok")
}
"#,
    );
}

#[test]
fn hm_typed_mixed_numeric_add_is_compile_mismatch() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let x: Int = 1 + 2.5
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn hm_pattern_binding_from_some_is_constrained() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let x: String = match Some(1) {
        Some(v) -> v,
        None -> 0,
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn hm_if_concrete_branch_mismatch_reports_contextual_message() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#,
    );
    assert_eq!(code, "E300");

    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#,
    );
    assert!(
        rendered.contains("The branches of this `if` expression produce different types."),
        "expected contextual if mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_if_any_or_unresolved_branch_does_not_report_contextual_e300() {
    let unresolved_rendered = compile_rendered_or_empty(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { mystery_value }
}
"#,
    );
    assert!(
        !unresolved_rendered
            .contains("The branches of this `if` expression produce different types."),
        "did not expect contextual if mismatch text for unresolved/Any branch, got:\n{}",
        unresolved_rendered
    );

    let nested_any_rendered = compile_rendered_or_empty(
        r#"
fn concrete_fn(x: Int) -> Int { x }
fn any_param_fn(x: Any) -> Int { 0 }
fn main() -> Unit {
    let _f = if true { concrete_fn } else { any_param_fn }
}
"#,
    );
    assert!(
        !nested_any_rendered
            .contains("The branches of this `if` expression produce different types."),
        "did not expect contextual if mismatch text for nested Any branch type, got:\n{}",
        nested_any_rendered
    );
}

#[test]
fn hm_fixture_134_if_concrete_branch_mismatch_reports_contextual_message() {
    let source =
        include_str!("../examples/type_system/failing/134_if_concrete_branch_mismatch.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("The branches of this `if` expression produce different types."),
        "expected contextual if mismatch text for fixture 134, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_135_if_nested_any_branch_suppresses_contextual_message() {
    let source = include_str!("../examples/type_system/failing/135_if_any_branch_suppressed.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        !rendered.contains("The branches of this `if` expression produce different types."),
        "did not expect contextual if mismatch text for fixture 135, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("E056"),
        "expected fixture 135 to fail due to follow-up known call arity error, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_136_tuple_projection_uses_precise_hm_type() {
    let source =
        include_str!("../examples/type_system/failing/136_tuple_projection_precise_mismatch.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 136, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("String"),
        "expected typed-let mismatch details with Int/String for fixture 136, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "known tuple projection should not be unresolved in fixture 136, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_137_tuple_projection_unresolved_behavior_is_stable() {
    let source = include_str!(
        "../examples/type_system/failing/137_tuple_projection_unresolved_path_unchanged.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected strict unresolved boundary error for fixture 137, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("does not match its type annotation"),
        "did not expect contextual typed-let mismatch noise for unresolved fixture 137, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_138_match_scrutinee_constraint_propagates() {
    let source = include_str!(
        "../examples/type_system/failing/138_match_scrutinee_constraint_propagates.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 138, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("String")
            && rendered.contains("Int"),
        "expected constrained downstream typed-let mismatch details for fixture 138, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_139_match_scrutinee_constraint_mixed_family_no_propagation() {
    let source = include_str!(
        "../examples/type_system/failing/139_match_scrutinee_constraint_no_propagation_mixed_family.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E056]"),
        "expected independent follow-up E056 for fixture 139, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("The arms of this `match` expression produce different types."),
        "did not expect contextual match-arm mismatch noise for fixture 139, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_142_bool_missing_true_emits_e015() {
    let source = include_str!("../examples/type_system/failing/142_match_bool_missing_true.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for fixture 142, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Match is non-exhaustive: missing Bool case(s): true."),
        "expected missing-true Bool exhaustiveness message for fixture 142, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_143_bool_missing_false_emits_e015() {
    let source = include_str!("../examples/type_system/failing/143_match_bool_missing_false.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for fixture 143, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Match is non-exhaustive: missing Bool case(s): false."),
        "expected missing-false Bool exhaustiveness message for fixture 143, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_144_guarded_wildcard_only_targeted_message() {
    let source = include_str!(
        "../examples/type_system/failing/144_guarded_wildcard_only_non_exhaustive_targeted.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E015]"),
        "expected E015 for fixture 144, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("guarded wildcard"),
        "expected targeted guarded wildcard message for fixture 144, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_151_array_literal_concrete_conflict_prefers_e300() {
    let source = include_str!(
        "../examples/type_system/failing/151_array_literal_concrete_conflict_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 151, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect strict unresolved E425 when concrete array mismatch already exists in fixture 151, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_152_array_literal_callarg_conflict_prefers_e300() {
    let source = include_str!(
        "../examples/type_system/failing/152_array_literal_callarg_conflict_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 152, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect strict unresolved E425 when concrete array arg mismatch already exists in fixture 152, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_153_match_branch_conflict_prefers_e300() {
    let source =
        include_str!("../examples/type_system/failing/153_match_branch_conflict_prefers_e300.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 153, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_154_unresolved_projection_strict_e425() {
    let source =
        include_str!("../examples/type_system/failing/154_unresolved_projection_strict_e425.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for fixture 154, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_155_unresolved_member_access_strict_e425() {
    let source = include_str!(
        "../examples/type_system/failing/155_unresolved_member_access_strict_e425.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for fixture 155, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_156_unresolved_call_arg_strict_e425() {
    let source =
        include_str!("../examples/type_system/failing/156_unresolved_call_arg_strict_e425.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for fixture 156, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E300]"),
        "did not expect concrete E300 for unresolved call-arg fixture 156, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_146_constructor_pattern_arity_some_too_many() {
    let source = include_str!(
        "../examples/type_system/failing/146_constructor_pattern_arity_some_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E085]"),
        "expected E085 for fixture 146, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_147_constructor_pattern_arity_none_too_many() {
    let source = include_str!(
        "../examples/type_system/failing/147_constructor_pattern_arity_none_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E085]"),
        "expected E085 for fixture 147, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_148_constructor_pattern_arity_left_too_many() {
    let source = include_str!(
        "../examples/type_system/failing/148_constructor_pattern_arity_left_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E085]"),
        "expected E085 for fixture 148, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_149_cross_module_constructor_access_strict() {
    let source = r#"
module M { type MaybeInt = SomeInt(Int) | NoneInt }
fn main() { M.SomeInt(1); }
"#;
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E086]"),
        "expected E086 for fixture 149 in strict mode, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_150_cross_module_constructor_access_nonstrict_warning() {
    let source = r#"
module M { type MaybeInt = SomeInt(Int) | NoneInt }
fn main() { M.SomeInt(1); }
"#;
    let warnings = compile_ok_with_warnings_in("examples/test.flx", source, false);
    assert!(
        warnings.iter().any(|d| d.code() == Some("W201")),
        "expected W201 warning for fixture 150, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.code(), d.title().to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn hm_match_concrete_arm_mismatch_reports_contextual_message() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
    }
}
"#,
    );
    assert_eq!(code, "E300");

    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
    }
}
"#,
    );
    assert!(
        rendered.contains("The arms of this `match` expression produce different types."),
        "expected contextual match-arm mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_call_arg_named_fn_reports_contextual_message_and_definition_site() {
    let rendered = compile_err_rendered(
        r#"
fn greet(name: String) -> String { name }
fn main() -> Unit {
    let _x = greet(42)
}
"#,
    );
    assert!(
        rendered.contains("The 1st argument to `greet` has the wrong type."),
        "expected named call-arg mismatch message, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_131_call_arg_primary_label_is_argument_subspan() {
    let source = include_str!("../examples/type_system/failing/131_call_arg_span_precision.flx");
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message()
                    .is_some_and(|m| m.contains("The 2nd argument to `pair` has the wrong type."))
        })
        .expect("expected contextual call-arg E300 diagnostic for fixture 131");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 131 diagnostic");
    assert_eq!(primary.span.start.line, 4);
    assert_eq!(primary.span.start.column, 21);
}

#[test]
fn hm_call_arg_unresolved_or_any_paths_do_not_report_contextual_message() {
    let unresolved_rendered = compile_rendered_or_empty(
        r#"
fn main() -> Unit {
    let _x = unknown_fn(42)
}
"#,
    );
    assert!(
        !unresolved_rendered.contains("argument to `"),
        "did not expect contextual call-arg mismatch text for unresolved callee, got:\n{}",
        unresolved_rendered
    );

    let nested_any_rendered = compile_rendered_or_empty(
        r#"
fn accepts_any_param_fn(f: (Any) -> Int) -> Int { f(0) }
fn concrete_fn(x: Int) -> Int { x }
fn main() -> Unit {
    let _x = accepts_any_param_fn(concrete_fn)
}
"#,
    );
    assert!(
        !nested_any_rendered.contains("argument to `accepts_any_param_fn` has the wrong type."),
        "did not expect contextual call-arg mismatch text when expected type contains Any, got:\n{}",
        nested_any_rendered
    );
}

#[test]
fn hm_let_annotation_mismatch_reports_dual_labels() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let x: Int = "hello"
}
"#,
    );
    assert!(
        rendered.contains("The value of `x` does not match its type annotation."),
        "expected contextual let annotation mismatch message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Change `x` to a `Int` value or update the annotation to `String`."),
        "expected actionable let-annotation help text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_132_let_annotation_primary_label_is_initializer_subspan() {
    let source =
        include_str!("../examples/type_system/failing/132_let_initializer_span_precision.flx");
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The value of `x` does not match its type annotation.")
                })
        })
        .expect("expected contextual let-annotation E300 diagnostic for fixture 132");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 132 diagnostic");
    assert_eq!(primary.span.start.line, 2);
    assert_eq!(primary.span.start.column, 17);
}

#[test]
fn hm_function_return_annotation_mismatch_reports_dual_labels() {
    let rendered = compile_err_rendered(
        r#"
fn add() -> Int {
    "oops"
}
fn main() -> Unit { add() }
"#,
    );
    assert!(
        rendered.contains("The return value of `add` does not match its declared return type."),
        "expected contextual return-annotation mismatch message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Return a `Int` value from `add` or change the declared return type."),
        "expected actionable return-annotation help text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_133_if_primary_label_is_else_value_subspan() {
    let source =
        include_str!("../examples/type_system/failing/133_if_branch_value_span_precision.flx");
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The branches of this `if` expression produce different types.")
                })
        })
        .expect("expected contextual if-branch E300 diagnostic for fixture 133");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 133 diagnostic");
    let secondary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Secondary)
        .expect("expected secondary label on fixture 133 diagnostic");
    assert_eq!(primary.span.start.line, 2);
    assert_eq!(primary.span.start.column, 35);
    assert_eq!(secondary.span.start.line, 2);
    assert_eq!(secondary.span.start.column, 23);
}

#[test]
fn known_call_too_many_args_emits_e056() {
    let code = compile_err(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2, 3)
}
"#,
    );
    assert_eq!(code, "E056");

    let rendered = compile_err_rendered(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2, 3)
}
"#,
    );
    assert!(
        rendered.contains("WRONG NUMBER OF ARGUMENTS"),
        "expected E056 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Remove 1 extra argument(s), for example: `add(arg1, arg2)`."),
        "expected actionable too-many-args hint in rendered diagnostics, got:\n{}",
        rendered
    );
}

#[test]
fn known_call_too_few_args_emits_e056() {
    let code = compile_err(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1)
}
"#,
    );
    assert_eq!(code, "E056");

    let rendered = compile_err_rendered(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1)
}
"#,
    );
    assert!(
        rendered.contains("takes 2 arguments, but 1 were provided"),
        "expected contextual arity text in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Add 1 missing argument(s), for example: `add(arg1, arg2)`."),
        "expected actionable too-few-args hint in rendered diagnostics, got:\n{}",
        rendered
    );
}

#[test]
fn known_call_correct_arity_no_e056() {
    compile_ok_in(
        "<unknown>",
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2)
}
"#,
    );
}

#[test]
fn pass2_multi_error_continuation_reports_independent_errors_in_order() {
    let source = include_str!("../examples/type_system/failing/99_multi_error_continuation.flx");
    let rendered = compile_err_rendered(source);

    let e002_idx = rendered
        .find("error[E002]")
        .expect("expected E002 immutability error in rendered diagnostics");
    let e300_idx = rendered
        .find("error[E300]")
        .expect("expected E300 type-unification error in rendered diagnostics");

    assert!(
        e002_idx < e300_idx,
        "expected deterministic source-order diagnostics (E002 before E300), got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_161_tuple_destructure_concrete_mismatch_prefers_e300() {
    let source = include_str!(
        "../examples/type_system/failing/161_tuple_destructure_concrete_mismatch_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 161, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 for concrete destructure mismatch fixture 161, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_162_tuple_destructure_unresolved_strict_e425() {
    let source = include_str!(
        "../examples/type_system/failing/162_tuple_destructure_unresolved_strict_e425.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for unresolved destructure fixture 162, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_163_match_concrete_disagreement_prefers_e300() {
    let source = include_str!(
        "../examples/type_system/failing/163_match_concrete_disagreement_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 163, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_164_match_unresolved_arm_stays_suppressed() {
    let source = include_str!(
        "../examples/type_system/failing/164_match_unresolved_arm_stays_suppressed.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        !rendered.contains("The arms of this `match` expression produce different types."),
        "did not expect contextual match-arm mismatch for unresolved-arm fixture 164, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_165_self_recursive_precision_prefers_e300() {
    let source = include_str!(
        "../examples/type_system/failing/165_self_recursive_precision_prefers_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 165, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_166_self_recursive_guard_stable_unresolved() {
    let source = include_str!(
        "../examples/type_system/failing/166_self_recursive_guard_stable_unresolved.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E004]"),
        "expected unresolved symbol baseline for fixture 166, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("The function return type does not match its annotation."),
        "did not expect recursion hardening to introduce unrelated return-mismatch noise for fixture 166, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_167_tuple_destructure_ordered_concrete_conflict_e300() {
    let source = include_str!(
        "../examples/type_system/failing/167_tuple_destructure_ordered_concrete_conflict_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 167, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 for concrete tuple-destructure conflict fixture 167, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_168_tuple_destructure_unresolved_guard_strict_e425() {
    let source = include_str!(
        "../examples/type_system/failing/168_tuple_destructure_unresolved_guard_strict_e425.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for unresolved tuple-destructure fixture 168, got:\n{}",
        rendered
    );
}

#[test]
fn strict_only_unresolved_fixtures_are_e425_in_strict_but_may_be_e004_in_non_strict() {
    let strict_only = [
        include_str!("../examples/type_system/failing/154_unresolved_projection_strict_e425.flx"),
        include_str!(
            "../examples/type_system/failing/155_unresolved_member_access_strict_e425.flx"
        ),
        include_str!("../examples/type_system/failing/156_unresolved_call_arg_strict_e425.flx"),
        include_str!(
            "../examples/type_system/failing/162_tuple_destructure_unresolved_strict_e425.flx"
        ),
        include_str!(
            "../examples/type_system/failing/168_tuple_destructure_unresolved_guard_strict_e425.flx"
        ),
    ];

    for source in strict_only {
        let strict_rendered = compile_err_strict_rendered(source);
        assert!(
            strict_rendered.contains("error[E425]"),
            "expected strict E425 for strict-only unresolved fixture, got:\n{}",
            strict_rendered
        );

        let non_strict_rendered = compile_err_rendered(source);
        assert!(
            non_strict_rendered.contains("error[E004]"),
            "expected non-strict unresolved baseline E004 for strict-only unresolved fixture, got:\n{}",
            non_strict_rendered
        );
    }
}

#[test]
fn hm_fixture_169_match_disagreement_first_arm_unresolved_still_e300() {
    let source = include_str!(
        "../examples/type_system/failing/169_match_disagreement_first_arm_unresolved_still_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 169, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 to mask concrete match disagreement in fixture 169, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_170_match_disagreement_all_concrete_ordering_invariant_e300() {
    let source = include_str!(
        "../examples/type_system/failing/170_match_disagreement_all_concrete_ordering_invariant_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 170, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_171_self_recursive_refinement_concrete_chain_e300() {
    let source = include_str!(
        "../examples/type_system/failing/171_self_recursive_refinement_concrete_chain_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 171, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_172_self_recursive_unresolved_guard_no_false_positive() {
    let source = include_str!(
        "../examples/type_system/failing/172_self_recursive_unresolved_guard_no_false_positive.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E004]"),
        "expected unresolved symbol baseline for fixture 172, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("The function return type does not match its annotation."),
        "did not expect recursion hardening to introduce unrelated return-mismatch noise for fixture 172, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_param_mismatch_reports_param_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn takes_int(x: Int) -> Int { x }
fn takes_string(x: String) -> Int { 0 }
fn main() -> Unit {
    let _f = if true {
        takes_int
    } else {
        takes_string
    }
}
"#,
    );
    assert!(
        rendered
            .contains("Function parameter 1 type does not match: expected `Int`, found `String`."),
        "expected function parameter mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_return_mismatch_reports_return_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn ret_int() -> Int { 1 }
fn ret_string() -> String { "x" }
fn main() -> Unit {
    let _f = if true {
        ret_int
    } else {
        ret_string
    }
}
"#,
    );
    assert!(
        rendered.contains("Function return types do not match: expected `Int`, found `String`."),
        "expected function return mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_arity_mismatch_reports_arity_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn one_arg(x: Int) -> Int { x }
fn two_args(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit {
    let _f = if true {
        one_arg
    } else {
        two_args
    }
}
"#,
    );
    assert!(
        rendered.contains("Function arity does not match."),
        "expected function arity mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn perform_arg_type_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with IO {
    let _x = (perform Console.print(1)) handle Console {
        print(_resume, _msg) -> None
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn perform_wrong_arity_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with IO {
    (perform Console.print()) handle Console {
        print(resume, _msg) -> resume(())
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn handle_arm_param_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    1 handle Console {
        print(resume) -> resume(())
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn handle_arm_result_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    1 handle Console {
        print(resume, _msg) -> "oops"
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn valid_handle_with_resume_and_correct_types_compiles() {
    compile_ok_in(
        "test.flx",
        r#"
effect Console {
    print: String -> Int
}
fn run() -> Int with Console {
    perform Console.print("x")
}
fn main() -> Unit with IO {
    let _ = run() handle Console {
        print(resume, _msg) -> resume(1)
    }
}
"#,
    );
}

#[test]
fn strict_member_access_non_module_path_reports_unresolved_boundary() {
    let code = compile_err_strict(
        r#"
fn main() -> Unit {
    let h = { "a": 1 }
    let x: Int = h.a
}
"#,
    );
    assert_eq!(code, "E425");
}

#[test]
fn strict_unresolved_generic_boundary_has_stable_diagnostic_shape() {
    let rendered = compile_err_strict_rendered(
        r#"
public fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    id(1)
}
"#,
    );
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 in rendered diagnostics:\n{}",
        rendered
    );
    assert!(
        rendered.contains("STRICT UNRESOLVED BOUNDARY TYPE"),
        "expected title in rendered diagnostics:\n{}",
        rendered
    );
    assert!(
        rendered.contains("unresolved expression type in function return expression"),
        "expected unresolved-context message in rendered diagnostics:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_perform_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with IO {
    perform I.print("x")
}
"#,
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in perform diagnostic, got:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_function_annotation_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with I {
}
"#,
    );
    assert!(
        rendered.contains("error[E407]"),
        "expected E407 in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("UNKNOWN FUNCTION EFFECT"),
        "expected E407 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in function-annotation diagnostic, got:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_handle_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with IO {
    1 handle I {
        print(resume, _msg) -> resume(())
    }
}
"#,
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in handle diagnostic, got:\n{}",
        rendered
    );
}

#[test]
fn forward_reference_nested_call() {
    // Function a calls b, b calls c, c is defined last
    compile_ok_in("test.flx", "fn a() { b(); } fn b() { c(); } fn c() { 42; }");
}

#[test]
fn mutual_recursion_two_functions() {
    // Functions f and g call each other
    compile_ok_in(
        "test.flx",
        "fn f(x) { if x > 0 { g(x - 1); } else { 0; } } fn g(x) { if x > 0 { f(x - 1); } else { 1; } }",
    );
}

#[test]
fn mutual_recursion_three_functions() {
    // Functions a, b, c form a circular dependency
    compile_ok_in(
        "test.flx",
        "fn a(x) { if x > 0 { b(x - 1); } else { 0; } } fn b(x) { if x > 0 { c(x - 1); } else { 1; } } fn c(x) { if x > 0 { a(x - 1); } else { 2; } }",
    );
}

#[test]
fn self_recursion_still_works() {
    // Ensure basic recursion still works
    compile_ok_in(
        "test.flx",
        "fn factorial(n) { if n < 2 { 1; } else { n * factorial(n - 1); } }",
    );
}

#[test]
fn hm_fixture_140_recursive_self_reference_refines_type() {
    let source = include_str!(
        "../examples/type_system/failing/140_recursive_self_reference_return_precision.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E300]"),
        "expected E300 for fixture 140, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("String")
            && rendered.contains("Int"),
        "expected concrete downstream typed-let mismatch in fixture 140, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_141_recursive_self_reference_guard_no_regression() {
    let source = include_str!(
        "../examples/type_system/failing/141_recursive_self_reference_negative_guard.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E056]"),
        "expected independent follow-up E056 in fixture 141, got:\n{}",
        rendered
    );
}

#[test]
fn import_base_as_alias_is_rejected() {
    let code = compile_err("import Base as Core");
    assert_eq!(code, "E078");
}

#[test]
fn import_base_except_hides_unqualified_name() {
    let code = compile_err("import Base except [print]\nprint(1);");
    assert_eq!(code, "E004");
}

#[test]
fn import_base_except_keeps_qualified_access() {
    compile_ok_in("test.flx", "import Base except [print]\nBase.print(1);");
}

#[test]
fn import_base_except_unknown_name_is_error() {
    let code = compile_err("import Base except [does_not_exist]");
    assert_eq!(code, "E080");
}

#[test]
fn import_base_except_duplicate_name_is_error() {
    let code = compile_err("import Base except [print, print]");
    assert_eq!(code, "E079");
}

#[test]
fn import_non_base_except_is_accepted() {
    compile_ok_in("test.flx", "import Foo except [drop]\n1;");
}

#[test]
fn import_non_base_alias_except_is_accepted() {
    compile_ok_in("test.flx", "import Foo as F except [drop]\n1;");
}

#[test]
fn base_qualified_unknown_member_is_error() {
    let code = compile_err("Base.not_real();");
    assert_eq!(code, "E080");
}

#[test]
fn top_level_binding_can_shadow_base_name() {
    compile_ok_in(
        "test.flx",
        r#"
let len = fn(x) { 42; };
len([1, 2, 3]);
Base.len([1, 2, 3]);
"#,
    );
}

#[test]
fn forward_reference_with_variables() {
    // Forward reference with let bindings in between
    compile_ok_in("test.flx", "fn f() { g(); } let x = 10; fn g() { x; }");
}

#[test]
fn duplicate_function_still_errors() {
    // Ensure duplicate function names still produce an error
    let code = compile_err("fn f() { 1; } fn f() { 2; }");
    assert_eq!(code, "E001");
}

#[test]
fn module_forward_reference() {
    // Function in module uses another function defined later in the same module
    compile_ok_in(
        "test.flx",
        "module Math { fn quadruple(x) { double(double(x)); } fn double(x) { x * 2; } }",
    );
}

#[test]
fn module_mutual_recursion() {
    // Functions within a module call each other
    compile_ok_in(
        "test.flx",
        "module Parity { fn isEven(n) { if n == 0 { true; } else { isOdd(n - 1); } } fn isOdd(n) { if n == 0 { false; } else { isEven(n - 1); } } }",
    );
}
