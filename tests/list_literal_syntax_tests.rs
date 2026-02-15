use flux::bytecode::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::runtime::vm::VM;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

fn run(input: &str) -> Value {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

#[test]
fn list_literal_matches_cons_and_empty() {
    let value = run(r#"
let xs = [1, 2, 3];
match xs {
    [h | t] -> h,
    [] -> 0,
    _ -> -1,
};
"#);
    assert_eq!(value, Value::Integer(1));
}

#[test]
fn prefixed_array_literal_supports_indexing() {
    let value = run("let arr = [|1, 2, 3|]; arr[0];");
    assert_eq!(value, Value::Some(std::rc::Rc::new(Value::Integer(1))));
}

#[test]
fn map_on_list_literal_returns_list() {
    let value = run(r#"to_array(map([1, 2, 3], \x -> x * 2));"#);
    assert_eq!(
        value,
        Value::Array(vec![Value::Integer(2), Value::Integer(4), Value::Integer(6)].into())
    );
}
