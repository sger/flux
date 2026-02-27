use flux::bytecode::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
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
fn module_adt_constructor_access_uses_boundary_diagnostic() {
    let code = compile_err(
        "module M { type MaybeInt = SomeInt(Int) | NoneInt } module Main { fn main() { M.SomeInt(1); } }",
    );
    assert_eq!(code, "E084");
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
fn match_tuple_without_catchall_is_conservatively_non_exhaustive() {
    let code = compile_err("let t = (1, true); match t { (1, true) -> 1, (2, false) -> 2 }");
    assert_eq!(code, "E015");
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
