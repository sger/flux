//! Integration tests for Proposal 0151 — module-scoped type classes.
//!
//! Phase 1, Step 1: confirm that the bytecode compiler's module-body validator
//! accepts `class`, `instance`, and `import` declarations inside a `module { }`
//! block. Before Proposal 0151 these were rejected with `INVALID_MODULE_CONTENT`.
//!
//! This file does NOT yet assert that class semantics work end-to-end inside
//! modules — that lands in later Phase 1a/1b commits. The minimum guarantee
//! here is "the validator no longer rejects the source."

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

/// Parse `source` and run it through the bytecode compiler. Returns the list
/// of diagnostics. The test asserts on the diagnostic codes.
fn compile_source(source: &str) -> Vec<flux::diagnostics::Diagnostic> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    match compiler.compile(&program) {
        Ok(()) => Vec::new(),
        Err(diags) => diags,
    }
}

/// Returns true if any diagnostic in `diags` has the `INVALID_MODULE_CONTENT`
/// error code (`E054`). The exact code is checked by string match against the
/// rendered diagnostic to avoid taking a hard dependency on the internal
/// `ErrorCode` type from the test crate.
fn has_invalid_module_content(diags: &[flux::diagnostics::Diagnostic]) -> bool {
    let rendered = render_diagnostics(diags, None, None);
    rendered.contains("Invalid content in module")
}

/// Compile `source` to bytecode, run it through the VM, and return the last
/// value popped from the operand stack. Panics with rendered diagnostics on
/// any compile or runtime error so test failures show useful messages.
fn compile_and_run(source: &str) -> Value {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    if let Err(diags) = compiler.compile(&program) {
        panic!(
            "compile errors:\n{}",
            render_diagnostics(&diags, Some(source), None)
        );
    }

    let bytecode = compiler.bytecode();
    let mut vm = VM::new(bytecode);
    vm.run().unwrap_or_else(|err| panic!("VM error: {err}"));
    vm.last_popped_stack_elem()
}

#[test]
fn module_body_accepts_class_declaration() {
    // Bare class declaration inside a module body. Before Proposal 0151 this
    // hit the catch-all in compile_module_statement and produced
    // INVALID_MODULE_CONTENT. After the validator whitelist update, the
    // compiler must not emit that diagnostic.
    let source = r#"
module Phase1.SmokeClass {
    class Eq2<a> {
        fn eq2(x: a, y: a) -> Bool
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body class should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_body_accepts_instance_declaration() {
    let source = r#"
module Phase1.SmokeInstance {
    class Eq2<a> {
        fn eq2(x: a, y: a) -> Bool
    }

    instance Eq2<Int> {
        fn eq2(x, y) { x == y }
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body instance should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_body_accepts_import_declaration() {
    // Module-body imports are also whitelisted (Proposal 0151 §5a).
    // The import target need not exist for this test — we only care that
    // the validator does not reject the *form*.
    let source = r#"
module Phase1.SmokeImport {
    import Flow.Option as Option

    fn ping() -> Int { 1 }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body import should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

/// End-to-end exploratory test for the surprise win discovered in commit #3:
///
/// `class_env::collect_classes`, `class_env::collect_instances`, and
/// `class_dispatch::generate_from_statements` ALL already recurse into module
/// bodies. The only thing that was blocking module-scoped classes from
/// working end-to-end was the validator at `bytecode/compiler/statement.rs`
/// which we whitelisted in commit #1.
///
/// If the existing dispatch path "just works" for module-scoped classes,
/// this test should compile, run, and produce 42. If something is missing,
/// the failure tells us the next concrete obstacle to fix.
#[test]
fn module_scoped_class_with_int_instance_runs_via_existing_dispatch() {
    let source = r#"
module Phase1.RuntimeUse {
    class DoublerCls<a> {
        fn double_it(x: a) -> a
    }

    instance DoublerCls<Int> {
        fn double_it(x) { x + x }
    }
}

fn main() {
    double_it(21)
}
"#;
    let result = compile_and_run(source);
    assert_eq!(
        result,
        Value::Integer(42),
        "module-scoped class+instance should resolve and run; got {:?}",
        result
    );
}

/// Walks a parsed program and returns `Some(is_public)` for the first
/// `Statement::Class` it finds (recursing into module bodies). Used by the
/// `public class` parser tests.
fn first_class_visibility(program: &flux::syntax::program::Program) -> Option<bool> {
    fn walk(
        statements: &[flux::syntax::statement::Statement],
    ) -> Option<bool> {
        use flux::syntax::statement::Statement;
        for stmt in statements {
            match stmt {
                Statement::Class { is_public, .. } => return Some(*is_public),
                Statement::Module { body, .. } => {
                    if let Some(found) = walk(&body.statements) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&program.statements)
}

/// Same shape as `first_class_visibility` but for the first `Statement::Instance`.
fn first_instance_visibility(program: &flux::syntax::program::Program) -> Option<bool> {
    fn walk(
        statements: &[flux::syntax::statement::Statement],
    ) -> Option<bool> {
        use flux::syntax::statement::Statement;
        for stmt in statements {
            match stmt {
                Statement::Instance { is_public, .. } => return Some(*is_public),
                Statement::Module { body, .. } => {
                    if let Some(found) = walk(&body.statements) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&program.statements)
}

fn parse_program_only(source: &str) -> flux::syntax::program::Program {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );
    program
}

#[test]
fn bare_class_is_private_by_default() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(false));
}

#[test]
fn public_class_sets_is_public_true() {
    let program = parse_program_only(
        r#"
public class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(true));
}

#[test]
fn bare_instance_is_private_by_default() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}

instance Eq2<Int> {
    fn eq2(x, y) { x == y }
}
"#,
    );
    assert_eq!(first_instance_visibility(&program), Some(false));
}

#[test]
fn public_instance_sets_is_public_true() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}

public instance Eq2<Int> {
    fn eq2(x, y) { x == y }
}
"#,
    );
    assert_eq!(first_instance_visibility(&program), Some(true));
}

#[test]
fn module_body_public_class_and_instance_set_is_public_true() {
    // The full target shape: module-scoped public class and public instance,
    // both should round-trip is_public=true through the parser.
    let program = parse_program_only(
        r#"
module Phase1.PublicCheck {
    public class Sizeable<a> {
        fn size_of(x: a) -> Int
    }

    public instance Sizeable<Int> {
        fn size_of(x) { x }
    }
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(true));
    assert_eq!(first_instance_visibility(&program), Some(true));
}

#[test]
fn module_body_still_rejects_unsupported_statements() {
    // Negative regression: a return statement at module body level is still
    // not a valid module member, and the validator must continue to reject it.
    let source = r#"
module Phase1.SmokeReject {
    return 1
}
"#;
    let diags = compile_source(source);
    assert!(
        has_invalid_module_content(&diags),
        "stray return inside module body should still be rejected, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}
