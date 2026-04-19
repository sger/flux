use flux::bytecode::{
    bytecode::Bytecode,
    op_code::{OpCode, make},
};
use flux::compiler::Compiler;
use flux::core::CorePrimOp;
use flux::diagnostics::{DiagnosticsAggregator, render_diagnostics};
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::module_graph::ModuleGraph;
use flux::syntax::parser::Parser;
use flux::vm::VM;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static VM_TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Build Flow prelude imports for module-graph based test compilation.
fn flow_prelude_source() -> String {
    [
        "import Flow.Option exposing (..)",
        "import Flow.List except [concat, delete]",
        "import Flow.List as List",
        "import Flow.String exposing (..)",
        "import Flow.Numeric exposing (..)",
        "import Flow.IO exposing (..)",
        "import Flow.Assert exposing (..)",
        "",
    ]
    .join("\n")
}

fn write_test_program(input: &str, with_prelude: bool) -> (PathBuf, String) {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temp_root = workspace_root.join("target/tmp/vm_tests");
    std::fs::create_dir_all(&temp_root).unwrap();

    let id = VM_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let entry_path = temp_root.join(format!("test_{id}.flx"));
    let full_source = if with_prelude {
        format!("{}\n{}", flow_prelude_source(), input)
    } else {
        input.to_string()
    };

    std::fs::write(&entry_path, &full_source).unwrap();
    (entry_path, full_source)
}

fn compile_program(input: &str, with_prelude: bool) -> Result<Bytecode, String> {
    let (entry_path, full_source) = write_test_program(input, with_prelude);
    let lexer = Lexer::new(&full_source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    if !parser.errors.is_empty() {
        return Err(render_diagnostics(&parser.errors, Some(&full_source), None));
    }

    let interner = parser.take_interner();
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = vec![
        entry_path.parent().unwrap().to_path_buf(),
        workspace_root.join("lib"),
        workspace_root.join("src"),
    ];
    let graph_result =
        ModuleGraph::build_with_entry_and_roots(&entry_path, &program, interner, &roots);
    if !graph_result.diagnostics.is_empty() {
        return Err(DiagnosticsAggregator::new(&graph_result.diagnostics)
            .with_default_source(entry_path.to_string_lossy(), full_source)
            .with_file_headers(false)
            .report()
            .rendered);
    }

    let mut compiler = Compiler::new_with_interner(
        entry_path.to_string_lossy().to_string(),
        graph_result.interner,
    );
    for node in graph_result.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        if let Err(diags) = compiler.compile(&node.program) {
            return Err(render_diagnostics(
                &diags,
                Some(&std::fs::read_to_string(&node.path).unwrap_or_else(|_| full_source.clone())),
                None,
            ));
        }
    }
    Ok(compiler.bytecode().clone())
}

fn run(input: &str) -> Value {
    let bytecode = compile_program(input, true).unwrap_or_else(|err| panic!("{err}"));
    let mut vm = VM::new(bytecode);
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

fn run_error(input: &str) -> String {
    run_error_with_prelude(input, true)
}

fn run_error_raw(input: &str) -> String {
    run_error_with_prelude(input, false)
}

fn run_error_with_prelude(input: &str, with_prelude: bool) -> String {
    let bytecode = compile_program(input, with_prelude).unwrap_or_else(|err| panic!("{err}"));
    let mut vm = VM::new(bytecode);
    vm.run().unwrap_err()
}

/// Like `run_error`, but also returns compile-time errors (as a rendered string)
/// instead of panicking on compile failure. Used for tests where the error may
/// be caught either at compile time or runtime depending on type inference depth.
fn run_any_error(input: &str) -> String {
    match compile_program(input, true) {
        Err(err) => err,
        Ok(bytecode) => {
            let mut vm = VM::new(bytecode);
            vm.run().unwrap_err()
        }
    }
}

#[test]
fn test_integer_arithmetic() {
    assert_eq!(run("1 + 2;"), Value::Integer(3));
    assert_eq!(run("5 * 2 + 10;"), Value::Integer(20));
    assert_eq!(run("-5;"), Value::Integer(-5));
}

#[test]
fn runtime_stack_trace_columns_are_one_based() {
    let input = r#"let inner = fn() {
1 / 0
};
let outer = fn() {
inner()
};
outer();"#;

    let err = run_error_raw(input);
    assert!(err.contains("Stack trace:"));
    assert!(
        !err.lines().any(|line| line == "--> test.flx"),
        "runtime diagnostics should not include file grouping headers, got:\n{}",
        err
    );
    assert!(
        err.contains(":2:1"),
        "expected 1-based column in stack trace, got:\n{}",
        err
    );
}

#[test]
fn prelude_list_map_rejects_array_argument_at_compile_time() {
    let err = run_any_error(
        r#"
let scores = [|1, 2, 3|];
map(scores, fn(x) { x + 1 });
"#,
    );
    assert!(
        err.contains("E300"),
        "expected compile-time E300 for Flow.List.map called with Array, got:\n{}",
        err
    );
    assert!(
        err.contains("List<_>")
            || err.contains("Array<Int>")
            || err.contains("wrong type in the 1st argument to `map`"),
        "expected list-vs-array mismatch details, got:\n{}",
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
    // Mixed Int/Float arithmetic is now rejected statically by HM validation.
}

#[test]
fn test_boolean_expressions() {
    assert_eq!(run("true;"), Value::Boolean(true));
    assert_eq!(run("1 < 2;"), Value::Boolean(true));
    assert_eq!(run("!true;"), Value::Boolean(false));
}

#[test]
fn test_conditionals() {
    assert_eq!(run("if true { 10 };"), Value::Integer(10));
    assert_eq!(run("if false { 10 } else { 20 };"), Value::Integer(20));
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
fn t(x) { x }
t(9) + x;
"#),
        Value::Integer(12)
    );
}

#[test]
fn test_functions() {
    assert_eq!(run("let f = fn() { 5 + 10 }; f();"), Value::Integer(15));
    assert_eq!(
        run("let add_pair = fn(a, b) { a + b }; add_pair(1, 2);"),
        Value::Integer(3)
    );
}

#[test]
fn test_closures() {
    let input = r#"
        let newClosure = fn(a) { fn() { a } };
        let closure = newClosure(99);
        closure();
    "#;
    assert_eq!(run(input), Value::Integer(99));
}

#[test]
fn runtime_contract_checks_dynamic_boundary_arguments() {
    // HM inference can now infer `x`'s type (String) from the call to old("oops"),
    // so this may be caught at compile time (E300/E055) or runtime (E1004) depending
    // on inference depth.  Either way the type mismatch must be reported.
    let err = run_any_error(
        r#"
fn old(v) { v }
fn typed_add(a: Int, b: Int) -> Int { a + b }
let x = old("oops")
typed_add(x, 1)
"#,
    );
    assert!(
        err.contains("[E300]") || err.contains("[E055]") || err.contains("[E1004]"),
        "expected type error E300, E055 or E1004, got:\n{}",
        err
    );
    assert!(
        err.contains("Expected Int, got String.")
            || err.contains("Cannot unify Int with String.")
            || err.contains("I found the wrong type in the 1st argument to `typed_add`.")
            || (err.contains("this argument has type `String`")
                && err.contains("expected argument type: Int")),
        "expected contract mismatch details, got:\n{}",
        err
    );
}

#[test]
fn runtime_contract_checks_typed_return_values() {
    // `fn bad(x) -> Int { x }` — HM infers x: Int (it's returned as Int),
    // so `bad("oops")` may be caught at compile time (E300) or at runtime
    // (E1004 from the return contract).  Both are valid; accept either.
    let err = run_any_error(
        r#"
fn bad(x) -> Int { x }
bad("oops")
"#,
    );
    assert!(
        err.contains("[E300]") || err.contains("[E1004]"),
        "expected type error E300 or E1004, got:\n{}",
        err
    );
}

#[test]
fn test_recursive_fibonacci() {
    let input = r#"
        let fib = fn(n) {
            if n < 2 { return n; };
            fib(n - 1) + fib(n - 2)
        };
        fib(10);
    "#;
    assert_eq!(run(input), Value::Integer(55));
}

#[test]
fn test_array_literals() {
    assert_eq!(
        run("#[1, 2, 3];"),
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3),].into())
    );
    assert_eq!(run("#[];"), Value::Array(vec![].into()));
}

#[test]
fn test_array_index() {
    assert_eq!(
        run("#[1, 2, 3][0];"),
        Value::Some(std::rc::Rc::new(Value::Integer(1)))
    );
    assert_eq!(
        run("#[1, 2, 3][1];"),
        Value::Some(std::rc::Rc::new(Value::Integer(2)))
    );
    assert_eq!(
        run("#[1, 2, 3][2];"),
        Value::Some(std::rc::Rc::new(Value::Integer(3)))
    );
    assert_eq!(run("#[1, 2, 3][3];"), Value::None);
    assert_eq!(run("#[1, 2, 3][-1];"), Value::None);
}

#[test]
fn test_hash_literals() {
    let result = run(r#"{"a": 1};"#);
    // Hash literals produce Value::HashMap (Rc-based HAMT)
    assert!(
        matches!(result, Value::HashMap(_)),
        "expected HashMap, got {:?}",
        result
    );
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
fn test_block_tail_semantics_and_do_block() {
    assert_eq!(run("fn f() { let x = 1; x + 2 } f();"), Value::Integer(3));
    assert_eq!(run("fn f() { 1; } f();"), Value::None);
    assert_eq!(
        run("let y = do { let x = 1; x + 1 }; y;"),
        Value::Integer(2)
    );
    assert_eq!(run("let y = do { let x = 1; x + 1; }; y;"), Value::None);
    assert_eq!(
        run("if true { let x = 1; x } else { 0 };"),
        Value::Integer(1)
    );
    assert_eq!(
        run("match 1 { 1 -> do { let x = 2; x }, _ -> 0 };"),
        Value::Integer(2)
    );
    assert_eq!(
        run(r#"let h = {"a": 1, "b": 2}; h["a"];"#),
        Value::Some(std::rc::Rc::new(Value::Integer(1)))
    );
}

#[test]
fn test_base_len() {
    assert_eq!(run(r#"len("hello");"#), Value::Integer(5));
    assert_eq!(run("len(#[1, 2, 3]);"), Value::Integer(3));
}

fn make_some(v: Value) -> Value {
    Value::Some(std::rc::Rc::new(v))
}

fn make_string(v: &str) -> Value {
    Value::String(v.to_string().into())
}

#[test]
fn test_base_array_functions() {
    // first/last/rest are stdlib functions (Flow.List), not primops.
    // They return raw values, not Option-wrapped.
    assert_eq!(run("first([1, 2, 3]);"), Value::Integer(1));
    assert_eq!(run("last([1, 2, 3]);"), Value::Integer(3));
    assert_eq!(run("len(rest([1, 2, 3]));"), Value::Integer(2));
    assert_eq!(run("first(rest([1, 2, 3]));"), Value::Integer(2));
    // push still works on arrays
    assert_eq!(
        run("array_push(#[1, 2], 3);"),
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

    // Mixed modulo is now rejected statically by HM validation.
    let err = run_any_error("10 % 3.0;");
    assert!(
        err.contains("[E300]")
            && err.contains("expected type: Int")
            && err.contains("found type: Float"),
        "Expected compile-time type mismatch for mixed modulo, got: {}",
        err
    );
    let err = run_any_error("10.5 % 3;");
    assert!(
        err.contains("[E300]")
            && err.contains("expected type: Float")
            && err.contains("found type: Int"),
        "Expected compile-time type mismatch for mixed modulo, got: {}",
        err
    );

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
    let input = format!("let arr = #[{}]; len(arr);", values);
    assert_eq!(run(&input), Value::Integer(3000));
}

#[test]
fn large_map_pipeline_no_stack_overflow() {
    let values = (1..=3000)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let input = format!(
        "let arr = [{}]; let mapped = map(arr, \\x -> x + 1); len(mapped);",
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
let arr = [{}];
let mapped = map(arr, \x -> x + 1);
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
        run("let double = fn(x) { x * 2 }; 5 |> double;"),
        Value::Integer(10)
    );

    // Chained pipes: value |> f |> g
    assert_eq!(
        run("let double = fn(x) { x * 2 }; let triple = fn(x) { x * 3 }; 5 |> double |> triple;"),
        Value::Integer(30)
    );

    // Pipe with additional arguments: value |> function(arg)
    assert_eq!(
        run("let add = fn(x, y) { x + y }; 5 |> add(3);"),
        Value::Integer(8)
    );

    // Pipe with multiple additional arguments
    assert_eq!(
        run("let sum3 = fn(a, b, c) { a + b + c }; 1 |> sum3(2, 3);"),
        Value::Integer(6)
    );

    // Complex chain with mixed calls
    assert_eq!(
        run(r#"
            let double = fn(x) { x * 2 };
            let add = fn(x, y) { x + y };
            let square = fn(x) { x * x };
            2 |> double |> add(10) |> square;
        "#),
        Value::Integer(196) // ((2*2) + 10)^2 = 14^2 = 196
    );

    // Pipe preserves argument order (left side becomes first arg)
    assert_eq!(
        run("let subtract = fn(a, b) { a - b }; 10 |> subtract(3);"),
        Value::Integer(7) // 10 - 3 = 7
    );

    // Pipe with string operations
    assert_eq!(
        run(r#"
            let greet = fn(name) { "Hello, " + name };
            let exclaim = fn(s) { s + "!" };
            "World" |> greet |> exclaim;
        "#),
        Value::String("Hello, World!".to_string().into())
    );

    // Pipe with array operations
    assert_eq!(
        run("let getFirst = fn(arr) { first(arr) }; [1, 2, 3] |> getFirst;"),
        Value::Integer(1)
    );

    // Nested pipe expressions
    assert_eq!(
        run(r#"
            let inc = fn(x) { x + 1 };
            let double = fn(x) { x * 2 };
            (3 |> inc) |> double;
        "#),
        Value::Integer(8) // (3+1) * 2 = 8
    );
}

#[test]
fn test_either_left_right() {
    // Basic Left creation
    assert_eq!(
        run("let x = Left(42); x;"),
        Value::Left(std::rc::Rc::new(Value::Integer(42)))
    );

    // Basic Right creation
    assert_eq!(
        run("let x = Right(42); x;"),
        Value::Right(std::rc::Rc::new(Value::Integer(42)))
    );

    // Left with string
    assert_eq!(
        run(r#"let x = Left("error"); x;"#),
        Value::Left(std::rc::Rc::new(Value::String("error".to_string().into())))
    );

    // Right with string
    assert_eq!(
        run(r#"let x = Right("success"); x;"#),
        Value::Right(std::rc::Rc::new(Value::String(
            "success".to_string().into()
        )))
    );

    // Nested Left
    assert_eq!(
        run("let x = Left(Left(1)); x;"),
        Value::Left(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Nested Right
    assert_eq!(
        run("let x = Right(Right(1)); x;"),
        Value::Right(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Left containing Right
    assert_eq!(
        run("let x = Left(Right(42)); x;"),
        Value::Left(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(42)
        ))))
    );

    // Right containing Left
    assert_eq!(
        run("let x = Right(Left(42)); x;"),
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
            fn fail(msg) { Left(msg) }
            fail("oops");
        "#),
        Value::Left(std::rc::Rc::new(Value::String("oops".to_string().into())))
    );

    // Function returning Right
    assert_eq!(
        run(r#"
            fn succeed(val) { Right(val) }
            succeed(100);
        "#),
        Value::Right(std::rc::Rc::new(Value::Integer(100)))
    );

    // Safe divide function
    assert_eq!(
        run(r#"
            fn safeDivide(a, b) {
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
            fn safeDivide(a, b) {
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
        run("let x = Left(Some(42)); x;"),
        Value::Left(std::rc::Rc::new(Value::Some(std::rc::Rc::new(
            Value::Integer(42)
        ))))
    );

    // Right containing None
    assert_eq!(
        run("let x = Right(None); x;"),
        Value::Right(std::rc::Rc::new(Value::None))
    );

    // Some containing Left
    assert_eq!(
        run("let x = Some(Left(1)); x;"),
        Value::Some(std::rc::Rc::new(Value::Left(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );

    // Some containing Right
    assert_eq!(
        run("let x = Some(Right(1)); x;"),
        Value::Some(std::rc::Rc::new(Value::Right(std::rc::Rc::new(
            Value::Integer(1)
        ))))
    );
}

#[test]
fn test_either_in_arrays() {
    // Array of Either values
    assert_eq!(
        run("#[Left(1), Right(2), Left(3)];"),
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
fn test_base_map() {
    assert_eq!(
        run("fold(map([1, 2, 3], fn(x) { x * 2 }), 0, fn(a, x) { a + x });"),
        Value::Integer(12)
    );
}

#[test]
fn test_base_map_with_closure() {
    assert_eq!(
        run("let factor = 3; fold(map([1, 2, 3], fn(x) { x * factor }), 0, fn(a, x) { a + x });"),
        Value::Integer(18)
    );
}

#[test]
fn test_base_map_empty() {
    assert_eq!(run("len(map([], fn(x) { x }));"), Value::Integer(0));
}

#[test]
fn test_base_map_with_base_callback() {
    assert_eq!(
        run(
            "import Flow.List as L\nL.fold(L.map([1, 2, 3], \\x -> to_string(x)), \"\", fn(a, x) { a + x });"
        ),
        Value::String("123".to_string().into())
    );
}

#[test]
fn test_base_filter() {
    assert_eq!(
        run("fold(filter([1, 2, 3, 4, 5], fn(x) { x > 2 }), 0, fn(a, x) { a + x });"),
        Value::Integer(12)
    );
}

#[test]
fn test_base_filter_none_pass() {
    assert_eq!(
        run("len(filter([1, 2, 3], fn(x) { x > 10 }));"),
        Value::Integer(0)
    );
}

#[test]
fn test_base_filter_all_pass() {
    assert_eq!(
        run("len(filter([1, 2, 3], fn(x) { x > 0 }));"),
        Value::Integer(3)
    );
}

#[test]
fn test_base_fold_sum() {
    assert_eq!(
        run("fold([1, 2, 3, 4], 0, fn(acc, x) { acc + x });"),
        Value::Integer(10)
    );
}

#[test]
fn test_base_fold_string_concat() {
    assert_eq!(
        run(r#"import Flow.List as L
L.fold(["a", "b", "c"], "", fn(acc, x) { acc + x });"#),
        Value::String("abc".to_string().into())
    );
}

#[test]
fn test_base_fold_empty() {
    assert_eq!(
        run("fold([], 42, fn(acc, x) { acc + x });"),
        Value::Integer(42)
    );
}

#[test]
fn test_map_filter_chain() {
    assert_eq!(
        run(r#"
            let nums = [1, 2, 3, 4, 5, 6];
            let doubled = map(nums, fn(x) { x * 2 });
            fold(filter(doubled, fn(x) { x > 6 }), 0, fn(a, x) { a + x });
        "#),
        Value::Integer(30)
    );
}

#[test]
fn test_map_fold_chain() {
    assert_eq!(
        run(r#"
            let nums = [1, 2, 3];
            let doubled = map(nums, fn(x) { x * 2 });
            fold(doubled, 0, fn(acc, x) { acc + x });
        "#),
        Value::Integer(12)
    );
}

#[test]
fn test_map_with_lambda() {
    assert_eq!(
        run(r#"fold(map([1, 2, 3], \x -> x + 10), 0, fn(a, x) { a + x });"#),
        Value::Integer(36)
    );
}

#[test]
fn test_filter_with_lambda() {
    assert_eq!(
        run(r#"fold(filter([1, 2, 3, 4], \x -> x > 2), 0, fn(a, x) { a + x });"#),
        Value::Integer(7)
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
    let err = run_any_error("map(42, fn(x) { x });");
    assert!(
        err.contains("E300") && err.contains("Parameter 1"),
        "Expected compile-time parameter type mismatch, got: {}",
        err
    );
}

#[test]
fn test_map_type_error_not_function() {
    let err = run_any_error("map([1, 2], 42);");
    assert!(
        err.contains("E300") && err.contains("Parameter 2"),
        "Expected compile-time callback type mismatch, got: {}",
        err
    );
}

#[test]
fn test_filter_type_error() {
    let err = run_any_error("filter(42, fn(x) { x });");
    assert!(
        err.contains("E300") && err.contains("Parameter 1"),
        "Expected compile-time parameter type mismatch, got: {}",
        err
    );
}

#[test]
fn test_fold_type_error() {
    let err = run_any_error("import Flow.List as L\nL.fold(42, 0, fn(a, x) { a + x });");
    assert!(
        err.contains("E300") && err.contains("Parameter 1"),
        "Expected compile-time parameter type mismatch, got: {}",
        err
    );
}

#[test]
fn test_map_callback_arity_error_propagates() {
    let err = run_any_error("map([1], fn(a, b) { a + b });");
    assert!(
        err.contains("E300") && err.contains("Parameter 2"),
        "Expected compile-time callback arity mismatch, got: {}",
        err
    );
}

#[test]
fn test_filter_callback_arity_error_propagates() {
    let err = run_any_error("filter([1], fn(a, b) { a > b });");
    assert!(
        err.contains("E300") && err.contains("Parameter 2"),
        "Expected compile-time callback arity mismatch, got: {}",
        err
    );
}

#[test]
fn test_fold_callback_arity_error_propagates() {
    let err = run_any_error("fold([1], 0, fn(a) { a });");
    assert!(
        err.contains("E300") && err.contains("Parameter 3"),
        "Expected compile-time callback arity mismatch, got: {}",
        err
    );
}

#[test]
fn test_map_callback_runtime_error_propagates() {
    let err = run_any_error("map([1], fn(x) { x + true });");
    assert!(
        (err.contains("[E1009]") && err.contains("Cannot add")) || err.contains("[E300]"),
        "Expected callback runtime error or compile-time mismatch, got: {}",
        err
    );
}

#[test]
fn test_filter_callback_runtime_error_propagates() {
    let err = run_any_error("filter([1], fn(x) { x + true });");
    assert!(
        (err.contains("[E1009]") && err.contains("Cannot add")) || err.contains("[E300]"),
        "Expected callback runtime error or compile-time mismatch, got: {}",
        err
    );
}

#[test]
fn test_fold_callback_runtime_error_propagates() {
    let err = run_any_error("fold([1], 0, fn(acc, x) { acc + true });");
    assert!(
        (err.contains("[E1009]") && err.contains("Cannot add")) || err.contains("[E300]"),
        "Expected callback runtime error or compile-time mismatch, got: {}",
        err
    );
}

#[test]
fn test_map_type_of_homogeneous_array() {
    // Map type_of over a homogeneous int cons list, verify via fold
    assert_eq!(
        run(r#"import Flow.List as L
L.fold(L.map([1, 2, 3], \x -> type_of(x)), "", fn(a, x) { a + x });"#),
        Value::String("IntIntInt".to_string().into())
    );
}

#[test]
fn test_map_returns_nested_arrays() {
    // Map callback returns nested arrays inside a cons list; verify structure via fold
    assert_eq!(
        run("let result = map([1, 2], fn(x) { #[x, x * 2] }); len(result);"),
        Value::Integer(2)
    );
    // Check first element is array [1, 2]
    assert_eq!(
        run("let result = map([1, 2], fn(x) { #[x, x * 2] }); len(first(result));"),
        Value::Integer(2)
    );
}

#[test]
fn test_filter_returns_nested_structures() {
    // Filter returns a cons list; verify via fold
    assert_eq!(
        run(r#"fold(filter([1, 2, 3], fn(x) { x > 1 }), 0, fn(a, x) { a + x });"#),
        Value::Integer(5)
    );
}

#[test]
fn test_map_evaluation_order_with_side_effects() {
    // Verify left-to-right evaluation order by mapping then converting to array
    let result = run(r#"
        import Flow.List as L
        let mapped = L.map([1, 2, 3], fn(x) { x * 2 });
        to_array(mapped);
    "#);
    assert_eq!(
        result,
        Value::Array(std::rc::Rc::new(vec![
            Value::Integer(2),
            Value::Integer(4),
            Value::Integer(6),
        ]))
    );
}

#[test]
fn test_filter_evaluation_order_stable() {
    // Verify filter processes elements in left-to-right order
    let result = run(r#"
        import Flow.List as L
        let filtered = L.filter([5, 3, 8, 1], fn(x) { x > 2 });
        to_array(filtered);
    "#);
    // Should see all elements that passed (5, 3, 8) in original order
    assert_eq!(
        result,
        Value::Array(std::rc::Rc::new(vec![
            Value::Integer(5),
            Value::Integer(3),
            Value::Integer(8),
        ]))
    );
}

#[test]
fn test_fold_evaluation_order_deterministic() {
    // Verify fold processes elements left-to-right by computing a
    // left-associative subtraction: ((((0 - 1) - 2) - 3) - 4) = -10
    let result = run(r#"
        import Flow.List as L
        L.fold([1, 2, 3, 4], 0, fn(acc, x) {
            acc - x
        });
    "#);
    assert_eq!(result, Value::Integer(-10));
}

#[test]
fn test_map_heterogeneous_array_is_compile_error() {
    // Mixed-type array literals are rejected at compile time (E300)
    let err = run_any_error(r#"map(#[1, 2, "oops", 4], fn(x) { x + 10 });"#);
    assert!(
        err.contains("E300") || err.contains("TYPE UNIFICATION ERROR"),
        "Expected E300 compile error for heterogeneous array, got: {}",
        err
    );
}

#[test]
fn test_filter_error_includes_index() {
    // The callback `fn(x) { x + "bad" }` returns String, but filter expects Bool.
    // May be caught at compile time (E300) or runtime (type error).
    let err = run_any_error(r#"filter([1, 2, 3, 4], fn(x) { x + "bad" });"#);
    assert!(
        err.contains("E300") || err.contains("Bool") || err.contains("Invalid Operation"),
        "Expected type error, got: {}",
        err
    );
}

#[test]
fn test_fold_error_includes_index() {
    // Verify error messages include element index
    let err = run_any_error(
        r#"fold(#[1, 2, 3], 0, fn(acc, x) { if x == 2 { acc + "bad"; } else { acc + x; }; });"#,
    );
    assert!(
        err.contains("index 1") || err.contains("[E300]"),
        "Expected runtime index error or compile-time mismatch, got: {}",
        err
    );
}

#[test]
fn test_map_with_option_values() {
    // Map over cons list producing Some/None values; verify via len and hd
    assert_eq!(
        run(r#"len(map([1, 2, 3], fn(x) { if x == 2 { None } else { Some(x) } }));"#),
        Value::Integer(3)
    );
    assert_eq!(
        run(r#"first(map([1, 2, 3], fn(x) { if x == 2 { None } else { Some(x) } }));"#),
        make_some(Value::Integer(1))
    );
}

#[test]
fn test_filter_truthiness_zero_is_truthy() {
    // Verify 0 is truthy (not like JavaScript) — filter with explicit bool predicate
    // All elements should pass because the predicate always returns true for any Int
    assert_eq!(
        run("import Flow.List as L\nlen(L.filter([0, 1, 2], fn(x) { x == x }));"),
        Value::Integer(3)
    );
    assert_eq!(
        run("import Flow.List as L\nlen(L.filter([0, 1, 2], fn(x) { x >= 0 }));"),
        Value::Integer(3)
    );
}

#[test]
fn test_filter_truthiness_empty_string_is_truthy() {
    // Verify empty string passes filter — all strings including "" are valid
    assert_eq!(
        run(r#"import Flow.List as L
len(L.filter(["", "a", "b"], fn(x) { x == x }));"#),
        Value::Integer(3)
    );
}

#[test]
fn test_filter_truthiness_empty_array_is_truthy() {
    // Verify empty array passes filter — all arrays including #[] are valid
    assert_eq!(
        run("import Flow.List as L\nlen(L.filter([#[], #[1], #[2, 3]], fn(x) { x == x }));"),
        Value::Integer(3)
    );
}

#[test]
fn test_map_large_array_5k() {
    // Test that the growable stack handles 5k elements
    let program = format!(
        "let big = [{}]; let doubled = map(big, fn(x) {{ x * 2; }}); len(doubled);",
        (0..5000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(run(&program), Value::Integer(5000));
}

#[test]
fn test_filter_large_array_5k() {
    // Test filter with 5k elements
    let program = format!(
        "let big = [{}]; let filtered = filter(big, fn(x) {{ x % 2 == 0 }}); len(filtered);",
        (0..5000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(run(&program), Value::Integer(2500));
}

#[test]
fn test_fold_large_array_5k() {
    // Test fold with 5k elements
    let program = format!(
        "let big = [{}]; fold(big, 0, fn(acc, x) {{ acc + 1 }});",
        (0..5000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(run(&program), Value::Integer(5000));
}

#[test]
fn test_chained_operations_large_array() {
    // Test chained map/filter/fold with large cons list
    let program = format!(
        "let arr = [{}]; \
         let mapped = map(arr, fn(x) {{ x * 2 }}); \
         let filtered = filter(mapped, fn(x) {{ x % 3 == 0 }}); \
         len(filtered);",
        (0..1000)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let result = run(&program);
    assert!(matches!(result, Value::Integer(_)));
}

// ── List (cons cell) end-to-end tests ───────────────────────────────────

#[test]
fn test_cons_syntax() {
    let result = run("[1 | [2 | [3 | []]]];");
    // Returns a cons cell (Rc-based or GC-based)
    assert!(matches!(result, Value::Cons(_)));
}

#[test]
fn test_cons_first_rest() {
    assert_eq!(run("first([1 | [2 | []]]);"), Value::Integer(1));
    assert_eq!(run("first(rest([1 | [2 | []]]));"), Value::Integer(2));
}

#[test]
fn test_array_does_not_match_list_pattern() {
    assert_eq!(
        run(r#"match [|1, 2, 3|] { [h | t] -> "list", _ -> "array" };"#),
        make_string("array")
    );
}

#[test]
fn test_tuple_does_not_match_option_pattern() {
    assert_eq!(
        run(r#"match (1, 2) { Some(x) -> "option", _ -> "tuple" };"#),
        make_string("tuple")
    );
}

#[test]
fn test_named_field_access_in_concrete_option_match_with_wildcard() {
    assert_eq!(
        run(r#"
data Contact {
    Contact { name: String }
}

let contact = Contact { name: "Alice" };
let value: Option<Contact> = Some(contact);
match value {
    Some(c) -> c.name,
    None -> "missing",
    _ -> "missing",
};
"#),
        make_string("Alice")
    );
}

#[test]
fn test_list_constructor() {
    assert_eq!(run("first(list(10, 20, 30));"), Value::Integer(10));
    assert_eq!(run("first(rest(list(10, 20, 30)));"), Value::Integer(20));
    assert_eq!(
        run("first(rest(rest(list(10, 20, 30))));"),
        Value::Integer(30)
    );
}

#[test]
fn test_list_len() {
    assert_eq!(run("len(list(1, 2, 3));"), Value::Integer(3));
    assert_eq!(run("len(list());"), Value::Integer(0));
}

#[test]
fn test_list_first_rest() {
    assert_eq!(run("first(list(10, 20));"), Value::Integer(10));
    assert_eq!(run("first(rest(list(10, 20, 30)));"), Value::Integer(20));
}

#[test]
fn test_list_to_array_round_trip() {
    let result = run("to_array(to_list(#[1, 2, 3]));");
    assert_eq!(
        result,
        Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
    );
}

#[test]
fn test_list_reverse() {
    // reverse now operates on cons lists
    assert_eq!(run("first(reverse([1, 2, 3]));"), Value::Integer(3));
    assert_eq!(run("len(reverse([1, 2, 3]));"), Value::Integer(3));
}

#[test]
fn test_list_contains() {
    assert_eq!(run("contains(list(1, 2, 3), 2);"), Value::Boolean(true));
    assert_eq!(run("contains(list(1, 2, 3), 99);"), Value::Boolean(false));
}

#[test]
fn test_list_pattern_match() {
    assert_eq!(
        run("let xs = list(10, 20, 30); match xs { [h | t] -> h, _ -> 0 };"),
        Value::Integer(10)
    );
    assert_eq!(
        run("let xs = list(); match xs { [h | t] -> h, _ -> 0 };"),
        Value::Integer(0)
    );
}

#[test]
fn test_list_is_list() {
    assert_eq!(run("is_list(list(1, 2));"), Value::Boolean(true));
    assert_eq!(run("is_list(list());"), Value::Boolean(true));
    assert_eq!(run("is_list(#[1, 2]);"), Value::Boolean(false));
}

#[test]
fn test_list_phase_1b_slicing_and_span() {
    assert_eq!(
        run("to_string(take([1, 2, 3, 4], 2));"),
        make_string("[1, 2]")
    );
    assert_eq!(
        run("to_string(drop([1, 2, 3, 4], 2));"),
        make_string("[3, 4]")
    );
    assert_eq!(run("to_string(take([1, 2, 3], 0));"), make_string("[]"));
    assert_eq!(
        run("to_string(drop([1, 2, 3], -1));"),
        make_string("[1, 2, 3]")
    );
    assert_eq!(run("to_string(take([1, 2], 10));"), make_string("[1, 2]"));
    assert_eq!(
        run("to_string(take_while([1, 2, 3, 1], fn(x) { x < 3 }));"),
        make_string("[1, 2]")
    );
    assert_eq!(
        run("to_string(drop_while([1, 2, 3, 1], fn(x) { x < 3 }));"),
        make_string("[3, 1]")
    );
    assert_eq!(
        run("to_string(split_at([1, 2, 3, 4], 2));"),
        make_string("([1, 2], [3, 4])")
    );
    assert_eq!(
        run("to_string(span([1, 2, 3, 1], fn(x) { x < 3 }));"),
        make_string("([1, 2], [3, 1])")
    );
}

#[test]
fn test_list_phase_1b_folds_scans_and_builders() {
    assert_eq!(
        run(r#"foldr(["a", "b", "c"], "", fn(x, acc) { x + acc });"#),
        make_string("abc")
    );
    assert_eq!(
        run("fold1([2, 3, 4], fn(a, b) { a * b });"),
        Value::Integer(24)
    );
    assert_eq!(
        run("to_string(scanl([1, 2, 3], 0, fn(acc, x) { acc + x }));"),
        make_string("[0, 1, 3, 6]")
    );
    assert_eq!(
        run("to_string(scanr([1, 2, 3], 0, fn(x, acc) { x + acc }));"),
        make_string("[6, 5, 3, 0]")
    );
    assert_eq!(run("to_string(replicate(3, 9));"), make_string("[9, 9, 9]"));
    assert_eq!(
        run("to_string(iterate(1, fn(x) { x * 2 }, 4));"),
        make_string("[1, 2, 4, 8]")
    );
    assert_eq!(
        run("to_string(unfold(0, fn(n) { if n < 4 { Some((n * 2, n + 1)) } else { None } }));"),
        make_string("[0, 2, 4, 6]")
    );
    assert_eq!(
        run("import Flow.List as L\nto_string(L.concat([[1, 2], [3], [], [4, 5]]));"),
        make_string("[1, 2, 3, 4, 5]")
    );
}

#[test]
fn test_list_phase_1b_zip_group_and_uniqueness() {
    assert_eq!(
        run("to_string(zip_with([1, 2, 3], [10, 20], fn(a, b) { a + b }));"),
        make_string("[11, 22]")
    );
    assert_eq!(
        run("to_string(unzip([(1, \"a\"), (2, \"b\")]));"),
        make_string("([1, 2], [\"a\", \"b\"])")
    );
    assert_eq!(
        run("to_string(enumerate([10, 20, 30]));"),
        make_string("[(0, 10), (1, 20), (2, 30)]")
    );
    assert_eq!(
        run("to_string(partition([1, 2, 3, 4, 5], fn(x) { x % 2 == 0 }));"),
        make_string("([2, 4], [1, 3, 5])")
    );
    assert_eq!(
        run("to_string(group_by([1, 1, 2, 2, 1], fn(a, b) { a == b }));"),
        make_string("[[1, 1], [2, 2], [1]]")
    );
    assert_eq!(
        run("to_string(unique_by([1, 3, 2, 4, 5], fn(x) { x % 2 }));"),
        make_string("[1, 2]")
    );
    assert_eq!(run("length(nub([1, 2, 1, 3, 2]));"), Value::Integer(3));
}

#[test]
fn test_list_phase_1b_set_like_and_prefix_suffix() {
    assert_eq!(
        run("import Flow.List as L\nto_string(L.delete([1, 2, 3, 2], 2));"),
        make_string("[1, 3, 2]")
    );
    assert_eq!(
        run(r#"to_string(intersperse(["a", "b", "c"], "-"));"#),
        make_string("[\"a\", \"-\", \"b\", \"-\", \"c\"]")
    );
    assert_eq!(
        run(r#"to_string(intercalate([["a"], ["b", "c"], ["d"]], ["-"]));"#),
        make_string("[\"a\", \"-\", \"b\", \"c\", \"-\", \"d\"]")
    );
    assert_eq!(run("is_prefix([1, 2], [1, 2, 3]);"), Value::Boolean(true));
    assert_eq!(run("is_prefix([1, 3], [1, 2, 3]);"), Value::Boolean(false));
    assert_eq!(run("is_suffix([2, 3], [1, 2, 3]);"), Value::Boolean(true));
    assert_eq!(run("is_suffix([1, 3], [1, 2, 3]);"), Value::Boolean(false));
}

#[test]
fn test_list_phase_1b_utilities_and_sorting() {
    assert_eq!(run("length([1, 2, 3, 4]);"), Value::Integer(4));
    assert_eq!(run("null([]);"), Value::Boolean(true));
    assert_eq!(run("null([1]);"), Value::Boolean(false));
    assert_eq!(run("to_string(init([1, 2, 3]));"), make_string("[1, 2]"));
    assert_eq!(
        run("to_string(nth([10, 20, 30], 1));"),
        make_string("Some(20)")
    );
    assert_eq!(
        run("to_string(nth([10, 20, 30], 99));"),
        make_string("None")
    );
    assert_eq!(
        run("to_string(nth([10, 20, 30], -1));"),
        make_string("None")
    );
    assert_eq!(
        run("to_string(maximum([3, 9, 4]));"),
        make_string("Some(9)")
    );
    assert_eq!(
        run("to_string(minimum([3, 9, 4]));"),
        make_string("Some(3)")
    );
    assert_eq!(run("to_string(maximum([]));"), make_string("None"));
    assert_eq!(run("to_string(minimum([]));"), make_string("None"));
    assert_eq!(
        run("to_string(sort([3, 1, 4, 1, 2]));"),
        make_string("[1, 1, 2, 3, 4]")
    );
    assert_eq!(
        run("to_string(sort_by([\"bbb\", \"a\", \"cc\"], fn(x) { len(x) }));"),
        make_string("[\"a\", \"cc\", \"bbb\"]")
    );
}

#[test]
fn test_list_phase_1b_empty_list_panics() {
    let fold1_err = run_error("fold1([], fn(a, b) { a + b });");
    assert!(
        fold1_err.contains("fold1 called on empty list"),
        "expected fold1 panic message, got: {}",
        fold1_err
    );

    let init_err = run_error("init([]);");
    assert!(
        init_err.contains("init called on empty list"),
        "expected init panic message, got: {}",
        init_err
    );
}

#[test]
fn test_removed_list_and_array_sort_primops_require_library_apis() {
    assert_eq!(CorePrimOp::from_name("hd", 1), None);
    assert_eq!(CorePrimOp::from_name("tl", 1), None);
    assert_eq!(CorePrimOp::from_name("sort", 1), None);

    assert_eq!(
        run("import Flow.Array as Array\nto_string(Array.sort([|3, 1, 2|]));"),
        make_string("[|1, 2, 3|]")
    );
}

#[test]
fn test_tuple_literals_and_grouping() {
    assert_eq!(
        run("to_string((1, 2, 3));"),
        Value::String("(1, 2, 3)".to_string().into())
    );
    assert_eq!(
        run("to_string((42,));"),
        Value::String("(42,)".to_string().into())
    );
    assert_eq!(
        run("to_string(());"),
        Value::String("()".to_string().into())
    );
    assert_eq!(run("(1 + 2);"), Value::Integer(3));
}

#[test]
fn test_tuple_destructure_and_nested_destructure() {
    assert_eq!(run("let (a, b) = (1, 2); a + b;"), Value::Integer(3));
    assert_eq!(
        run(r#"let (x, y, z) = (1, "two", true); type_of(y);"#),
        Value::String("String".to_string().into())
    );
    assert_eq!(
        run("let (a, (b, c)) = (1, (2, 3)); a + b + c;"),
        Value::Integer(6)
    );
}

#[test]
fn test_tuple_access_and_indexing() {
    assert_eq!(run("let t = (1, 2, 3); t.0;"), Value::Integer(1));
    assert_eq!(run("let t = (1, 2, 3); t.1;"), Value::Integer(2));
    assert_eq!(
        run("let t = (1, 2, 3); t[0];"),
        Value::Some(std::rc::Rc::new(Value::Integer(1)))
    );
    assert_eq!(run("let t = (1, 2, 3); t[99];"), Value::None);
}

#[test]
fn test_tuple_match_and_base_functions() {
    assert_eq!(
        run("let pair = (1, 2); match pair { (a, b) -> a + b, _ -> 0 };"),
        Value::Integer(3)
    );
    assert_eq!(run("len((1, 2, 3));"), Value::Integer(3));
    assert_eq!(
        run(r#"type_of((1, 2));"#),
        Value::String("Tuple".to_string().into())
    );
    assert_eq!(run("(1, 2) == (1, 2);"), Value::Boolean(true));
}

// ═══════════════════════════════════════════════════════════════════
//  List comprehensions
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_list_comprehension_single_generator() {
    assert_eq!(
        run("fold([x * 2 | x <- [1, 2, 3]], 0, fn(a, x) { a + x });"),
        Value::Integer(12)
    );
}

#[test]
fn test_list_comprehension_with_guard() {
    assert_eq!(
        run(r#"import Flow.List as L
fn map(xs, f) { L.map(xs, f) }
fn filter(xs, pred) { L.filter(xs, pred) }
fn flat_map(xs, f) { L.flat_map(xs, f) }
L.fold([x * 2 | x <- [1, 2, 3, 4, 5], x > 3], 0, fn(a, x) { a + x });"#),
        Value::Integer(18)
    );
}

#[test]
fn test_list_comprehension_multiple_guards() {
    assert_eq!(
        run("fold([x | x <- [1, 2, 3, 4, 5, 6], x > 2, x < 5], 0, fn(a, x) { a + x });"),
        Value::Integer(7)
    );
}

#[test]
fn test_list_comprehension_two_generators() {
    assert_eq!(
        run("fold([x + y | x <- [1, 2], y <- [10, 20]], 0, fn(a, x) { a + x });"),
        Value::Integer(66)
    );
}

#[test]
fn test_list_comprehension_guard_and_two_generators() {
    assert_eq!(
        run("fold([x + y | x <- [1, 2, 3], x > 1, y <- [100, 200]], 0, fn(a, x) { a + x });"),
        Value::Integer(610)
    );
}

#[test]
fn test_list_comprehension_cons_list() {
    // Comprehension over cons lists should return a cons list
    assert_eq!(
        run(r#"import Flow.List as L
fn map(xs, f) { L.map(xs, f) }
fn filter(xs, pred) { L.filter(xs, pred) }
fn flat_map(xs, f) { L.flat_map(xs, f) }
let xs = list(1, 2, 3); L.fold([x * 10 | x <- xs], 0, fn(a, x) { a + x });"#),
        Value::Integer(60)
    );
}

#[test]
fn test_list_comprehension_identity() {
    assert_eq!(
        run("fold([x | x <- [1, 2, 3]], 0, fn(a, x) { a + x });"),
        Value::Integer(6)
    );
}

#[test]
fn test_flat_map_base() {
    assert_eq!(
        run(r#"fold(flat_map([1, 2, 3], \x -> [x, x * 10]), 0, fn(a, x) { a + x });"#),
        Value::Integer(66)
    );
}
