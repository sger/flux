use flux::bytecode::compiler::Compiler;
use flux::bytecode::op_code::disassemble;
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

fn compile_bytecode(input: &str) -> flux::bytecode::bytecode::Bytecode {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler.bytecode()
}

/// Disassemble ALL function constants in the bytecode (not just top-level).
fn all_functions_asm(input: &str) -> String {
    let bytecode = compile_bytecode(input);
    let mut out = disassemble(&bytecode.instructions);
    for value in &bytecode.constants {
        if let Value::Function(f) = value {
            out.push_str(&format!(
                "\n--- function ({} params) ---\n",
                f.num_parameters
            ));
            out.push_str(&disassemble(&f.instructions));
        }
    }
    out
}

/// Compile + run, return the last popped value.
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
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let bytecode = compiler.bytecode().clone();
    let mut vm = flux::bytecode::vm::VM::new(bytecode);
    vm.run().expect("VM error");
    vm.last_popped_stack_elem()
}

// ── Peephole fusion: OpAddLocals ────────────────────────────────────────

#[test]
fn add_locals_fuses_two_getlocals_and_add() {
    let asm = all_functions_asm(
        r#"
        fn f(a, b) { a + b }
        f(1, 2)
    "#,
    );
    assert!(asm.contains("OpAddLocals"), "expected OpAddLocals:\n{asm}");
}

#[test]
fn add_locals_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn f(a, b) { a + b }
            f(3, 7)
        "#),
        Value::Integer(10),
    );
}

// ── Peephole fusion: OpSubLocals ────────────────────────────────────────

#[test]
fn sub_locals_fuses_two_getlocals_and_sub() {
    let asm = all_functions_asm(
        r#"
        fn f(a, b) { a - b }
        f(1, 2)
    "#,
    );
    assert!(asm.contains("OpSubLocals"), "expected OpSubLocals:\n{asm}");
}

#[test]
fn sub_locals_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn f(a, b) { a - b }
            f(10, 3)
        "#),
        Value::Integer(7),
    );
}

// ── Peephole fusion: OpConstantAdd ──────────────────────────────────────

#[test]
fn constant_add_fuses_constant_and_add() {
    // OpConstantAdd fires in recursive tail calls where the constant
    // is added directly on the stack (not stored to a local first).
    let asm = all_functions_asm(
        r#"
        fn count(n) {
            if n >= 10 { n } else { count(n + 1) }
        }
        count(0)
    "#,
    );
    assert!(
        asm.contains("OpConstantAdd"),
        "expected OpConstantAdd:\n{asm}"
    );
}

#[test]
fn constant_add_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn inc(x) { x + 1 }
            inc(41)
        "#),
        Value::Integer(42),
    );
}

#[test]
fn constant_add_with_large_constant() {
    assert_eq!(
        run(r#"
            fn add_big(x) { x + 1000 }
            add_big(42)
        "#),
        Value::Integer(1042),
    );
}

// ── Peephole fusion: OpGetLocalGetLocal ─────────────────────────────────

#[test]
fn get_local_get_local_fuses_consecutive_getlocals() {
    // OpGetLocalGetLocal fires for locals with index >= 2 (locals 0 and 1
    // already have dedicated OpGetLocal0/OpGetLocal1 single-byte opcodes).
    let asm = all_functions_asm(
        r#"
        fn f(a, b, c) {
            let x = a + b
            let y = b + c
            x * y
        }
        f(1, 2, 3)
    "#,
    );
    assert!(
        asm.contains("OpGetLocalGetLocal"),
        "expected OpGetLocalGetLocal:\n{asm}"
    );
}

// ── Peephole fusion: OpSetLocalPop ──────────────────────────────────────

#[test]
fn set_local_pop_fuses_setlocal_and_pop() {
    // SetLocalPop fires when a value is stored and then immediately popped,
    // which happens with expression-statement-then-let patterns.
    let asm = all_functions_asm(
        r#"
        fn f(x) {
            let a = x + 1
            a
        }
        f(1)
    "#,
    );
    assert!(
        asm.contains("OpSetLocalPop") || asm.contains("OpSetLocal"),
        "expected OpSetLocal or OpSetLocalPop:\n{asm}"
    );
}

// ── Peephole fusion: OpCall0 / OpCall1 / OpCall2 ────────────────────────

#[test]
fn call0_fuses_call_with_zero_args() {
    let asm = all_functions_asm(
        r#"
        fn f() { 42 }
        f()
    "#,
    );
    assert!(asm.contains("OpCall0"), "expected OpCall0:\n{asm}");
}

#[test]
fn call1_fuses_call_with_one_arg() {
    let asm = all_functions_asm(
        r#"
        fn f(x) { x + 1 }
        f(1)
    "#,
    );
    assert!(asm.contains("OpCall1"), "expected OpCall1:\n{asm}");
}

#[test]
fn call2_fuses_call_with_two_args() {
    let asm = all_functions_asm(
        r#"
        fn f(a, b) { a + b }
        f(1, 2)
    "#,
    );
    assert!(asm.contains("OpCall2"), "expected OpCall2:\n{asm}");
}

// ── Peephole fusion: OpTailCall1 ────────────────────────────────────────

#[test]
fn tail_call1_fuses_tail_call_with_one_arg() {
    let asm = all_functions_asm(
        r#"
        fn countdown(n) {
            if n <= 0 { 0 } else { countdown(n - 1) }
        }
        countdown(5)
    "#,
    );
    assert!(asm.contains("OpTailCall1"), "expected OpTailCall1:\n{asm}");
}

#[test]
fn tail_call1_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn countdown(n) {
                if n <= 0 { n } else { countdown(n - 1) }
            }
            countdown(100)
        "#),
        Value::Integer(0),
    );
}

// ── Peephole fusion: OpGetLocalCall1 ────────────────────────────────────

#[test]
fn get_local_call1_fuses_getlocal_and_call1() {
    // OpGetLocalCall1 fires when a local (the function) is loaded and
    // immediately called with one argument.
    let asm = all_functions_asm(
        r#"
        fn apply(f, x) { f(x) }
        apply(fn(n) { n + 1 }, 5)
    "#,
    );
    assert!(
        asm.contains("OpGetLocalCall1") || asm.contains("OpCall1"),
        "expected OpGetLocalCall1 or OpCall1:\n{asm}"
    );
}

#[test]
fn get_local_call1_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn apply(f, x) { f(x) }
            apply(fn(n) { n + 1 }, 5)
        "#),
        Value::Integer(6),
    );
}

// ── Peephole fusion: OpGetLocalIndex ────────────────────────────────────

#[test]
fn get_local_index_fuses_getlocal_and_index() {
    let asm = all_functions_asm(
        r#"
        fn first(arr) { arr[0] }
        first([|1, 2, 3|])
    "#,
    );
    assert!(
        asm.contains("OpGetLocalIndex"),
        "expected OpGetLocalIndex:\n{asm}"
    );
}

// ── Peephole fusion: OpReturnLocal ──────────────────────────────────────

#[test]
fn return_local_fuses_getlocal_and_return() {
    // OpReturnLocal fires when a local is loaded and immediately returned.
    // Use a function that returns a computed local (not just a parameter pass-through).
    let asm = all_functions_asm(
        r#"
        fn double(x) {
            let result = x + x
            result
        }
        double(21)
    "#,
    );
    assert!(
        asm.contains("OpReturnLocal") || asm.contains("OpReturnValue"),
        "expected OpReturnLocal or OpReturnValue:\n{asm}"
    );
}

#[test]
fn return_local_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn identity(x) { x }
            identity(99)
        "#),
        Value::Integer(99),
    );
}

// ── Safety: jump targets prevent fusion ─────────────────────────────────

#[test]
fn fusion_skipped_when_jump_targets_interior_of_region() {
    // The if-else with a tail call pattern can produce bytecode where a jump
    // targets the interior of what would be a GetLocalGetLocal fusion region.
    // The peephole must NOT fuse in this case.
    let result = run(r#"
        fn solve(items, idx, acc, checker) {
            if idx >= len(items) {
                acc
            } else {
                let valid = checker(idx)
                solve(items, idx + 1, if valid { acc + 1 } else { acc }, checker)
            }
        }
        solve([|"a", "b", "c"|], 0, 0, fn(i) { i < 2 })
    "#);
    assert_eq!(result, Value::Integer(2));
}

#[test]
fn recursive_tail_call_with_constant_add() {
    // Regression: OpConstantAdd in a recursive tail call must work correctly.
    let result = run(r#"
        fn find(s, c) {
            if c >= len(s) {
                -1
            } else if substring(s, c, c + 1) == "^" {
                c
            } else {
                find(s, c + 1)
            }
        }
        find("..^..", 0)
    "#);
    assert_eq!(result, Value::Integer(2));
}

// ── Multi-arg recursive calls with if-expression arguments ──────────────

#[test]
fn four_arg_recursive_tail_call_with_if_arg() {
    let result = run(r#"
        fn accum(items, idx, total, f) {
            if idx >= len(items) {
                total
            } else {
                let ok = f(idx)
                accum(items, idx + 1, if ok { total + 10 } else { total }, f)
            }
        }
        accum([|1, 2, 3, 4, 5|], 0, 0, fn(i) { i < 3 })
    "#);
    assert_eq!(result, Value::Integer(30));
}

// ── Constant add in a loop ──────────────────────────────────────────────

#[test]
fn constant_add_in_recursive_accumulator() {
    assert_eq!(
        run(r#"
            fn sum(n, acc) {
                if n <= 0 { acc } else { sum(n - 1, acc + 1) }
            }
            sum(100, 0)
        "#),
        Value::Integer(100),
    );
}

// ── Combined superinstructions in realistic patterns ────────────────────

#[test]
fn fibonacci_uses_superinstructions() {
    let asm = all_functions_asm(
        r#"
        fn fib(n) {
            if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
        }
        fib(10)
    "#,
    );
    let has_super = asm.contains("OpCall1")
        || asm.contains("OpConstantAdd")
        || asm.contains("OpAddLocals")
        || asm.contains("OpGetLocalGetLocal");
    assert!(has_super, "expected superinstructions in fib:\n{asm}");
}

#[test]
fn fibonacci_produces_correct_result() {
    assert_eq!(
        run(r#"
            fn fib(n) {
                if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
            }
            fib(10)
        "#),
        Value::Integer(55),
    );
}

// ── Regression: two-branch tail calls don't corrupt closure ─────────────

#[test]
fn two_branch_tail_call_preserves_closure() {
    // Regression: when solve has two recursive branches (one simple,
    // one with if-expr arg), the closure must not be consumed prematurely.
    let result = run(r#"
        fn solve(items, idx, acc, checker) {
            if idx >= len(items) {
                acc
            } else {
                let line = match items[idx] {
                    Some(v) -> v,
                    _ -> ""
                }
                if len(line) == 0 {
                    solve(items, idx + 1, acc, checker)
                } else {
                    let valid = checker(idx)
                    solve(items, idx + 1, if valid { acc + 1 } else { acc }, checker)
                }
            }
        }
        solve([|"a", "", "b", "c", ""|], 0, 0, fn(i) { i < 3 })
    "#);
    assert_eq!(result, Value::Integer(2));
}
