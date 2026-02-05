use flux::bytecode::{
    bytecode::Bytecode,
    compiler::Compiler,
    op_code::{OpCode, make},
};
use flux::frontend::diagnostics::render_diagnostics;
use flux::frontend::lexer::Lexer;
use flux::frontend::parser::Parser;
use flux::runtime::object::Object;
use flux::runtime::vm::VM;

fn run(input: &str) -> Object {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let mut compiler = Compiler::new();
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

fn run_error(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let mut compiler = Compiler::new_with_file_path("test.flx");
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap_err()
}

#[test]
fn test_integer_arithmetic() {
    assert_eq!(run("1 + 2;"), Object::Integer(3));
    assert_eq!(run("5 * 2 + 10;"), Object::Integer(20));
    assert_eq!(run("-5;"), Object::Integer(-5));
}

#[test]
fn runtime_stack_trace_columns_are_one_based() {
    let input = r#"let inner = fun() {
1 / 0;
};
let outer = fun() {
inner();
};
outer();"#;

    let err = run_error(input);
    assert!(err.contains("Stack trace:"));
    assert!(
        !err.lines().any(|line| line == "--> test.flx"),
        "runtime diagnostics should not include file grouping headers, got:\n{}",
        err
    );
    assert!(
        err.contains("test.flx:2:1"),
        "expected 1-based column in stack trace, got:\n{}",
        err
    );
}

#[test]
fn unmigrated_runtime_error_maps_not_a_function_code() {
    let instructions = make(OpCode::OpClosure, &[0, 0]);
    let bytecode = Bytecode {
        instructions,
        constants: vec![Object::Integer(1)],
        debug_info: None,
    };

    let mut vm = VM::new(bytecode);
    let err = vm.run().unwrap_err();

    assert!(err.contains("[E1001]"), "expected E1001, got:\n{}", err);
    assert!(
        !err.contains("[E1002]"),
        "should not map to E1002, got:\n{}",
        err
    );
}

#[test]
fn test_float_arithmetic() {
    assert_eq!(run("1.5 + 2.25;"), Object::Float(3.75));
    assert_eq!(run("2.0 * 3.5;"), Object::Float(7.0));
    assert_eq!(run("-0.5;"), Object::Float(-0.5));
    assert_eq!(run("1 + 2.5;"), Object::Float(3.5));
    assert_eq!(run("2.5 + 1;"), Object::Float(3.5));
}

#[test]
fn test_boolean_expressions() {
    assert_eq!(run("true;"), Object::Boolean(true));
    assert_eq!(run("1 < 2;"), Object::Boolean(true));
    assert_eq!(run("!true;"), Object::Boolean(false));
}

#[test]
fn test_conditionals() {
    assert_eq!(run("if true { 10; };"), Object::Integer(10));
    assert_eq!(run("if false { 10; } else { 20; };"), Object::Integer(20));
}

#[test]
fn test_global_variables() {
    assert_eq!(run("let x = 5; x;"), Object::Integer(5));
    assert_eq!(run("let x = 5; let y = x; y;"), Object::Integer(5));
}

#[test]
fn test_functions() {
    assert_eq!(run("let f = fun() { 5 + 10; }; f();"), Object::Integer(15));
    assert_eq!(
        run("let sum = fun(a, b) { a + b; }; sum(1, 2);"),
        Object::Integer(3)
    );
}

#[test]
fn test_closures() {
    let input = r#"
        let newClosure = fun(a) { fun() { a; }; };
        let closure = newClosure(99);
        closure();
    "#;
    assert_eq!(run(input), Object::Integer(99));
}

#[test]
fn test_recursive_fibonacci() {
    let input = r#"
        let fib = fun(n) {
            if n < 2 { return n; };
            fib(n - 1) + fib(n - 2);
        };
        fib(10);
    "#;
    assert_eq!(run(input), Object::Integer(55));
}

#[test]
fn test_array_literals() {
    assert_eq!(
        run("[1, 2, 3];"),
        Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ])
    );
    assert_eq!(run("[];"), Object::Array(vec![]));
}

#[test]
fn test_array_index() {
    assert_eq!(
        run("[1, 2, 3][0];"),
        Object::Some(Box::new(Object::Integer(1)))
    );
    assert_eq!(
        run("[1, 2, 3][1];"),
        Object::Some(Box::new(Object::Integer(2)))
    );
    assert_eq!(
        run("[1, 2, 3][2];"),
        Object::Some(Box::new(Object::Integer(3)))
    );
    assert_eq!(run("[1, 2, 3][3];"), Object::None);
    assert_eq!(run("[1, 2, 3][-1];"), Object::None);
}

#[test]
fn test_hash_literals() {
    let result = run(r#"{"a": 1};"#);
    match result {
        Object::Hash(h) => {
            assert_eq!(h.len(), 1);
        }
        _ => panic!("expected hash"),
    }
}

#[test]
fn test_hash_index() {
    assert_eq!(
        run(r#"{"a": 1}["a"];"#),
        Object::Some(Box::new(Object::Integer(1)))
    );
    assert_eq!(run(r#"{"a": 1}["b"];"#), Object::None);
    assert_eq!(
        run(r#"{1: "one"}[1];"#),
        Object::Some(Box::new(Object::String("one".to_string())))
    );
}

#[test]
fn test_builtin_len() {
    assert_eq!(run(r#"len("hello");"#), Object::Integer(5));
    assert_eq!(run("len([1, 2, 3]);"), Object::Integer(3));
}

#[test]
fn test_builtin_array_functions() {
    assert_eq!(run("first([1, 2, 3]);"), Object::Integer(1));
    assert_eq!(run("last([1, 2, 3]);"), Object::Integer(3));
    assert_eq!(
        run("rest([1, 2, 3]);"),
        Object::Array(vec![Object::Integer(2), Object::Integer(3),])
    );
    assert_eq!(
        run("push([1, 2], 3);"),
        Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ])
    );
}

#[test]
fn test_less_than_or_equal_operator() {
    assert_eq!(run("5 <= 10;"), Object::Boolean(true));
    assert_eq!(run("10 <= 5;"), Object::Boolean(false));
    assert_eq!(run("5 <= 5;"), Object::Boolean(true));
    assert_eq!(run("5.5 <= 10.5;"), Object::Boolean(true));
    assert_eq!(run("10.5 <= 5.5;"), Object::Boolean(false));
    assert_eq!(run("5.5 <= 5.5;"), Object::Boolean(true));
    assert_eq!(run(r#""apple" <= "banana";"#), Object::Boolean(true));
    assert_eq!(run(r#""banana" <= "apple";"#), Object::Boolean(false));
    assert_eq!(run(r#""apple" <= "apple";"#), Object::Boolean(true));
}

#[test]
fn test_greater_than_or_equal_operator() {
    assert_eq!(run("10 >= 5;"), Object::Boolean(true));
    assert_eq!(run("5 >= 10;"), Object::Boolean(false));
    assert_eq!(run("5 >= 5;"), Object::Boolean(true));
    assert_eq!(run("10.5 >= 5.5;"), Object::Boolean(true));
    assert_eq!(run("5.5 >= 10.5;"), Object::Boolean(false));
    assert_eq!(run("5.5 >= 5.5;"), Object::Boolean(true));
    assert_eq!(run(r#""banana" >= "apple";"#), Object::Boolean(true));
    assert_eq!(run(r#""apple" >= "banana";"#), Object::Boolean(false));
    assert_eq!(run(r#""apple" >= "apple";"#), Object::Boolean(true));
}

#[test]
fn test_modulo_operator() {
    // Integer modulo
    assert_eq!(run("10 % 3;"), Object::Integer(1));
    assert_eq!(run("7 % 2;"), Object::Integer(1)); // odd check
    assert_eq!(run("8 % 2;"), Object::Integer(0)); // even check
    assert_eq!(run("15 % 4;"), Object::Integer(3));
    assert_eq!(run("100 % 7;"), Object::Integer(2));
    assert_eq!(run("5 % 5;"), Object::Integer(0));

    // Float modulo
    assert_eq!(run("10.5 % 3.0;"), Object::Float(1.5));
    assert_eq!(run("7.5 % 2.0;"), Object::Float(1.5));
    assert_eq!(run("10.0 % 3.0;"), Object::Float(1.0));
    assert_eq!(run("5.5 % 2.5;"), Object::Float(0.5));

    // Mixed integer-float modulo
    assert_eq!(run("10 % 3.0;"), Object::Float(1.0));
    assert_eq!(run("7 % 2.5;"), Object::Float(2.0));

    // Mixed float-integer modulo
    assert_eq!(run("10.5 % 3;"), Object::Float(1.5));
    assert_eq!(run("7.5 % 2;"), Object::Float(1.5));

    // Edge cases
    assert_eq!(run("1 % 10;"), Object::Integer(1)); // smaller % larger
    assert_eq!(run("0 % 5;"), Object::Integer(0)); // zero % n
}

#[test]
fn test_pipe_operator() {
    // Basic pipe: value |> function
    assert_eq!(
        run("let double = fun(x) { x * 2; }; 5 |> double;"),
        Object::Integer(10)
    );

    // Chained pipes: value |> f |> g
    assert_eq!(
        run(
            "let double = fun(x) { x * 2; }; let triple = fun(x) { x * 3; }; 5 |> double |> triple;"
        ),
        Object::Integer(30)
    );

    // Pipe with additional arguments: value |> function(arg)
    assert_eq!(
        run("let add = fun(x, y) { x + y; }; 5 |> add(3);"),
        Object::Integer(8)
    );

    // Pipe with multiple additional arguments
    assert_eq!(
        run("let sum3 = fun(a, b, c) { a + b + c; }; 1 |> sum3(2, 3);"),
        Object::Integer(6)
    );

    // Complex chain with mixed calls
    assert_eq!(
        run(r#"
            let double = fun(x) { x * 2; };
            let add = fun(x, y) { x + y; };
            let square = fun(x) { x * x; };
            2 |> double |> add(10) |> square;
        "#),
        Object::Integer(196) // ((2*2) + 10)^2 = 14^2 = 196
    );

    // Pipe preserves argument order (left side becomes first arg)
    assert_eq!(
        run("let subtract = fun(a, b) { a - b; }; 10 |> subtract(3);"),
        Object::Integer(7) // 10 - 3 = 7
    );

    // Pipe with string operations
    assert_eq!(
        run(r#"
            let greet = fun(name) { "Hello, " + name; };
            let exclaim = fun(s) { s + "!"; };
            "World" |> greet |> exclaim;
        "#),
        Object::String("Hello, World!".to_string())
    );

    // Pipe with array operations
    assert_eq!(
        run("let getFirst = fun(arr) { first(arr); }; [1, 2, 3] |> getFirst;"),
        Object::Integer(1)
    );

    // Nested pipe expressions
    assert_eq!(
        run(r#"
            let inc = fun(x) { x + 1; };
            let double = fun(x) { x * 2; };
            (3 |> inc) |> double;
        "#),
        Object::Integer(8) // (3+1) * 2 = 8
    );
}

#[test]
fn test_either_left_right() {
    // Basic Left creation
    assert_eq!(
        run("Left(42);"),
        Object::Left(Box::new(Object::Integer(42)))
    );

    // Basic Right creation
    assert_eq!(
        run("Right(42);"),
        Object::Right(Box::new(Object::Integer(42)))
    );

    // Left with string
    assert_eq!(
        run(r#"Left("error");"#),
        Object::Left(Box::new(Object::String("error".to_string())))
    );

    // Right with string
    assert_eq!(
        run(r#"Right("success");"#),
        Object::Right(Box::new(Object::String("success".to_string())))
    );

    // Nested Left
    assert_eq!(
        run("Left(Left(1));"),
        Object::Left(Box::new(Object::Left(Box::new(Object::Integer(1)))))
    );

    // Nested Right
    assert_eq!(
        run("Right(Right(1));"),
        Object::Right(Box::new(Object::Right(Box::new(Object::Integer(1)))))
    );

    // Left containing Right
    assert_eq!(
        run("Left(Right(42));"),
        Object::Left(Box::new(Object::Right(Box::new(Object::Integer(42)))))
    );

    // Right containing Left
    assert_eq!(
        run("Right(Left(42));"),
        Object::Right(Box::new(Object::Left(Box::new(Object::Integer(42)))))
    );
}

#[test]
fn test_either_pattern_matching() {
    // Simple Left match with wildcard
    assert_eq!(
        run(r#"
            let x = Left(1);
            match x {
                Left(_) -> true;
                _ -> false;
            };
        "#),
        Object::Boolean(true)
    );

    // Simple Right match with wildcard
    assert_eq!(
        run(r#"
            let x = Right(1);
            match x {
                Right(_) -> true;
                _ -> false;
            };
        "#),
        Object::Boolean(true)
    );

    // Left doesn't match Right pattern
    assert_eq!(
        run(r#"
            let x = Left(1);
            match x {
                Right(_) -> true;
                _ -> false;
            };
        "#),
        Object::Boolean(false)
    );

    // Right doesn't match Left pattern
    assert_eq!(
        run(r#"
            let x = Right(1);
            match x {
                Left(_) -> true;
                _ -> false;
            };
        "#),
        Object::Boolean(false)
    );

    // Match on Left with binding
    assert_eq!(
        run(r#"
            let x = Left(42);
            match x {
                Left(v) -> v;
                _ -> 0;
            };
        "#),
        Object::Integer(42)
    );

    // Match on Right with binding
    assert_eq!(
        run(r#"
            let x = Right(42);
            match x {
                Right(v) -> v;
                _ -> 0;
            };
        "#),
        Object::Integer(42)
    );
}

#[test]
fn test_either_in_functions() {
    // Function returning Left
    assert_eq!(
        run(r#"
            fun fail(msg) { Left(msg) }
            fail("oops");
        "#),
        Object::Left(Box::new(Object::String("oops".to_string())))
    );

    // Function returning Right
    assert_eq!(
        run(r#"
            fun succeed(val) { Right(val) }
            succeed(100);
        "#),
        Object::Right(Box::new(Object::Integer(100)))
    );

    // Safe divide function
    assert_eq!(
        run(r#"
            fun safeDivide(a, b) {
                if b == 0 {
                    Left("division by zero")
                } else {
                    Right(a / b)
                }
            }
            safeDivide(10, 2);
        "#),
        Object::Right(Box::new(Object::Integer(5)))
    );

    assert_eq!(
        run(r#"
            fun safeDivide(a, b) {
                if b == 0 {
                    Left("division by zero")
                } else {
                    Right(a / b)
                }
            }
            safeDivide(10, 0);
        "#),
        Object::Left(Box::new(Object::String("division by zero".to_string())))
    );
}

#[test]
fn test_either_equality() {
    // Left equality
    assert_eq!(run("Left(1) == Left(1);"), Object::Boolean(true));
    assert_eq!(run("Left(1) == Left(2);"), Object::Boolean(false));
    assert_eq!(run("Left(1) != Left(2);"), Object::Boolean(true));

    // Right equality
    assert_eq!(run("Right(1) == Right(1);"), Object::Boolean(true));
    assert_eq!(run("Right(1) == Right(2);"), Object::Boolean(false));
    assert_eq!(run("Right(1) != Right(2);"), Object::Boolean(true));

    // Left vs Right
    assert_eq!(run("Left(1) == Right(1);"), Object::Boolean(false));
    assert_eq!(run("Left(1) != Right(1);"), Object::Boolean(true));
}

#[test]
fn test_either_with_option() {
    // Left containing Some
    assert_eq!(
        run("Left(Some(42));"),
        Object::Left(Box::new(Object::Some(Box::new(Object::Integer(42)))))
    );

    // Right containing None
    assert_eq!(run("Right(None);"), Object::Right(Box::new(Object::None)));

    // Some containing Left
    assert_eq!(
        run("Some(Left(1));"),
        Object::Some(Box::new(Object::Left(Box::new(Object::Integer(1)))))
    );

    // Some containing Right
    assert_eq!(
        run("Some(Right(1));"),
        Object::Some(Box::new(Object::Right(Box::new(Object::Integer(1)))))
    );
}

#[test]
fn test_either_in_arrays() {
    // Array of Either values
    assert_eq!(
        run("[Left(1), Right(2), Left(3)];"),
        Object::Array(vec![
            Object::Left(Box::new(Object::Integer(1))),
            Object::Right(Box::new(Object::Integer(2))),
            Object::Left(Box::new(Object::Integer(3))),
        ])
    );
}

#[test]
fn test_either_in_hash() {
    // Hash with Either values
    assert_eq!(
        run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["ok"];"#),
        Object::Some(Box::new(Object::Right(Box::new(Object::Integer(1)))))
    );

    assert_eq!(
        run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["err"];"#),
        Object::Some(Box::new(Object::Left(Box::new(Object::String(
            "fail".to_string()
        )))))
    );
}
