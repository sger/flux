use std::collections::HashMap;

use flux::core::{CoreExpr, lower_ast::lower_program_ast, passes::run_core_passes_with_interner};
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};
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

    let mut compiler =
        flux::bytecode::compiler::Compiler::new_with_interner("<test>", interner.clone());
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
    let mut compiler = flux::bytecode::compiler::Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("compile ok");
    let mut vm = flux::bytecode::vm::VM::new(compiler.bytecode());
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

fn lowered_core(src: &str) -> flux::core::CoreProgram {
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");
    core
}

#[test]
fn borrow_local_call_emits_aether_call() {
    let src = r#"
fn my_len(xs) { len(xs) }
fn len_twice(xs) { my_len(xs) + my_len(xs) }
fn main() { len_twice([1, 2, 3]) }
"#;
    let core = lowered_core(&src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(
                expr,
                CoreExpr::AetherCall { arg_modes, .. }
                    if arg_modes == &[flux::aether::borrow_infer::BorrowMode::Borrowed]
            ))
    );
    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn borrow_base_call_stays_dup_free() {
    let src = r#"
fn len_twice(xs) { len(xs) + len(xs) }
fn main() { len_twice([1, 2, 3]) }
"#;
    let core = lowered_core(&src);
    assert!(
        !core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::Dup { .. }))
    );
    assert_eq!(run(src), Value::Integer(6));
}

#[test]
fn borrow_mixed_arg_modes_are_explicit() {
    let src = r#"
fn borrow_then_return(xs, y) { if len(xs) > 0 { y } else { y } }
fn main() { borrow_then_return([1, 2, 3], 42) }
"#;
    let core = lowered_core(&src);
    assert!(
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(
                expr,
                CoreExpr::AetherCall { arg_modes, .. }
                    if arg_modes == &[
                        flux::aether::borrow_infer::BorrowMode::Borrowed,
                        flux::aether::borrow_infer::BorrowMode::Owned,
                    ]
            ))
    );
}

#[test]
fn recursive_borrow_signature_stays_precise() {
    let src = r#"
fn loop(xs, n) {
    if n == 0 { len(xs) } else { loop(xs, n - 1) }
}
fn main() { loop([1, 2, 3], 3) }
"#;
    let core = lowered_core(src);
    let loop_def = core
        .defs
        .iter()
        .find(|def| def.name == core.defs[0].name)
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
fn even(xs, n) {
    if n == 0 { len(xs) } else { odd(xs, n - 1) }
}
fn odd(xs, n) {
    if n == 0 { len(xs) } else { even(xs, n - 1) }
}
fn main() { even([1, 2, 3], 4) }
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
fn use_closure(xs) {
    let f = fn() { len(xs) };
    f() + f()
}
fn main() { use_closure([1, 2, 3]) }
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
fn use_closure(xs) {
    let f = fn() { len(xs) };
    f()
}
fn main() { use_closure([1, 2, 3]) }
"#;
    let core = lowered_core(src);
    let use_closure = core.defs.first().expect("use_closure def");
    let sig = use_closure
        .borrow_signature
        .as_ref()
        .expect("borrow signature");
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
    let core = lowered_core(&src);
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
    let core = lowered_core(&src);
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
    let core = lowered_core(&src);
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
    let core = lowered_core(&src);
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
    let core = lowered_core(&src);
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
    let core = lowered_core(&src);
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
fn drop_spec_list_multiuse_field_emits_drop_specialized() {
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
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. }))
    );
}

#[test]
fn branch_join_borrow_only_path_avoids_extra_drop() {
    let src = r#"
fn branch_read(xs, choose) {
    if choose { len(xs) } else { 0 }
}
fn main() { branch_read([1, 2, 3], true) }
"#;
    let core = lowered_core(src);
    let drops = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .filter(|expr| matches!(expr, CoreExpr::Drop { .. }))
        .count();
    assert_eq!(
        drops, 0,
        "borrow-only branch joins should not need explicit drops"
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
fn drop_spec_branchy_list_update_emits_drop_specialized() {
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
        core.defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. }))
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
fn maintained_either_match_fixture_survives_aether_passes() {
    let src = std::fs::read_to_string("examples/patterns/either_match.flx")
        .expect("fixture should exist");
    let core = lowered_core(&src);
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
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");
    let fbip = flux::aether::check_fbip::check_fbip(&core, &interner);
    assert!(
        fbip.error.is_none(),
        "verify_aether should not hard-fail FBIP"
    );
}

#[test]
fn fbip_failure_fixture_stays_non_provable() {
    let src = std::fs::read_to_string("examples/aether/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist");
    let (program, types, interner) = parse_and_infer(&src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner)
        .expect_err("fbip failure fixture should error during core passes");
}

#[test]
fn bench_reuse_fixture_my_map_shows_borrowed_recursion_not_plain_reuse() {
    let src =
        std::fs::read_to_string("examples/aether/bench_reuse.flx").expect("fixture should exist");
    let core = lowered_core(&src);
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
        "bench_reuse my_map should preserve borrowed recursive traversal"
    );
    assert!(
        !has_reuse,
        "bench_reuse my_map does not currently emit plain Reuse and the fixture should not claim that it does"
    );
    assert!(
        borrowed_benchmark_wrappers,
        "bench_reuse wrappers should thread the benchmark list input through borrowed call modes"
    );
}

#[test]
fn verify_aether_fixture_claimed_fast_paths_match_current_core_shape() {
    let src =
        std::fs::read_to_string("examples/aether/verify_aether.flx").expect("fixture should exist");
    let core = lowered_core(&src);

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

    let my_filter = core
        .defs
        .iter()
        .find(|def| {
            collect_core_exprs(&def.expr)
                .into_iter()
                .any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. }))
        })
        .expect("my_filter-like def");
    assert!(
        collect_core_exprs(&my_filter.expr)
            .into_iter()
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
            })
    );

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
