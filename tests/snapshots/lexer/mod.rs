use flux::frontend::lexer::Lexer;
use insta::assert_snapshot;

fn tokenize_to_string(input: &str) -> String {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize();

    let mut output = String::new();
    for token in tokens {
        output.push_str(&format!(
            "{:>3}:{:<3} {:15} {:?}\n",
            token.position.line,
            token.position.column,
            token.token_type.to_string(),
            token.literal
        ));
    }
    output
}

#[test]
fn snapshot_simple_arithmetic() {
    let input = "let result = 10 + 20 * 5;";
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_function_definition() {
    let input = r#"
fun add(a, b) {
    return a + b;
}
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_conditional_logic() {
    let input = r#"
if (x > 10) {
    print("large");
} else {
    print("small");
}
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_string_interpolation() {
    let input = r#"let message = "Hello ${name}, you are ${age} years old";"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_array_and_hash() {
    let input = r#"
let arr = [1, 2, 3];
let obj = {name: "Alice", age: 30};
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_match_expression() {
    let input = r#"
match value {
    Some(x) => x,
    None => 0
}
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_operators() {
    let input = "a == b && c != d || e <= f && g >= h";
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_import_statement() {
    let input = r#"import utils;"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_loop_construct() {
    let input = r#"
for (item in items) {
    print(item);
}
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_range_operator() {
    let input = "let numbers = 1..10;";
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_lambda_expression() {
    let input = "let double = |x| x * 2;";
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_comments() {
    let input = r#"
// This is a single line comment
let x = 5; // inline comment
/* Multi-line
   comment */
let y = 10;
"#;
    let output = tokenize_to_string(input);
    assert_snapshot!(output);
}
