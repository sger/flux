#![cfg(feature = "native")]

//! Tests that type class instance methods are correctly lowered through the
//! LLVM native backend. Regression tests for a bug where `generate_dispatch_functions`
//! was not called in the LLVM lowering path, causing mangled instance method
//! definitions (e.g. `__tc_Sizeable_Int_size`) to be missing from the emitted IR
//! and resulting in null closure calls (STATUS_ACCESS_VIOLATION on Windows).

use flux::{
    ast::type_infer::constraint::SchemeConstraint,
    bytecode::compiler::Compiler,
    syntax::{effect_expr::EffectExpr, lexer::Lexer, parser::Parser, type_expr::TypeExpr},
    types::{infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme},
    types::module_interface::{
        ModuleInterface, PublicClassEntry, PublicClassMethodEntry, PublicInstanceEntry,
        PublicInstanceMethodEntry,
    },
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

fn compile_per_module_llvm_ir_with_preloaded_interfaces(
    src: &str,
    interfaces: &[ModuleInterface],
) -> String {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    for interface in interfaces {
        compiler.preload_module_interface(interface);
    }

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

#[test]
fn contextual_type_class_instance_emits_mangled_method_through_native_lowering() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Bool
}

instance Eq<a> => MyEq<List<a>> {
    fn my_eq(xs, ys) {
        match xs {
            [h1 | _] -> match ys {
                [h2 | _] -> h1 == h2,
                _ -> false
            },
            _ -> true
        }
    }
}

fn main() {
    my_eq([1], [1])
}
"#,
    );

    assert!(
        rendered.contains("tc_MyEq_List"),
        "expected contextual mangled method in LLVM IR"
    );
}

#[test]
fn multi_parameter_type_class_method_preserves_full_head_in_native_lowering() {
    let rendered = compile_per_module_llvm_ir_with_classes(
        r#"
class Convert<a, b> {
    fn convert(x: a) -> b
}

instance Convert<Int, String> {
    fn convert(x) { "ok" }
}

fn main() {
    convert(42)
}
"#,
    );

    assert!(
        rendered.contains("tc_Convert_Int_String_convert"),
        "expected full multi-parameter mangled instance symbol in LLVM IR"
    );
}

#[test]
fn imported_public_instance_method_is_emitted_and_called_directly_in_native_lowering() {
    let mut interner = flux::syntax::interner::Interner::new();
    let logger_h = interner.intern("h");
    let log_name = interner.intern("log");
    let string_name = interner.intern("String");
    let unit_name = interner.intern("Unit");
    let int_name = interner.intern("Int");
    let console_name = interner.intern("Console");

    let class_interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
        cache_format_version: flux::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::from([(
            "log".to_string(),
            Scheme {
                forall: vec![0],
                constraints: vec![SchemeConstraint {
                    class_name: interner.intern("Logger"),
                    type_vars: vec![0],
                }],
                infer_type: InferType::Fun(
                    vec![InferType::Var(0), InferType::Con(flux::types::type_constructor::TypeConstructor::String)],
                    Box::new(InferType::Con(flux::types::type_constructor::TypeConstructor::Unit)),
                    InferEffectRow::closed_empty(),
                ),
            },
        )]),
        borrow_signatures: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: vec![PublicClassEntry {
            class_module: "Example.Logger".to_string(),
            name: "Logger".to_string(),
            type_param_arity: 1,
            type_params: vec![logger_h],
            superclasses: vec![],
            methods: vec![PublicClassMethodEntry {
                name: log_name,
                type_params: vec![],
                param_types: vec![
                    TypeExpr::Named {
                        name: logger_h,
                        args: vec![],
                        span: Default::default(),
                    },
                    TypeExpr::Named {
                        name: string_name,
                        args: vec![],
                        span: Default::default(),
                    },
                ],
                return_type: TypeExpr::Named {
                    name: unit_name,
                    args: vec![],
                    span: Default::default(),
                },
                effects: vec![],
            }],
            default_methods: vec![],
            method_names: vec!["log".to_string()],
            pinned_row_placeholder: None,
        }],
        public_instances: Vec::new(),
    };
    let instance_interface = ModuleInterface {
        module_name: "Example.StdLog".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
        cache_format_version: flux::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: Vec::new(),
        public_instances: vec![PublicInstanceEntry {
            class_module: "Example.Logger".to_string(),
            class_name: "Logger".to_string(),
            instance_module: "Example.StdLog".to_string(),
            head_type_repr: "Int".to_string(),
            type_args: vec![TypeExpr::Named {
                name: int_name,
                args: vec![],
                span: Default::default(),
            }],
            context: vec![],
            methods: vec![PublicInstanceMethodEntry {
                name: log_name,
                effects: vec![EffectExpr::Named {
                    name: console_name,
                    span: Default::default(),
                }],
            }],
            pinned_row_placeholder: None,
        }],
    };

    let rendered = compile_per_module_llvm_ir_with_preloaded_interfaces(
        r#"
import Example.Logger exposing (log)
import Example.StdLog as StdLog

effect Console {
    print: String -> Unit
}

fn main() with Console {
    log(1, "x")
}
"#,
        &[class_interface, instance_interface],
    );

    assert!(
        !rendered.contains("hamt_get_option"),
        "expected direct imported instance call, not dynamic member lookup"
    );
}
