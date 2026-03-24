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
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreLit, CorePrimOp, CoreProgram,
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
fn lowers_option_and_either_constructor_patterns() {
    let src = r#"
fn main() {
    match Some(Left(21)) {
        Some(Left(v)) -> v + 1,
        Some(Right(v)) -> v,
        _ -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @flux_make_adt("));
    assert!(rendered.contains("@flux_adt_tag("));
    assert!(rendered.contains("call fastcc i64 @flux_make_adt(ptr"));
}

#[test]
fn lowers_list_literals_and_list_patterns() {
    let src = r#"
fn sum(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + sum(rest),
    }
}

fn main() {
    sum([1, 2, 3])
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @flux_make_cons(i64 %head, i64 %tail)"));
    assert!(rendered.contains("call fastcc i64 @flux_make_cons("));
    assert!(rendered.contains("call fastcc i64 @sum(i64"));
}

#[test]
fn lowers_tuple_literals_patterns_and_field_access() {
    let src = r#"
fn first(pair) {
    let same = pair.0
    match pair {
        (x, _) -> x + same,
    }
}

fn main() {
    first((10, 2))
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @flux_make_tuple("));
    assert!(rendered.contains("call fastcc i32 @flux_tuple_len("));
    assert!(rendered.contains("call fastcc ptr @flux_tuple_field_ptr("));
}

#[test]
fn lowers_user_defined_adt_nested_patterns() {
    let src = r#"
data ResultI {
    Ok(Int),
    Err(Int),
}

data Wrap {
    Wrap(ResultI),
}

fn main() {
    match Wrap(Ok(3)) {
        Wrap(Ok(v)) -> v,
        Wrap(Err(_)) -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @flux_make_adt("));
    assert!(rendered.contains("@flux_adt_tag("));
    assert!(rendered.contains("call fastcc ptr @flux_adt_field_ptr("));
}

#[test]
fn lowers_multi_constructor_match_as_switch() {
    let src = r#"
data Color {
    Red,
    Green,
    Blue,
}

fn to_int(c) {
    match c {
        Red -> 0,
        Green -> 1,
        Blue -> 2,
    }
}

fn main() {
    to_int(Green)
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    // Should emit a switch instruction, not a chain of icmp+condbr
    assert!(
        rendered.contains("switch i32"),
        "expected switch instruction for multi-constructor match, got:\n{}",
        rendered
    );
    assert!(rendered.contains("switch.arm"));
}

#[test]
fn lowers_none_construction_and_matching() {
    let src = r#"
fn safe_head(xs) {
    match xs {
        [h | _] -> Some(h),
        _ -> None,
    }
}
fn main() {
    match safe_head([]) {
        Some(v) -> v,
        None -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @safe_head("));
    // None is an immediate NaN-box value, not a heap-allocated ADT
    assert!(!rendered.contains("@flux_make_adt(ptr") || rendered.contains("case.none"));
}

#[test]
fn guarded_matches_still_fail_fast() {
    let src = r#"
fn main() {
    match Some(8) {
        Some(v) if v > 5 -> v,
        _ -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let err =
        compile_program_with_interner(&core, Some(&interner)).expect_err("should reject guards");
    assert!(err.to_string().contains("case guards"));
}

#[test]
fn member_access_remains_unsupported() {
    let mut interner = Interner::new();
    let main_name = interner.intern("main");
    let field_name = interner.intern("field");
    let main_binder = CoreBinder::new(CoreBinderId(1), main_name);
    let span = Span::default();
    let core = CoreProgram {
        defs: vec![CoreDef {
            name: main_name,
            binder: main_binder,
            expr: CoreExpr::Lam {
                params: vec![],
                body: Box::new(CoreExpr::PrimOp {
                    op: CorePrimOp::MemberAccess(field_name),
                    args: vec![CoreExpr::Lit(CoreLit::Int(1), span)],
                    span,
                }),
                span,
            },
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span,
        }],
        top_level_items: vec![],
    };

    let err = compile_program(&core).expect_err("should reject member access");
    assert!(err.to_string().contains("MemberAccess"));
}

#[test]
fn emitted_phase5_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let src = r#"
data ResultI {
    Ok(Int),
    Err(Int),
}

fn sum(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + sum(rest),
    }
}

fn main() {
    let pair = (sum([1, 2]), Ok(3))
    match pair {
        (left, Ok(right)) -> left + right,
        (_, Err(_)) -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let ll = render_module(&module);
    let path = std::env::temp_dir().join(format!(
        "core_to_llvm_phase5_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, ll).expect("write ll");
    let output = Command::new("opt")
        .arg("--disable-output")
        .arg("-passes=verify")
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
