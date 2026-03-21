#![cfg(all(feature = "jit", feature = "llvm"))]

use std::collections::HashMap;

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::core::{CoreExpr, lower_ast::lower_program_ast, passes::run_core_passes_with_interner};
use flux::diagnostics::render_diagnostics;
use flux::jit::{JitOptions, jit_compile_and_run};
use flux::llvm::{LlvmOptions, llvm_compile_and_run};
use flux::runtime::value::Value;
use flux::syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser};
use flux::types::infer_type::InferType;

#[derive(Clone, Copy)]
enum CaseKind {
    ExactListReuse,
    BlockedListReuse,
    BranchyList,
    NamedAdtReuse,
    BranchyTreeDropSpec,
    QueueReuse,
    HigherOrderBorrowed,
    ForwardedWrapperReuse,
    ForwardedWrapperNearMiss,
    BaseLenTraversal,
}

fn parse_and_infer(
    src: &str,
) -> (
    flux::syntax::program::Program,
    HashMap<ExprId, InferType>,
    Interner,
) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<gen>", interner.clone());
    let hm_expr_types = compiler.infer_expr_types_for_program(&program);
    (program, hm_expr_types, interner)
}

fn lowered_core(src: &str) -> flux::core::CoreProgram {
    let (program, types, interner) = parse_and_infer(src);
    let mut core = lower_program_ast(&program, &types);
    run_core_passes_with_interner(&mut core, &interner).expect("core passes should succeed");
    core
}

fn collect_core_exprs(expr: &CoreExpr) -> Vec<&CoreExpr> {
    let mut out = vec![expr];
    match expr {
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            out.extend(collect_core_exprs(body))
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            out.extend(collect_core_exprs(func));
            for arg in args {
                out.extend(collect_core_exprs(arg));
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
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                out.extend(collect_core_exprs(field));
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for arg in args {
                out.extend(collect_core_exprs(arg));
            }
        }
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
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => {}
    }
    out
}

fn run_vm(src: &str) -> Value {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<gen>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(src), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().expect("vm run should succeed");
    vm.last_popped_stack_elem().clone()
}

fn run_jit(src: &str) -> Value {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let (value, _) =
        jit_compile_and_run(&program, &interner, &JitOptions::default()).expect("jit run ok");
    value
}

fn run_llvm(src: &str) -> Value {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let (value, _) = llvm_compile_and_run(
        &program,
        &interner,
        &LlvmOptions {
            source_file: Some("<gen>".to_string()),
            source_text: Some(src.to_string()),
            opt_level: 0,
        },
    )
    .expect("llvm run ok");
    value
}

fn generate_case(seed: u32) -> (String, bool, bool, bool) {
    let case_kind = match seed % 10 {
        0 => CaseKind::ExactListReuse,
        1 => CaseKind::BlockedListReuse,
        2 => CaseKind::BranchyList,
        3 => CaseKind::NamedAdtReuse,
        4 => CaseKind::BranchyTreeDropSpec,
        5 => CaseKind::QueueReuse,
        6 => CaseKind::HigherOrderBorrowed,
        7 => CaseKind::ForwardedWrapperReuse,
        8 => CaseKind::ForwardedWrapperNearMiss,
        _ => CaseKind::BaseLenTraversal,
    };
    let n = 2 + (seed % 4);
    let choose = if seed % 2 == 0 { "true" } else { "false" };

    let src = match case_kind {
        CaseKind::ExactListReuse => format!(
            r#"
fn rebuild(xs) {{
    match xs {{
        [h | t] -> [h + {n} | t],
        _ -> [],
    }}
}}
fn main() {{
    rebuild([1, 2, 3, 4])
}}
"#
        ),
        CaseKind::BlockedListReuse => format!(
            r#"
fn touch_token(token, value) {{ value }}
fn rebuild(xs) {{
    match xs {{
        [h | t] -> [touch_token(xs, h + {n}) | t],
        _ -> [],
    }}
}}
fn main() {{
    rebuild([1, 2, 3, 4])
}}
"#
        ),
        CaseKind::BranchyList => format!(
            r#"
fn branchy(xs, choose) {{
    match xs {{
        [h | t] -> if choose {{ [h + {n} | t] }} else {{ t }},
        _ -> [],
    }}
}}
fn main() {{
    branchy([1, 2, 3, 4], {choose})
}}
"#
        ),
        CaseKind::NamedAdtReuse => format!(
            r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)
fn set_black(t) {{
    match t {{
        Node(_, left, key, right) -> Node(Black, left, key, right),
        _ -> t,
    }}
}}
fn main() {{
    set_black(Node(Red, Leaf, {n}, Leaf))
}}
"#
        ),
        CaseKind::BranchyTreeDropSpec => format!(
            r#"
type Color = Red | Black
type Tree = Leaf | Node(Color, Tree, Int, Tree)
fn keep_or_dup_left(t, keep) {{
    match t {{
        Node(color, left, key, right) ->
            if keep {{ Node(color, left, key, right) }}
            else {{ Node(color, left, key, left) }},
        _ -> t,
    }}
}}
fn main() {{
    keep_or_dup_left(Node(Red, Leaf, {n}, Leaf), {choose})
}}
"#
        ),
        CaseKind::QueueReuse => format!(
            r#"
type Queue<T> = Queue(List<T>, List<T>)
fn push(q, value) {{
    match q {{
        Queue(front, back) -> Queue(front, [value | back]),
    }}
}}
fn main() {{
    push(Queue([1, 2], [3]), {n})
}}
"#
        ),
        CaseKind::HigherOrderBorrowed => format!(
            r#"
fn sum_by(xs, f) {{
    match xs {{
        [h | t] -> f(h) + sum_by(t, f),
        _ -> 0,
    }}
}}
fn inc(x) {{ x + {n} }}
fn main() {{
    sum_by([1, 2, 3, 4], inc)
}}
"#
        ),
        CaseKind::ForwardedWrapperReuse => format!(
            r#"
type Pair2<T> = Pair2(T, T)
fn forwarded(xs, acc) {{
    match xs {{
        [y | ys] -> Pair2([y | acc], ys),
        _ -> Pair2(xs, acc),
    }}
}}
fn main() {{
    forwarded([1, 2, 3, 4], [{n}, {n} + 1])
}}
"#
        ),
        CaseKind::ForwardedWrapperNearMiss => format!(
            r#"
type Pair2<T> = Pair2(T, T)
fn forwarded(xs, acc) {{
    match xs {{
        [y | ys] -> Pair2([y | acc], [y | acc]),
        _ -> Pair2(xs, acc),
    }}
}}
fn main() {{
    forwarded([1, 2, 3, 4], [{n}, {n} + 1])
}}
"#
        ),
        CaseKind::BaseLenTraversal => format!(
            r#"
fn walk(xs) {{
    match xs {{
        [_ | t] -> len(t) + walk(t),
        _ -> 0,
    }}
}}
fn main() {{
    walk([1, 2, 3, 4])
}}
"#
        ),
    };

    let expect_reuse = matches!(
        case_kind,
        CaseKind::ExactListReuse
            | CaseKind::BranchyList
            | CaseKind::NamedAdtReuse
            | CaseKind::BranchyTreeDropSpec
            | CaseKind::QueueReuse
            | CaseKind::ForwardedWrapperReuse
    );
    let expect_dropspec = matches!(case_kind, CaseKind::BranchyTreeDropSpec);
    let expect_borrowed_call =
        matches!(case_kind, CaseKind::HigherOrderBorrowed | CaseKind::QueueReuse);
    (src, expect_reuse, expect_dropspec, expect_borrowed_call)
}

#[test]
fn generated_aether_heavy_programs_keep_vm_jit_llvm_in_parity() {
    for seed in 0..20u32 {
        let (src, expect_reuse, expect_dropspec, expect_borrowed_call) = generate_case(seed);
        let core = lowered_core(&src);
        let exprs = core
            .defs
            .iter()
            .flat_map(|def| collect_core_exprs(&def.expr))
            .collect::<Vec<_>>();

        let reuse_count = exprs
            .iter()
            .filter(|expr| matches!(expr, CoreExpr::Reuse { .. }))
            .count();
        let dropspec_count = exprs
            .iter()
            .filter(|expr| matches!(expr, CoreExpr::DropSpecialized { .. }))
            .count();
        let borrowed_call_count = exprs
            .iter()
            .filter(|expr| {
                matches!(
                    expr,
                    CoreExpr::AetherCall { arg_modes, .. }
                        if arg_modes
                            .iter()
                            .any(|mode| *mode == flux::aether::borrow_infer::BorrowMode::Borrowed)
                )
            })
            .count();

        if expect_reuse {
            assert!(
                reuse_count >= 1,
                "seed {seed} should emit at least one Reuse\n{src}"
            );
        } else {
            assert_eq!(reuse_count, 0, "seed {seed} should stay non-reusing\n{src}");
        }
        if expect_dropspec {
            assert!(
                dropspec_count >= 1,
                "seed {seed} should emit DropSpecialized\n{src}"
            );
        }
        if expect_borrowed_call {
            assert!(
                borrowed_call_count >= 1,
                "seed {seed} should preserve a borrowed call mode\n{src}"
            );
        }

        let vm = run_vm(&src);
        let jit = run_jit(&src);
        let llvm = run_llvm(&src);
        assert_eq!(vm, jit, "VM/JIT mismatch for seed {seed}\n{src}");
        assert_eq!(vm, llvm, "VM/LLVM mismatch for seed {seed}\n{src}");
    }
}
