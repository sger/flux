use flux::syntax::formatter::format_source;
use insta::assert_snapshot;

#[test]
fn snapshot_format_simple_let() {
    let input = "let    x    =    5;";
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_function() {
    let input = r#"fun add(  a,b  ){return a+b;}"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_nested_blocks() {
    let input = r#"
if(x>10){
if(x>20){print("big");}
else{print("medium");}
}else{print("small");}
"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_array() {
    let input = "[1,2,3,4,5]";
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_hash() {
    let input = r#"{name:"Alice",age:30,city:"NYC"}"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_binary_expressions() {
    let input = "a+b*c/d-e%f";
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_function_calls() {
    let input = "foo(  1  ,  2  ,  3  )";
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_complex_program() {
    let input = r#"
fun fibonacci(n){
if(n<=1){return n;}
return fibonacci(n-1)+fibonacci(n-2);
}
let result=fibonacci(10);
print(result);
"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_string_interpolation() {
    let input = r#"let msg="Hello ${name}, you are ${age} years old";"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_match_expression() {
    let input = r#"
match value{
Some(x)=>x*2,
None=>0
}
"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_for_loop() {
    let input = r#"for(item in items){print(item);}"#;
    let output = format_source(input);
    assert_snapshot!(output);
}

#[test]
fn snapshot_format_lambda() {
    let input = "let double=|x|x*2;";
    let output = format_source(input);
    assert_snapshot!(output);
}
