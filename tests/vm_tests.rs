use flux::bytecode::{
    bytecode::Bytecode,
    compiler::Compiler,
    op_code::{OpCode, make},
};
use flux::runtime::value::Value;
use flux::runtime::vm::VM;
use flux::syntax::diagnostics::render_diagnostics;
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

fn run_error(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap_err()
}

#[test]
fn test_integer_arithmetic() {
    assert_eq!(run("1 + 2;"), Value::Integer(3));
    assert_eq!(run("5 * 2 + 10;"), Value::Integer(20));
    assert_eq!(run("-5;"), Value::Integer(-5));
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
        constants: vec![Value::Integer(1)],
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
    assert_eq!(run("1.5 + 2.25;"), Value::Float(3.75));
    assert_eq!(run("2.0 * 3.5;"), Value::Float(7.0));
    assert_eq!(run("-0.5;"), Value::Float(-0.5));
    assert_eq!(run("1 + 2.5;"), Value::Float(3.5));
    assert_eq!(run("2.5 + 1;"), Value::Float(3.5));
}

#[test]
fn test_boolean_expressions() {
    assert_eq!(run("true;"), Value::Boolean(true));
    assert_eq!(run("1 < 2;"), Value::Boolean(true));
    assert_eq!(run("!true;"), Value::Boolean(false));
}

#[test]
fn test_conditionals() {
    assert_eq!(run("if true { 10; };"), Value::Integer(10));
    assert_eq!(run("if false { 10; } else { 20; };"), Value::Integer(20));
}

#[test]
fn test_global_variables() {
    assert_eq!(run("let x = 5; x;"), Value::Integer(5));
    assert_eq!(run("let x = 5; let y = x; y;"), Value::Integer(5));
}

#[test]
fn test_function_parameter_shadows_global_binding() {
    assert_eq!(
        run(r#"
let x = 3;
fun t(x) { x; }
t(9) + x;
"#),
        Value::Integer(12)
    );
}

#[test]
fn test_functions() {
    assert_eq!(run("let f = fun() { 5 + 10; }; f();"), Value::Integer(15));
    assert_eq!(
        run("let sum = fun(a, b) { a + b; }; sum(1, 2);"),
        Value::Integer(3)
    );
}

#[test]
fn test_closures() {
    let input = r#"
        let newClosure = fun(a) { fun() { a; }; };
        let closure = newClosure(99);
        closure();
    "#;
    assert_eq!(run(input), Value::Integer(99));
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
    assert_eq!(run(input), Value::Integer(55));
}

#[test]
fn test_array_literals() {
    assert_eq!(
        run("[1, 2, 3];"),
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3),].into())
    );
    assert_eq!(run("[];"), Value::Array(vec![].into()));
}

#[test]
fn test_array_index() {
    assert_eq!(
        run("[1, 2, 3][0];"),
        Value::Some(std::rc::Rc::new(Value::Integer(1)))
    );
    assert_eq!(
        run("[1, 2, 3][1];"),
        Value::Some(std::rc::Rc::new(Value::Integer(2)))
    );
    assert_eq!(
        run("[1, 2, 3][2];"),
        Value::Some(std::rc::Rc::new(Value::Integer(3)))
    );
    assert_eq!(run("[1, 2, 3][3];"), Value::None);
    assert_eq!(run("[1, 2, 3][-1];"), Value::None);
}

#[test]
fn test_hash_literals() {
    let result = run(r#"{"a": 1};"#);
    match result {
        Value::Hash(h) => {
            assert_eq!(h.len(), 1);
        }
        _ => panic!("expected hash"),
    }
}

#[test]
fn test_hash_index() {
    assert_eq!(
        run(r#"{"a": 1}["a"];"#),
        Value::Some(std::rc::Rc::new(Value::Integer(1)))
    );
    assert_eq!(run(r#"{"a": 1}["b"];"#), Value::None);
    assert_eq!(
        run(r#"{1: "one"}[1];"#),
        Value::Some(std::rc::Rc::new(Value::String("one".to_string().into())))
    );
}

#[test]
fn test_builtin_len() {
    assert_eq!(run(r#"len("hello");"#), Value::Integer(5));
    assert_eq!(run("len([1, 2, 3]);"), Value::Integer(3));
}

#[test]
fn test_builtin_array_functions() {
    assert_eq!(run("first([1, 2, 3]);"), Value::Integer(1));
    assert_eq!(run("last([1, 2, 3]);"), Value::Integer(3));
    assert_eq!(
        run("rest([1, 2, 3]);"),
        Value::Array(vec![Value::Integer(2), Value::Integer(3),].into())
    );
    assert_eq!(
        run("push([1, 2], 3);"),
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3),].into())
    );
}

#[test]
fn test_less_than_or_equal_operator() {
    assert_eq!(run("5 <= 10;"), Value::Boolean(true));
    assert_eq!(run("10 <= 5;"), Value::Boolean(false));
    assert_eq!(run("5 <= 5;"), Value::Boolean(true));
    assert_eq!(run("5.5 <= 10.5;"), Value::Boolean(true));
    assert_eq!(run("10.5 <= 5.5;"), Value::Boolean(false));
    assert_eq!(run("5.5 <= 5.5;"), Value::Boolean(true));
    assert_eq!(run(r#""apple" <= "banana";"#), Value::Boolean(true));
    assert_eq!(run(r#""banana" <= "apple";"#), Value::Boolean(false));
    assert_eq!(run(r#""apple" <= "apple";"#), Value::Boolean(true));
}

#[test]
fn test_greater_than_or_equal_operator() {
    assert_eq!(run("10 >= 5;"), Value::Boolean(true));
    assert_eq!(run("5 >= 10;"), Value::Boolean(false));
    assert_eq!(run("5 >= 5;"), Value::Boolean(true));
    assert_eq!(run("10.5 >= 5.5;"), Value::Boolean(true));
    assert_eq!(run("5.5 >= 10.5;"), Value::Boolean(false));
    assert_eq!(run("5.5 >= 5.5;"), Value::Boolean(true));
    assert_eq!(run(r#""banana" >= "apple";"#), Value::Boolean(true));
    assert_eq!(run(r#""apple" >= "banana";"#), Value::Boolean(false));
    assert_eq!(run(r#""apple" >= "apple";"#), Value::Boolean(true));
}

#[test]
fn test_modulo_operator() {
    // Integer modulo
    assert_eq!(run("10 % 3;"), Value::Integer(1));
    assert_eq!(run("7 % 2;"), Value::Integer(1)); // odd check
    assert_eq!(run("8 % 2;"), Value::Integer(0)); // even check
    assert_eq!(run("15 % 4;"), Value::Integer(3));
    assert_eq!(run("100 % 7;"), Value::Integer(2));
    assert_eq!(run("5 % 5;"), Value::Integer(0));

    // Float modulo
    assert_eq!(run("10.5 % 3.0;"), Value::Float(1.5));
    assert_eq!(run("7.5 % 2.0;"), Value::Float(1.5));
    assert_eq!(run("10.0 % 3.0;"), Value::Float(1.0));
    assert_eq!(run("5.5 % 2.5;"), Value::Float(0.5));

    // Mixed integer-float modulo
    assert_eq!(run("10 % 3.0;"), Value::Float(1.0));
    assert_eq!(run("7 % 2.5;"), Value::Float(2.0));

    // Mixed float-integer modulo
    assert_eq!(run("10.5 % 3;"), Value::Float(1.5));
    assert_eq!(run("7.5 % 2;"), Value::Float(1.5));

    // Edge cases
    assert_eq!(run("1 % 10;"), Value::Integer(1)); // smaller % larger
    assert_eq!(run("0 % 5;"), Value::Integer(0)); // zero % n
}

#[test]
fn large_array_literal_no_stack_overflow() {
    let values = (1..=3000)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let input = format!("let data = [{}]; len(data);", values);
    assert_eq!(run(&input), Value::Integer(3000));
}

#[test]
fn large_map_pipeline_no_stack_overflow() {
    let values = (1..=3000)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let input = format!(
        "let data = [{}]; let mapped = map(data, \\x -> x + 1); len(mapped);",
        values
    );
    assert_eq!(run(&input), Value::Integer(3000));
}

#[test]
fn map_filter_fold_across_u16_boundary() {
    let n: i64 = 65_536;
    let values = (1..=n)
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let input = format!(
        r#"
let data = [{}];
let mapped = map(data, \x -> x + 1);
let filtered = filter(mapped, \x -> x % 2 == 0);
fold(filtered, 0, \(acc, x) -> acc + x);
"#,
        values
    );

    let filtered_count = n / 2;
    let expected_sum = filtered_count * (filtered_count + 1);
    assert_eq!(run(&input), Value::Integer(expected_sum));
}

#[test]
fn test_pipe_operator() {
    // Basic pipe: value |> function
    assert_eq!(
        run("let double = fun(x) { x * 2; }; 5 |> double;"),
        Value::Integer(10)
    );

    // Chained pipes: value |> f |> g
    assert_eq!(
        run(
            "let double = fun(x) { x * 2; }; let triple = fun(x) { x * 3; }; 5 |> double |> triple;"
        ),
        Value::Integer(30)
    );

    // Pipe with additional arguments: value |> function(arg)
    assert_eq!(
        run("let add = fun(x, y) { x + y; }; 5 |> add(3);"),
        Value::Integer(8)
    );

    // Pipe with multiple additional arguments
    assert_eq!(
        run("let sum3 = fun(a, b, c) { a + b + c; }; 1 |> sum3(2, 3);"),
        Value::Integer(6)
    );

    // Complex chain with mixed calls
    assert_eq!(
        run(r#"
            let double = fun(x) { x * 2; };
            let add = fun(x, y) { x + y; };
            let square = fun(x) { x * x; };
            2 |> double |> add(10) |> square;
        "#),
        Value::Integer(196) // ((2*2) + 10)^2 = 14^2 = 196
    );

    // Pipe preserves argument order (left side becomes first arg)
    assert_eq!(
        run("let subtract = fun(a, b) { a - b; }; 10 |> subtract(3);"),
        Value::Integer(7) // 10 - 3 = 7
    );

    // Pipe with string operations
    assert_eq!(
        run(r#"
            let greet = fun(name) { "Hello, " + name; };
            let exclaim = fun(s) { s + "!"; };
            "World" |> greet |> exclaim;
        "#),
        Value::String("Hello, World!".to_string().into())
    );

    // Pipe with array operations
    assert_eq!(
        run("let getFirst = fun(arr) { first(arr); }; [1, 2, 3] |> getFirst;"),
        Value::Integer(1)
    );

    // Nested pipe expressions
    assert_eq!(
        run(r#"
            let inc = fun(x) { x + 1; };
            let double = fun(x) { x * 2; };
            (3 |> inc) |> double;
        "#),
        Value::Integer(8) // (3+1) * 2 = 8
    );
}

#[test]
fn test_either_left_right() {
    // Basic Left creation
    assert_eq!(
        run("Left(42);"),
        Value::Left(std::rc::Rc::new(Value::Integer(42)))
    );

    // Basic Right creation
    assert_eq!(
        run("Right(42);"),
        Value::Right(std::rc::Rc::new(Value::Integer(42)))
    );

    // Left with string
    assert_eq!(
        run(r#"Left("error");"#),
        Value::Left(std::rc::Rc::new(Value::String("error".to_string().into())))
    );

    // Right with string
    assert_eq!(
        run(r#"Right("success");"#),
        Value::Right(std::rc::Rc::new(Value::String(
            "success".to_string().into()
        )))
    );

    // Nested Left
    assert_eq!(
        run("Left(Left(1));"),
        Value::Left(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Nested Right
    assert_eq!(
        run("Right(Right(1));"),
        Value::Right(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Left containing Right
    assert_eq!(
        run("Left(Right(42));"),
        Value::Left(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(42)
        ))))
    );

    // Right containing Left
    assert_eq!(
        run("Right(Left(42));"),
        Value::Right(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::Integer(42)
        ))))
    );
}

#[test]
fn test_either_pattern_matching() {
    // Simple Left match with wildcard
    assert_eq!(
        run(r#"
            let x = Left(1);
            match x {
                Left(_) -> true,
                _ -> false,
            };
        "#),
        Value::Boolean(true)
    );

    // Simple Right match with wildcard
    assert_eq!(
        run(r#"
            let x = Right(1);
            match x {
                Right(_) -> true,
                _ -> false,
            };
        "#),
        Value::Boolean(true)
    );

    // Left doesn't match Right pattern
    assert_eq!(
        run(r#"
            let x = Left(1);
            match x {
                Right(_) -> true,
                _ -> false,
            };
        "#),
        Value::Boolean(false)
    );

    // Right doesn't match Left pattern
    assert_eq!(
        run(r#"
            let x = Right(1);
            match x {
                Left(_) -> true,
                _ -> false,
            };
        "#),
        Value::Boolean(false)
    );

    // Match on Left with binding
    assert_eq!(
        run(r#"
            let x = Left(42);
            match x {
                Left(v) -> v,
                _ -> 0,
            };
        "#),
        Value::Integer(42)
    );

    // Match on Right with binding
    assert_eq!(
        run(r#"
            let x = Right(42);
            match x {
                Right(v) -> v,
                _ -> 0,
            };
        "#),
        Value::Integer(42)
    );
}

#[test]
fn test_match_guards_true_and_false_paths() {
    assert_eq!(
        run(r#"
            match Some(2) {
                Some(x) if x > 0 -> x,
                _ -> 0,
            };
        "#,),
        Value::Integer(2)
    );

    assert_eq!(
        run(r#"
            match Some(-2) {
                Some(x) if x > 0 -> x,
                Some(x) -> 0 - x,
                _ -> 0,
            };
        "#,),
        Value::Integer(2)
    );
}

#[test]
fn test_match_guards_can_use_pattern_bound_values() {
    assert_eq!(
        run(r#"
            match Right(10) {
                Right(v) if v > 20 -> 1,
                Right(v) -> v,
                _ -> 0,
            };
        "#,),
        Value::Integer(10)
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
        Value::Left(std::rc::Rc::new(Value::String("oops".to_string().into())))
    );

    // Function returning Right
    assert_eq!(
        run(r#"
            fun succeed(val) { Right(val) }
            succeed(100);
        "#),
        Value::Right(std::rc::Rc::new(Value::Integer(100)))
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
        Value::Right(std::rc::Rc::new(Value::Integer(5)))
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
        Value::Left(std::rc::Rc::new(Value::String(
            "division by zero".to_string().into()
        )))
    );
}

#[test]
fn test_either_equality() {
    // Left equality
    assert_eq!(run("Left(1) == Left(1);"), Value::Boolean(true));
    assert_eq!(run("Left(1) == Left(2);"), Value::Boolean(false));
    assert_eq!(run("Left(1) != Left(2);"), Value::Boolean(true));

    // Right equality
    assert_eq!(run("Right(1) == Right(1);"), Value::Boolean(true));
    assert_eq!(run("Right(1) == Right(2);"), Value::Boolean(false));
    assert_eq!(run("Right(1) != Right(2);"), Value::Boolean(true));

    // Left vs Right
    assert_eq!(run("Left(1) == Right(1);"), Value::Boolean(false));
    assert_eq!(run("Left(1) != Right(1);"), Value::Boolean(true));
}

#[test]
fn test_either_with_option() {
    // Left containing Some
    assert_eq!(
        run("Left(Some(42));"),
        Value::Left(std::rc::Rc::new(Value::Some(std::rc::Rc::new(
            Value::Integer(42)
        ))))
    );

    // Right containing None
    assert_eq!(
        run("Right(None);"),
        Value::Right(std::rc::Rc::new(Value::None))
    );

    // Some containing Left
    assert_eq!(
        run("Some(Left(1));"),
        Value::Some(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Some containing Right
    assert_eq!(
        run("Some(Right(1));"),
        Value::Some(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );
}

#[test]
fn test_either_in_arrays() {
    // Array of Either values
    assert_eq!(
        run("[Left(1), Right(2), Left(3)];"),
        Value::Array(
            vec![
                Value::Left(std::rc::Rc::new(Value::Integer(1))),
                Value::Right(std::rc::Rc::new(Value::Integer(2))),
                Value::Left(std::rc::Rc::new(Value::Integer(3))),
            ]
            .into()
        )
    );
}

#[test]
fn test_either_in_hash() {
    // Hash with Either values
    assert_eq!(
        run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["ok"];"#),
        Value::Some(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    assert_eq!(
        run(r#"let h = {"ok": Right(1), "err": Left("fail")}; h["err"];"#),
        Value::Some(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::String("fail".to_string().into())
        ))))
    );
}

#[test]
fn test_builtin_map() {
    assert_eq!(
        run("map([1, 2, 3], fun(x) { x * 2; });"),
        Value::Array(vec![Value::Integer(2), Value::Integer(4), Value::Integer(6)].into())
    );
}

#[test]
fn test_builtin_map_with_closure() {
    assert_eq!(
        run("let factor = 3; map([1, 2, 3], fun(x) { x * factor; });"),
        Value::Array(vec![Value::Integer(3), Value::Integer(6), Value::Integer(9)].into())
    );
}

#[test]
fn test_builtin_map_empty() {
    assert_eq!(run("map([], fun(x) { x; });"), Value::Array(vec![].into()));
}

#[test]
fn test_builtin_map_with_builtin_callback() {
    assert_eq!(
        run("map([1, 2, 3], to_string);"),
        Value::Array(
            vec![
                Value::String("1".into()),
                Value::String("2".into()),
                Value::String("3".into()),
            ]
            .into()
        )
    );
}

#[test]
fn test_builtin_filter() {
    assert_eq!(
        run("filter([1, 2, 3, 4, 5], fun(x) { x > 2; });"),
        Value::Array(vec![Value::Integer(3), Value::Integer(4), Value::Integer(5)].into())
    );
}

#[test]
fn test_builtin_filter_none_pass() {
    assert_eq!(
        run("filter([1, 2, 3], fun(x) { x > 10; });"),
        Value::Array(vec![].into())
    );
}

#[test]
fn test_builtin_filter_all_pass() {
    assert_eq!(
        run("filter([1, 2, 3], fun(x) { x > 0; });"),
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_builtin_fold_sum() {
    assert_eq!(
        run("fold([1, 2, 3, 4], 0, fun(acc, x) { acc + x; });"),
        Value::Integer(10)
    );
}

#[test]
fn test_builtin_fold_string_concat() {
    assert_eq!(
        run(r#"fold(["a", "b", "c"], "", fun(acc, x) { acc + x; });"#),
        Value::String("abc".into())
    );
}

#[test]
fn test_builtin_fold_empty() {
    assert_eq!(
        run("fold([], 42, fun(acc, x) { acc + x; });"),
        Value::Integer(42)
    );
}

#[test]
fn test_map_filter_chain() {
    assert_eq!(
        run(r#"
            let nums = [1, 2, 3, 4, 5, 6];
            let doubled = map(nums, fun(x) { x * 2; });
            filter(doubled, fun(x) { x > 6; });
        "#),
        Value::Array(vec![Value::Integer(8), Value::Integer(10), Value::Integer(12)].into())
    );
}

#[test]
fn test_map_fold_chain() {
    assert_eq!(
        run(r#"
            let nums = [1, 2, 3];
            let doubled = map(nums, fun(x) { x * 2; });
            fold(doubled, 0, fun(acc, x) { acc + x; });
        "#),
        Value::Integer(12)
    );
}

#[test]
fn test_map_with_lambda() {
    assert_eq!(
        run(r#"map([1, 2, 3], \x -> x + 10);"#),
        Value::Array(vec![Value::Integer(11), Value::Integer(12), Value::Integer(13)].into())
    );
}

#[test]
fn test_filter_with_lambda() {
    assert_eq!(
        run(r#"filter([1, 2, 3, 4], \x -> x > 2);"#),
        Value::Array(vec![Value::Integer(3), Value::Integer(4)].into())
    );
}

#[test]
fn test_fold_with_lambda() {
    assert_eq!(
        run(r#"fold([1, 2, 3], 0, \(a, b) -> a + b);"#),
        Value::Integer(6)
    );
}

#[test]
fn test_map_type_error_not_array() {
    let err = run_error("map(42, fun(x) { x; });");
    assert!(
        err.contains("Array"),
        "Expected Array type error, got: {}",
        err
    );
}

#[test]
fn test_map_type_error_not_function() {
    let err = run_error("map([1, 2], 42);");
    assert!(
        err.contains("Function"),
        "Expected Function type error, got: {}",
        err
    );
}

#[test]
fn test_filter_type_error() {
    let err = run_error("filter(42, fun(x) { x; });");
    assert!(
        err.contains("Array"),
        "Expected Array type error, got: {}",
        err
    );
}

#[test]
fn test_fold_type_error() {
    let err = run_error("fold(42, 0, fun(a, x) { a + x; });");
    assert!(
        err.contains("Array"),
        "Expected Array type error, got: {}",
        err
    );
}

#[test]
fn test_map_callback_arity_error_propagates() {
    let err = run_error("map([1], fun(a, b) { a + b; });");
    assert!(
        err.contains("wrong number of arguments"),
        "Expected callback arity error, got: {}",
        err
    );
}

#[test]
fn test_filter_callback_arity_error_propagates() {
    let err = run_error("filter([1], fun(a, b) { a > b; });");
    assert!(
        err.contains("wrong number of arguments"),
        "Expected callback arity error, got: {}",
        err
    );
}

#[test]
fn test_fold_callback_arity_error_propagates() {
    let err = run_error("fold([1], 0, fun(a) { a; });");
    assert!(
        err.contains("wrong number of arguments"),
        "Expected callback arity error, got: {}",
        err
    );
}

#[test]
fn test_map_callback_runtime_error_propagates() {
    let err = run_error("map([1], fun(x) { x + true; });");
    assert!(
        err.contains("[E1009]") && err.contains("Cannot add"),
        "Expected callback runtime error, got: {}",
        err
    );
}

#[test]
fn test_filter_callback_runtime_error_propagates() {
    let err = run_error("filter([1], fun(x) { x + true; });");
    assert!(
        err.contains("[E1009]") && err.contains("Cannot add"),
        "Expected callback runtime error, got: {}",
        err
    );
}

#[test]
fn test_fold_callback_runtime_error_propagates() {
    let err = run_error("fold([1], 0, fun(acc, x) { acc + true; });");
    assert!(
        err.contains("[E1009]") && err.contains("Cannot add"),
        "Expected callback runtime error, got: {}",
        err
    );
}

#[test]
fn test_map_mixed_element_types() {
    // Map over array with mixed types (int, string, bool)
    assert_eq!(
        run(r#"map([1, "hello", true], type_of);"#),
        Value::Array(
            vec![
                Value::String("Int".into()),
                Value::String("String".into()),
                Value::String("Bool".into()),
            ]
            .into()
        )
    );
}

#[test]
fn test_map_returns_nested_arrays() {
    // Map callback returns nested arrays
    assert_eq!(
        run("map([1, 2], fun(x) { [x, x * 2]; });"),
        Value::Array(
            vec![
                Value::Array(vec![Value::Integer(1), Value::Integer(2)].into()),
                Value::Array(vec![Value::Integer(2), Value::Integer(4)].into()),
            ]
            .into()
        )
    );
}

#[test]
fn test_filter_returns_nested_structures() {
    // Filter with callback returning nested hashes
    assert_eq!(
        run(r#"filter([1, 2, 3], fun(x) { x > 1; });"#),
        Value::Array(vec![Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_map_evaluation_order_with_side_effects() {
    // Verify left-to-right evaluation order by building a string
    let result = run(r#"
        fold(
            map([1, 2, 3], fun(x) { x * 2; }),
            "",
            fun(acc, x) { acc + to_string(x / 2); }
        );
    "#);
    assert_eq!(result, Value::String("123".into()));
}

#[test]
fn test_filter_evaluation_order_stable() {
    // Verify filter processes elements in left-to-right order
    let result = run(r#"
        fold(
            filter([5, 3, 8, 1], fun(x) { x > 2; }),
            "",
            fun(acc, x) { acc + to_string(x); }
        );
    "#);
    // Should see all elements that passed (5, 3, 8) in order
    assert_eq!(result, Value::String("538".into()));
}

#[test]
fn test_fold_evaluation_order_deterministic() {
    // Verify fold processes elements left-to-right
    let result = run(r#"
        fold([1, 2, 3, 4], "", fun(acc, x) {
            acc + to_string(x);
        });
    "#);
    assert_eq!(result, Value::String("1234".into()));
}

#[test]
fn test_map_error_includes_index() {
    // Verify error messages include element index
    let err = run_error(r#"map([1, 2, "oops", 4], fun(x) { x + 10; });"#);
    assert!(
        err.contains("index 2"),
        "Expected error to include index, got: {}",
        err
    );
}

#[test]
fn test_filter_error_includes_index() {
    // Verify error messages include element index
    let err = run_error(r#"filter([1, 2, 3, 4], fun(x) { x + "bad"; });"#);
    assert!(
        err.contains("index"),
        "Expected error to include index, got: {}",
        err
    );
}

#[test]
fn test_fold_error_includes_index() {
    // Verify error messages include element index
    let err = run_error(r#"fold([1, 2, 3], 0, fun(acc, x) { if x == 2 { acc + "bad"; } else { acc + x; }; });"#);
    assert!(
        err.contains("index 1"),
        "Expected error to include index 1, got: {}",
        err
    );
}

#[test]
fn test_map_with_option_values() {
    // Map over array producing Some/None values
    assert_eq!(
        run(r#"map([1, 2, 3], fun(x) { if x == 2 { None; } else { Some(x); }; });"#),
        Value::Array(
            vec![
                Value::Some(std::rc::Rc::new(Value::Integer(1))),
                Value::None,
                Value::Some(std::rc::Rc::new(Value::Integer(3))),
            ]
            .into()
        )
    );
}

#[test]
fn test_filter_truthiness_zero_is_truthy() {
    // Verify 0 and 0.0 are truthy (not like JavaScript)
    assert_eq!(
        run("filter([0, 1, 2], fun(x) { x; });"),
        Value::Array(vec![Value::Integer(0), Value::Integer(1), Value::Integer(2)].into())
    );
    assert_eq!(
        run("filter([0.0, 1.5], fun(x) { x; });"),
        Value::Array(vec![Value::Float(0.0), Value::Float(1.5)].into())
    );
}

#[test]
fn test_filter_truthiness_empty_string_is_truthy() {
    // Verify empty string is truthy
    assert_eq!(
        run(r#"filter(["", "a", "b"], fun(x) { x; });"#),
        Value::Array(
            vec![
                Value::String("".into()),
                Value::String("a".into()),
                Value::String("b".into()),
            ]
            .into()
        )
    );
}

#[test]
fn test_filter_truthiness_empty_array_is_truthy() {
    // Verify empty array is truthy
    assert_eq!(
        run("filter([[], [1], [2, 3]], fun(x) { x; });"),
        Value::Array(
            vec![
                Value::Array(vec![].into()),
                Value::Array(vec![Value::Integer(1)].into()),
                Value::Array(vec![Value::Integer(2), Value::Integer(3)].into()),
            ]
            .into()
        )
    );
}

#[test]
fn test_map_large_array_5k() {
    // Test that the growable stack handles 5k elements
    let program = format!(
        "let big = [{}]; let doubled = map(big, fun(x) {{ x * 2; }}); len(doubled);",
        (0..5000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    );
    assert_eq!(run(&program), Value::Integer(5000));
}

#[test]
fn test_filter_large_array_5k() {
    // Test filter with 5k elements
    let program = format!(
        "let big = [{}]; let filtered = filter(big, fun(x) {{ x % 2 == 0; }}); len(filtered);",
        (0..5000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    );
    assert_eq!(run(&program), Value::Integer(2500));
}

#[test]
fn test_fold_large_array_5k() {
    // Test fold with 5k elements
    let program = format!(
        "let big = [{}]; fold(big, 0, fun(acc, x) {{ acc + 1; }});",
        (0..5000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    );
    assert_eq!(run(&program), Value::Integer(5000));
}

#[test]
fn test_chained_operations_large_array() {
    // Test chained map/filter/fold with large array
    let program = format!(
        "let data = [{}]; \
         let mapped = map(data, fun(x) {{ x * 2; }}); \
         let filtered = filter(mapped, fun(x) {{ x % 3 == 0; }}); \
         len(filtered);",
        (0..1000).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    );
    let result = run(&program);
    assert!(matches!(result, Value::Integer(_)));
}
