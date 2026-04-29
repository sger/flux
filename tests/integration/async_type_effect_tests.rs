use flux::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

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
    let mut compiler =
        Compiler::new_with_interner("async_type_effect_test.flx".to_string(), interner);
    match compiler.compile(&program) {
        Ok(()) => Vec::new(),
        Err(diags) => diags,
    }
}

#[test]
fn contextual_async_callback_accepts_narrower_effect_body() {
    let source = r#"
effect Suspend { sleep: Int -> Unit }
effect Fork
effect GetContext
effect AsyncFail { raise: String -> a }

alias Async = <Suspend | Fork | GetContext | AsyncFail>

fn both<a, b>(
    left: (() -> a with Async),
    right: (() -> b with Async)
) -> (a, b) with Async {
    (left(), right())
}

fn demo() with Async {
    both(
        fn() {
            perform Suspend.sleep(1);
            20
        },
        fn() {
            22
        }
    )
}
"#;

    let diags = compile_source(source);
    assert!(
        diags.is_empty(),
        "expected contextual async callback to compile, got:\n{}",
        render_diagnostics(&diags, Some(source), None)
    );
}
