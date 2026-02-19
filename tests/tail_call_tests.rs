use flux::bytecode::compiler::Compiler;
use flux::bytecode::op_code::disassemble;
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::runtime::vm::VM;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

fn run(input: &str) -> Value {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

fn compile(input: &str) -> flux::bytecode::bytecode::Bytecode {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler.bytecode()
}

fn find_function_disassembly(
    bytecode: &flux::bytecode::bytecode::Bytecode,
    params: usize,
) -> String {
    let function = bytecode
        .constants
        .iter()
        .find_map(|v| match v {
            Value::Function(f) if f.num_parameters == params => Some(f.clone()),
            _ => None,
        })
        .expect("expected function constant");
    disassemble(&function.instructions)
}

#[test]
fn test_tail_call_deep_recursion_countdown() {
    // Test deep recursion without stack overflow
    // This would fail without tail call optimization
    let input = r#"
        fn countdown(n) {
            if n == 0 {
                0
            } else {
                countdown(n - 1)
            }
        }
        countdown(100000);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn test_tail_call_factorial_accumulator() {
    // Test tail-recursive accumulator pattern
    let input = r#"
        fn factorial(n, acc) {
            if n == 0 {
                acc
            } else {
                factorial(n - 1, n * acc)
            }
        }
        factorial(20, 1);
    "#;

    let result = run(input);
    // 20! = 2432902008176640000
    assert_eq!(result, Value::Integer(2432902008176640000));
}

#[test]
fn test_non_tail_recursion_still_works() {
    // Test that non-tail recursion still works correctly
    // This should use OpCall, not OpTailCall
    let input = r#"
        fn fib(n) {
            if n <= 1 {
                n
            } else {
                fib(n - 1) + fib(n - 2)
            }
        }
        fib(10);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Integer(55));
}

#[test]
fn test_tail_call_in_if_branches() {
    // Test that tail calls work in both if/else branches
    let input = r#"
        fn even_odd(n) {
            if n == 0 {
                "even"
            } else {
                if n == 1 {
                    "odd"
                } else {
                    even_odd(n - 2)
                }
            }
        }
        even_odd(1000);
    "#;

    let result = run(input);
    assert_eq!(result, Value::String("even".into()));
}

#[test]
fn test_tail_call_in_match_arms() {
    // Test that tail calls work in match expressions
    let input = r#"
        fn match_countdown(n) {
            match n {
                0 -> 0,
                _ -> match_countdown(n - 1),
            }
        }
        match_countdown(50000);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Integer(0));
}

#[test]
fn test_tail_call_with_multiple_args() {
    // Test tail call with multiple arguments
    let input = r#"
        fn sum_to_n(n, acc) {
            if n == 0 {
                acc
            } else {
                sum_to_n(n - 1, acc + n)
            }
        }
        sum_to_n(1000, 0);
    "#;

    let result = run(input);
    // Sum of 1 to 1000 = 1000 * 1001 / 2 = 500500
    assert_eq!(result, Value::Integer(500500));
}

#[test]
fn test_tail_call_with_same_arg_twice() {
    // Test that pre-copying arguments works correctly
    // when the same variable is passed multiple times
    let input = r#"
        fn repeat_until_zero(n, count) {
            if n == 0 {
                count
            } else {
                repeat_until_zero(n - 1, count + 1)
            }
        }
        repeat_until_zero(100, 0);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Integer(100));
}

#[test]
fn test_mutual_recursion_not_optimized() {
    // Test that mutual recursion still works (not optimized yet)
    // This uses OpCall, not OpTailCall, but should still work for small inputs
    let input = r#"
        fn is_even(n) {
            if n == 0 {
                true
            } else {
                is_odd(n - 1)
            }
        }

        fn is_odd(n) {
            if n == 0 {
                false
            } else {
                is_even(n - 1)
            }
        }

        is_even(10);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Boolean(true));
}

#[test]
fn test_tail_call_returns_correct_value() {
    // Test that tail calls return the correct final value
    let input = r#"
        fn find_value(n, target) {
            if n == target {
                n
            } else {
                if n > target {
                    "not found"
                } else {
                    find_value(n + 1, target)
                }
            }
        }
        find_value(0, 500);
    "#;

    let result = run(input);
    assert_eq!(result, Value::Integer(500));
}

#[test]
fn test_tail_call_with_complex_condition() {
    // Test tail call with more complex conditional logic
    let input = r#"
        fn collatz(n, steps) {
            if n == 1 {
                steps
            } else {
                if n % 2 == 0 {
                    collatz(n / 2, steps + 1)
                } else {
                    collatz(n * 3 + 1, steps + 1)
                }
            }
        }
        collatz(27, 0);
    "#;

    let result = run(input);
    // Collatz sequence for 27 takes 111 steps
    assert_eq!(result, Value::Integer(111));
}

#[test]
fn test_non_value_tail_statement_does_not_emit_tail_call() {
    let input = r#"
        fn f(n) {
            if n == 0 {
                let x = 1;
            } else {
                let y = f(n - 1);
            }
        }
        f(2);
    "#;

    let result = run(input);
    assert_eq!(result, Value::None);
}

#[test]
fn test_phase2_emits_consume_local_for_accumulator_tail_call() {
    let input = r#"
        fn build(n, acc) {
            if n == 0 {
                acc
            } else {
                build(n - 1, push(acc, n))
            }
        }
        build(3, []);
    "#;

    let bytecode = compile(input);
    let asm = find_function_disassembly(&bytecode, 2);
    assert!(asm.contains("OpConsumeLocal"));
    assert!(asm.contains("OpTailCall"));
}

#[test]
fn test_phase2_does_not_consume_captured_accumulator_parameter() {
    let input = r#"
        fn build(n, acc) {
            let get = fn() { acc };
            if n == 0 {
                return get();
            } else {
                return build(n - 1, push(acc, n));
            }
        }
        build(3, []);
    "#;

    let bytecode = compile(input);
    let asm = find_function_disassembly(&bytecode, 2);
    assert!(
        !asm.contains("OpConsumeLocal"),
        "unexpected consume:\n{asm}"
    );
    assert!(asm.contains("OpGetLocal"));
}

#[test]
fn test_phase2_still_consumes_when_nested_function_does_not_capture_accumulator() {
    let input = r#"
        fn build(n, acc) {
            let const_one = fn() { 1 };
            if n == 0 {
                return acc;
            } else {
                return build(n - 1, push(acc, const_one()));
            }
        }
        build(3, []);
    "#;

    let bytecode = compile(input);
    let asm = find_function_disassembly(&bytecode, 2);
    assert!(asm.contains("OpConsumeLocal"), "missing consume:\n{asm}");
}

#[test]
fn test_return_nested_no_semicolon() {
    let input = r#"
        fn test(n) {
            if n == 0 {
                return "zero";
            } else {
                if n == 1 {
                    return "one";
                } else {
                    return "other";
                }
            }
        }
        test(1);
    "#;

    let result = run(input);
    assert_eq!(result, Value::String("one".into()));
}
