#![cfg(feature = "llvm")]

use crate::support::scratch;
use scratch::Scratch;

use std::{collections::HashMap, fs, process::Command};

use flux::{
    compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    lir::{self, emit_llvm::emit_llvm_ir, lower::lower_program_with_interner},
    llvm::render_module,
    syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser},
    types::infer_type::InferType,
};

fn parse_and_lower_core(src: &str) -> (flux::core::CoreProgram, Interner) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
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
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let llvm = compiler
        .lower_to_lir_llvm_module_per_module(&program, false, export_user_ctor_name_helper, true)
        .expect("per-module lowering should succeed");
    flux::llvm::render_module(&llvm)
}

#[test]
fn lowers_option_and_either_constructor_patterns() {
    let rendered = compile_to_llvm_ir(
        r#"
fn read(x) {
    match x {
        Some(Left(v)) -> v + 1,
        Some(Right(v)) -> v,
        _ -> 0,
    }
}

fn main() {
    read(Some(Left(read(None))))
}
"#,
    );

    assert!(
        rendered.contains("@flux_bump_alloc_filled(i32 16, i32 1, i32 242)"),
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
        rendered.contains("@flux_bump_alloc_filled(i32 24, i32 2, i32 242)"),
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
        rendered.contains("@flux_bump_alloc_filled(i32 24, i32 2, i32 243)"),
        "expected tuple construction"
    );
}

#[test]
fn tuple_pattern_dispatch_guards_pointer_tagged_sentinels() {
    let rendered = compile_to_llvm_ir(
        r#"
fn read_pair(pair) {
    match pair {
        (x, _) -> x,
        _ -> 0,
    }
}

fn main() {
    read_pair((10, 2))
}
"#,
    );

    assert!(
        rendered.contains("icmp ule i64 12"),
        "tuple dispatch must guard pointer-tagged sentinels before dereference"
    );
    assert!(rendered.contains("match.ptrck"), "expected a dedicated tuple pointer-check block");
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

/// A nullary constructor is one read-only object with a sticky count, shared
/// by every use, so building it allocates nothing.
#[test]
fn nullary_constructors_are_static_objects() {
    let rendered = compile_to_llvm_ir(
        r#"
data Color {
    Red,
    Green,
    Blue,
}

fn pick(n) {
    if n > 0 { Red } else { Green }
}

fn main() {
    pick(1)
}
"#,
    );

    let statics: Vec<&str> =
        rendered.lines().filter(|line| line.starts_with("@flux.ctor.")).collect();
    assert_eq!(statics.len(), 2, "one static object each for Red and Green:\n{rendered}");
    for line in statics {
        assert!(line.contains("private constant"), "{line}");
        assert!(line.contains("i32 1073741824"), "{line}");
    }
    assert!(!rendered.contains("@flux_make_adt(ptr null"), "{rendered}");
}

/// Fields passed to a helper through a stack buffer kept the buffer and a
/// copy in optimized code wherever the constructor was not in the function's
/// first block, so each one is stored straight into the new block.
#[test]
fn constructor_and_tuple_fields_are_stored_straight_into_the_block() {
    let rendered = compile_to_llvm_ir(
        r#"
data Tree {
    Node(Tree, Int, Tree),
    Tip,
}

fn node(n) { Node(Tip, n, Tip) }
fn some(n) { Some(n) }
fn right(n) { Right(n) }
fn list(n) { [n, n + 1] }
fn pair(n) { (n, n + 1) }

fn main() {
    let a = node(1);
    let b = some(2);
    let c = right(3);
    let d = list(4);
    let e = pair(5);
    0
}
"#,
    );

    let body = |name: &str| -> &str {
        let define = rendered
            .lines()
            .find(|line| line.starts_with("define") && line.contains(&format!("_{name}(")))
            .unwrap_or_else(|| panic!("no function {name}:\n{rendered}"));
        let start = rendered.find(define).unwrap();
        &rendered[start..start + rendered[start..].find("\n}").unwrap()]
    };
    // (function, block bytes, scanned fields, object tag, stores of i64 fields)
    for (name, bytes, fields, obj_tag) in [
        ("node", 32, 3, 0xF2),
        ("some", 16, 1, 0xF2),
        ("right", 16, 1, 0xF2),
        ("list", 24, 2, 0xF2),
        ("pair", 24, 2, 0xF3),
    ] {
        let body = body(name);
        assert!(!body.contains("alloca"), "{name}:\n{body}");
        for helper in [
            "@flux_make_adt(",
            "@flux_make_cons(",
            "@flux_make_tuple(",
            "@flux_wrap_some(",
            "@flux_make_right(",
        ] {
            assert!(!body.contains(helper), "{name} calls {helper}:\n{body}");
        }
        let alloc = format!("@flux_bump_alloc_filled(i32 {bytes}, i32 {fields}, i32 {obj_tag})");
        assert!(body.contains(&alloc), "{name} has no {alloc}:\n{body}");
        let stores = body.matches("store i64 ").count();
        assert!(stores >= fields as usize, "{name} stores {stores} words:\n{body}");
    }
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
fn emitted_module_verifies_with_opt_when_available() {
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
    let scratch = Scratch::new("llvm_phase5");
    let path = scratch.join("module.ll");
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

// ---------------------------------------------------------------------------
// Snapshot coverage (previously tests/llvm_adt_snapshots.rs)
// ---------------------------------------------------------------------------

fn parse_and_lower_to_llvm_module(src: &str) -> (flux::llvm::LlvmModule, Interner) {
    let (core, interner) = parse_and_lower_core(src);
    let lir_program = lir::lower::lower_program_with_interner(&core, Some(&interner), None);
    let module = lir::emit_llvm::emit_llvm_module(&lir_program);
    (module, interner)
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the snapshot records POSIX function attributes; Windows keeps Flux functions nounwind"
)]
fn snapshot_for_adt_list_and_tuple_lowering() {
    let src = r#"
data Wrap {
    Wrap(Int),
    Empty,
}

fn fold(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + fold(rest),
    }
}

fn main() {
    let value = (fold([1, 2]), Wrap(3))
    match value {
        (left, Wrap(right)) -> left + right,
        (_, Empty) -> 0,
    }
}
"#;
    let (module, _interner) = parse_and_lower_to_llvm_module(src);

    insta::with_settings!({
        snapshot_path => "../snapshots/llvm",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("adt_list_tuple_module", render_module(&module));
    });
}
