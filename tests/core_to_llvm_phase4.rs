#![cfg(feature = "core_to_llvm")]

use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreLit, CoreProgram,
        lower_ast::lower_program_ast, passes::run_core_passes_with_interner,
    },
    core_to_llvm::{compile_program, compile_program_with_interner, render_module},
    diagnostics::position::Span,
    syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser},
    types::infer_type::InferType,
};

fn parse_and_lower_core(src: &str) -> (flux::core::CoreProgram, Interner) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner.clone());
    let hm_expr_types: HashMap<ExprId, InferType> = compiler.infer_expr_types_for_program(&program);
    let mut core = lower_program_ast(&program, &hm_expr_types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    (core, interner)
}

#[test]
fn lowers_lambda_expression_and_indirect_call() {
    let src = r#"
fn main() {
    let f = \x -> x + 1
    f(10)
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("call fastcc i64 @flux_make_closure("));
    assert!(rendered.contains("call fastcc i64 @flux_call_closure("));
    assert!(rendered.contains(".lambda."));
}

#[test]
fn lowers_top_level_function_value_via_wrapper_closure() {
    let src = r#"
fn add1(x) { x + 1 }
fn apply(f, x) { f(x) }
fn main() { apply(add1, 10) }
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("@add1.closure_wrapper"));
    assert!(rendered.contains("call fastcc i64 @flux_make_closure(ptr @add1.closure_wrapper"));
    assert!(rendered.contains("define internal fastcc i64 @apply(i64 %arg0, i64 %arg1)"));
    assert!(rendered.contains("call fastcc i64 @flux_call_closure("));
}

#[test]
fn lowers_returned_closure_chain() {
    let src = r#"
fn make_adder(n) {
    \x -> x + n
}

fn main() {
    make_adder(5)(10)
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.matches(".lambda.").count() >= 1);
    assert!(rendered.contains("call fastcc i64 @flux_make_closure("));
    assert!(
        rendered
            .matches("call fastcc i64 @flux_call_closure")
            .count()
            >= 1
    );
}

#[test]
fn lowers_recursive_local_closure_from_handwritten_core() {
    let mut interner = Interner::new();
    let main_name = interner.intern("main");
    let loop_name = interner.intern("loop");
    let x_name = interner.intern("x");

    let loop_binder = CoreBinder::new(CoreBinderId(1), loop_name);
    let x_binder = CoreBinder::new(CoreBinderId(2), x_name);
    let main_binder = CoreBinder::new(CoreBinderId(3), main_name);
    let span = Span::default();

    let loop_body = CoreExpr::App {
        func: Box::new(CoreExpr::bound_var(loop_binder, span)),
        args: vec![CoreExpr::Lit(CoreLit::Int(0), span)],
        span,
    };
    let loop_lam = CoreExpr::Lam {
        params: vec![x_binder],
        body: Box::new(loop_body),
        span,
    };
    let main_expr = CoreExpr::Lam {
        params: vec![],
        body: Box::new(CoreExpr::LetRec {
            var: loop_binder,
            rhs: Box::new(loop_lam),
            body: Box::new(CoreExpr::App {
                func: Box::new(CoreExpr::bound_var(loop_binder, span)),
                args: vec![CoreExpr::Lit(CoreLit::Int(1), span)],
                span,
            }),
            span,
        }),
        span,
    };

    let core = CoreProgram {
        defs: vec![CoreDef {
            name: main_name,
            binder: main_binder,
            expr: main_expr,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span,
        }],
        top_level_items: vec![],
    };

    let module = compile_program(&core).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("call fastcc i64 @flux_tag_boxed_ptr(ptr %closure)"));
    assert!(rendered.contains("call fastcc i64 @flux_call_closure("));
    assert!(rendered.contains(".lambda."));
}

#[test]
fn emitted_closure_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let src = r#"
fn make_adder(n) {
    \x -> x + n
}

fn main() {
    make_adder(5)(10)
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let ll = render_module(&module);
    let path = std::env::temp_dir().join(format!(
        "core_to_llvm_phase4_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, ll).expect("write ll");
    let output = Command::new("opt")
        .arg("--disable-output")
        .arg("--verify")
        .arg(&path)
        .output()
        .expect("run opt");
    let _ = fs::remove_file(&path);
    assert!(
        output.status.success(),
        "opt verify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
