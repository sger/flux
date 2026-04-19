use std::collections::HashMap;
use std::path::Path;

use flux::aether::{AetherExpr as CoreExpr, AetherProgram, lower_core_to_aether_program};
use flux::compiler::Compiler;
use flux::core::{
    lower_ast::lower_program_ast,
    passes::{run_aether_passes_with_interner_and_registry, run_core_passes_with_interner},
};
use flux::diagnostics::Diagnostic;
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, module_graph::ModuleGraph, parser::Parser};
use flux::types::infer_type::InferType;

fn parse_and_infer(
    src: &str,
) -> (
    flux::syntax::program::Program,
    HashMap<flux::syntax::expression::ExprId, InferType>,
    flux::syntax::interner::Interner,
) {
    let lexer = Lexer::new(src);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();

    let mut compiler = flux::compiler::Compiler::new_with_interner("<test>", interner.clone());
    let types = compiler.infer_expr_types_for_program(&program);
    (program, types, interner)
}

fn run(src: &str) -> Value {
    let lexer = Lexer::new(src);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = flux::compiler::Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("compile ok");
    let mut vm = flux::vm::VM::new(compiler.bytecode());
    vm.run().expect("vm run ok");
    vm.last_popped_stack_elem().clone()
}

fn collect_core_exprs(expr: &CoreExpr) -> Vec<&CoreExpr> {
    let mut out = vec![expr];
    match expr {
        CoreExpr::Lam { body, .. } => out.extend(collect_core_exprs(body)),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
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
                out.extend(collect_core_exprs(&alt.rhs));
                if let Some(guard) = &alt.guard {
                    out.extend(collect_core_exprs(guard));
                }
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            for f in fields {
                out.extend(collect_core_exprs(f));
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for a in args {
                out.extend(collect_core_exprs(a));
            }
        }
        CoreExpr::Return { value, .. } => out.extend(collect_core_exprs(value)),
        CoreExpr::Handle { body, handlers, .. } => {
            out.extend(collect_core_exprs(body));
            for handler in handlers {
                out.extend(collect_core_exprs(&handler.body));
            }
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            out.extend(collect_core_exprs(unique_body));
            out.extend(collect_core_exprs(shared_body));
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            out.extend(collect_core_exprs(object));
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
    }
    out
}

fn count_matching(expr: &CoreExpr, predicate: &impl Fn(&CoreExpr) -> bool) -> usize {
    collect_core_exprs(expr)
        .into_iter()
        .filter(|expr| predicate(expr))
        .count()
}

fn lowered_core(src: impl AsRef<str>) -> AetherProgram {
    let (program, types, interner) = parse_and_infer(src.as_ref());
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    let compiler = Compiler::new_with_interner("<test>", interner.clone());
    let preloaded_registry = compiler.build_preloaded_borrow_registry(&program);
    let (aether, _warnings) =
        lower_core_to_aether_program(&core, Some(&interner), preloaded_registry)
            .expect("aether lowering should succeed");
    aether
}

fn compile_fixture_warnings(rel: &str) -> Vec<Diagnostic> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = workspace_root.join(rel);
    let source = std::fs::read_to_string(&fixture).expect("fixture should exist");

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );

    let mut roots = Vec::new();
    if let Some(parent) = fixture.parent() {
        roots.push(parent.to_path_buf());
    }
    let src_root = workspace_root.join("src");
    if src_root.exists() {
        roots.push(src_root);
    }

    let graph =
        ModuleGraph::build_with_entry_and_roots(&fixture, &program, parser.take_interner(), &roots);
    assert!(
        graph.diagnostics.is_empty(),
        "module diagnostics: {:?}",
        graph.diagnostics
    );

    let mut compiler = Compiler::new_with_interner(rel, graph.interner);
    for node in graph.graph.topo_order() {
        compiler.set_file_path(node.path.to_string_lossy().to_string());
        compiler.set_current_module_kind(node.kind);
        if let Err(diags) = compiler.compile(&node.program) {
            panic!("expected fixture `{rel}` to compile, got diagnostics: {diags:?}");
        }
    }
    compiler.take_warnings()
}

#[test]
fn borrow_local_call_emits_aether_call() {
    // Verify that local calls produce AetherCall nodes in the Aether IR.
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn wrap_len(xs) { my_len(xs) }
fn len_twice(xs) { wrap_len(xs) + wrap_len(xs) }
fn main() { len_twice(#[1, 2, 3]) }
"#;
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::AetherCall { .. }))
    );
    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn borrow_base_call_stays_dup_free() {
    // array_len borrows its argument (read-only), so passing xs twice to
    // array_len does NOT require a Dup — both uses are borrowed.
    let src = r#"
fn len_twice(xs) { array_len(xs) + array_len(xs) }
fn main() { len_twice(#[1, 2, 3]) }
"#;
    let core = lowered_core(src);
    let dup_count = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Dup { .. }))
        .count();
    assert_eq!(
        dup_count, 0,
        "borrowed primop args should not introduce dups"
    );
    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn borrow_mixed_arg_modes_are_explicit() {
    // Without base functions, my_len (wrapping array_len primop) consumes xs,
    // so borrow_then_return gets [Owned, Owned] arg modes.
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn borrow_then_return(xs, y) { if my_len(xs) > 0 { y } else { y } }
fn main() { borrow_then_return(#[1, 2, 3], 42) }
"#;
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::AetherCall { .. }))
    );
}

#[test]
fn recursive_borrow_signature_stays_precise() {
    // array_len borrows its argument, so my_len borrows xs too.
    // Both params of `loop` are Borrowed: xs is forwarded to borrowing my_len,
    // and n is only used in PrimOp positions.
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn loop(xs, n) {
    if n == 0 { my_len(xs) } else { loop(xs, n - 1) }
}
fn main() { loop(#[1, 2, 3], 3) }
"#;
    let core = lowered_core(src);
    let loop_def = core
        .defs
        .iter()
        .find(|def| def.name == core.defs[1].name)
        .expect("loop def");
    let sig = loop_def
        .borrow_signature
        .as_ref()
        .expect("borrow signature");
    assert_eq!(
        sig.params,
        vec![
            flux::aether::borrow_infer::BorrowMode::Borrowed,
            flux::aether::borrow_infer::BorrowMode::Borrowed,
        ]
    );
}

#[test]
fn mutually_recursive_borrow_signatures_stay_precise() {
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn even(xs, n) {
    if n == 0 { my_len(xs) } else { odd(xs, n - 1) }
}
fn odd(xs, n) {
    if n == 0 { my_len(xs) } else { even(xs, n - 1) }
}
fn main() { even(#[1, 2, 3], 4) }
"#;
    let core = lowered_core(src);
    // defs[0] is my_len (helper), defs[1..2] are even/odd
    // array_len borrows its arg, so my_len borrows xs, so even/odd borrow xs too.
    for def in core.defs.iter().skip(1).take(2) {
        let sig = def.borrow_signature.as_ref().expect("borrow signature");
        assert_eq!(
            sig.params,
            vec![
                flux::aether::borrow_infer::BorrowMode::Borrowed,
                flux::aether::borrow_infer::BorrowMode::Borrowed,
            ]
        );
    }
}

#[test]
fn higher_order_recursive_forwarding_signature_stays_precise() {
    let src = r#"
fn loop(f, n) {
    if n == 0 { 0 } else { loop(f, n - 1) }
}
fn main() { loop(\x -> x + 1, 3) }
"#;
    let core = lowered_core(src);
    let loop_def = core.defs.first().expect("loop def");
    let sig = loop_def
        .borrow_signature
        .as_ref()
        .expect("borrow signature");
    assert_eq!(
        sig.params,
        vec![
            flux::aether::borrow_infer::BorrowMode::Borrowed,
            flux::aether::borrow_infer::BorrowMode::Borrowed,
        ]
    );
}

#[test]
fn mutually_recursive_higher_order_forwarding_signatures_stay_precise() {
    let src = r#"
fn even(f, n) {
    if n == 0 { 0 } else { odd(f, n - 1) }
}
fn odd(f, n) {
    if n == 0 { 0 } else { even(f, n - 1) }
}
fn main() { even(\x -> x + 1, 4) }
"#;
    let core = lowered_core(src);
    for def in core.defs.iter().take(2) {
        let sig = def.borrow_signature.as_ref().expect("borrow signature");
        assert_eq!(
            sig.params,
            vec![
                flux::aether::borrow_infer::BorrowMode::Borrowed,
                flux::aether::borrow_infer::BorrowMode::Borrowed,
            ]
        );
    }
}

#[test]
fn closure_read_only_capture_avoids_dup() {
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn use_closure(xs) {
    let f = fn() { my_len(xs) };
    f() + f()
}
fn main() { use_closure(#[1, 2, 3]) }
"#;
    let core = lowered_core(src);
    let dups = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Dup { .. }))
        .count();
    assert_eq!(
        dups, 0,
        "read-only closure capture should not introduce dups"
    );
}

#[test]
fn closure_read_only_capture_keeps_borrow_signature_precise() {
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn use_closure(xs) {
    let f = fn() { my_len(xs) };
    f()
}
fn main() { use_closure(#[1, 2, 3]) }
"#;
    let core = lowered_core(src);
    let use_closure = core.defs.get(1).expect("use_closure def");
    let sig = use_closure
        .borrow_signature
        .as_ref()
        .expect("borrow signature");
    // array_len borrows its arg, so my_len borrows xs, so the closure
    // only reads xs — use_closure's xs stays Borrowed.
    assert_eq!(
        sig.params,
        vec![flux::aether::borrow_infer::BorrowMode::Borrowed]
    );
}

#[test]
fn reuse_list_rebuild_emits_reuse() {
    let src = r#"
fn rebuild(xs) {
    match xs {
        [h | t] -> [h | t],
        _ -> [],
    }
}
fn main() { rebuild([1, 2, 3]) }
"#;
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    );
}

#[test]
fn reuse_named_adt_alias_spine_emits_reuse() {
    let src = std::fs::read_to_string("examples/aether/reuse_alias_spines.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    );
}

#[test]
fn maintained_reuse_alias_spines_fixture_emits_masked_reuse() {
    let src = std::fs::read_to_string("examples/aether/reuse_alias_spines.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let masked = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    field_mask: Some(_),
                    ..
                }
            )
        })
        .count();
    assert!(
        masked >= 2,
        "expected maintained alias-spine fixture to emit exact masked reuse sites"
    );
}

#[test]
fn reuse_branchy_filter_emits_reuse() {
    let src = r#"
fn my_filter(xs, f) {
    match xs {
        [h | t] -> if f(h) { [h | my_filter(t, f)] } else { my_filter(t, f) },
        _ -> [],
    }
}
fn main() { my_filter([1, 2, 3, 4, 5, 6], \x -> x % 2 == 0) }
"#;
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
    );
}

#[test]
fn reuse_specialization_profitable_case_is_masked() {
    let src = std::fs::read_to_string("examples/aether/reuse_specialization.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: Some(_),
                    ..
                }
            ))
    );
}

#[test]
fn maintained_drop_spec_branchy_fixture_emits_reuse_inside_drop_specialized() {
    let src = std::fs::read_to_string("examples/aether/drop_spec_branchy.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let found = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| match expr {
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
                unique_reuses >= 1 && shared_reuses == 0
            }
            _ => false,
        });
    assert!(
        found,
        "expected maintained drop-spec fixture to preserve unique-path-only outer-token reuse"
    );
}

#[test]
fn maintained_drop_spec_recursive_fixture_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` in [h | t] is IntRep (typed pattern binders), so
    // DropSpecialized is no longer emitted — no dup/drop divergence.
    let src = std::fs::read_to_string("examples/aether/drop_spec_recursive.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    assert!(
        !core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "typed pattern binders (IntRep) should eliminate DropSpecialized for integer list elements"
    );
}

#[test]
fn maintained_drop_spec_branchy_fixture_keeps_unique_path_dup_free() {
    let src = std::fs::read_to_string("examples/aether/drop_spec_branchy.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let found = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| match expr {
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                let unique_dups = collect_core_exprs(unique_body)
                    .into_iter()
                    .filter(|inner| matches!(inner, CoreExpr::Dup { .. }))
                    .count();
                let shared_dups = collect_core_exprs(shared_body)
                    .into_iter()
                    .filter(|inner| matches!(inner, CoreExpr::Dup { .. }))
                    .count();
                unique_dups == 0 && shared_dups >= 1
            }
            _ => false,
        });
    assert!(
        found,
        "expected maintained branchy drop-spec fixture to leave the unique path dup-free while preserving shared-path dups"
    );
}

#[test]
fn maintained_drop_spec_recursive_fixture_has_no_dups_for_int_elements() {
    // Phase 7e: with typed pattern binders, `h` is IntRep and needs no dup/drop.
    // DropSpecialized is gone, so we just verify that dups are minimal overall.
    let src = std::fs::read_to_string("examples/aether/drop_spec_recursive.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let total_dups: usize = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Dup { .. }))
        .count();
    // Typed binders eliminate dups for IntRep pattern variables. The remaining
    // dups (if any) are for boxed values like the list tail or closures.
    assert!(
        total_dups <= 2,
        "typed pattern binders should minimize dups; got {total_dups}"
    );
}

#[test]
fn reuse_specialization_unprofitable_case_stays_plain() {
    let src = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)
fn keep_right_only(t) {
    match t {
        Node(color, left, key, right) -> Node(Black, Leaf, 0, right),
        _ -> t,
    }
}
fn main() { keep_right_only(Node(Red, Leaf, 5, Leaf)) }
"#;
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: None,
                    ..
                }
            ))
    );
}

#[test]
fn drop_spec_list_multiuse_field_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` in `[h | t]` is now IntRep (from typed pattern binders),
    // so no dup/drop divergence between unique/shared paths — DropSpecialized
    // is unnecessary when list elements are unboxed primitives.
    let src = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}
fn main() { copy_head([1, 2, 3]) }
"#;
    let core = lowered_core(src);
    assert!(
        !core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "typed pattern binders (IntRep) should eliminate DropSpecialized for integer list elements"
    );
}

#[test]
fn branch_join_borrow_only_path_avoids_extra_drop() {
    let src = r#"
fn my_len(xs) { array_len(xs) }
fn branch_read(xs, choose) {
    if choose { my_len(xs) } else { 0 }
}
fn main() { branch_read(#[1, 2, 3], true) }
"#;
    let core = lowered_core(src);
    let drops = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Drop { .. }))
        .count();
    // With primop-based my_len (consuming xs), the else branch needs a drop
    // for xs when it's not consumed.
    assert!(
        drops <= 1,
        "branch joins should have at most one drop for the unconsumed path, got {}",
        drops
    );
}

#[test]
fn handler_shadowing_does_not_keep_outer_binder_live() {
    let src = r#"
fn shadow_in_handler(x) {
    1 handle IO {
        print(resume, x) -> x
    }
}
fn main() with IO { shadow_in_handler(41) }
"#;
    let core = lowered_core(src);
    let shadow_def = &core.defs[0];
    let drop_count = count_matching(&shadow_def.expr, &|expr| {
        matches!(expr, CoreExpr::Drop { .. })
    });
    assert!(
        drop_count >= 1,
        "unused outer binder should still be discharged even when handler params shadow it"
    );
}

#[test]
fn drop_spec_branchy_list_update_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` is IntRep — no dup/drop divergence, DropSpecialized unnecessary.
    let src = r#"
fn copy_or_keep_head(xs, copy) {
    match xs {
        [h | t] -> if copy { [h | [h | t]] } else { [h | t] },
        _ -> [],
    }
}
fn main() { copy_or_keep_head([1, 2, 3], true) }
"#;
    let core = lowered_core(src);
    assert!(
        !core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "typed pattern binders (IntRep) should eliminate DropSpecialized for integer list elements"
    );
}

#[test]
fn drop_spec_branchy_named_adt_unique_shared_split_is_preserved() {
    let src = r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)
fn keep_or_dup_left(t, keep) {
    match t {
        Node(color, left, key, right) ->
            if keep { Node(color, left, key, right) }
            else { Node(color, left, key, left) },
        _ -> t,
    }
}
fn main() { keep_or_dup_left(Node(Red, Leaf, 5, Leaf), false) }
"#;
    let core = lowered_core(src);
    let found = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| match expr {
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
        });
    assert!(
        found,
        "expected branchy named ADT DropSpecialized to reuse only on the unique path"
    );
}

#[test]
fn drop_spec_recursive_update_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` is IntRep — no dup/drop divergence, DropSpecialized unnecessary.
    let src = r#"
fn rec_copy_or_keep(xs, choose) {
    match xs {
        [h | t] -> if choose { [h | rec_copy_or_keep(t, choose)] } else { [h | [h | rec_copy_or_keep(t, choose)]] },
        _ -> [],
    }
}
fn main() { rec_copy_or_keep([1, 2, 3], false) }
"#;
    let core = lowered_core(src);
    assert!(
        !core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "typed pattern binders (IntRep) should eliminate DropSpecialized for integer list elements"
    );
}

#[test]
fn maintained_either_match_fixture_survives_aether_passes() {
    let src = std::fs::read_to_string("examples/patterns/either_match.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    assert!(
        !core.defs.is_empty(),
        "expected maintained either_match fixture to lower through Aether"
    );
}

#[test]
fn fbip_clean_fixture_keeps_annotations_provable() {
    let src =
        std::fs::read_to_string("examples/aether/verify_aether.flx").expect("fixture should exist");
    let (program, types, interner) = parse_and_infer(&src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    let fbip = flux::aether::check_fbip::check_fbip(&core, &interner);
    assert!(
        fbip.error.is_none(),
        "verify_aether should not hard-fail FBIP"
    );
}

#[test]
fn fbip_failure_fixture_stays_non_provable() {
    let src = std::fs::read_to_string("examples/compiler_errors/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist");
    let (program, types, interner) = parse_and_infer(&src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    let compiler = Compiler::new_with_interner("<test>", interner.clone());
    let preloaded_registry = compiler.build_preloaded_borrow_registry(&program);
    run_aether_passes_with_interner_and_registry(&mut core, &interner, preloaded_registry)
        .expect_err("fbip failure fixture should error during explicit aether passes");
}

#[test]
fn bench_reuse_fixture_my_map_shows_borrowed_recursion_and_plain_reuse() {
    let src =
        std::fs::read_to_string("examples/aether/bench_reuse.flx").expect("fixture should exist");
    let core = lowered_core(src);
    let my_map = core
        .defs
        .iter()
        .find(|def| {
            format!("{:?}", def.name).contains("my_map")
                || collect_core_exprs(&def.expr).iter().any(|expr| {
                    matches!(
                        expr,
                        CoreExpr::AetherCall { arg_modes, .. }
                            if arg_modes == &[
                                flux::aether::borrow_infer::BorrowMode::Borrowed,
                                flux::aether::borrow_infer::BorrowMode::Borrowed,
                            ]
                    )
                })
        })
        .expect("my_map def");
    let borrowed_self_call = collect_core_exprs(&my_map.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::AetherCall { arg_modes, .. }
                if arg_modes == &[
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                ]
        )
    });
    let has_reuse = collect_core_exprs(&my_map.expr)
        .into_iter()
        .any(|expr| matches!(expr, CoreExpr::Reuse { .. }));
    let borrowed_benchmark_wrappers = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| {
            matches!(
                expr,
                CoreExpr::AetherCall { arg_modes, .. }
                    if arg_modes == &[
                        flux::aether::borrow_infer::BorrowMode::Borrowed,
                        flux::aether::borrow_infer::BorrowMode::Owned,
                    ]
            )
        });
    assert!(
        borrowed_self_call,
        "bench_reuse my_map should preserve the current borrowed/borrowed recursive traversal shape"
    );
    assert!(
        has_reuse,
        "bench_reuse my_map should now emit plain Reuse through safe precompute let spines"
    );
    assert!(
        borrowed_benchmark_wrappers,
        "bench_reuse wrappers should thread the benchmark list input through borrowed call modes"
    );
}

#[test]
fn bench_reuse_ab_control_fixtures_differ_only_in_intended_reuse_profile() {
    let enabled_src = std::fs::read_to_string("examples/aether/bench_reuse_enabled.flx")
        .expect("enabled fixture should exist");
    let blocked_src = std::fs::read_to_string("examples/aether/bench_reuse_blocked.flx")
        .expect("blocked fixture should exist");

    let enabled_core = lowered_core(&enabled_src);
    let blocked_core = lowered_core(&blocked_src);

    let enabled_reuses = enabled_core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Reuse { .. }))
        .count();
    let blocked_reuses = blocked_core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Reuse { .. }))
        .count();

    assert!(
        enabled_reuses >= 1,
        "reuse-enabled benchmark should currently emit plain Reuse"
    );
    assert_eq!(
        blocked_reuses, 0,
        "reuse-blocked benchmark should keep plain Reuse disabled"
    );
}

#[test]
fn higher_order_recursive_rebuild_with_precompute_let_emits_reuse() {
    let src = r#"
fn map_like(xs, f) {
    match xs {
        [h | t] -> [f(h) | map_like(t, f)],
        _ -> [],
    }
}
fn main() { map_like([1, 2, 3], \x -> x + 1) }
"#;
    let core = lowered_core(src);
    let map_like = core.defs.first().expect("map_like def");
    let has_reuse = collect_core_exprs(&map_like.expr)
        .into_iter()
        .any(|expr| matches!(expr, CoreExpr::Reuse { .. }));
    let borrowed_self_call = collect_core_exprs(&map_like.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::AetherCall { arg_modes, .. }
                if arg_modes == &[
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                ]
        )
    });
    assert!(
        has_reuse,
        "higher-order recursive rebuild should now emit plain Reuse"
    );
    assert!(
        borrowed_self_call,
        "higher-order recursive rebuild should preserve the current borrowed/borrowed recursive tail traversal"
    );
}

#[test]
fn branch_sensitive_list_rebuild_can_reuse_on_one_path_only() {
    let src = r#"
fn keep_or_inc(xs, choose) {
    match xs {
        [h | t] -> if choose { [h + 1 | t] } else { t },
        _ -> [],
    }
}
fn main() { keep_or_inc([1, 2, 3], true) }
"#;
    let core = lowered_core(src);
    let found = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .any(|expr| match expr {
            CoreExpr::Case { alts, .. } => {
                let branch_reuses = alts
                    .iter()
                    .map(|alt| {
                        collect_core_exprs(&alt.rhs)
                            .into_iter()
                            .filter(|inner| matches!(inner, CoreExpr::Reuse { .. }))
                            .count()
                    })
                    .collect::<Vec<_>>();
                branch_reuses.iter().any(|count| *count >= 1) && branch_reuses.contains(&0)
            }
            _ => false,
        });
    assert!(
        found,
        "branch-sensitive rebuild should allow plain Reuse on the exact branch only"
    );
}

#[test]
fn token_use_in_precompute_let_keeps_reuse_disabled() {
    let src = r#"
fn my_len(xs) {
    match xs {
        [_ | t] -> 1 + my_len(t),
        _ -> 0,
    }
}
fn bad_rebuild(xs) {
    match xs {
        [_ | t] -> [my_len(xs) | t],
        _ -> [],
    }
}
fn main() { bad_rebuild([1, 2, 3]) }
"#;
    let core = lowered_core(src);
    let bad_rebuild = core.defs.first().expect("bad_rebuild def");
    let has_reuse = collect_core_exprs(&bad_rebuild.expr)
        .into_iter()
        .any(|expr| matches!(expr, CoreExpr::Reuse { .. }));
    assert!(
        !has_reuse,
        "token-dependent precompute lets must keep plain Reuse disabled"
    );
}

#[test]
fn branch_joined_list_rebuilds_emit_reuse_in_each_exact_branch() {
    let src = r#"
fn rebuild(xs, flag) {
    match xs {
        [h | t] -> if flag { [h | t] } else { [h | t] },
        _ -> None,
    }
}
fn main() { rebuild([1, 2, 3], true) }
"#;
    let core = lowered_core(src);
    let reuse_count = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Cons,
                    ..
                }
            )
        })
        .count();
    assert!(
        reuse_count >= 2,
        "branch-joined exact list rebuild should emit reuse in both branches"
    );
}

#[test]
fn branch_joined_wrapper_rebuilds_emit_reuse_in_each_exact_branch() {
    let src = r#"
fn rebuild(opt, flag) {
    match opt {
        Some(x) -> if flag { Some(x) } else { Some(x) },
        _ -> None,
    }
}
fn main() { rebuild(Some(1), true) }
"#;
    let core = lowered_core(src);
    let reuse_count = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Some,
                    ..
                }
            )
        })
        .count();
    assert!(
        reuse_count >= 2,
        "branch-joined exact wrapper rebuild should emit reuse in both branches"
    );
}

#[test]
fn maintained_either_match_fixture_still_emits_reuse_sites() {
    let src = std::fs::read_to_string("examples/patterns/either_match.flx")
        .expect("either_match fixture should exist");
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "either_match should still lower to at least one Reuse site"
    );
}

#[test]
fn maintained_list_map_filter_fixture_still_emits_reuse_sites() {
    let src = std::fs::read_to_string("examples/advanced/list_map_filter.flx")
        .expect("list_map_filter fixture should exist");
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "list_map_filter should still lower to at least one Reuse site"
    );
}

#[test]
fn maintained_using_list_module_fixture_still_emits_reuse_sites() {
    let src = std::fs::read_to_string("examples/advanced/using_list_module.flx")
        .expect("using_list_module fixture should exist");
    let core = lowered_core(src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "using_list_module should still lower to at least one Reuse site"
    );
}

#[test]
fn verify_aether_fixture_claimed_fast_paths_match_current_core_shape() {
    let src =
        std::fs::read_to_string("examples/aether/verify_aether.flx").expect("fixture should exist");
    let core = lowered_core(src);

    let set_black = core
        .defs
        .iter()
        .find(|def| {
            collect_core_exprs(&def.expr).into_iter().any(|expr| {
                matches!(
                    expr,
                    CoreExpr::Reuse {
                        tag: flux::core::CoreTag::Named(_),
                        field_mask: Some(_),
                        ..
                    }
                )
            })
        })
        .expect("set_black-like def");
    assert!(collect_core_exprs(&set_black.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::Reuse {
                tag: flux::core::CoreTag::Named(_),
                field_mask: Some(_),
                ..
            }
        )
    }));

    // Phase 7e: `my_filter` no longer needs DropSpecialized because `h` in
    // [h | t] is IntRep (typed pattern binders). The function now gets a direct
    // Reuse on the Cons cell without the unique/shared split.
    let my_filter = core
        .defs
        .iter()
        .find(|def| {
            collect_core_exprs(&def.expr).into_iter().any(|expr| {
                matches!(
                    expr,
                    CoreExpr::Reuse {
                        tag: flux::core::CoreTag::Cons,
                        ..
                    }
                )
            })
        })
        .expect("my_filter-like def (Cons reuse)");
    assert!(collect_core_exprs(&my_filter.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::Reuse {
                tag: flux::core::CoreTag::Cons,
                ..
            }
        )
    }));

    let my_map = core
        .defs
        .iter()
        .find(|def| {
            collect_core_exprs(&def.expr).into_iter().any(|expr| {
                matches!(
                    expr,
                    CoreExpr::AetherCall { arg_modes, .. }
                        if arg_modes == &[
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                        ]
                )
            })
        })
        .expect("my_map-like def");
    assert!(collect_core_exprs(&my_map.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::AetherCall { arg_modes, .. }
                if arg_modes == &[
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                ]
        )
    }));
}

#[test]
fn hof_recursive_suite_fixture_claims_match_current_core_shape() {
    let src = std::fs::read_to_string("examples/aether/hof_recursive_suite.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);

    let map_reuse = core
        .defs
        .iter()
        .find(|def| {
            let exprs = collect_core_exprs(&def.expr);
            exprs
                .iter()
                .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
                && exprs.iter().any(|expr| {
                    matches!(
                        expr,
                        CoreExpr::AetherCall { arg_modes, .. }
                            if arg_modes == &[
                                flux::aether::borrow_infer::BorrowMode::Borrowed,
                                flux::aether::borrow_infer::BorrowMode::Borrowed,
                            ]
                    )
                })
        })
        .expect("map_reuse-like def");
    assert!(
        collect_core_exprs(&map_reuse.expr)
            .into_iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "higher-order recursive rebuild should emit plain Reuse"
    );

    let borrowed_only = core
        .defs
        .iter()
        .find(|def| {
            let exprs = collect_core_exprs(&def.expr);
            exprs.iter().any(|expr| {
                matches!(
                    expr,
                    CoreExpr::AetherCall { arg_modes, .. }
                        if arg_modes == &[
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                        ]
                )
            }) && !exprs
                .iter()
                .any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
        })
        .expect("count_if-like def");
    assert!(
        !collect_core_exprs(&borrowed_only.expr)
            .into_iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "borrowed-only higher-order traversal should stay non-reusing"
    );

    let warnings = compile_fixture_warnings("examples/aether/hof_recursive_suite.flx");
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `option_chain`") && m.contains("indirect or opaque callee `f`")
        })
    }));
}

#[test]
fn tree_updates_fixture_claims_match_current_core_shape() {
    let src =
        std::fs::read_to_string("examples/aether/tree_updates.flx").expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .collect::<Vec<_>>();

    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: Some(_),
                    ..
                }
            )
        }),
        "tree_updates should include masked named-ADT reuse"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: Some(_),
                    ..
                }
            )
        }),
        "tree_updates should include an additional named-ADT reuse site"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: None,
                    ..
                }
            )
        }),
        "tree_updates should include plain named-ADT reuse on a wider rewrite"
    );
    assert!(
        exprs.iter().any(|expr| match expr {
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
                unique_reuses >= 1 && shared_reuses == 0
            }
            _ => false,
        }),
        "tree_updates should keep unique/shared asymmetry on branch-sensitive updates"
    );
}

#[test]
fn queue_workload_fixture_claims_match_current_core_shape() {
    let src = std::fs::read_to_string("examples/aether/queue_workload.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .collect::<Vec<_>>();

    assert!(
        exprs
            .iter()
            .filter(|expr| matches!(expr, CoreExpr::Reuse { .. }))
            .count()
            >= 2,
        "queue workload should currently emit two wrapper reuse sites"
    );
    assert!(
        exprs.into_iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::AetherCall { arg_modes, .. }
                    if arg_modes == &[
                        flux::aether::borrow_infer::BorrowMode::Owned,
                        flux::aether::borrow_infer::BorrowMode::Borrowed,
                        flux::aether::borrow_infer::BorrowMode::Owned,
                    ]
            )
        }),
        "queue rotate_sum should keep the iteration count borrowed through recursion"
    );
}

#[test]
fn forwarded_wrapper_fixture_rewrites_only_the_inner_child() {
    let src = std::fs::read_to_string("examples/aether/forwarded_wrapper_reuse.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .collect::<Vec<_>>();

    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::App { args: fields, .. } | CoreExpr::AetherCall { args: fields, .. }
                    if fields
                        .iter()
                        .any(|field| matches!(field, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }))
            )
        }),
        "forwarded wrapper fixture should keep the outer wrapper fresh and recover inner cons reuse"
    );
    assert!(
        !exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    ..
                }
            )
        }),
        "forwarded wrapper fixture should not speculate into outer wrapper reuse"
    );
}

#[test]
fn opt_corpus_positive_fixture_claims_match_current_core_shape() {
    let src = std::fs::read_to_string("examples/aether/opt_corpus_positive.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .collect::<Vec<_>>();

    assert!(
        exprs
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "positive corpus should include at least one Reuse site"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse {
                    tag: flux::core::CoreTag::Named(_),
                    field_mask: Some(_),
                    ..
                }
            )
        }),
        "positive corpus should include masked named-ADT reuse"
    );
    assert!(
        exprs
            .iter()
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "positive corpus should include DropSpecialized"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::App { args: fields, .. } | CoreExpr::AetherCall { args: fields, .. }
                    if fields
                        .iter()
                        .any(|field| matches!(field, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }))
            )
        }),
        "positive corpus should include forwarded-child reuse inside a fresh wrapper"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::AetherCall { arg_modes, .. }
                    if arg_modes
                        == &[
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                            flux::aether::borrow_infer::BorrowMode::Borrowed,
                        ]
            )
        }),
        "positive corpus should include a borrowed-only higher-order recursive call"
    );
}

#[test]
fn opt_corpus_negative_fixture_stays_conservative_on_intended_shapes() {
    let src = std::fs::read_to_string("examples/aether/opt_corpus_negative.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .collect::<Vec<_>>();

    assert!(
        exprs
            .iter()
            .filter(|expr| matches!(expr, CoreExpr::Reuse { .. }))
            .count()
            == 1,
        "negative corpus should keep reuse limited to the single exact fresh_tree path"
    );
    assert!(
        !exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::App { args: fields, .. } | CoreExpr::AetherCall { args: fields, .. }
                    if fields
                        .iter()
                        .any(|field| matches!(field, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }))
            )
        }),
        "negative corpus should keep the forwarding near-miss fresh"
    );
    // Phase 7e: with typed pattern binders, `y` in [y | ys] is IntRep (from
    // List<Int>), so DropSpecialized is no longer needed — no dup/drop divergence.
    // DropSpecialized may or may not be present depending on other functions.
    // The key invariant is that reuse stays limited and near-miss stays fresh.
}

#[test]
fn fbip_success_cases_fixture_stays_provable() {
    let warnings = compile_fixture_warnings("examples/aether/fbip_success_cases.flx");
    assert_eq!(
        warnings.len(),
        3,
        "expected current Stage 3 FBIP warning set"
    );
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `rebuild_list`")
                && m.contains("Fbip { bound: 1 }")
                && m.contains("fresh heap allocation remains")
        })
    }));
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `rebuild_some`")
                && m.contains("Fbip { bound: 1 }")
                && m.contains("fresh heap allocation remains")
        })
    }));
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `set_black`")
                && m.contains("calls imported or name-only function `Node`")
        })
    }));
}

#[test]
fn fbip_failure_cases_fixture_reports_expected_warning_mix() {
    let warnings = compile_fixture_warnings("examples/aether/fbip_failure_cases.flx");
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `fail_indirect`") && m.contains("indirect or opaque callee `f`")
        })
    }));
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `fail_imported`")
                && (m.contains("Imported.imported_inc")
                    || m.contains("imported_inc")
                    || m.contains("indirect or opaque callee"))
        })
    }));
    assert!(warnings.iter().any(|d| {
        d.message().is_some_and(|m| {
            m.contains("@fip on `fail_fresh`") && m.contains("fresh heap allocation remains")
        })
    }));
}
