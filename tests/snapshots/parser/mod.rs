use flux::frontend::{lexer::Lexer, parser::Parser};
use insta::assert_snapshot;

fn parse_to_string(input: &str) -> String {
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

    format!("{:#?}", program)
}

#[test]
fn snapshot_simple_arithmetic() {
    let input = "let result = 10 + 20 * 5;";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_function_definition() {
    let input = r#"
fun add(a, b) {
    return a + b;
}
"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_nested_if_else() {
    let input = r#"
if (x > 10) {
    if (x > 20) {
        print("very large");
    } else {
        print("large");
    }
} else {
    print("small");
}
"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_array_literal() {
    let input = "let arr = [1, 2 + 3, foo(4), [5, 6]];";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_hash_literal() {
    let input = r#"let obj = {name: "Alice", age: 30, nested: {x: 1}};"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_function_call_chain() {
    let input = "foo().bar().baz(1, 2);";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_index_expression() {
    let input = "arr[0]; obj[key]; matrix[i][j];";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_match_expression() {
    let input = r#"
let result = match value {
    Some(x) => x * 2,
    None => 0
};
"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_for_loop() {
    let input = r#"
for (item in items) {
    print(item);
}
"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_lambda_expression() {
    let input = "let double = |x| x * 2;";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_complex_operators() {
    let input = "let result = (a + b) * c / d - e % f;";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_string_interpolation() {
    let input = r#"let msg = "Hello ${name}, you have ${count} items";"#;
    let output = parse_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_import_statement() {
    let input = "import math; import utils;";
    let output = parse_to_string(input);
    assert_snapshot!(output);
}
