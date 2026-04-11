#![cfg(feature = "native")]

use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    lir::{emit_llvm::emit_llvm_ir, lower::lower_program_with_interner},
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

fn compile_to_llvm_ir(src: &str) -> String {
    let (core, interner) = parse_and_lower_core(src);
    let lir = lower_program_with_interner(&core, Some(&interner), None);
    emit_llvm_ir(&lir)
}

fn compile_per_module_llvm_ir(src: &str, export_user_ctor_name_helper: bool) -> String {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let llvm = compiler
        .lower_to_lir_llvm_module_per_module(&program, false, export_user_ctor_name_helper)
        .expect("per-module lowering should succeed");
    flux::core_to_llvm::render_module(&llvm)
}

#[test]
fn lowers_option_and_either_constructor_patterns() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main() {
    match Some(Left(21)) {
        Some(Left(v)) -> v + 1,
        Some(Right(v)) -> v,
        _ -> 0,
    }
}
"#,
    );

    assert!(
        rendered.contains("flux_make_adt") || rendered.contains("flux_make_cons"),
        "expected ADT construction"
    );
    // Phase 4 (Proposal 0140): ADT tag check is now inlined as GEP+load.
    assert!(
        rendered.contains("getelementptr inbounds %FluxAdt"),
        "expected inline ADT tag extraction"
    );
}

#[test]
fn lowers_list_literals_and_list_patterns() {
    let rendered = compile_to_llvm_ir(
        r#"
fn sum(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + sum(rest),
    }
}

fn main() {
    sum([1, 2, 3])
}
"#,
    );

    assert!(
        rendered.contains("flux_make_cons"),
        "expected cons construction"
    );
}

#[test]
fn lowers_tuple_literals_patterns_and_field_access() {
    let rendered = compile_to_llvm_ir(
        r#"
fn first(pair) {
    let same = pair.0
    match pair {
        (x, _) -> x + same,
    }
}

fn main() {
    first((10, 2))
}
"#,
    );

    assert!(
        rendered.contains("flux_make_tuple"),
        "expected tuple construction"
    );
}

#[test]
fn lowers_user_defined_adt_nested_patterns() {
    let rendered = compile_to_llvm_ir(
        r#"
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
"#,
    );

    // Phase 4 (Proposal 0140): ADT tag/field access is now inlined as GEP+load.
    assert!(
        rendered.contains("getelementptr inbounds %FluxAdt"),
        "expected inline ADT tag/field extraction"
    );
}

#[test]
fn lowers_multi_constructor_match_as_switch() {
    let rendered = compile_to_llvm_ir(
        r#"
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
"#,
    );

    assert!(
        rendered.contains("switch i32"),
        "expected switch instruction for multi-constructor match"
    );
}

#[test]
fn per_module_adt_owner_emits_ctor_name_helper_without_main() {
    let rendered = compile_per_module_llvm_ir(
        r#"
module Colors {
    data Color {
        Red,
        Blue,
    }

    public fn pick() {
        Red
    }
}
"#,
        true,
    );

    assert!(
        rendered.contains("@flux_user_ctor_name"),
        "expected ctor-name helper in per-module LLVM output"
    );
}

#[test]
fn lowers_none_construction_and_matching() {
    let rendered = compile_to_llvm_ir(
        r#"
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
"#,
    );

    assert!(
        rendered.contains("flux_safe_head") || rendered.contains("@flux_main"),
        "expected function in output"
    );
}

#[test]
fn emitted_phase5_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let ll = compile_to_llvm_ir(
        r#"
fn sum(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + sum(rest),
    }
}

fn main() {
    sum([1, 2, 3])
}
"#,
    );
    let path = std::env::temp_dir().join(format!(
        "core_to_llvm_phase5_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, &ll).expect("write ll");
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
