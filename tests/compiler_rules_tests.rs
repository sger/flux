use flux::bytecode::compiler::Compiler;
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
