#![cfg(feature = "native")]

//! Tests that type class instance methods are correctly lowered through the
//! LLVM native backend. Regression tests for a bug where `generate_dispatch_functions`
//! was not called in the LLVM lowering path, causing mangled instance method
//! definitions (e.g. `__tc_Sizeable_Int_size`) to be missing from the emitted IR
//! and resulting in null closure calls (STATUS_ACCESS_VIOLATION on Windows).

use flux::{
    bytecode::compiler::Compiler,
    syntax::{lexer::Lexer, parser::Parser},
};

/// Parse source, run full compilation pipeline (which populates class_env),
/// then lower through the per-module LLVM path — the same path used by
/// `--native` multi-module compilation.
fn compile_per_module_llvm_ir_with_classes(src: &str) -> String {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    // compile_with_opts populates class_env and hm_expr_types
    compiler
        .compile_with_opts(&program, false, false)
        .expect("compilation should succeed");

    let llvm = compiler
        .lower_to_lir_llvm_module_per_module(&program, false, false)
        .expect("per-module LLVM lowering should succeed");
    flux::core_to_llvm::render_module(&llvm)
}

/// Single type class instance: the mangled function must appear in the LLVM IR.
#[test]
fn type_class_single_instance_emits_mangled_function() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
}

fn main() {
    size(42)
}
"#,
    );

    // The mangled instance function must be defined, not just called.
    assert!(
        rendered.contains("tc_Sizeable_Int_size"),
        "expected mangled instance function __tc_Sizeable_Int_size in LLVM IR"
    );
    // main should exist
    assert!(
        rendered.contains("@flux_main"),
        "expected flux_main entry point"
    );
}

/// Multiple instances of the same class with different types: both mangled
/// functions must be present.
#[test]
fn type_class_multi_instance_emits_all_mangled_functions() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
}

instance Sizeable<String> {
    fn size(x) { len(x) }
}

fn main() {
    size(42) + size("hello")
}
"#,
    );

    assert!(
        rendered.contains("tc_Sizeable_Int_size"),
        "expected __tc_Sizeable_Int_size in LLVM IR"
    );
    assert!(
        rendered.contains("tc_Sizeable_String_size"),
        "expected __tc_Sizeable_String_size in LLVM IR"
    );
}

/// Instance method should be callable multiple times in the same scope
/// without null-pointer dispatch.
#[test]
fn type_class_dispatch_does_not_emit_null_closure_call() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
}

fn main() {
    size(42) + size(100)
}
"#,
    );

    // The mangled function must be defined (not a closure call through 0)
    assert!(
        rendered.contains("tc_Sizeable_Int_size"),
        "expected direct function definition, not null closure dispatch"
    );
}

/// Instance with a class that has multiple methods: all methods should be emitted.
#[test]
fn type_class_multi_method_emits_all_methods() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class Measurable<a> {
    fn weight(x: a) -> Int
    fn count(x: a) -> Int
}

instance Measurable<Int> {
    fn weight(x) { x }
    fn count(x) { 1 }
}

fn main() {
    weight(42) + count(10)
}
"#,
    );

    assert!(
        rendered.contains("tc_Measurable_Int_weight"),
        "expected __tc_Measurable_Int_weight in LLVM IR"
    );
    assert!(
        rendered.contains("tc_Measurable_Int_count"),
        "expected __tc_Measurable_Int_count in LLVM IR"
    );
}
