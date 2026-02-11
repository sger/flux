use flux::bytecode::{compiler::Compiler, op_code::disassemble};
use flux::syntax::{lexer::Lexer, parser::Parser};
use insta::assert_snapshot;

fn compile_to_string(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    if !parser.errors.is_empty() {
        let mut output = String::from("Parse Errors:\n");
        for error in &parser.errors {
            output.push_str(&format!("  - {:?}\n", error));
        }
        return output;
    }

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx", interner);
    if let Err(diags) = compiler.compile(&program) {
        let mut output = String::from("Compile Errors:\n");
        for diag in &diags {
            output.push_str(&format!("  - {:?}\n", diag));
        }
        return output;
    }

    let bytecode = compiler.bytecode();

    let mut output = String::new();
    output.push_str("Constants:\n");
    for (i, constant) in bytecode.constants.iter().enumerate() {
        output.push_str(&format!("  {}: {}\n", i, constant));
    }

    output.push_str("\nInstructions:\n");
    output.push_str(&disassemble(&bytecode.instructions));

    output
}

#[test]
fn snapshot_simple_arithmetic() {
    let input = "1 + 2;";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_variable_assignment() {
    let input = "let x = 10; x;";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_function_definition() {
    let input = r#"
fun add(a, b) {
    return a + b;
}
add(5, 10);
"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_conditional() {
    let input = r#"
if (true) {
    10;
} else {
    20;
}
"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_array_literal() {
    let input = "let arr = [1, 2, 3];";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_hash_literal() {
    let input = r#"let obj = {"a": 1, "b": 2};"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_index_expression() {
    let input = "let arr = [1, 2, 3]; arr[1];";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_closure() {
    let input = r#"
let newAdder = fun(x) {
    return fun(y) { x + y; };
};
let addTwo = newAdder(2);
addTwo(3);
"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_string_concatenation() {
    let input = r#"let msg = "Hello" + " " + "World";"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_comparison_operators() {
    let input = "5 > 3; 10 == 10; 7 != 8;";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_logical_operators() {
    let input = "true && false; true || false;";
    let output = compile_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_nested_scopes() {
    let input = r#"
let x = 1;
{
    let y = 2;
    {
        let z = 3;
        x + y + z;
    }
}
"#;
    let output = compile_to_string(input);
    assert_snapshot!(output);
}
