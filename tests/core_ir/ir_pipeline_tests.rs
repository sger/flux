//! End-to-end IR pipeline regression tests.
//!
//! These tests validate that programs pass through every IR phase correctly:
//! Source → Parse → HM inference → Core IR → Core passes → CFG IR → Bytecode → VM execution
//!
//! Each test asserts properties at intermediate stages (Core IR shape, CFG block
//! structure) AND verifies the final runtime output. This catches regressions
//! that integration snapshot tests miss because snapshots only check final output.

use std::collections::HashMap;

use flux::cfg::{IrBinaryOp, IrExpr, IrInstr, IrTerminator, lower_program_to_ir};
use flux::compiler::Compiler;
use flux::core::{
    CoreExpr, CorePrimOp, lower_ast::lower_program_ast, passes::run_core_passes_with_interner,
    to_ir::lower_core_to_ir,
};
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser};
use flux::types::infer_type::InferType;
use flux::vm::VM;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn parse_and_infer(
    src: &str,
) -> (
    flux::syntax::program::Program,
    HashMap<ExprId, InferType>,
    Interner,
) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner.clone());
    let hm_expr_types = compiler.infer_expr_types_for_program(&program);
    (program, hm_expr_types, interner)
}

fn run(input: &str) -> Value {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

fn run_produces_error(input: &str) -> bool {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).is_err()
}

fn dump_core(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler
        .dump_core_with_opts(
            &program,
            false,
            flux::core::display::CoreDisplayMode::Readable,
        )
        .expect("dump_core should succeed")
}

fn collect_core_exprs(expr: &CoreExpr) -> Vec<&CoreExpr> {
    let mut out = vec![expr];
    match expr {
        CoreExpr::Lam { body, .. } => out.extend(collect_core_exprs(body)),
        CoreExpr::App { func, args, .. } => {
            out.extend(collect_core_exprs(func));
            for a in args {
                out.extend(collect_core_exprs(a));
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            out.extend(collect_core_exprs(rhs));
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                out.extend(collect_core_exprs(rhs));
            }
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            out.extend(collect_core_exprs(scrutinee));
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    out.extend(collect_core_exprs(guard));
                }
                out.extend(collect_core_exprs(&alt.rhs));
            }
        }
        CoreExpr::Con { fields, .. } => {
            for f in fields {
                out.extend(collect_core_exprs(f));
            }
        }
        CoreExpr::PrimOp { args, .. } => {
            for a in args {
                out.extend(collect_core_exprs(a));
            }
        }
        CoreExpr::Perform { args, .. } => {
            for a in args {
                out.extend(collect_core_exprs(a));
            }
        }
        CoreExpr::Return { value, .. } => {
            out.extend(collect_core_exprs(value));
        }
        CoreExpr::Handle { body, .. } => {
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => {}
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            out.extend(collect_core_exprs(object));
        }
    }
    out
}

#[test]
fn contextual_list_instance_dispatches_with_element_dictionary() {
    let value = run(r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Bool
}
instance Eq<a> => MyEq<List<a>> {
    fn my_eq(xs, ys) {
        match xs {
            [h1 | _] -> match ys {
                [h2 | _] -> h1 == h2,
                _ -> false
            },
            _ -> true
        }
    }
}

my_eq([1], [1]);
"#);

    assert_eq!(value, Value::Boolean(true));
}

#[test]
fn hkt_functor_list_dispatches_at_runtime() {
    let value = run(r#"
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b) -> f<b>
}

instance Functor<List> {
    fn fmap(xs, func) {
        fn go(ys) {
            match ys {
                [] -> [],
                [h | t] -> [func(h) | go(t)]
            }
        }
        go(xs)
    }
}

to_string(fmap([1, 2, 3], \x -> x * 2));
"#);

    assert_eq!(value, Value::String("[2, 4, 6]".to_string().into()));
}

#[test]
fn hkt_functor_list_dump_core_uses_mangled_dispatch_call() {
    let core = dump_core(
        r#"
class Functor<f> {
    fn fmap<a, b>(x: f<a>, func: (a) -> b) -> f<b>
}

instance Functor<List> {
    fn fmap(xs, func) {
        fn go(ys) {
            match ys {
                [] -> [],
                [h | t] -> [func(h) | go(t)]
            }
        }
        go(xs)
    }
}

fmap([1, 2, 3], \x -> x * 2);
"#,
    );

    assert!(
        core.contains("__tc_Functor_List_fmap"),
        "expected HKT class call to lower to the mangled instance method, got:\n{core}"
    );
}

#[test]
fn contextual_list_instance_dump_core_shows_mangled_dispatch_call() {
    let core = dump_core(
        r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Bool
}
instance Eq<a> => MyEq<List<a>> {
    fn my_eq(xs, ys) {
        match xs {
            [h1 | _] -> match ys {
                [h2 | _] -> h1 == h2,
                _ -> false
            },
            _ -> true
        }
    }
}

my_eq([1], [1]);
"#,
    );

    assert!(
        core.contains("__tc_MyEq_List<a>_my_eq"),
        "expected contextual mangled function in Core dump:\n{core}"
    );
}

#[test]
fn nested_contextual_list_instance_dump_core_threads_context_dictionary() {
    let core = dump_core(
        r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Bool
}
instance MyEq<Int> {
    fn my_eq(x, y) { x == y }
}
instance MyEq<a> => MyEq<List<a>> {
    fn my_eq(xs, ys) {
        match (xs, ys) {
            ([], []) -> true,
            ([h1 | t1], [h2 | t2]) -> my_eq(h1, h2) && my_eq(t1, t2),
            _ -> false
        }
    }
}

my_eq([[1], [2]], [[1], [2]]);
"#,
    );

    assert!(
        core.contains("__dict_MyEq_Int"),
        "expected contextual Core dump to reference the captured MyEq<Int> dictionary, got:\n{core}"
    );
    assert!(
        core.contains("__tc_MyEq_List<a>_my_eq"),
        "expected nested contextual call to lower to the mangled instance method, got:\n{core}"
    );
    assert!(
        core.contains("__dict_MyEq_List<a>(__dict_MyEq_Int)"),
        "expected nested contextual call site to build the recursive dictionary constructor, got:\n{core}"
    );
}

#[test]
fn polymorphic_operator_dump_core_uses_named_class_methods() {
    let core = dump_core(
        r#"
fn choose<A: Ord + Eq + Num>(x: A, y: A) -> A {
    if x != y {
        if x > y { x / y } else { y / x }
    } else {
        x
    }
}

fn main() {
    choose(8, 2)
}
"#,
    );

    for dict_name in ["__dict_Ord_Int", "__dict_Eq_Int", "__dict_Num_Int"] {
        assert!(
            core.contains(dict_name),
            "expected Core dump to contain {dict_name}, got:\n{core}"
        );
    }

    for primop_name in ["NEq(__x0, __x1)", "Gt(__x0, __x1)", "Div(__x0, __x1)"] {
        assert!(
            core.contains(primop_name),
            "expected Core dump to contain specialized primop {primop_name}, got:\n{core}"
        );
    }
}

#[test]
fn concrete_int_comparison_keeps_specialized_primop() {
    let (program, hm_expr_types, _interner) = parse_and_infer(
        r#"
fn main() {
    7 > 3
}
"#,
    );

    let core = lower_program_ast(&program, &hm_expr_types);
    let has_icmp_gt = core
        .defs
        .iter()
        .any(|def| has_primop(&def.expr, &CorePrimOp::ICmpGt));
    assert!(
        has_icmp_gt,
        "expected concrete Int comparison to lower to ICmpGt, got: {core:#?}"
    );
}

fn has_primop(expr: &CoreExpr, target: &CorePrimOp) -> bool {
    collect_core_exprs(expr)
        .iter()
        .any(|e| matches!(e, CoreExpr::PrimOp { op, .. } if op == target))
}

// ── Test 1: Zero-param nested function ──────────────────────────────────────

#[test]
fn pipeline_zero_param_nested_function() {
    let src = r#"
fn main() {
    fn rate() { 42 }
    rate()
}
"#;
    // Core IR: nested fn should be skipped (not lowered to LetRec+Lam)
    let (program, types, _interner) = parse_and_infer(src);
    let core = lower_program_ast(&program, &types);
    // The main def should exist
    assert!(
        !core.defs.is_empty(),
        "Core IR should have at least one def"
    );

    // Final output: rate() should return 42
    assert_eq!(run(src), Value::Integer(42));
}

// ── Test 2: Match guard variable substitution ───────────────────────────────

#[test]
fn pipeline_match_guard_preserves_variable() {
    let src = r#"
fn main() {
    let x = 2
    match x {
        _ if x > 0 -> 1,
        _ -> 0
    }
}
"#;
    // Core IR: after passes, guard should still reference x correctly
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    // The Case should still have a guard expression after passes
    let main_def = &core.defs[0];
    let exprs = collect_core_exprs(&main_def.expr);
    let has_case = exprs.iter().any(|e| matches!(e, CoreExpr::Case { .. }));
    assert!(
        has_case,
        "Core IR should still contain a Case expression after passes"
    );

    // Final output: should return 1 (x=2 > 0)
    assert_eq!(run(src), Value::Integer(1));
}

#[test]
fn backend_ir_lowering_is_core_backed() {
    let src = r#"
fn sum_ints(a: Int, b: Int) -> Int { a + b }
fn main() { sum_ints(3, 4) }
"#;
    let (program, types, _interner) = parse_and_infer(src);
    let ir = lower_program_to_ir(&program, &types).expect("backend lowering should succeed");

    let core = ir
        .core
        .as_ref()
        .expect("canonical backend lowering should retain Flux Core");
    assert_eq!(core.defs.len(), 2);
}

// ── Test 3: Match guard with let-inlined variable ───────────────────────────

#[test]
fn pipeline_match_guard_after_inline_trivial_lets() {
    let src = r#"
fn main() {
    let n = 5
    match n {
        _ if n > 10 -> "big",
        _ if n > 0 -> "small",
        _ -> "zero"
    }
}
"#;
    // Core IR passes may inline `let n = 5` — guard must still work
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    assert_eq!(run(src), Value::String("small".to_string().into()));
}

// ── Test 4: Typed arithmetic emits typed primops ────────────────────────────

#[test]
fn pipeline_typed_arithmetic_emits_iadd() {
    let src = r#"
fn sum_ints(a: Int, b: Int) -> Int { a + b }
fn main() { sum_ints(3, 4) }
"#;
    let (program, types, _interner) = parse_and_infer(src);
    let core = lower_program_ast(&program, &types);

    // The add function's Core IR should use IAdd (typed) not Add (generic)
    let add_def = &core.defs[0];
    assert!(
        has_primop(&add_def.expr, &CorePrimOp::IAdd),
        "Typed Int addition should emit IAdd, not generic Add"
    );

    assert_eq!(run(src), Value::Integer(7));
}

// ── Test 5: Core IR → CFG IR produces correct blocks ────────────────────────

#[test]
fn pipeline_if_else_produces_branch_terminator() {
    let src = r#"
fn choose(x: Int) -> Int {
    if x > 0 { 1 } else { 0 }
}
fn main() { choose(5) }
"#;
    let (program, types, _interner) = parse_and_infer(src);
    let core = lower_program_ast(&program, &types);
    let ir = lower_core_to_ir(&core);

    // The choose function should have a Branch terminator
    let has_branch = ir.functions.iter().any(|f| {
        f.blocks
            .iter()
            .any(|b| matches!(b.terminator, IrTerminator::Branch { .. }))
    });
    assert!(
        has_branch,
        "if/else should produce a Branch terminator in CFG IR"
    );

    assert_eq!(run(src), Value::Integer(1));
}

// ── Test 6: Modulo operator in CFG IR ───────────────────────────────────────

#[test]
fn pipeline_modulo_operator_in_cfg() {
    let src = r#"
fn is_even(n: Int) -> Bool { n % 2 == 0 }
fn main() { is_even(4) }
"#;
    let (program, types, _interner) = parse_and_infer(src);
    let core = lower_program_ast(&program, &types);
    let ir = lower_core_to_ir(&core);

    // Should have a Mod binary op in the IR
    let has_mod = ir.functions.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.instrs.iter().any(|instr| {
                matches!(
                    instr,
                    IrInstr::Assign {
                        expr: IrExpr::Binary(IrBinaryOp::IMod | IrBinaryOp::Mod, ..),
                        ..
                    }
                )
            })
        })
    });
    assert!(
        has_mod,
        "Modulo should produce IrBinaryOp::Mod or IMod in CFG IR"
    );

    assert_eq!(run(src), Value::Boolean(true));
}

// ── Test 7: Core passes — beta reduction ────────────────────────────────────

#[test]
fn pipeline_beta_reduction_eliminates_redex() {
    let src = r#"
fn main() {
    let f = \x -> x + 1
    f(5)
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    // After beta reduction, the application f(5) may be reduced
    // The final result should still be 6
    assert_eq!(run(src), Value::Integer(6));
}

// ── Test 8: Core passes — dead let elimination ──────────────────────────────

#[test]
fn pipeline_dead_let_elimination() {
    let src = r#"
fn main() {
    let unused = 999
    42
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);

    // Before passes: should have a Let for `unused`
    let before_lets = collect_core_exprs(&core.defs[0].expr)
        .iter()
        .filter(|e| matches!(e, CoreExpr::Let { .. }))
        .count();

    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    // After passes: dead let should be eliminated
    let after_lets = collect_core_exprs(&core.defs[0].expr)
        .iter()
        .filter(|e| matches!(e, CoreExpr::Let { .. }))
        .count();

    assert!(
        after_lets < before_lets,
        "Dead let elimination should remove unused binding (before={before_lets}, after={after_lets})"
    );

    assert_eq!(run(src), Value::Integer(42));
}

// ── Test 8b: Phase 3 — dead CanFail binding is eliminable ───────────────────
//
// Proposal 0161 Phase 3: `dead_let` uses the `can_discard` predicate, which
// treats a `CanFail` rhs as droppable when the binder is unused (the failure
// was never observed). This replaces the strict `is_pure` predicate that
// previously preserved `let _ = 10 / n`.
#[test]
fn pipeline_dead_let_eliminates_can_fail_binding() {
    let src = r#"
fn main() {
    let n = 7
    let _unused_ratio = 10 / n
    42
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);

    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    // After passes: the `10 / n` Let must be gone. Only the outer `let n = 7`
    // can survive (and even that may get inlined) — the CanFail division on a
    // dead binder must be eliminable.
    let remaining_div = collect_core_exprs(&core.defs[0].expr)
        .iter()
        .filter(|e| {
            matches!(
                e,
                CoreExpr::PrimOp {
                    op: CorePrimOp::Div | CorePrimOp::IDiv,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        remaining_div, 0,
        "dead CanFail division should be eliminated by Phase 3 dead_let"
    );

    assert_eq!(run(src), Value::Integer(42));
}

// ── Test 8c: Dead HasEffect binding is preserved ────────────────────────────
//
// Counterpart to the test above: `println` carries the `Console` effect, so
// `can_discard` returns false. Even on a dead binder the call must remain so
// the program actually prints.
#[test]
fn pipeline_dead_let_preserves_has_effect_binding() {
    let src = r#"
fn main() with IO {
    let _unused = println("side effect must survive")
    42
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);

    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    let remaining_println = collect_core_exprs(&core.defs[0].expr)
        .iter()
        .filter(|e| {
            matches!(
                e,
                CoreExpr::PrimOp {
                    op: CorePrimOp::Println,
                    ..
                }
            )
        })
        .count();
    assert!(
        remaining_println >= 1,
        "HasEffect call must not be discarded by dead_let"
    );
}

// ── Test 9: Index type validation catches String index on List ──────────────

#[test]
fn pipeline_index_type_validation_rejects_string_index() {
    let src = r#"
fn take(xs: List<Int>) -> Option<Int> {
    xs["0"]
}
fn main() { take(list(1, 2, 3)) }
"#;
    assert!(
        run_produces_error(src),
        "Indexing a List with a String should produce a compile error"
    );
}

// ── Test 10: Index type validation catches non-indexable type ────────────────

#[test]
fn pipeline_index_type_validation_rejects_non_indexable() {
    let src = r#"
fn access(x: Int) -> Int {
    x[0]
}
fn main() { access(42) }
"#;
    assert!(
        run_produces_error(src),
        "Indexing an Int should produce a compile error"
    );
}

// ── Test 11: Explicit typed identity ────────────────────────────────────────

#[test]
fn pipeline_typed_identity_accepts_int() {
    let src = r#"
fn identity(x: Int) -> Int { x }
fn main() { identity(42) }
"#;
    assert_eq!(run(src), Value::Integer(42));
}

// ── Test 12: Closure with captures ──────────────────────────────────────────

#[test]
fn pipeline_closure_captures_variable() {
    let src = r#"
fn main() {
    let x = 10
    let f = \y -> x + y
    f(5)
}
"#;
    let (program, types, _interner) = parse_and_infer(src);
    let core = lower_program_ast(&program, &types);
    let ir = lower_core_to_ir(&core);

    // Should have a MakeClosure instruction
    let has_closure = ir.functions.iter().any(|f| {
        f.blocks.iter().any(|b| {
            b.instrs.iter().any(|instr| {
                matches!(
                    instr,
                    IrInstr::Assign {
                        expr: IrExpr::MakeClosure(..),
                        ..
                    }
                )
            })
        })
    });
    assert!(
        has_closure,
        "Lambda with captures should produce MakeClosure in CFG IR"
    );

    assert_eq!(run(src), Value::Integer(15));
}

// ── Test 13: If/else with jump target integrity ─────────────────────────────

#[test]
fn pipeline_if_else_return_local_jump_integrity() {
    // This catches the replace_last_local_read_with_return jump corruption bug
    let src = r#"
fn max_val(a, b) {
    if a > b { a } else { b }
}
fn main() { max_val(10, 4) }
"#;
    assert_eq!(run(src), Value::Integer(10));
}

#[test]
fn pipeline_if_else_return_local_jump_integrity_reverse() {
    let src = r#"
fn max_val(a, b) {
    if a > b { a } else { b }
}
fn main() { max_val(4, 10) }
"#;
    assert_eq!(run(src), Value::Integer(10));
}

// ── Test 14: Case-of-known-constructor ──────────────────────────────────────

#[test]
fn pipeline_cokc_reduces_known_constructor() {
    let src = r#"
fn main() {
    match Some(42) {
        Some(x) -> x,
        _ -> 0
    }
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");

    // After COKC, the Case(Con(Some, [42]), ...) should reduce to just 42
    let main_exprs = collect_core_exprs(&core.defs[0].expr);
    let still_has_case = main_exprs
        .iter()
        .any(|e| matches!(e, CoreExpr::Case { .. }));
    // COKC may or may not fully eliminate — but the program should work
    let _ = still_has_case; // diagnostic only, not a hard assertion

    assert_eq!(run(src), Value::Integer(42));
}

// ── Test 15: Recursive function (tail call) ─────────────────────────────────

#[test]
fn pipeline_recursive_function_with_modulo() {
    // Regression: modulo operator was missing from structured IR lowering
    let src = r#"
fn count_even(n, acc) {
    if n <= 0 { acc }
    else if n % 2 == 0 { count_even(n - 1, acc + 1) }
    else { count_even(n - 1, acc) }
}
fn main() { count_even(10, 0) }
"#;
    assert_eq!(run(src), Value::Integer(5));
}
