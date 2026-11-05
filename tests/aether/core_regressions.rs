use std::collections::HashMap;
use std::path::Path;

use flux::aether::{AetherExpr as CoreExpr, AetherProgram, lower_core_to_aether_program};
use flux::compiler::Compiler;
use flux::core::{
    CorePat, CoreProgram,
    lower_ast::lower_program_ast,
    passes::{run_aether_passes_with_interner_and_registry, run_core_passes_with_interner},
};
use flux::diagnostics::Diagnostic;
use flux::runtime::value::Value;
use flux::syntax::{interner::Interner, lexer::Lexer, module_graph::ModuleGraph, parser::Parser};
use flux::types::infer_type::InferType;

fn parse_and_infer(
    src: &str,
) -> (flux::syntax::program::Program, HashMap<flux::syntax::expression::ExprId, InferType>, Interner)
{
    let lexer = Lexer::new(src);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();

    let mut compiler = flux::compiler::Compiler::new_with_interner("<test>", interner.clone());
    let types = compiler.infer_expr_types_for_program(&program);
    (program, types, interner)
}

fn run(src: &str) -> Value {
    let lexer = Lexer::new(src);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
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
        CoreExpr::Case { scrutinee, alts, .. } => {
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
        CoreExpr::Dup { body, .. }
        | CoreExpr::Drop { body, .. }
        | CoreExpr::FreeShell { body, .. } => {
            out.extend(collect_core_exprs(body));
        }
        CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
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
    collect_core_exprs(expr).into_iter().filter(|expr| predicate(expr)).count()
}

fn lowered_core(src: impl AsRef<str>) -> AetherProgram {
    lowered_core_and_interner(src).0
}

fn lowered_core_and_interner(src: impl AsRef<str>) -> (AetherProgram, Interner) {
    let (program, types, interner) = parse_and_infer(src.as_ref());
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    let compiler = Compiler::new_with_interner("<test>", interner.clone());
    let preloaded_registry = compiler.build_preloaded_borrow_registry(&program);
    let (aether, _warnings) =
        lower_core_to_aether_program(&core, Some(&interner), preloaded_registry)
            .expect("aether lowering should succeed");
    (aether, interner)
}

/// How many `Dup`s of a binder named `name` the program holds.
fn dups_named(core: &AetherProgram, interner: &Interner, name: &str) -> usize {
    let name = interner.lookup(name).unwrap_or_else(|| panic!("the program binds `{name}`"));
    core.defs
        .iter()
        .map(|def| {
            count_matching(
                &def.expr,
                &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.name == name),
            )
        })
        .sum()
}

fn compile_fixture_warnings(rel: &str) -> Vec<Diagnostic> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = workspace_root.join(rel);
    let source = std::fs::read_to_string(&fixture).expect("fixture should exist");

    let lexer = Lexer::new(&source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);

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
    assert!(graph.diagnostics.is_empty(), "module diagnostics: {:?}", graph.diagnostics);

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
    assert_eq!(dup_count, 0, "borrowed primop args should not introduce dups");
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
    let loop_def = core.defs.iter().find(|def| def.name == core.defs[1].name).expect("loop def");
    let sig = loop_def.borrow_signature.as_ref().expect("borrow signature");
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
    let sig = loop_def.borrow_signature.as_ref().expect("borrow signature");
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
    let use_closure = core.defs.get(1).expect("use_closure def");
    let CoreExpr::Lam { params, .. } = &use_closure.expr else {
        panic!("use_closure is a lambda");
    };
    let xs = params[0].id;
    let dups = collect_core_exprs(&use_closure.expr)
        .into_iter()
        .filter(|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(xs)))
        .count();
    assert_eq!(dups, 0, "read-only closure capture should not introduce dups");
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
    let sig = use_closure.borrow_signature.as_ref().expect("borrow signature");
    // array_len borrows its arg, so my_len borrows xs, so the closure
    // only reads xs — use_closure's xs stays Borrowed.
    assert_eq!(sig.params, vec![flux::aether::borrow_infer::BorrowMode::Borrowed]);
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
fn pattern_fields_are_duplicated_before_collection_rebuild() {
    let src = r#"
fn rebuild(xs) {
    match xs {
        [(key, value) | rest] -> [(key, value) | rest],
        _ -> [],
    }
}

fn main() { rebuild([("key", [1, 2])]) }
"#;
    let core = lowered_core(src);
    // `key` and `value` sit in the tuple cell, which the release of `xs` can
    // free with the list cell, so the arm copies both first (audit L2).
    // Nothing names the tuple cell for a unique path to release, so the arm
    // keeps its plain release, after the copies.
    let (key, value, rhs) = core
        .defs
        .iter()
        .flat_map(|def| collect_core_exprs(&def.expr))
        .find_map(|expr| match expr {
            CoreExpr::Case { alts, .. } => alts.iter().find_map(|alt| match &alt.pat {
                CorePat::Con { fields, .. } => match fields.first() {
                    Some(CorePat::Tuple(pair)) => match pair.as_slice() {
                        [CorePat::Var(key), CorePat::Var(value)] => {
                            Some((key.id, value.id, &alt.rhs))
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            }),
            _ => None,
        })
        .expect("the [(key, value) | rest] arm");
    let dups_of = |binder| {
        count_matching(
            rhs,
            &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(binder)),
        )
    };
    assert_eq!((dups_of(key), dups_of(value)), (1, 1), "{rhs:?}");
    assert_eq!(
        count_matching(rhs, &|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        0,
        "{rhs:?}"
    );
}

/// KI-164: `nodes` owns `tree` (its `Tip` arm stores it), and its `Node`
/// arm builds no cell to reuse `tree`'s for. On the unique path the arm
/// still owns the cell and its fields, so it frees the cell alone and copies
/// nothing; the shared path copies the fields and releases its reference.
#[test]
fn a_unique_cell_whose_arm_builds_no_cell_frees_its_shell() {
    let src = r#"
type Tree = Node(Tree, Tree) | Tip

fn make(depth: Int) -> Tree {
    if depth <= 0 { Tip } else { Node(make(depth - 1), make(depth - 1)) }
}

fn nodes(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> nodes(left) + nodes(right) + 1,
        Tip -> len([tree]) - 1,
    }
}

fn main() with IO {
    print(nodes(make(20)))
}
"#;
    let (core, interner) = lowered_core_and_interner(src);
    let def = core.defs.iter().find(|def| interner.resolve(def.name) == "nodes").expect("nodes");
    let specialized: Vec<(&CoreExpr, &CoreExpr)> = collect_core_exprs(&def.expr)
        .into_iter()
        .filter_map(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
                Some((unique_body.as_ref(), shared_body.as_ref()))
            }
            _ => None,
        })
        .collect();
    let [(unique, shared)] = specialized.as_slice() else {
        panic!("one drop specialization in nodes, got {}: {:?}", specialized.len(), def.expr);
    };
    let tree_name = interner.lookup("tree").expect("the program binds `tree`");
    let tree = |var: &flux::core::CoreVarRef| var.name == tree_name;
    assert!(
        matches!(unique, CoreExpr::FreeShell { var, .. } if tree(var)),
        "the unique path frees tree first: {unique:?}"
    );
    assert_eq!(count_matching(unique, &|expr| matches!(expr, CoreExpr::Dup { .. })), 0);
    assert_eq!(
        count_matching(shared, &|expr| matches!(expr, CoreExpr::Drop { var, .. } if tree(var))),
        1,
        "{shared:?}"
    );
    flux::aether::verify::verify_def_contract_aether(&def.expr, def.borrow_signature.as_ref())
        .expect("the contract holds");
}

/// KI-165: `[h | t] -> match h { Wrap(x) -> [x | t], .. }`. The plain-drop
/// rewrite releases `h` before the rebuild, which is safe only while `xs`
/// still holds `h`. A unique path owns `h`'s only reference, so that release
/// frees `x` before the rebuild reads it. The plan keeps the reuse, under the
/// plain release.
#[test]
fn a_rebuild_around_a_field_of_a_field_reads_it_before_its_parent_is_released_ki165() {
    let src = std::fs::read_to_string("tests/parity/unique_cell_field_of_a_field_ki165.flx")
        .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    for name in ["peel", "peel_or_drop"] {
        let def = core.defs.iter().find(|def| interner.resolve(def.name) == name).expect(name);
        flux::aether::verify::verify_def_contract_aether(&def.expr, def.borrow_signature.as_ref())
            .unwrap_or_else(|errors| panic!("{name}: {errors:?}"));
        assert!(
            count_matching(&def.expr, &|expr| matches!(expr, CoreExpr::Reuse { .. })) > 0,
            "{name} still reuses the cell: {:?}",
            def.expr
        );
    }
}

/// The arms of the case on the unique path of `name`'s one split.
fn unique_path_arms<'a>(
    core: &'a AetherProgram,
    interner: &Interner,
    name: &str,
) -> (&'a CoreExpr, &'a [flux::aether::AetherAlt]) {
    let def = core.defs.iter().find(|def| interner.resolve(def.name) == name).expect(name);
    flux::aether::verify::verify_def_contract_aether(&def.expr, def.borrow_signature.as_ref())
        .expect("the contract holds");
    let unique: Vec<&CoreExpr> = collect_core_exprs(&def.expr)
        .into_iter()
        .filter_map(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, .. } => Some(unique_body.as_ref()),
            _ => None,
        })
        .collect();
    let [unique] = unique.as_slice() else {
        panic!("one split in {name}, got {}: {:?}", unique.len(), def.expr);
    };
    let CoreExpr::Case { alts, .. } = unique else {
        panic!("a case on {name}'s unique path: {unique:?}");
    };
    (unique, alts)
}

/// A unique path that can reuse its cell on one arm only: the other arm frees
/// it alone, so neither arm pays for field copies or a release walk.
#[test]
fn a_unique_path_frees_its_cell_on_the_arm_that_does_not_reuse_it() {
    let src =
        std::fs::read_to_string("tests/debug/heap/owned_moves/unique_path_reuse_in_one_arm.flx")
            .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    let (unique, arms) = unique_path_arms(&core, &interner, "split");
    assert_eq!(
        count_matching(unique, &|expr| matches!(expr, CoreExpr::Dup { .. })),
        0,
        "{unique:?}"
    );
    assert_eq!(count_matching(&arms[0].rhs, &|expr| matches!(expr, CoreExpr::Reuse { .. })), 1);
    assert_eq!(count_matching(&arms[0].rhs, &|expr| matches!(expr, CoreExpr::FreeShell { .. })), 0);
    assert_eq!(count_matching(&arms[1].rhs, &|expr| matches!(expr, CoreExpr::Reuse { .. })), 0);
    assert_eq!(count_matching(&arms[1].rhs, &|expr| matches!(expr, CoreExpr::FreeShell { .. })), 1);
}

/// The arm that frees its cell then calls a function that can suspend. The
/// free has to come first: past the call a resumed continuation may have
/// copied the cell, so it would no longer be this path's alone.
#[test]
fn a_unique_path_frees_its_cell_before_a_call_that_can_suspend() {
    let src = std::fs::read_to_string(
        "tests/debug/heap/owned_moves/unique_path_frees_before_a_resumed_call.flx",
    )
    .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    let (_, arms) = unique_path_arms(&core, &interner, "step");
    assert_eq!(count_matching(&arms[0].rhs, &|expr| matches!(expr, CoreExpr::Reuse { .. })), 1);
    assert!(matches!(arms[1].rhs, CoreExpr::FreeShell { .. }), "{:?}", arms[1].rhs);
}

/// Audit L1: `Node(color, left, key, right) -> Node(color, left, key, left)`
/// stores `left` twice. A unique cell hands over one reference to it, so the
/// unique path keeps one copy for the second store.
#[test]
fn a_unique_cell_reused_with_a_field_twice_keeps_one_copy() {
    let src = std::fs::read_to_string("examples/aether/drop_spec_branchy.flx")
        .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    let def = core
        .defs
        .iter()
        .find(|def| interner.resolve(def.name) == "keep_or_dup_left")
        .expect("keep_or_dup_left");
    let unique_dups: Vec<usize> = collect_core_exprs(&def.expr)
        .into_iter()
        .filter_map(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, .. } => {
                Some(count_matching(unique_body, &|inner| {
                    matches!(inner, CoreExpr::Dup { var, .. } if interner.resolve(var.name) == "left")
                }))
            }
            _ => None,
        })
        .collect();
    assert_eq!(unique_dups, [1]);
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
        .filter(|expr| matches!(expr, CoreExpr::Reuse { field_mask: Some(_), .. }))
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
    assert!(core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| matches!(
        expr,
        CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
    )));
}

#[test]
fn maintained_drop_spec_branchy_fixture_emits_reuse_inside_drop_specialized() {
    let src = std::fs::read_to_string("examples/aether/drop_spec_branchy.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let found =
        core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
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
    // Phase 7e: `h` in [h | t] is IntRep (typed pattern binders), so it is
    // never copied. `t` is, where it is given away (0189 F3-2a), so the arm
    // may still be drop-specialized for it.
    let src = std::fs::read_to_string("examples/aether/drop_spec_recursive.flx")
        .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    assert_eq!(
        dups_named(&core, &interner, "h"),
        0,
        "typed pattern binders (IntRep) keep integer list elements uncounted"
    );
}

#[test]
fn maintained_drop_spec_branchy_fixture_keeps_unique_path_dup_free() {
    let src = std::fs::read_to_string("examples/aether/drop_spec_branchy.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let found =
        core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
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
    assert!(total_dups <= 2, "typed pattern binders should minimize dups; got {total_dups}");
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
    assert!(core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| matches!(
        expr,
        CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: None, .. }
    )));
}

#[test]
fn drop_spec_list_multiuse_field_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` in `[h | t]` is IntRep (from typed pattern binders), so
    // however often it is used it is never copied.
    let src = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}
fn main() { copy_head([1, 2, 3]) }
"#;
    let (core, interner) = lowered_core_and_interner(src);
    assert_eq!(
        dups_named(&core, &interner, "h"),
        0,
        "typed pattern binders (IntRep) keep integer list elements uncounted"
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
    let drop_count =
        count_matching(&shadow_def.expr, &|expr| matches!(expr, CoreExpr::Drop { .. }));
    // `x` is unused, so it is Borrowed: its caller keeps it, and the
    // function never releases it, shadowed or not (F2).
    assert_eq!(
        drop_count, 0,
        "an unused, lent parameter is never released, even when handler params shadow it"
    );
}

#[test]
fn drop_spec_branchy_list_update_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` is IntRep, so it is never copied.
    let src = r#"
fn copy_or_keep_head(xs, copy) {
    match xs {
        [h | t] -> if copy { [h | [h | t]] } else { [h | t] },
        _ -> [],
    }
}
fn main() { copy_or_keep_head([1, 2, 3], true) }
"#;
    let (core, interner) = lowered_core_and_interner(src);
    assert_eq!(
        dups_named(&core, &interner, "h"),
        0,
        "typed pattern binders (IntRep) keep integer list elements uncounted"
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
    let found =
        core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
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
    assert!(found, "expected branchy named ADT DropSpecialized to reuse only on the unique path");
}

#[test]
fn drop_spec_recursive_update_skips_drop_specialized_for_int_elements() {
    // Phase 7e: `h` is IntRep, so it is never copied.
    let src = r#"
fn rec_copy_or_keep(xs, choose) {
    match xs {
        [h | t] -> if choose { [h | rec_copy_or_keep(t, choose)] } else { [h | [h | rec_copy_or_keep(t, choose)]] },
        _ -> [],
    }
}
fn main() { rec_copy_or_keep([1, 2, 3], false) }
"#;
    let (core, interner) = lowered_core_and_interner(src);
    assert_eq!(
        dups_named(&core, &interner, "h"),
        0,
        "typed pattern binders (IntRep) keep integer list elements uncounted"
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
    assert!(fbip.error.is_none(), "verify_aether should not hard-fail FBIP");
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
                                flux::aether::borrow_infer::BorrowMode::Owned,
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
                    flux::aether::borrow_infer::BorrowMode::Owned,
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                ]
        )
    });
    let has_reuse = collect_core_exprs(&my_map.expr)
        .into_iter()
        .any(|expr| matches!(expr, CoreExpr::Reuse { .. }));
    let borrowed_benchmark_wrappers =
        core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| {
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
        "bench_reuse my_map owns the list it rebuilds and borrows its function (F2: a list it may reuse in place is owned)"
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

    assert!(enabled_reuses >= 1, "reuse-enabled benchmark should currently emit plain Reuse");
    assert_eq!(blocked_reuses, 0, "reuse-blocked benchmark should keep plain Reuse disabled");
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
                    flux::aether::borrow_infer::BorrowMode::Owned,
                    flux::aether::borrow_infer::BorrowMode::Borrowed,
                ]
        )
    });
    assert!(has_reuse, "higher-order recursive rebuild should now emit plain Reuse");
    assert!(
        borrowed_self_call,
        "higher-order recursive rebuild owns the list it reuses and borrows its function (F2)"
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
    let found =
        core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).any(|expr| match expr {
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
    assert!(found, "branch-sensitive rebuild should allow plain Reuse on the exact branch only");
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
    assert!(!has_reuse, "token-dependent precompute lets must keep plain Reuse disabled");
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
        .filter(|expr| matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }))
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
        .filter(|expr| matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Some, .. }))
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
    let src = std::fs::read_to_string("examples/guide/list_map_filter.flx")
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
    let src = std::fs::read_to_string("examples/guide/using_list_module.flx")
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
                    CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
                )
            })
        })
        .expect("set_black-like def");
    assert!(collect_core_exprs(&set_black.expr).into_iter().any(|expr| {
        matches!(
            expr,
            CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
        )
    }));

    // Phase 7e: `my_filter` no longer needs DropSpecialized because `h` in
    // [h | t] is IntRep (typed pattern binders). The function now gets a direct
    // Reuse on the Cons cell without the unique/shared split.
    let my_filter = core
        .defs
        .iter()
        .find(|def| {
            collect_core_exprs(&def.expr)
                .into_iter()
                .any(|expr| matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }))
        })
        .expect("my_filter-like def (Cons reuse)");
    assert!(
        collect_core_exprs(&my_filter.expr)
            .into_iter()
            .any(|expr| { matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }) })
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
                            flux::aether::borrow_infer::BorrowMode::Owned,
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
                    flux::aether::borrow_infer::BorrowMode::Owned,
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
            exprs.iter().any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
                && exprs.iter().any(|expr| {
                    matches!(
                        expr,
                        CoreExpr::AetherCall { arg_modes, .. }
                            if arg_modes == &[
                                flux::aether::borrow_infer::BorrowMode::Owned,
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
            }) && !exprs.iter().any(|expr| matches!(expr, CoreExpr::Reuse { .. }))
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
    let exprs = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).collect::<Vec<_>>();

    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
            )
        }),
        "tree_updates should include masked named-ADT reuse"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
            )
        }),
        "tree_updates should include an additional named-ADT reuse site"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: None, .. }
            )
        }),
        "tree_updates should include plain named-ADT reuse on a wider rewrite"
    );
    assert!(
        exprs.iter().any(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, shared_body, .. } => {
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
    let exprs = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).collect::<Vec<_>>();

    assert!(
        exprs.iter().filter(|expr| matches!(expr, CoreExpr::Reuse { .. })).count() >= 2,
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
    let exprs = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).collect::<Vec<_>>();

    assert!(
        exprs
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. })),
        "forwarded wrapper fixture should keep the outer wrapper fresh and recover inner cons reuse"
    );
    assert!(
        !exprs.iter().any(|expr| {
            matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), .. })
        }),
        "forwarded wrapper fixture should not speculate into outer wrapper reuse"
    );
}

/// `[y | ys] -> Pair2([y | acc], ys)`: the inner cons is rebuilt in the
/// matched cell. The binding it was read from goes with it, or the unique
/// path builds the cons a second time and gives `acc` away twice.
#[test]
fn a_forwarded_child_is_built_once() {
    let src = std::fs::read_to_string("examples/aether/forwarded_wrapper_reuse.flx")
        .expect("fixture should exist");
    let (core, interner) = lowered_core_and_interner(src);
    let def = core
        .defs
        .iter()
        .find(|def| interner.resolve(def.name) == "forwarded_child")
        .expect("forwarded_child");
    let unique_bodies: Vec<&CoreExpr> = collect_core_exprs(&def.expr)
        .into_iter()
        .filter_map(|expr| match expr {
            CoreExpr::DropSpecialized { unique_body, .. } => Some(unique_body.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(unique_bodies.len(), 1, "{:?}", def.expr);
    let unique = unique_bodies[0];
    let cons_built = count_matching(unique, &|expr| {
        matches!(
            expr,
            CoreExpr::Con { tag: flux::core::CoreTag::Cons, .. }
                | CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. }
        )
    });
    assert_eq!(cons_built, 1, "{unique:?}");
}

#[test]
fn opt_corpus_positive_fixture_claims_match_current_core_shape() {
    let src = std::fs::read_to_string("examples/aether/opt_corpus_positive.flx")
        .expect("fixture should exist");
    let core = lowered_core(src);
    let exprs = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).collect::<Vec<_>>();

    assert!(
        exprs.iter().any(|expr| matches!(expr, CoreExpr::Reuse { .. })),
        "positive corpus should include at least one Reuse site"
    );
    assert!(
        exprs.iter().any(|expr| {
            matches!(
                expr,
                CoreExpr::Reuse { tag: flux::core::CoreTag::Named(_), field_mask: Some(_), .. }
            )
        }),
        "positive corpus should include masked named-ADT reuse"
    );
    assert!(
        exprs.iter().any(|expr| matches!(expr, CoreExpr::DropSpecialized { .. })),
        "positive corpus should include DropSpecialized"
    );
    assert!(
        exprs
            .iter()
            .any(|expr| matches!(expr, CoreExpr::Reuse { tag: flux::core::CoreTag::Cons, .. })),
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
    let exprs = core.defs.iter().flat_map(|def| collect_core_exprs(&def.expr)).collect::<Vec<_>>();

    assert!(
        exprs.iter().filter(|expr| matches!(expr, CoreExpr::Reuse { .. })).count() == 1,
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
    assert_eq!(warnings.len(), 3, "expected current Stage 3 FBIP warning set");
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

/// Nested matches that rebuild the inner list (`list_zip`'s shape): each list is
/// released once on each path, never twice. The outer list is dropped on three
/// paths, and its cell is reused where the inner list is shared. The inner list is
/// dropped on the two empty-list paths and the shared rebuild path; the unique
/// rebuild consumes it through reuse. This guards against dropping the outer list on
/// entry and again in each inner arm.
#[test]
fn nested_match_drops_neither_list_twice() {
    let src = r#"
fn pairs(a, b) {
    match a {
        [ha | ta] -> match b {
            [hb | tb] -> [ha + hb | pairs(ta, tb)],
            _ -> [],
        },
        _ -> [],
    }
}
fn main() { pairs([1, 2], [10, 20]) }
"#;
    let core = lowered_core(src);
    let (params, body) = core
        .defs
        .iter()
        .find_map(|def| match &def.expr {
            CoreExpr::Lam { params, body, .. } if params.len() == 2 => Some((params, body)),
            _ => None,
        })
        .expect("pairs");
    let (a, b) = (params[0].id, params[1].id);
    let drops_of = |binder| {
        count_matching(
            body,
            &|expr| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(binder)),
        )
    };
    let reuses_of = |binder| {
        count_matching(
            body,
            &|expr| matches!(expr, CoreExpr::Reuse { token, .. } if token.binder == Some(binder)),
        )
    };
    assert_eq!(drops_of(a), 3, "{body:?}");
    assert_eq!(reuses_of(a), 1, "{body:?}");
    assert_eq!(reuses_of(b), 1, "{body:?}");
    assert_eq!(drops_of(b), 3, "{body:?}");
    assert!(flux::aether::verify::verify_contract_aether(body).is_ok());
}

/// Every build checks the Aether contract, and says nothing when it holds: the
/// `FLUX_AETHER_VERIFY=1` report is for someone who asked for it.
#[test]
fn a_build_that_checks_the_contract_says_nothing() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(["tests/parity/unique_cell_field_of_a_field_ki165.flx", "--no-cache", "--quiet"])
        .env("NO_COLOR", "1")
        .env_remove("FLUX_AETHER_VERIFY")
        .output()
        .expect("run flux");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("Aether verify"), "{stderr}");
}

/// `v` is the program's highest binder here, so the temporary holding the
/// indexed value used to get the same id. The temporary's drop then cancelled
/// `v`'s dup, and the arm never released the temporary (KI-167).
#[test]
fn a_temporary_and_the_field_it_holds_stay_two_variables_ki167() {
    let core = lowered_core(
        r#"
fn at(arr, idx) {
    match arr[idx] {
        Some(v) -> v,
        _ -> [],
    }
}
"#,
    );
    let def = core.defs().iter().last().expect("the function");
    let drops = count_matching(&def.expr, &|expr| matches!(expr, CoreExpr::Drop { .. }));
    assert_eq!(drops, 2, "each arm releases the temporary: {:?}", def.expr);
}

/// `next` lives in the owned `stack`. Lent to a borrowed `tree`, `stack` had to
/// outlive the call and its release came after it, so the call kept a frame and
/// a long walk overflowed the stack (KI-163).
#[test]
fn a_walk_that_passes_on_a_field_of_its_stack_ends_in_a_jump_ki163() {
    let (core, interner) = lowered_core_and_interner(
        r#"
type Tree = Node(Tree, Tree) | Tip
type Stack = Empty | Push(Tree, Stack)

fn walk(tree: Tree, stack: Stack, acc: Int) -> Int {
    match tree {
        Node(left, right) -> do {
            let pushed = Push(right, stack)
            walk(left, pushed, acc + 1)
        },
        Tip -> match stack {
            Push(next, rest) -> walk(next, rest, acc),
            Empty -> acc,
        },
    }
}
"#,
    );
    let walk = core
        .defs()
        .iter()
        .find(|def| interner.try_resolve(def.name) == Some("walk"))
        .expect("walk");
    let modes = &walk.borrow_signature.as_ref().expect("a signature").params;
    assert_eq!(modes[0], flux::aether::borrow_infer::BorrowMode::Owned, "{:?}", walk.expr);
    let held_across_the_call = count_matching(&walk.expr, &|expr| {
        matches!(
            expr,
            CoreExpr::Let { rhs, .. } if matches!(
                rhs.as_ref(),
                CoreExpr::AetherCall { func, .. } if matches!(
                    func.as_ref(),
                    CoreExpr::Var { var, .. } if var.name == walk.name
                )
            )
        )
    });
    assert_eq!(held_across_the_call, 0, "{:?}", walk.expr);
}

/// `balance1`'s common arm builds two nodes from the two it matched. Each cell
/// takes one, so its unique path allocates nothing, and frees nothing (KI-171).
#[test]
fn a_path_that_builds_two_constructors_reuses_both_of_its_cells_ki171() {
    let (core, interner) = lowered_core_and_interner(
        r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Bool, Tree)

fn balance(left_tree: Tree, right_tree: Tree) -> Tree {
    match left_tree {
        Node(_, _, kv, vv, tail) -> match right_tree {
            Node(_, l, ky, vy, r) -> Node(Black, Node(Red, l, ky, vy, r), kv, vv, tail),
            _ -> Leaf,
        },
        _ -> Leaf,
    }
}
"#,
    );
    let balance = core
        .defs()
        .iter()
        .find(|def| interner.try_resolve(def.name) == Some("balance"))
        .expect("balance");
    let CoreExpr::Lam { params, .. } = &balance.expr else { panic!("{:?}", balance.expr) };
    let (left, right) = (params[0].id, params[1].id);
    let reuses_of = |expr: &CoreExpr, cell| {
        count_matching(
            expr,
            &|expr| matches!(expr, CoreExpr::Reuse { token, .. } if token.binder == Some(cell)),
        )
    };
    let unique_path = collect_core_exprs(&balance.expr)
        .into_iter()
        .find_map(|expr| match expr {
            CoreExpr::DropSpecialized { scrutinee, unique_body, .. }
                if scrutinee.binder == Some(right) =>
            {
                Some(unique_body.as_ref())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", balance.expr));
    assert_eq!(reuses_of(unique_path, right), 1, "{unique_path:?}");
    assert_eq!(reuses_of(unique_path, left), 1, "{unique_path:?}");
    let allocated = count_matching(unique_path, &|expr| {
        matches!(
            expr,
            CoreExpr::AetherCall { func, .. } if matches!(
                func.as_ref(),
                CoreExpr::Var { var, .. } if interner.try_resolve(var.name) == Some("Node")
            )
        )
    });
    assert_eq!(allocated, 0, "{unique_path:?}");
}

/// The candidate parameters of `name` in `core` (proposal 0200 C1).
fn candidates_of(core: &CoreProgram, interner: &Interner, name: &str) -> Vec<usize> {
    let found = flux::aether::owned_clone::candidates(core, interner);
    let symbol = interner.lookup(name).unwrap_or_else(|| panic!("the program binds `{name}`"));
    let def = core.defs.iter().find(|def| def.name == symbol).expect("a definition");
    found
        .get(&def.binder.id)
        .map(|positions| positions.iter().copied().collect())
        .unwrap_or_default()
}

fn candidates_in(src: &str, name: &str) -> Vec<usize> {
    let (aether, interner) = lowered_core_and_interner(src);
    candidates_of(aether.as_core(), &interner, name)
}

const TREE_WALK: &str = r#"
type Tree = Node(Tree, Tree) | Tip
fn check_tree(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> check_tree(left) + check_tree(right) + 1,
        Tip -> 0,
    }
}
"#;

fn with_tree_walk(rest: &str) -> String {
    format!("{TREE_WALK}{rest}\nfn main() {{ check_tree(Tip) }}\n")
}

#[test]
fn a_walker_that_hands_its_fields_on_is_a_candidate() {
    assert_eq!(candidates_in(&with_tree_walk(""), "check_tree"), [0]);
}

#[test]
fn a_lookup_is_a_candidate_though_its_callers_may_keep_the_tree() {
    let src = with_tree_walk(
        r#"
fn leftmost(tree: Tree, depth: Int) -> Int {
    match tree {
        Node(left, _) -> leftmost(left, depth + 1),
        Tip -> depth,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "leftmost"), [0]);
}

#[test]
fn walkers_that_call_each_other_qualify_together() {
    let src = with_tree_walk(
        r#"
fn even_walk(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> odd_walk(left) + odd_walk(right) + 1,
        Tip -> 0,
    }
}
fn odd_walk(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> even_walk(left) + even_walk(right),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "even_walk"), [0]);
    assert_eq!(candidates_in(&src, "odd_walk"), [0]);
}

#[test]
fn both_parameters_of_a_walk_over_two_trees_are_candidates() {
    let src = with_tree_walk(
        r#"
fn same(a: Tree, b: Tree) -> Bool {
    match a {
        Node(a_left, a_right) -> match b {
            Node(b_left, b_right) -> same(a_left, b_left) && same(a_right, b_right),
            Tip -> false,
        },
        Tip -> match b {
            Tip -> true,
            _ -> false,
        },
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "same"), [0, 1]);
}

/// A field handed to a function that is only a candidate because of its own fields.
#[test]
fn a_field_handed_to_a_candidate_makes_its_parent_one() {
    let src = with_tree_walk(
        r#"
fn left_size(tree: Tree) -> Int {
    match tree {
        Node(left, _) -> check_tree(left),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "left_size"), [0]);
}

#[test]
fn a_parameter_a_guard_reads_is_no_candidate() {
    let src = with_tree_walk(
        r#"
fn is_tip(tree: Tree) -> Bool {
    match tree {
        Tip -> true,
        _ -> false,
    }
}
fn guarded(tree: Tree) -> Int {
    match tree {
        Node(left, _) if is_tip(tree) -> guarded(left),
        Node(left, _) -> guarded(left) + 1,
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "guarded"), Vec::<usize>::new());
}

#[test]
fn a_parameter_a_primop_reads_is_no_candidate() {
    let src = r#"
fn walk(xs: List<Int>) -> Int {
    let n = len(xs)
    match xs {
        [_ | rest] -> walk(rest) + n,
        _ -> 0,
    }
}
fn main() { walk([1]) }
"#;
    assert_eq!(candidates_in(src, "walk"), Vec::<usize>::new());
}

/// The lambda holds a field and may run later, or twice, so nothing here is a last
/// reference, though the field does reach a candidate position.
#[test]
fn a_function_with_a_lambda_is_no_candidate() {
    let src = with_tree_walk(
        r#"
type Later = Held((Int) -> Int)
fn delayed(tree: Tree) -> Later {
    match tree {
        Node(left, _) -> Held(\x -> x + check_tree(left)),
        Tip -> Held(\x -> x),
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "delayed"), Vec::<usize>::new());
}

/// A constructor isn't a function call with an effect to look for.
#[test]
fn a_function_that_builds_a_constructor_is_a_candidate() {
    let src = with_tree_walk(
        r#"
type Sized = Measured(Int)
fn measured(tree: Tree) -> Sized {
    match tree {
        Node(left, _) -> Measured(check_tree(left)),
        Tip -> Measured(0),
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "measured"), [0]);
}

/// Turns the first call in `expr` into a call to a name nothing binds, as one to
/// another module can be.
fn unresolve_first_call(expr: &mut flux::core::CoreExpr, name: flux::syntax::Identifier) -> bool {
    use flux::core::CoreExpr as E;
    match expr {
        E::App { func, .. } => match func.as_mut() {
            E::Var { var, .. } if var.binder.is_some() => {
                *var = flux::core::CoreVarRef::unresolved(name);
                true
            }
            _ => false,
        },
        E::Lam { body, .. } => unresolve_first_call(body, name),
        E::Let { rhs, body, .. } => {
            unresolve_first_call(rhs, name) || unresolve_first_call(body, name)
        }
        E::Case { alts, .. } => alts.iter_mut().any(|alt| unresolve_first_call(&mut alt.rhs, name)),
        E::PrimOp { args, .. } => args.iter_mut().any(|arg| unresolve_first_call(arg, name)),
        _ => false,
    }
}

/// `check_tree` still hands a field to itself through its other call, so E4 is the
/// only reason left.
#[test]
fn a_function_that_calls_a_name_it_cannot_see_is_no_candidate() {
    let (aether, mut interner) = lowered_core_and_interner(with_tree_walk(""));
    let mut core = aether.as_core().clone();
    let symbol = interner.lookup("check_tree").expect("the program binds it");
    let mystery = interner.intern("mystery");
    let def = core.defs.iter_mut().find(|def| def.name == symbol).expect("a definition");
    assert!(unresolve_first_call(&mut def.expr, mystery));
    assert_eq!(candidates_of(&core, &interner, "check_tree"), Vec::<usize>::new());
}

#[test]
fn a_function_that_performs_is_no_candidate() {
    let src = with_tree_walk(
        r#"
effect Note {
    note : () -> Int
}
fn noted(tree: Tree) -> Int with Note {
    match tree {
        Node(left, _) -> noted(left) + perform Note.note(),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "noted"), Vec::<usize>::new());
}

/// E4 follows calls: this walker performs nothing itself.
#[test]
fn a_function_that_calls_one_that_performs_is_no_candidate() {
    let src = with_tree_walk(
        r#"
effect Note {
    note : () -> Int
}
fn noted() -> Int with Note { perform Note.note() }
fn calls_noted(tree: Tree) -> Int with Note {
    match tree {
        Node(left, _) -> calls_noted(left) + noted(),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "calls_noted"), Vec::<usize>::new());
}

#[test]
fn a_function_that_calls_a_parameter_is_no_candidate() {
    let src = with_tree_walk(
        r#"
fn mapped(tree: Tree, f: (Int) -> Int) -> Int {
    match tree {
        Node(left, _) -> f(mapped(left, f)),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "mapped"), Vec::<usize>::new());
}

#[test]
fn a_function_over_the_size_limit_is_no_candidate() {
    let sum = vec!["1"; 300].join(" + ");
    let src = with_tree_walk(&format!(
        r#"
fn large(tree: Tree) -> Int {{
    match tree {{
        Node(left, _) -> large(left) + {sum},
        Tip -> 0,
    }}
}}
"#
    ));
    assert_eq!(candidates_in(&src, "large"), Vec::<usize>::new());
}

#[test]
fn an_arm_whose_field_is_used_twice_makes_no_candidate() {
    let src = with_tree_walk(
        r#"
fn twice(tree: Tree) -> Int {
    match tree {
        Node(left, _) -> twice(left) + twice(left),
        Tip -> 0,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "twice"), Vec::<usize>::new());
}

/// Drop specialization leaves a nested pattern alone, so the clone couldn't free
/// the cell it reads.
#[test]
fn an_arm_with_a_nested_pattern_makes_no_candidate() {
    let src = r#"
fn nested(xs: List<(Int, Int)>) -> Int {
    match xs {
        [(a, _) | rest] -> nested(rest) + a,
        _ -> 0,
    }
}
fn main() { nested([(1, 2)]) }
"#;
    assert_eq!(candidates_in(src, "nested"), Vec::<usize>::new());
}

/// The fixpoint: `outer` hands a field to `middle`, which hands one to `reads`, which
/// isn't a candidate. So neither of the first two is.
#[test]
fn losing_a_candidate_takes_its_callers_with_it() {
    let src = r#"
fn outer(xsss: List<List<List<Int>>>) -> Int {
    match xsss {
        [xss | _] -> middle(xss),
        _ -> 0,
    }
}
fn middle(xss: List<List<Int>>) -> Int {
    match xss {
        [xs | _] -> reads(xs),
        _ -> 0,
    }
}
fn reads(xs: List<Int>) -> Int { len(xs) }
fn main() { outer([[[1]]]) }
"#;
    assert_eq!(candidates_in(src, "reads"), Vec::<usize>::new());
    assert_eq!(candidates_in(src, "middle"), Vec::<usize>::new());
    assert_eq!(candidates_in(src, "outer"), Vec::<usize>::new());
}

/// Every read of `tree` is a scrutinee and its fields go to a candidate position, but
/// the arm rebuilds the cell, so inference makes it owned already.
#[test]
fn a_parameter_only_taken_apart_but_inferred_owned_is_no_candidate() {
    let src = with_tree_walk(
        r#"
fn mirror(tree: Tree) -> Tree {
    match tree {
        Node(left, right) -> Node(mirror(right), mirror(left)),
        Tip -> Tip,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "mirror"), Vec::<usize>::new());
}

/// The fixture's `nodes`: the `Tip` arm stores the tree, so the parameter is owned
/// already and needs no clone.
#[test]
fn a_parameter_inferred_owned_is_no_candidate() {
    let src = with_tree_walk(
        r#"
fn nodes(tree: Tree) -> Int {
    match tree {
        Node(left, right) -> check_tree(left) + nodes(right) + 1,
        Tip -> len([tree]) - 1,
    }
}
"#,
    );
    assert_eq!(candidates_in(&src, "nodes"), Vec::<usize>::new());
}

/// The flags and names E1 reads, set by hand on a function that otherwise qualifies:
/// the source can't always spell them.
#[test]
fn main_a_dictionary_an_anonymous_def_and_an_unboxed_parameter_are_no_candidates() {
    let (aether, mut interner) = lowered_core_and_interner(with_tree_walk(""));
    let core = aether.as_core().clone();
    let symbol = interner.lookup("check_tree").expect("the program binds it");
    let position = core.defs.iter().position(|def| def.name == symbol).expect("a definition");
    let qualifies = |core: &CoreProgram, interner: &Interner| {
        flux::aether::owned_clone::candidates(core, interner)
            .contains_key(&core.defs[position].binder.id)
    };
    assert!(qualifies(&core, &interner));

    let mut dictionary = core.clone();
    dictionary.defs[position].is_dict_def = true;
    assert!(!qualifies(&dictionary, &interner));

    let mut anonymous = core.clone();
    anonymous.defs[position].is_anonymous = true;
    assert!(!qualifies(&anonymous, &interner));

    let mut unboxed = core.clone();
    let flux::core::CoreExpr::Lam { params, .. } = &mut unboxed.defs[position].expr else {
        panic!("a function");
    };
    params[0].rep = flux::core::FluxRep::IntRep;
    assert!(!qualifies(&unboxed, &interner));

    let mut named_main = core.clone();
    named_main.defs[position].name = interner.intern("main");
    assert!(!qualifies(&named_main, &interner));
}
