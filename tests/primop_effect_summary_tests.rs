use flux::bytecode::{compiler::Compiler, debug_info::EffectSummary};
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn compile_bytecode(input: &str) -> flux::bytecode::bytecode::Bytecode {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<effect_test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler.bytecode()
}

#[test]
fn main_effect_summary_is_pure_for_pure_primop_only_program() {
    let bytecode = compile_bytecode("len(#[1, 2, 3]);");
    let info = bytecode.debug_info.expect("main debug info");
    assert_eq!(info.effect_summary, EffectSummary::Pure);
}

#[test]
fn main_effect_summary_marks_effectful_primop() {
    let bytecode = compile_bytecode(r#"print("hello");"#);
    let info = bytecode.debug_info.expect("main debug info");
    assert_eq!(info.effect_summary, EffectSummary::HasEffects);
}

#[test]
fn main_effect_summary_is_unknown_for_generic_calls() {
    let bytecode = compile_bytecode(
        r#"
fn id(x) { x }
id(1);
"#,
    );
    let info = bytecode.debug_info.expect("main debug info");
    assert_eq!(info.effect_summary, EffectSummary::Unknown);
}

#[test]
fn function_debug_info_carries_effect_summary() {
    let bytecode = compile_bytecode(
        r#"
fn noisy() { print("x"); }
noisy();
"#,
    );
    let summaries: Vec<_> = bytecode
        .constants
        .iter()
        .filter_map(|value| match value {
            Value::Function(f) => f.debug_info.as_ref().map(|info| info.effect_summary),
            _ => None,
        })
        .collect();

    assert!(
        summaries.contains(&EffectSummary::HasEffects),
        "expected at least one function with HasEffects summary, got {:?}",
        summaries
    );
}
