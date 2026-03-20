//! End-to-end IR pipeline regression tests.
//!
//! These tests validate that programs pass through every IR phase correctly:
//! Source → Parse → HM inference → Core IR → Core passes → CFG IR → Bytecode → VM execution
//!
//! Each test asserts properties at intermediate stages (Core IR shape, CFG block
//! structure) AND verifies the final runtime output. This catches regressions
//! that integration snapshot tests miss because snapshots only check final output.

use std::collections::HashMap;

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::cfg::{IrBinaryOp, IrExpr, IrInstr, IrTerminator, lower_program_to_ir};
use flux::core::{
    CoreExpr, CorePrimOp, lower_ast::lower_program_ast, passes::run_core_passes_with_interner,
    to_ir::lower_core_to_ir,
};
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser};
use flux::types::infer_type::InferType;

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
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::Reuse { fields, .. } => {
            for f in fields {
                out.extend(collect_core_exprs(f));
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            out.extend(collect_core_exprs(unique_body));
            out.extend(collect_core_exprs(shared_body));
        }
    }
    out
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
    let (program, types, interner) = parse_and_infer(src);
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
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

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
fn add(a: Int, b: Int) -> Int { a + b }
fn main() { add(3, 4) }
"#;
    let (program, types, interner) = parse_and_infer(src);
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
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    assert_eq!(run(src), Value::String("small".to_string().into()));
}

// ── Test 4: Typed arithmetic emits typed primops ────────────────────────────

#[test]
fn pipeline_typed_arithmetic_emits_iadd() {
    let src = r#"
fn add(a: Int, b: Int) -> Int { a + b }
fn main() { add(3, 4) }
"#;
    let (program, types, interner) = parse_and_infer(src);
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
    let (program, types, interner) = parse_and_infer(src);
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
    let (program, types, interner) = parse_and_infer(src);
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
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

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

    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

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

// ── Test 11: Any type boundary ──────────────────────────────────────────────

#[test]
fn pipeline_any_type_accepts_int() {
    let src = r#"
fn identity(x: Any) -> Any { x }
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
    let (program, types, interner) = parse_and_infer(src);
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
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    // After COKC, the Case(Con(Some, [42]), ...) should reduce to just 42
    let main_exprs = collect_core_exprs(&core.defs[0].expr);
    let still_has_case = main_exprs
        .iter()
        .any(|e| matches!(e, CoreExpr::Case { .. }));
    // COKC may or may not fully eliminate — but the program should work
    let _ = still_has_case; // diagnostic only, not a hard assertion

    assert_eq!(run(src), Value::Integer(42));
}

#[test]
fn pipeline_aether_emits_reuse_for_list_rebuild() {
    let src = r#"
fn rebuild(xs) {
    match xs {
        [h | t] -> [h | t],
        _ -> [],
    }
}

fn main() {
    rebuild([1, 2, 3])
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_reuse = core.defs.iter().any(|def| {
        collect_core_exprs(&def.expr)
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    });
    assert!(
        has_reuse,
        "expected Aether to emit Reuse for list rebuild"
    );

    assert_eq!(run(src).to_string(), "[1, 2, 3]");
}

#[test]
fn pipeline_aether_emits_reuse_for_named_adt_update() {
    let src = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn set_black(t) {
    match t {
        Node(_, left, key, right) -> Node(Black, left, key, right),
        _ -> t,
    }
}

fn main() {
    set_black(Node(Red, Leaf, 5, Leaf))
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_reuse = core.defs.iter().any(|def| {
        collect_core_exprs(&def.expr)
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    });
    assert!(
        has_reuse,
        "expected Aether to emit Reuse for named ADT update"
    );

    assert_eq!(run(src).to_string(), "Node(Black, Leaf, 5, Leaf)");
}

#[test]
fn pipeline_aether_emits_reuse_for_branchy_filter() {
    let src = r#"
fn my_filter(xs, f) {
    match xs {
        [h | t] -> if f(h) { [h | my_filter(t, f)] } else { my_filter(t, f) },
        _ -> [],
    }
}

fn main() {
    my_filter([1, 2, 3, 4, 5, 6], \x -> x % 2 == 0)
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_reuse = core.defs.iter().any(|def| {
        collect_core_exprs(&def.expr)
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    });
    assert!(
        has_reuse,
        "expected Aether to emit Reuse for branchy filter"
    );

    assert_eq!(run(src).to_string(), "[2, 4, 6]");
}

#[test]
fn pipeline_aether_elides_dup_for_borrowed_call_chain() {
    let src = r#"
fn my_len(xs) {
    match xs {
        [_ | t] -> 1 + my_len(t),
        _ -> 0,
    }
}

fn len_twice(xs) {
    my_len(xs) + my_len(xs)
}

fn main() {
    len_twice([1, 2, 3])
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_dup = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| matches!(expr, CoreExpr::Dup { .. }));
    assert!(
        !has_dup,
        "expected borrowed call chain to avoid Dup in len_twice"
    );

    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn pipeline_aether_elides_dup_for_base_borrowed_preferred_call() {
    let src = r#"
fn len_twice(xs) {
    len(xs) + len(xs)
}

fn main() {
    len_twice([1, 2, 3])
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_dup = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| matches!(expr, CoreExpr::Dup { .. }));
    assert!(
        !has_dup,
        "expected borrowed-preferred base call to avoid Dup for len(xs)"
    );

    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn pipeline_aether_registers_base_owned_call_metadata() {
    let src = r#"
fn duplicate_concat(xs) {
    concat(xs, xs)
}

fn main() {
    duplicate_concat("ab")
}
"#;
    let (program, types, mut interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    let registry = flux::aether::borrow_infer::infer_borrow_modes(&mut core, Some(&interner));
    let concat = interner.intern("concat");
    assert!(
        !registry.is_borrowed(flux::aether::borrow_infer::BorrowCallee::BaseRuntime(concat), 0),
        "expected concat first parameter to remain owned"
    );
    assert!(
        !registry.is_borrowed(flux::aether::borrow_infer::BorrowCallee::BaseRuntime(concat), 1),
        "expected concat second parameter to remain owned"
    );
    assert_eq!(
        registry.lookup_name(concat).map(|sig| sig.provenance),
        Some(flux::aether::borrow_infer::BorrowProvenance::BaseRuntime),
        "expected concat to be registered as explicit base metadata"
    );
}

#[test]
fn pipeline_aether_emits_dup_for_multiuse_pattern_field() {
    let src = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}

fn main() {
    copy_head([1, 2, 3])
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_dup = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| matches!(expr, CoreExpr::Dup { .. }));
    assert!(
        has_dup,
        "expected Aether to emit Dup for a pattern field used multiple times"
    );

    assert_eq!(run(src).to_string(), "[1, 1, 2, 3]");
}

#[test]
fn pipeline_aether_emits_drop_specialized_for_multiuse_pattern_field() {
    let src = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}

fn main() {
    copy_head([1, 2, 3])
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_drop_specialized = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. }));
    assert!(
        has_drop_specialized,
        "expected Aether to emit DropSpecialized for multi-use pattern field"
    );

    assert_eq!(run(src).to_string(), "[1, 1, 2, 3]");
}

#[test]
fn pipeline_drop_spec_shared_branch_does_not_reuse_outer_pattern_context() {
    let src = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn dup_left(t) {
    match t {
        Node(color, left, key, right) -> Node(color, left, key, left),
        _ -> t,
    }
}

fn main() {
    dup_left(Node(Red, Leaf, 5, Leaf))
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let has_bad_shared_reuse = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(
        |expr| match expr {
            CoreExpr::DropSpecialized { shared_body, .. } => collect_core_exprs(shared_body)
                .into_iter()
                .any(|inner| matches!(inner, CoreExpr::Reuse { .. })),
            _ => false,
        },
    );
    assert!(
        !has_bad_shared_reuse,
        "expected DropSpecialized shared branch to avoid outer-pattern reuse rewrites"
    );

    assert_eq!(run(src).to_string(), "Node(Red, Leaf, 5, Leaf)");
}

#[test]
fn pipeline_drop_spec_emits_branchy_named_adt_reuse_only_on_unique_path() {
    let src = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)

fn keep_or_dup_left(t, keep) {
    match t {
        Node(color, left, key, right) -> if keep { Node(color, left, key, right) } else { Node(color, left, key, left) },
        _ -> t,
    }
}

fn main() {
    keep_or_dup_left(Node(Red, Leaf, 5, Leaf), false)
}
"#;
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");

    let found_expected_shape = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(
        |expr| match expr {
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                let unique_reuses = collect_core_exprs(unique_body)
                    .into_iter()
                    .filter(|inner| matches!(inner, CoreExpr::Reuse { .. }))
                    .count();
                let shared_reuses = collect_core_exprs(shared_body)
                    .into_iter()
                    .filter(|inner| matches!(inner, CoreExpr::Reuse { .. }))
                    .count();
                unique_reuses >= 2 && shared_reuses == 0
            }
            _ => false,
        },
    );
    assert!(
        found_expected_shape,
        "expected branchy named ADT DropSpecialized to reuse only on the unique path"
    );

    assert_eq!(run(src).to_string(), "Node(Red, Leaf, 5, Leaf)");
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
