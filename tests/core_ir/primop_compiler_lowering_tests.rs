use flux::bytecode::op_code::disassemble;
use flux::compiler::Compiler;
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

fn compile_error(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    if !parser.errors.is_empty() {
        return render_diagnostics(&parser.errors, Some(input), None);
    }
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    match compiler.compile(&program) {
        Ok(_) => panic!("expected compile error"),
        Err(diags) => render_diagnostics(&diags, Some(input), None),
    }
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
    assert_contains_primop("iadd(1, 2)");
    assert_contains_primop(r#"panic("boom")"#);
    assert_contains_primop(
        r#"
fn main() -> Unit with IO {
    print("ok")
}
"#,
    );
}

#[test]
fn print_arity_split_routes_one_arg_to_primop_and_multi_arg_to_unresolved_call() {
    assert_contains_primop(
        r#"
fn main() -> Unit with IO {
    print("ok")
}
"#,
    );

    let rendered = compile_error(
        r#"
fn main() -> Unit with IO {
    print("a", "b", "c")
}
"#,
    );
    assert!(
        rendered.contains("I can't find a value named `print`."),
        "expected unresolved multi-arg print call:\n{}",
        rendered
    );
}

#[test]
fn compiler_emits_op_primop_for_concat_array() {
    assert_contains_primop("array_concat([|1|], [|2|]);");
}

#[test]
fn compiler_emits_op_primop_for_float_math_builtins() {
    assert_contains_primop("sqrt(9.0);");
    assert_contains_primop("sin(0.0);");
    assert_contains_primop("cos(0.0);");
    assert_contains_primop("exp(0.0);");
    assert_contains_primop("log(1.0);");
    assert_contains_primop("floor(3.9);");
    assert_contains_primop("ceil(3.1);");
    assert_contains_primop("round(3.5);");
}

#[test]
fn compiler_emits_op_primop_for_bitwise_builtins() {
    assert_contains_primop("bit_and(6, 3);");
    assert_contains_primop("bit_or(6, 3);");
    assert_contains_primop("bit_xor(6, 3);");
    assert_contains_primop("bit_shl(3, 2);");
    assert_contains_primop("bit_shr(-8, 1);");
}

#[test]
fn compiler_emits_op_primop_for_explicit_array_reverse_and_contains() {
    assert_contains_primop("array_reverse([|1, 2, 3|]);");
    assert_contains_primop("array_contains([|1, 2, 3|], 2);");
}

#[test]
fn compiler_emits_op_primop_for_explicit_map_builtins() {
    assert_contains_primop(r#"map_get({"x": 1}, "x");"#);
    assert_contains_primop(r#"map_set({}, "x", 1);"#);
    assert_contains_primop(r#"map_has({"x": 1}, "x");"#);
    assert_contains_primop(r#"map_delete({"x": 1}, "x");"#);
}

#[test]
fn compiler_does_not_emit_op_primop_for_stdlib_facing_reverse_and_contains() {
    assert_not_contains_primop(
        r#"
fn reverse(xs) { xs }
reverse([1, 2, 3]);
"#,
    );
    assert_not_contains_primop(
        r#"
fn contains(xs, x) { false }
contains([1, 2, 3], 2);
"#,
    );
    assert_not_contains_primop(
        r#"
fn get(m, key) { None }
get({}, "x");
"#,
    );
    assert_not_contains_primop(
        r#"
fn put(m, key, value) { m }
put({}, "x", 1);
"#,
    );
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
