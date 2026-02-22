use flux::bytecode::{compiler::Compiler, op_code::disassemble};
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn compile_disassembly(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let bytecode = compiler.bytecode();

    let mut output = String::new();
    output.push_str("== main ==\n");
    output.push_str(&disassemble(&bytecode.instructions));

    for (idx, constant) in bytecode.constants.iter().enumerate() {
        if let Value::Function(function) = constant {
            output.push_str(&format!("\n== const_fn[{idx}] ==\n"));
            output.push_str(&disassemble(&function.instructions));
        }
    }

    output
}

fn assert_contains_primop(input: &str) {
    let asm = compile_disassembly(input);
    assert!(asm.contains("OpPrimOp"), "expected OpPrimOp:\n{}", asm);
}

fn assert_not_contains_primop(input: &str) {
    let asm = compile_disassembly(input);
    assert!(
        !asm.contains("OpPrimOp"),
        "did not expect OpPrimOp:\n{}",
        asm
    );
}

#[test]
fn compiler_emits_op_primop_for_existing_phase1_mappings() {
    assert_contains_primop(r#"len("abc")"#);
    assert_contains_primop("iadd(1, 2)");
    assert_contains_primop(r#"panic("boom")"#);
}

#[test]
fn compiler_emits_op_primop_for_phase2_base_mappings() {
    let programs = [
        "first(#[1, 2]);",
        "last(#[1, 2]);",
        "rest(#[1, 2]);",
        "contains(#[1, 2], 1);",
        "slice(#[1, 2, 3], 0, 2);",
        r#"trim("  hi  ");"#,
        r#"upper("hi");"#,
        r#"lower("HI");"#,
        r#"starts_with("hello", "he");"#,
        r#"ends_with("hello", "lo");"#,
        r#"replace("banana", "na", "X");"#,
        r#"chars("ab");"#,
        "keys({});",
        "values({});",
        r#"delete({}, "k");"#,
        "merge({}, {});",
        "is_map({});",
        r#"parse_int("1");"#,
        r#"parse_ints(#["1", "2"]);"#,
        r#"split_ints("1,2", ",");"#,
        "concat(#[1], #[2]);",
    ];

    for program in programs {
        assert_contains_primop(program);
    }
}

#[test]
fn compiler_emits_op_primop_for_concat_array() {
    assert_contains_primop("concat(#[1], #[2]);");
}

#[test]
fn compiler_does_not_emit_op_primop_for_shadowed_names() {
    assert_not_contains_primop(
        r#"
fn apply(trim) { trim("abc") }
apply(fn(x) { x })
"#,
    );
    assert_not_contains_primop(
        r#"
fn run() {
    let first = fn(x) { x }
    first(#[1, 2, 3])
}
run()
"#,
    );
    assert_not_contains_primop(
        r#"
fn parse_with(parse_int) { parse_int("123") }
parse_with(fn(x) { x })
"#,
    );
    assert_not_contains_primop(
        r#"
fn apply(concat) { concat(#[1], #[2]) }
apply(fn(a, b) { a })
"#,
    );
}
