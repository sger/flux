use crate::{
    bytecode::op_code::{OpCode, disassemble},
    compiler::Compiler,
    diagnostics::render_diagnostics,
    runtime::value::Value,
    syntax::{
        effect_expr::EffectExpr, interner::Interner, lexer::Lexer, parser::Parser,
        statement::Statement, type_expr::TypeExpr,
    },
    types::{
        infer_effect_row::InferEffectRow,
        infer_type::InferType,
        module_interface::{
            ModuleInterface, PublicClassEntry, PublicClassMethodEntry, PublicInstanceEntry,
            PublicInstanceMethodEntry,
        },
        scheme::Scheme,
        type_constructor::TypeConstructor,
    },
};

fn parse_program(source: &str) -> (crate::syntax::program::Program, Interner) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn parse_program_with_interner(
    source: &str,
    interner: Interner,
) -> (crate::syntax::program::Program, Interner) {
    let lexer = Lexer::new_with_interner(source, interner);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn top_level_has_function(statements: &[Statement], name: &str, interner: &Interner) -> bool {
    statements.iter().any(|statement| match statement {
        Statement::Function { name: sym, .. } => interner.try_resolve(*sym) == Some(name),
        Statement::Module { body, .. } => top_level_has_function(&body.statements, name, interner),
        _ => false,
    })
}

fn find_compiled_function(
    constants: &[Value],
    name: &str,
) -> Option<std::sync::Arc<crate::runtime::compiled_function::CompiledFunction>> {
    constants.iter().find_map(|value| match value {
        Value::Function(function)
            if function
                .debug_info
                .as_ref()
                .and_then(|debug| debug.name.as_deref())
                == Some(name) =>
        {
            Some(function.clone())
        }
        _ => None,
    })
}

fn compile_function_asm(source: &str, name: &str) -> String {
    let (program, interner) = parse_program(source);
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(source), None)));

    let function = find_compiled_function(&compiler.bytecode().constants, name)
        .unwrap_or_else(|| panic!("expected compiled function constant for {name}"));
    disassemble(&function.instructions)
}

#[test]
fn compile_integer_literals_emits_constants() {
    let (program, interner) = parse_program("1; 2;");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(bytecode.constants.contains(&Value::Integer(1)));
    assert!(bytecode.constants.contains(&Value::Integer(2)));
}

#[test]
fn compile_string_literal_emits_constant() {
    let (program, interner) = parse_program("\"hello\";");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(
        bytecode
            .constants
            .contains(&Value::String("hello".to_string().into()))
    );
}

#[test]
fn compile_function_decl_emits_function_constant() {
    let (program, interner) = parse_program("fn add(x, y) { return x + y; }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(
        bytecode
            .constants
            .iter()
            .any(|obj| matches!(obj, Value::Function(_)))
    );
}

#[test]
fn compile_with_opts_collects_program_free_vars_when_optimized() {
    let (program, interner) = parse_program(
        r#"
let x = 1;
let f = fn() { x + y; };
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let _ = compiler.compile_with_opts(&program, true, true);

    let x = compiler.interner.intern("x");
    let y = compiler.interner.intern("y");

    assert!(compiler.free_vars.contains(&y));
    assert!(!compiler.free_vars.contains(&x));
}

#[test]
fn compile_with_opts_skips_free_var_collection_without_optimization() {
    let (program, interner) = parse_program("let f = fn() { missing; };");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let _ = compiler.compile_with_opts(&program, false, false);
    assert!(compiler.free_vars.is_empty());
}

#[test]
fn compile_with_opts_collects_tail_calls_when_optimized() {
    let (program, interner) = parse_program("fn f(n) { f(n - 1) }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, true, true).unwrap();
    assert_eq!(compiler.tail_calls.len(), 1);
}

#[test]
fn final_compile_suppresses_expression_e430_when_specific_errors_exist() {
    let (program, interner) = parse_program("fn f(x) { mystery }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("expected compile failure");

    let e430_count = diags.iter().filter(|d| d.code() == Some("E430")).count();
    let e004_count = diags.iter().filter(|d| d.code() == Some("E004")).count();
    assert_eq!(
        e430_count, 1,
        "expected only binding-level E430, got: {diags:?}"
    );
    assert_eq!(
        e004_count, 1,
        "expected unresolved-name E004, got: {diags:?}"
    );
    assert!(
        !diags.iter().any(|d| {
            d.code() == Some("E430")
                && d.message().is_some_and(|msg| {
                    msg.starts_with("Could not determine a concrete type for this expression.")
                })
        }),
        "expression-level E430 should be suppressed once specific compiler errors exist: {diags:?}"
    );
}

#[test]
fn final_compile_preserves_specific_effect_errors_over_expression_e430() {
    let (program, interner) = parse_program("perform Missing.print(\"hello\")");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("expected compile failure");

    assert!(
        diags.iter().any(|d| d.code() == Some("E403")),
        "expected unknown effect error, got: {diags:?}"
    );
    assert!(
        !diags.iter().any(|d| {
            d.code() == Some("E430")
                && d.message().is_some_and(|msg| {
                    msg.starts_with("Could not determine a concrete type for this expression.")
                })
        }),
        "expression-level E430 should not survive alongside specific effect errors: {diags:?}"
    );
}

#[test]
fn final_compile_suppresses_expression_e430_when_hm_reports_specific_error() {
    let (program, interner) = parse_program(
        r#"
fn main() {
    [|1, "x"|]
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("expected compile failure");

    assert!(
        diags.iter().any(|d| d.code() == Some("E300")),
        "expected heterogeneous-array type mismatch, got: {diags:?}"
    );
    assert_eq!(
        diags.iter().filter(|d| d.code() == Some("E430")).count(),
        1,
        "expected only the binding-level E430 summary, got: {diags:?}"
    );
    assert!(
        !diags.iter().any(|d| {
            d.code() == Some("E430")
                && d.message().is_some_and(|msg| {
                    msg.starts_with("Could not determine a concrete type for this expression.")
                })
        }),
        "expression-level E430 should be suppressed when specific HM errors exist: {diags:?}"
    );
}

#[test]
fn compile_with_opts_handles_cfg_lowered_option_match_function() {
    let (program, interner) = parse_program("fn f(x) { match x { Some(n) -> n, None -> 0 } }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, true, true).unwrap();

    let function = find_compiled_function(&compiler.bytecode().constants, "f")
        .expect("expected compiled function constant for f");

    let instructions = &function.instructions;
    assert!(
        instructions.contains(&(OpCode::OpIsSome as u8)),
        "expected option-test opcode in compiled function"
    );
    assert!(
        instructions.contains(&(OpCode::OpUnwrapSome as u8)),
        "expected option-payload opcode in compiled function"
    );
}

#[test]
fn compile_with_opts_handles_cfg_lowered_constructor_match_function() {
    let (program, interner) = parse_program(
        "data MaybeInt { SomeInt(Int), NoneInt }\nfn f(x) { match x { SomeInt(n) -> n, NoneInt -> 0 } }",
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, true, true).unwrap();

    let function = find_compiled_function(&compiler.bytecode().constants, "f")
        .expect("expected compiled function constant for f");

    let instructions = &function.instructions;
    assert!(
        instructions.contains(&(OpCode::OpIsAdtJump as u8))
            || instructions.contains(&(OpCode::OpIsAdtJumpLocal as u8)),
        "expected constructor-test opcode in compiled function"
    );
    assert!(
        instructions.contains(&(OpCode::OpAdtField as u8)),
        "expected constructor-field opcode in compiled function"
    );
}

#[test]
fn compile_with_opts_handles_cfg_lowered_tuple_match_function() {
    let (program, interner) = parse_program("fn f(x) { match x { (a, b) -> a, _ -> 0 } }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, true, true).unwrap();

    let function = find_compiled_function(&compiler.bytecode().constants, "f")
        .expect("expected compiled function constant for f");

    let instructions = &function.instructions;
    assert!(
        instructions.contains(&(OpCode::OpIsTuple as u8)),
        "expected tuple-test opcode in compiled function"
    );
    assert!(
        instructions.contains(&(OpCode::OpTupleIndex as u8)),
        "expected tuple-field opcode in compiled function"
    );
}

#[test]
fn compile_with_opts_skips_tail_call_analysis_without_optimization() {
    let (program, interner) = parse_program("fn f(n) { f(n - 1) }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, false, false).unwrap();
    assert!(compiler.tail_calls.is_empty());
}

#[test]
fn infer_expr_types_for_program_stores_owned_program_after_0165_preparation() {
    let (program, interner) = parse_program("fn f() { 1 }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    let prepared = compiler.prepare_program_for_lowering(&program);

    assert!(
        matches!(prepared.effective_program, std::borrow::Cow::Owned(_)),
        "expected owned final AST after 0165 preparation"
    );
}

#[test]
fn infer_expr_types_for_program_skips_final_hm_pass_without_type_optimization() {
    let (program, interner) = parse_program("fn f() { 1 }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    compiler.infer_expr_types_for_program(&program);

    assert_eq!(compiler.hm_infer_runs, 1, "expected a single HM pass");
}

#[test]
fn infer_expr_types_for_program_stores_owned_program_when_desugared() {
    let (program, interner) = parse_program(
        r#"
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { true }
}

fn different<A: Eq>(x: A, y: A) -> Bool {
    x != y
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    let prepared = compiler.prepare_program_for_lowering(&program);

    assert!(
        matches!(prepared.effective_program, std::borrow::Cow::Owned(_)),
        "expected desugaring to materialize an owned final AST"
    );
}

#[test]
fn infer_expr_types_for_program_runs_final_hm_pass_when_desugared() {
    let (program, interner) = parse_program(
        r#"
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { true }
}

fn different<A: Eq>(x: A, y: A) -> Bool {
    x != y
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);

    compiler.infer_expr_types_for_program(&program);

    assert_eq!(
        compiler.hm_infer_runs, 2,
        "expected the final HM pass after operator desugaring"
    );
}

#[test]
fn infer_expr_types_for_program_skips_final_hm_pass_when_type_optimized_but_unchanged() {
    let (program, interner) = parse_program("fn f() { 1 }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.type_optimize = true;

    compiler.infer_expr_types_for_program(&program);

    assert_eq!(
        compiler.hm_infer_runs, 2,
        "expected only original and post-fold HM passes"
    );
}

#[test]
fn infer_expr_types_for_program_runs_three_hm_passes_when_type_optimized_and_desugared() {
    let (program, interner) = parse_program(
        r#"
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { true }
}

fn different<A: Eq>(x: A, y: A) -> Bool {
    x != y
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.type_optimize = true;

    compiler.infer_expr_types_for_program(&program);

    assert_eq!(
        compiler.hm_infer_runs, 3,
        "expected original, post-fold, and post-desugar HM passes"
    );
}

#[test]
fn compile_function_fuses_add_locals() {
    let asm = compile_function_asm("fn f(a, b) { a + b }", "f");
    assert!(
        asm.contains("OpAddLocals 0 1"),
        "expected fused add:\n{asm}"
    );
}

#[test]
fn compile_function_fuses_sub_locals() {
    let asm = compile_function_asm("fn f(a, b) { a - b }", "f");
    assert!(
        asm.contains("OpSubLocals 0 1"),
        "expected fused sub:\n{asm}"
    );
}

#[test]
fn compile_function_fuses_get_local_index() {
    let asm = compile_function_asm("fn f(arr, i) { arr[i] }", "f");
    assert!(
        asm.contains("OpGetLocalIndex"),
        "expected fused local index:\n{asm}"
    );
}

#[test]
fn compile_function_fuses_get_local_call1() {
    // Use a non-tail-position call so OpGetLocalCall1 fusion applies.
    // Tail-position calls emit OpTailCall instead (which is better).
    let asm = compile_function_asm("fn f(x) { let g = fn(n) { n }; g(x) + 1 }", "f");
    assert!(
        asm.contains("OpGetLocalCall1"),
        "expected fused local call1:\n{asm}"
    );
}

#[test]
fn compile_function_fuses_get_local_get_local() {
    let asm = compile_function_asm("fn f(a, b) { let c = 0; let d = 1; (c, d) }", "f");
    assert!(
        asm.contains("OpGetLocalGetLocal"),
        "expected fused local pair:\n{asm}"
    );
}

#[test]
fn compile_function_fuses_call_arities() {
    let source = r#"
fn zero() { 0 }
fn one(x) { x }
fn two(a, b) { a + b }
fn main() { zero(); one(1); two(1, 2) }
"#;
    let asm = compile_function_asm(source, "main");
    assert!(asm.contains("OpCall0"), "expected OpCall0:\n{asm}");
    assert!(asm.contains("OpCall1"), "expected OpCall1:\n{asm}");
    assert!(asm.contains("OpCall2"), "expected OpCall2:\n{asm}");
}

#[test]
fn compile_function_fuses_tail_call1() {
    let asm = compile_function_asm("fn f(n) { if n == 0 { 0 } else { return f(n - 1); } }", "f");
    assert!(asm.contains("OpTailCall1"), "expected OpTailCall1:\n{asm}");
}

// Base function registry tests removed — Proposal 0120 replaced
// Rust base functions with Flux stdlib in lib/Flow/*.flx.

#[test]
fn typed_let_mismatch_is_checked_for_identifier_expression() {
    let (program, interner) = parse_program(
        r#"
let y = 42.5
let x: Int = y
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile-time type mismatch");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E300]: Annotation Type Mismatch")
            && rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("Float"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn nested_function_can_shadow_base_name() {
    let (program, interner) = parse_program(
        r#"
import Flow except [flatten]

fn main() {
    fn flatten(x) { x }
    flatten(1)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("nested function should shadow excluded Flow name");
}

#[test]
fn preload_module_interface_remaps_adt_symbols_across_sessions() {
    use crate::syntax::symbol::Symbol;

    // Simulate session 1: ADT "Color" was interned as Symbol(5).
    let old_adt_id = 5u32;
    let interface = ModuleInterface {
        module_name: "Example.Types".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::from([(
            "make_color".to_string(),
            Scheme {
                forall: vec![],
                constraints: vec![],
                infer_type: InferType::Con(TypeConstructor::Adt(Symbol::new(old_adt_id))),
            },
        )]),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::from([(old_adt_id, "Color".to_string())]),
        public_classes: Vec::new(),
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    // Session 2: fresh compiler. "Color" will get a different symbol ID.
    let mut compiler = Compiler::new();
    // Pre-intern some other strings so "Color" gets a different ID
    let _ = compiler.interner.intern("Alpha");
    let _ = compiler.interner.intern("Beta");
    let _ = compiler.interner.intern("Gamma");
    compiler.preload_module_interface(&interface);

    let module = compiler.interner.intern("Example.Types");
    let member = compiler.interner.intern("make_color");
    let scheme = compiler
        .cached_member_schemes()
        .get(&(module, member))
        .cloned()
        .expect("scheme should be cached");

    // The ADT symbol in the scheme should now point to the correct
    // "Color" symbol in this session's interner.
    let color_sym = compiler.interner.intern("Color");
    match &scheme.infer_type {
        InferType::Con(TypeConstructor::Adt(sym)) => {
            assert_eq!(
                *sym, color_sym,
                "ADT symbol should be remapped to this session's Color symbol"
            );
        }
        other => panic!("expected Adt constructor, got: {:?}", other),
    }
}

#[test]
fn preload_module_interface_remaps_effect_symbols_across_sessions() {
    use crate::syntax::symbol::Symbol;

    let old_effect_id = 3u32;
    let interface = ModuleInterface {
        module_name: "Example.IO".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::from([(
            "run".to_string(),
            Scheme {
                forall: vec![],
                constraints: vec![],
                infer_type: InferType::Fun(
                    vec![InferType::Con(TypeConstructor::Unit)],
                    Box::new(InferType::Con(TypeConstructor::Unit)),
                    InferEffectRow::closed_from_symbols([Symbol::new(old_effect_id)]),
                ),
            },
        )]),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::from([(old_effect_id, "IO".to_string())]),
        public_classes: Vec::new(),
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    let mut compiler = Compiler::new();
    let _ = compiler.interner.intern("Foo");
    let _ = compiler.interner.intern("Bar");
    compiler.preload_module_interface(&interface);

    let module = compiler.interner.intern("Example.IO");
    let member = compiler.interner.intern("run");
    let scheme = compiler
        .cached_member_schemes()
        .get(&(module, member))
        .cloned()
        .expect("scheme should be cached");

    let io_sym = crate::syntax::builtin_effects::io_effect_symbol(&mut compiler.interner);
    match &scheme.infer_type {
        InferType::Fun(_, _, effects) => {
            assert!(
                effects.concrete().contains(&io_sym),
                "effect row should contain remapped IO symbol, got: {:?}",
                effects.concrete()
            );
        }
        other => panic!("expected Fun type, got: {:?}", other),
    }
}

#[test]
fn preload_module_interface_inserts_cached_public_schemes() {
    let mut compiler = Compiler::new();
    let interface = ModuleInterface {
        module_name: "Example.Math".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::from([(
            "double".to_string(),
            Scheme::mono(InferType::Con(TypeConstructor::Int)),
        )]),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: Vec::new(),
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    compiler.preload_module_interface(&interface);

    let module = compiler.interner.intern("Example.Math");
    let member = compiler.interner.intern("double");
    assert_eq!(
        compiler.cached_member_schemes().get(&(module, member)),
        Some(&Scheme::mono(InferType::Con(TypeConstructor::Int)))
    );
}

#[test]
fn flow_primops_missing_covered_scheme_does_not_fall_back_to_rust_injection() {
    let (program, interner) = parse_program(
        r#"
fn main() with Console {
    println("hello")
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let module = compiler.interner.intern("Flow.Primops");
    let idiv = compiler.interner.intern("idiv");
    compiler.cached_member_schemes.insert(
        (module, idiv),
        Scheme {
            forall: vec![],
            constraints: vec![],
            infer_type: InferType::Fun(
                vec![
                    InferType::Con(TypeConstructor::Int),
                    InferType::Con(TypeConstructor::Int),
                ],
                Box::new(InferType::Con(TypeConstructor::Int)),
                InferEffectRow::closed_empty(),
            ),
        },
    );

    let diags = compiler
        .compile(&program)
        .expect_err("println should not be supplied by Rust fallback once Flow.Primops is loaded");
    let rendered = render_diagnostics(&diags, None, None);
    assert!(
        rendered.contains("println") || rendered.contains("unresolved"),
        "expected missing Flow.Primops.println to surface as a compile failure:\n{rendered}"
    );
}

#[test]
fn builtin_effect_operation_registry_seeds_effectful_prelude_ops() {
    let mut compiler = Compiler::new();
    compiler.phase_reset();

    let expected = [
        ("Console", "print"),
        ("Console", "println"),
        ("FileSystem", "read_file"),
        ("FileSystem", "read_lines"),
        ("FileSystem", "write_file"),
        ("Stdin", "read_stdin"),
        ("Clock", "clock_now"),
        ("Clock", "now_ms"),
    ];

    for (effect, operation) in expected {
        let effect = compiler.interner.intern(effect);
        let operation = compiler.interner.intern(operation);
        assert!(
            compiler
                .effect_ops_registry
                .get(&effect)
                .is_some_and(|ops| ops.contains(&operation)),
            "expected seeded operation {effect:?}.{operation:?}"
        );
        assert!(
            compiler
                .effect_op_signatures
                .contains_key(&(effect, operation)),
            "expected seeded signature for {effect:?}.{operation:?}"
        );
    }
}

#[test]
fn main_println_without_annotation_compiles_via_default_handler() {
    let (program, interner) = parse_program(
        r#"
fn main() {
    println("hello")
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, None, None)));
}

#[test]
fn test_function_println_without_annotation_compiles_via_default_handler() {
    // `test_*` functions are entrypoints just like `main` and should
    // receive the same compiler-synthesized default handlers for the
    // built-in operational effects.
    let (program, interner) = parse_program(
        r#"
fn test_default_handlers_apply_to_test_entry() {
    println("from test_*")
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, None, None)));
}

#[test]
fn helper_called_from_main_does_not_inherit_default_handler() {
    // Default handlers wrap the entry's body only. An ordinary helper
    // called from `main` still has to declare its effects explicitly;
    // omitting `with Console` triggers E400 even when `main` itself
    // would have synthesized a Console handler.
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit {
    println("missing Console")
}

fn main() {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("helper without `with Console` should fail to compile");
    assert!(
        diags.iter().any(|d| d.code() == Some("E400")),
        "expected E400 from helper, got: {diags:?}"
    );
}

#[test]
fn helper_called_from_test_entry_does_not_inherit_default_handler() {
    // Symmetric with the `main` case: a `test_*` entry's default
    // handler does not propagate into helpers it calls.
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit {
    println("missing Console")
}

fn test_helper_does_not_inherit_entry_default() {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("helper without `with Console` should fail to compile");
    assert!(
        diags.iter().any(|d| d.code() == Some("E400")),
        "expected E400 from helper, got: {diags:?}"
    );
}

// =====================================================================
// Track 4 — three-way effect availability invariant
//
// Three passes ask "is effect E available here?" against three data
// shapes: HM inference (rows), CFG pre-validator (declared-only static),
// and lowering (`Compiler::is_effect_available`, declared + handled).
//
// Forward direction of the contract: if the pre-validator accepts an
// effect, the lowering predicate must accept the same effect for the
// same fixture. The tests below pin both directions of the contract for
// the canonical 0165 shape (a routed prelude call inside a helper that
// did not declare its effect) and a positive control.
// =====================================================================

#[test]
fn helper_routed_prelude_is_caught_by_pre_validator_not_just_lowering() {
    // The 0165 bug shape: a prelude call (`println`) inside a helper
    // without `with Console`. The routing pass synthesizes a
    // `perform Console.println(...)` and the CFG pre-validator must
    // catch the missing ambient effect. If pre-validation regresses
    // the same fixture would still fail at lowering, but the diagnostic
    // would arrive later and from a different code path.
    //
    // We assert E400 fires at all (forward contract) and that exactly
    // one E400 fires for the helper, not two — which would indicate
    // the pre-validator and lowering both reported the same failure
    // and the suppression contract has drifted.
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit {
    println("missing Console")
}

fn main() {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let diags = compiler
        .compile_with_opts(&program, false, false)
        .expect_err("routed prelude in helper without `with Console` should fail");

    let e400_count = diags.iter().filter(|d| d.code() == Some("E400")).count();
    assert_eq!(
        e400_count, 1,
        "expected exactly one E400 across HM/pre-validator/lowering, got {e400_count}: {diags:?}",
    );
}

#[test]
fn helper_with_explicit_effect_is_accepted_by_all_three_passes() {
    // Positive control for the three-way invariant: when the helper
    // declares `with Console` explicitly, all three passes (HM
    // inference, CFG pre-validator, lowering) must accept the same
    // routed call. A regression in any one would either reject the
    // helper outright or surface a spurious E400 at lowering.
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit with Console {
    println("explicit Console")
}

fn main() {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, None, None)));
}

#[test]
fn handled_effect_inside_helper_is_accepted_at_lowering_only() {
    // The asymmetric direction of the contract: lowering may accept
    // effects the pre-validator would reject, because handlers become
    // visible only at lowering. Here `helper` does not declare
    // `with Console`, but the routed `println` is inside a `handle
    // Console { ... }` block — the lowering predicate sees the
    // installed handler via `handled_effects` and accepts the call.
    //
    // This is the inverse-direction case the pre-validator's doc
    // calls out: it intentionally ignores synthesized/user handlers,
    // so a regression where the pre-validator started consulting them
    // would still pass this test, but a regression where lowering
    // *stopped* consulting them would break it.
    let (program, interner) = parse_program(
        r#"
fn helper() -> Int {
    do {
        println("captured")
        1
    } handle Console {
        print(resume, _msg) -> resume(())
        println(resume, _msg) -> resume(())
    }
}

fn main() {
    let _ = helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, None, None)));
}

#[test]
fn preload_module_interface_inserts_cached_borrow_signatures() {
    let (program, interner) = parse_program("import Example.Math as Math\nlet x = Math.double(1)");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let signature = crate::aether::borrow_infer::BorrowSignature::new(
        vec![crate::aether::borrow_infer::BorrowMode::Borrowed],
        crate::aether::borrow_infer::BorrowProvenance::Imported,
    );
    let interface = ModuleInterface {
        module_name: "Example.Math".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::from([(
            "double".to_string(),
            signature.clone(),
        )]),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: Vec::new(),
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    compiler.preload_module_interface(&interface);

    let registry = compiler.build_preloaded_borrow_registry(&program);
    let module_binding = compiler.interner.intern("Math");
    let member = compiler.interner.intern("double");
    assert_eq!(
        registry.lookup_member_access(module_binding, member),
        Some(&signature)
    );
}

#[test]
fn preload_module_interface_allows_imported_public_class_in_instance_head() {
    let (program, interner) = parse_program(
        r#"
import Example.Logger as Logger

module Local {
    public data StdoutHandle { Stdout }

    public instance Logger<StdoutHandle> {
        fn log(hnd, msg) { }
    }
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let logger_h = compiler.interner.intern("h");
    let logger_name = compiler.interner.intern("Logger");
    let log_name = compiler.interner.intern("log");
    let string_name = compiler.interner.intern("String");

    let interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: vec![PublicClassEntry {
            associated_types: Vec::new(),
            class_module: "Example.Logger".to_string(),
            name: "Logger".to_string(),
            type_param_arity: 1,
            parameter_kinds: vec![],
            type_params: vec![logger_h],
            superclasses: vec![],
            superclass_class_modules: vec![],
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
                return_type: TypeExpr::Tuple {
                    elements: vec![],
                    span: Default::default(),
                },
                effects: vec![],
            }],
            default_methods: vec![],
            method_names: vec!["log".to_string()],
            pinned_row_placeholder: None,
        }],
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    compiler.preload_module_interface(&interface);
    compiler
        .compile(&program)
        .expect("imported public class should be usable in downstream instance heads");

    assert!(
        compiler
            .class_env()
            .instances
            .iter()
            .any(|inst| inst.class_name == logger_name),
        "local instance should resolve against the imported class env"
    );
}

#[test]
fn preload_module_interface_propagates_imported_instance_effect_row() {
    let (_program, interner) = parse_program(
        r#"
import Example.Logger as Logger
import Example.StdLog as StdLog
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let logger_name = compiler.interner.intern("Logger");
    let logger_h = compiler.interner.intern("h");
    let log_name = compiler.interner.intern("log");
    let string_name = compiler.interner.intern("String");
    let int_name = compiler.interner.intern("Int");
    let console_name = compiler.interner.intern("Console");

    let class_interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: vec![PublicClassEntry {
            associated_types: Vec::new(),
            class_module: "Example.Logger".to_string(),
            name: "Logger".to_string(),
            type_param_arity: 1,
            parameter_kinds: vec![],
            type_params: vec![logger_h],
            superclasses: vec![],
            superclass_class_modules: vec![],
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
                return_type: TypeExpr::Tuple {
                    elements: vec![],
                    span: Default::default(),
                },
                effects: vec![],
            }],
            default_methods: vec![],
            method_names: vec!["log".to_string()],
            pinned_row_placeholder: None,
        }],
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };
    let instance_interface = ModuleInterface {
        module_name: "Example.StdLog".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: Vec::new(),
        public_instances: vec![PublicInstanceEntry {
            head_modules: Vec::new(),
            associated_types: Vec::new(),
            class_module: "Example.Logger".to_string(),
            class_name: "Logger".to_string(),
            instance_module: "Example.StdLog".to_string(),
            head_type_repr: "Int".to_string(),
            head_kinds: vec![],
            type_args: vec![TypeExpr::Named {
                name: int_name,
                args: vec![],
                span: Default::default(),
            }],
            context: vec![],
            context_class_modules: vec![],
            methods: vec![PublicInstanceMethodEntry {
                name: log_name,
                effects: vec![EffectExpr::Named {
                    name: console_name,
                    span: Default::default(),
                }],
            }],
            pinned_row_placeholder: None,
        }],
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    compiler.preload_module_interface(&class_interface);
    compiler.preload_module_interface(&instance_interface);
    let logger_class_id = crate::types::class_id::ClassId::new(
        crate::types::class_id::ModulePath::from_identifier(
            compiler.interner.intern("Example.Logger"),
        ),
        logger_name,
    );
    let mangled_name = crate::types::class_env::mangled_method_name(
        logger_class_id,
        "Int",
        "log",
        &compiler.interner,
    );
    let mangled = compiler.interner.intern(&mangled_name);
    let scheme = compiler
        .imported_instance_method_schemes
        .get(&mangled)
        .expect("expected imported instance method scheme to be preloaded");
    match &scheme.infer_type {
        InferType::Fun(_, _, effects) => {
            assert!(
                effects.concrete().contains(&console_name),
                "expected imported instance row to include Console, got: {:?}",
                effects
            );
        }
        other => panic!("expected function scheme for imported instance method, got {other:?}"),
    }
}

#[test]
fn prepare_program_for_lowering_synthesizes_imported_class_instance_dispatch_after_compile() {
    let (program, interner) = parse_program(
        r#"
import Example.Logger as Logger

module Local {
    public data StdoutHandle { Stdout }

    public instance Logger<StdoutHandle> {
        fn log(hnd, msg) { }
    }
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let logger_name = compiler.interner.intern("Logger");
    let logger_h = compiler.interner.intern("h");
    let log_name = compiler.interner.intern("log");
    let string_name = compiler.interner.intern("String");

    let interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: crate::shared::cache_paths::compiler_build_id().to_string(),
        cache_format_version: crate::types::module_interface::MODULE_INTERFACE_FORMAT_VERSION,
        semantic_config_hash: "cfg".to_string(),
        interface_fingerprint: "abi".to_string(),
        schemes: std::collections::HashMap::new(),
        borrow_signatures: std::collections::HashMap::new(),
        runtime_contracts: std::collections::HashMap::new(),
        member_is_value: std::collections::HashMap::new(),
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
        public_classes: vec![PublicClassEntry {
            associated_types: Vec::new(),
            class_module: "Example.Logger".to_string(),
            name: "Logger".to_string(),
            type_param_arity: 1,
            parameter_kinds: vec![],
            type_params: vec![logger_h],
            superclasses: vec![],
            superclass_class_modules: vec![],
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
                return_type: TypeExpr::Tuple {
                    elements: vec![],
                    span: Default::default(),
                },
                effects: vec![],
            }],
            default_methods: vec![],
            method_names: vec!["log".to_string()],
            pinned_row_placeholder: None,
        }],
        public_instances: Vec::new(),
        ctor_field_names: Default::default(),
        public_ctor_types: Default::default(),
        public_type_aliases: Default::default(),
    };

    compiler.preload_module_interface(&interface);
    compiler
        .compile(&program)
        .expect("program with imported public class instance should compile");

    let prepared = compiler.prepare_program_for_lowering_with_preloaded(&program);
    let expected_mangled = crate::types::class_env::mangled_method_name(
        crate::types::class_id::ClassId::new(
            crate::types::class_id::ModulePath::from_identifier(
                compiler.interner.intern("Example.Logger"),
            ),
            logger_name,
        ),
        // `StdoutHandle` is `Local`'s, so its key carries the module too.
        &crate::types::class_env::instance_type_key(
            &[TypeExpr::Named {
                name: compiler.interner.intern("StdoutHandle"),
                args: vec![],
                span: Default::default(),
            }],
            &[crate::types::class_id::ModulePath::from_identifier(
                compiler.interner.intern("Local"),
            )],
            &compiler.interner,
        ),
        "log",
        &compiler.interner,
    );
    assert!(
        top_level_has_function(
            &prepared.effective_program.statements,
            &expected_mangled,
            &compiler.interner,
        ),
        "expected imported-class instance dispatch to be synthesized in the effective program"
    );
}

#[test]
fn hydrate_cached_module_bytecode_restores_globals_and_bytecode() {
    let (module_program, interner) = parse_program("fn helper() { 41 }");
    let mut artifact_compiler = Compiler::new_with_interner("<module>", interner);
    let snapshot = artifact_compiler.module_cache_snapshot();
    artifact_compiler
        .compile(&module_program)
        .expect("module compilation should succeed");
    let cached_module = artifact_compiler.build_cached_module_bytecode(snapshot);

    let (baseline_module, baseline_interner) = parse_program("fn helper() { 41 }");
    let mut baseline = Compiler::new_with_interner("<baseline-module>", baseline_interner);
    baseline
        .compile(&baseline_module)
        .expect("baseline module compilation should succeed");
    let (baseline_entry, baseline_interner) =
        parse_program_with_interner("fn main() { helper() }", baseline.interner);
    baseline.interner = baseline_interner;
    baseline.set_file_path("<baseline-entry>");
    baseline
        .compile(&baseline_entry)
        .expect("baseline entry compilation should succeed");
    let baseline_bytecode = baseline.bytecode();

    let mut cached = Compiler::new();
    cached.hydrate_cached_module_bytecode(&cached_module);
    let (entry_program, entry_interner) =
        parse_program_with_interner("fn main() { helper() }", cached.interner);
    cached.interner = entry_interner;
    cached.set_file_path("<entry>");
    cached
        .compile(&entry_program)
        .expect("entry compilation with cached module should succeed");
    let cached_bytecode = cached.bytecode();

    assert_eq!(cached_bytecode.instructions, baseline_bytecode.instructions);
    let cached_helper =
        find_compiled_function(&cached_bytecode.constants, "helper").expect("cached helper fn");
    let baseline_helper =
        find_compiled_function(&baseline_bytecode.constants, "helper").expect("baseline helper fn");
    assert_eq!(cached_helper.instructions, baseline_helper.instructions);
    assert_eq!(cached_helper.num_locals, baseline_helper.num_locals);

    let cached_main =
        find_compiled_function(&cached_bytecode.constants, "main").expect("cached main fn");
    let baseline_main =
        find_compiled_function(&baseline_bytecode.constants, "main").expect("baseline main fn");
    assert_eq!(cached_main.instructions, baseline_main.instructions);
    assert_eq!(cached_main.num_locals, baseline_main.num_locals);

    let helper = cached.interner.intern("helper");
    let binding = cached
        .symbol_table
        .resolve(helper)
        .expect("cached helper binding should resolve");
    assert_eq!(binding.index, 0);
}

#[test]
fn local_let_can_shadow_flow_name() {
    let (program, interner) = parse_program(
        r#"
import Flow except [len]

fn main() {
    let len = 1
    len
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("local let should shadow excluded Flow name");
}

#[test]
fn match_pattern_can_shadow_flow_name() {
    let (program, interner) = parse_program(
        r#"
import Flow except [len]

fn main() {
    match Some(1) {
        Some(len) -> len,
        None -> 0,
    }
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("pattern binding should shadow excluded Flow name");
}

#[test]
fn parameter_can_shadow_flow_name() {
    let (program, interner) = parse_program(
        r#"
import Flow except [len]

fn id(len) { len }
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("parameter binding should shadow excluded Flow name");
}

#[test]
fn typed_let_mismatch_is_checked_for_typed_call_return() {
    let (program, interner) = parse_program(
        r#"
fn make() -> Float { 1.5 }
let x: Int = make()
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile-time type mismatch");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E300]: Annotation Type Mismatch")
            && rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("Float"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn typed_let_module_member_call_uses_hm_strict_path() {
    let (program, interner) = parse_program(
        r#"
module Local {
    public fn make_float() -> Float { 1.5 }
}
fn main() -> Unit {
    let x: Int = Local.make_float()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected HM strict-path mismatch for module member call");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E300]: Annotation Type Mismatch")
            && rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("Float"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn typed_let_private_module_member_call_is_rejected_before_hm_boundary_type_check() {
    let (program, interner) = parse_program(
        r#"
module Local {
    fn make_float() -> Float { 1.5 }
}

fn main() -> Unit {
    let x: Float = Local.make_float()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected private member access failure");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E011]: Private Member"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn render_aether_report_debug_includes_borrow_and_callsite_sections() {
    let (program, interner) = parse_program(
        r#"
fn first(x, y) { x }
fn main() { first(1, 2) }
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("program should compile");

    let report = compiler
        .render_aether_report(&program, false, true)
        .expect("debug aether report should render");

    assert!(report.contains("borrow signature:"));
    assert!(report.contains("call sites:"));
    assert!(report.contains("dups:"));
    assert!(report.contains("drops:"));
    assert!(report.contains("reuse:"));
    assert!(report.contains("first"));
    assert!(report.contains("line"));
}

#[test]
fn dump_core_with_opts_does_not_depend_on_prior_compiler_state() {
    let (program, interner) = parse_program(
        r#"
fn sum(x) { x + 1 }
"#,
    );
    let mut warmed = Compiler::new_with_interner("<test>", interner.clone());
    warmed.compile(&program).expect("program should compile");
    let warmed_dump = warmed
        .dump_core_with_opts(
            &program,
            false,
            crate::core::display::CoreDisplayMode::Readable,
        )
        .expect("dump_core should succeed after compile");

    let mut fresh = Compiler::new_with_interner("<test>", interner);
    let fresh_dump = fresh
        .dump_core_with_opts(
            &program,
            false,
            crate::core::display::CoreDisplayMode::Readable,
        )
        .expect("dump_core should succeed without prior compile");

    assert_eq!(fresh_dump, warmed_dump);
}

#[test]
fn typed_let_inference_path_does_not_use_runtime_compat_fallback_helpers() {
    let source = include_str!("statement.rs");
    assert!(
        !source.contains("self.hm_expr_type_compat(value)"),
        "typed let inference must not use hm_expr_type_compat fallback"
    );
    assert!(
        !source.contains("self.runtime_boundary_expr_type(value)"),
        "typed let inference must not use runtime_boundary_expr_type fallback"
    );
}

#[test]
fn typed_let_tuple_field_projection_uses_precise_hm_type() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit {
    let x: Int = (1, "s").1
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected tuple-field typed mismatch");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E300]: Annotation Type Mismatch")
            && rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("String"),
        "unexpected diagnostics:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "tuple-field projection should be typed, not unresolved:\n{}",
        rendered
    );
}

#[test]
fn typed_let_index_projection_uses_precise_hm_type() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit {
    let xs = [1, 2]
    let x: Int = xs[0]
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected index projection typed mismatch");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E300]: Annotation Type Mismatch")
            && rendered.contains("Option<Int>"),
        "unexpected diagnostics:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "index projection should be typed, not unresolved:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_requires_effect_annotation_for_non_public_effectful_function() {
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit {
    print("x")
}

fn main() -> Unit {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected missing strict effect annotation");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E418]: Strict Effect Annotation Required")
            && rendered.contains("Effectful function `helper` must declare `with Console`")
            && rendered.contains("Add explicit `with Console`"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_requires_time_annotation_for_non_public_effectful_function() {
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit {
    let _t = now_ms()
}

fn main() -> Unit {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected missing strict time annotation");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E418]: Strict Effect Annotation Required")
            && rendered.contains("Effectful function `helper` must declare `with Clock`")
            && rendered.contains("Add explicit `with Clock`"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_reports_missing_time_when_only_io_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit with IO {
    let _t = now_ms()
}

fn main() -> Unit with IO {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected missing Time annotation");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E418]: Strict Effect Annotation Required")
            && rendered.contains("Effectful function `helper` must declare `with Clock`")
            && rendered.contains("Add explicit `with Clock`"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_reports_missing_io_when_only_time_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn helper() -> Unit with Time {
    print("x")
}

fn main() -> Unit with Time {
    helper()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    let err = compiler
        .compile(&program)
        .expect_err("expected missing IO annotation");
    let rendered = render_diagnostics(&err, None, None);
    assert!(
        rendered.contains("error[E418]: Strict Effect Annotation Required")
            && rendered.contains("Effectful function `helper` must declare `with Console`")
            && rendered.contains("Add explicit `with Console`"),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

// Strict-mode accept tests for `print`/`now_ms` removed — those relied on
// base functions which are no longer registered in the symbol table.
// Effect annotation acceptance is now validated via integration tests with
// the Flux stdlib (`lib/Flow/*.flx`) loaded.

#[test]
fn function_compile_error_does_not_leak_scope() {
    let (program, interner) = parse_program(
        r#"
fn bad() -> Int {
    "oops"
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let _ = compiler
        .compile(&program)
        .expect_err("expected compile error");
    assert_eq!(
        compiler.scope_index, 0,
        "function compile error should not leak symbol table scope"
    );
}

mod unqualified_runtime_contract_resolution {
    //! Resolution rule for unqualified function names whose contracts live in
    //! multiple modules: an explicit user import beats the auto-injected
    //! `Flow.*` prelude. Without this, `sum` (defined in both `Flow.List` and
    //! `Flow.Array`) would resolve via non-deterministic HashMap iteration.

    use crate::compiler::Compiler;
    use crate::runtime::function_contract::FunctionContract;
    use crate::runtime::runtime_type::RuntimeType;
    use crate::syntax::interner::Interner;

    fn list_sum_contract() -> FunctionContract {
        FunctionContract {
            params: vec![Some(RuntimeType::List(Box::new(RuntimeType::Int)))],
            ret: Some(RuntimeType::Int),
            effects: vec![],
        }
    }

    fn array_sum_contract() -> FunctionContract {
        FunctionContract {
            params: vec![Some(RuntimeType::Array(Box::new(RuntimeType::Int)))],
            ret: Some(RuntimeType::Int),
            effects: vec![],
        }
    }

    fn make_compiler_with_sum_contracts() -> Compiler {
        let mut compiler = Compiler::new_with_interner("<test>", Interner::new());
        let list_mod = compiler.interner.intern("Flow.List");
        let array_mod = compiler.interner.intern("Flow.Array");
        let sum_member = compiler.interner.intern("sum");
        compiler
            .cached_member_runtime_contracts
            .insert((list_mod, sum_member), list_sum_contract());
        compiler
            .cached_member_runtime_contracts
            .insert((array_mod, sum_member), array_sum_contract());
        compiler
    }

    #[test]
    fn prelude_only_resolves_to_prelude_contract() {
        // Only Flow.List is imported (as it would be from the auto-prelude).
        // `sum` must resolve to Flow.List.sum even though Flow.Array.sum is
        // present in the contract cache (loaded transitively as a dependency).
        let mut compiler = make_compiler_with_sum_contracts();
        let list_mod = compiler.interner.intern("Flow.List");
        let sum_member = compiler.interner.intern("sum");
        compiler.imported_modules.insert(list_mod);

        let resolved = compiler
            .lookup_unqualified_runtime_contract(sum_member)
            .expect("expected to resolve sum");
        assert_eq!(resolved, &list_sum_contract());
    }

    #[test]
    fn explicit_import_beats_prelude() {
        // Both Flow.List (prelude) and Flow.Array (explicit) are imported.
        // `sum` must resolve to Flow.Array.sum — the explicit import wins.
        let mut compiler = make_compiler_with_sum_contracts();
        let list_mod = compiler.interner.intern("Flow.List");
        let array_mod = compiler.interner.intern("Flow.Array");
        let sum_member = compiler.interner.intern("sum");
        compiler.imported_modules.insert(list_mod);
        compiler.imported_modules.insert(array_mod);

        let resolved = compiler
            .lookup_unqualified_runtime_contract(sum_member)
            .expect("expected to resolve sum");
        assert_eq!(resolved, &array_sum_contract());
    }

    #[test]
    fn cached_contract_for_unimported_module_is_ignored() {
        // Flow.Array.sum is in the contract cache but not in imported_modules,
        // and no other module exposes `sum`. Resolution must return None
        // rather than leaking the unimported module's contract.
        let mut compiler = make_compiler_with_sum_contracts();
        let sum_member = compiler.interner.intern("sum");
        // No imports at all.
        assert!(
            compiler
                .lookup_unqualified_runtime_contract(sum_member)
                .is_none()
        );
    }
}

/// Guards the REPL-completion primop list against accidental corruption or
/// truncation. It mirrors the `primop_sigs` table in `inject_primop_hm_schemes`;
/// this checks it stays clean (no `__primop_*` aliases, no duplicates) and still
/// covers a representative name from each primop family.
#[test]
fn builtin_primop_names_are_clean_and_cover_each_family() {
    let names = Compiler::BUILTIN_PRIMOP_NAMES;
    assert!(
        names.iter().all(|name| !name.starts_with("__")),
        "BUILTIN_PRIMOP_NAMES must exclude the internal __primop_* aliases"
    );
    let mut deduped: Vec<&&str> = names.iter().collect();
    deduped.sort_unstable();
    let len_before = deduped.len();
    deduped.dedup();
    assert_eq!(
        len_before,
        deduped.len(),
        "BUILTIN_PRIMOP_NAMES contains duplicates"
    );
    for sentinel in [
        "print",
        "to_string",
        "abs",
        "bit_and",
        "len",
        "is_int",
        "map_get",
        "sum",
        "safe_div",
        "time",
    ] {
        assert!(
            names.contains(&sentinel),
            "BUILTIN_PRIMOP_NAMES missing `{sentinel}` — out of sync with primop_sigs?"
        );
    }
}

/// Proposal 0179 Stage 6: an imported instance must carry one associated-type
/// equation per declaration on its class.
///
/// The two lists cross the interface separately, and a short one does not fail
/// on its own — the application simply stays stuck and resurfaces later as an
/// unrelated error about the type it never became. The counts are compared on
/// reload so the stale interface is named directly.
#[test]
fn an_imported_instance_missing_an_associated_type_equation_is_a_stale_interface() {
    use crate::types::{
        class_env::{ClassDef, InstanceDef},
        class_id::{ClassId, ModulePath},
    };
    use std::collections::HashMap;

    let mut interner = Interner::new();
    let module = ModulePath::from_identifier(interner.intern("Collect"));
    let class_name = interner.intern("Collection");
    let element = interner.intern("Element");
    let class_id = ClassId::new(module, class_name);
    let span = crate::diagnostics::position::Span::default();

    let class_def = ClassDef {
        name: class_name,
        module,
        is_public: true,
        is_builtin: false,
        type_params: vec![interner.intern("c")],
        superclasses: Vec::new(),
        superclass_class_ids: Vec::new(),
        methods: Vec::new(),
        default_methods: Vec::new(),
        associated_types: vec![crate::syntax::type_class::AssociatedTypeDecl {
            name: element,
            params: Vec::new(),
            span,
        }],
        span,
    };
    let classes = HashMap::from([(class_id, class_def)]);

    let instance = |associated_types| InstanceDef {
        origin: crate::types::class_env::InstanceOrigin::Declared,
        class_name,
        class_id,
        instance_module: module,
        is_public: true,
        type_key: String::new(),
        head_modules: Vec::new(),
        type_args: Vec::new(),
        context: Vec::new(),
        context_class_ids: Vec::new(),
        method_names: Vec::new(),
        method_effects: Vec::new(),
        associated_types,
        span,
    };

    let complete = crate::compiler::validate_imported_associated_types(
        &classes,
        &[instance(vec![
            crate::syntax::type_class::AssociatedTypeEquation {
                name: element,
                head: Vec::new(),
                body: TypeExpr::Named {
                    name: interner.intern("Int"),
                    args: Vec::new(),
                    span,
                },
                span,
            },
        ])],
        &interner,
    );
    assert!(complete.is_empty(), "matching counts must not diagnose");

    let truncated = crate::compiler::validate_imported_associated_types(
        &classes,
        &[instance(Vec::new())],
        &interner,
    );
    assert_eq!(truncated.len(), 1);
    assert_eq!(truncated[0].code(), Some("E478"));
    assert!(
        truncated[0]
            .message()
            .is_some_and(|message| message.contains("declares 1 associated type(s) but 0")),
        "unexpected message: {:?}",
        truncated[0].message()
    );
}

/// Proposal 0179 Stage 4: `Convert<Int, ?b>` with exactly one instance whose
/// known arguments match takes the missing one from that instance's head, so
/// `let s = convert(42)` is solved — and its call has a dictionary to pass —
/// without an annotation.
#[test]
fn a_sole_candidate_instance_determines_a_multi_parameter_predicate() {
    use crate::syntax::{expression::Expression, statement::Statement};

    let (program, interner) = parse_program(
        r#"
class Convert<a, b> {
    fn convert(x: a) -> b
}

instance Convert<Int, String> {
    fn convert(x) { "n" }
}

fn main() {
    let s = convert(42)
    s
}
"#,
    );
    let call = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { body, .. } => match body.statements.first() {
                Some(Statement::Let {
                    value: call @ Expression::Call { .. },
                    ..
                }) => Some(call.expr_id()),
                _ => None,
            },
            _ => None,
        })
        .expect("`let s = convert(42)`");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("`convert(42)` is determined by its only instance");

    let args = compiler.evidence_map().dict_args_at(call);
    assert!(
        matches!(
            args.as_deref(),
            Some([crate::types::translate::DictArg::Global { .. }])
        ),
        "expected the `Convert<Int, String>` dictionary, got {args:?}"
    );
}

/// A dump or LLVM build lowers a program other than the one the unit's
/// inference solved: a merged program, whose ids are shifted past the modules
/// before it. Lowering re-infers that program, and the evidence it reads must
/// be keyed by the re-inferred program's ids, not the unit's.
#[test]
fn lowering_a_reinferred_program_reads_its_own_evidence() {
    assert_lowering_reads_the_shifted_programs_evidence(false);
}

/// The optimizing path folds and renames the program before lowering it, and
/// must still lower it with that program's own types and evidence.
#[test]
fn optimized_lowering_reads_the_programs_own_evidence() {
    assert_lowering_reads_the_shifted_programs_evidence(true);
}

fn assert_lowering_reads_the_shifted_programs_evidence(optimize: bool) {
    use crate::syntax::{expression::Expression, statement::Statement};

    let (program, interner) = parse_program(
        r#"
class Convert<a, b> {
    fn convert(x: a) -> b
}

instance Convert<Int, String> {
    fn convert(x) { "n" }
}

fn main() {
    let s: String = convert(42)
    s
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let shifted = crate::ast::shift_expr_ids::shift_expr_ids(program, 1000);
    let call = shifted
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { body, .. } => match body.statements.first() {
                Some(Statement::Let {
                    value: call @ Expression::Call { .. },
                    ..
                }) => Some(call.expr_id()),
                _ => None,
            },
            _ => None,
        })
        .expect("`let s: String = convert(42)`");
    compiler
        .dump_core_with_opts(
            &shifted,
            optimize,
            crate::core::display::CoreDisplayMode::Readable,
        )
        .expect("the shifted program lowers");

    let args = compiler.evidence_map().dict_args_at(call);
    assert!(
        matches!(
            args.as_deref(),
            Some([crate::types::translate::DictArg::Global { .. }])
        ),
        "expected the `Convert<Int, String>` dictionary at the shifted call, got {args:?}"
    );
}

/// 0.0.8 plan step 1d: lowering creates a constrained definition's dictionary
/// parameters from its recorded evidence, before dictionary elaboration runs.
/// Elaboration then reuses them by their canonical names instead of adding
/// its own.
#[test]
fn lowering_creates_a_constrained_definitions_dictionary_parameters() {
    let (program, interner) = parse_program(
        r#"
class Sized<a> {
    fn size(x: a) -> Int
}

instance Sized<Int> {
    fn size(x) { x }
}

fn measure<a: Sized>(x: a) -> Int {
    size(x)
}

fn main() {
    measure(3)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = compiler
        .lower_core_from_program(&program, false, false)
        .expect("the program lowers");
    let measure = core
        .defs
        .iter()
        .find(|def| compiler.interner.resolve(def.name) == "measure")
        .expect("`measure` is lowered");
    let crate::core::CoreExpr::Lam { params, .. } = &measure.expr else {
        panic!("`measure` lowers to a lambda, got {:?}", measure.expr);
    };
    let names: Vec<_> = params
        .iter()
        .map(|param| compiler.interner.resolve(param.name))
        .collect();
    assert_eq!(names, ["__dict_Sized", "x"]);
}

/// 0.0.8 plan step 1d: a method call whose evidence is the definition's own
/// dictionary parameter reads the method out of that parameter. Dispatching
/// straight to `__tc_Sized_Int_size` because `Sized<Int>` is the only
/// instance in sight is right only until another module adds one.
#[test]
fn a_method_call_answered_by_a_parameter_projects_from_it() {
    let (program, interner) = parse_program(
        r#"
class Sized<a> {
    fn size(x: a) -> Int
}

instance Sized<Int> {
    fn size(x) { x }
}

fn measure<a: Sized>(x: a) -> Int {
    size(x)
}

fn main() {
    measure(3)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = compiler
        .lower_core_from_program(&program, false, true)
        .expect("the program lowers");
    let measure = core
        .defs
        .iter()
        .find(|def| compiler.interner.resolve(def.name) == "measure")
        .expect("`measure` is lowered");
    let crate::core::CoreExpr::Lam { body, .. } = &measure.expr else {
        panic!("`measure` lowers to a lambda, got {:?}", measure.expr);
    };
    // Core passes may bind the projection before the call.
    let func = match body.as_ref() {
        crate::core::CoreExpr::App { func, .. } => func.as_ref(),
        crate::core::CoreExpr::Let { rhs, .. } => rhs.as_ref(),
        other => panic!("`size(x)` lowers to a call, got {other:?}"),
    };
    let crate::core::CoreExpr::TupleField { object, index, .. } = func else {
        panic!("expected `size` read from the dictionary parameter, got {func:?}");
    };
    let crate::core::CoreExpr::Var { var, .. } = object.as_ref() else {
        panic!("expected the dictionary parameter, got {object:?}");
    };
    assert_eq!(compiler.interner.resolve(var.name), "__dict_Sized");
    assert_eq!(*index, 0);
}

/// The optimizing compile inlines a zero-argument pure function into its
/// callers, so `staff()` inside a destructuring `let` becomes a named-field
/// literal sitting in that statement. The named-field desugar never visited a
/// destructuring `let` (nor `Assign` or a class's default bodies), and the
/// literal reached the bytecode compiler, which panicked.
#[test]
fn optimizing_compile_desugars_named_fields_inlined_into_a_destructuring_let() {
    let (program, interner) = parse_program(
        r#"
data Employee {
    Employee { name: String, years: Int }
}

fn staff() -> Employee {
    Employee { name: "ada", years: 9 }
}

fn main() {
    let (first, n) = (staff(), 1)
    first
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
}

/// The optimizer inlines a single-argument function into its callers. An
/// unannotated one is typed only by those callers (`fst`'s receiver is pinned
/// to `(Int, String)` by its one call), so inlining every call left the
/// definition with nothing to type it, and the optimizing compile reported
/// E491 on a program that compiles without it.
#[test]
fn optimizing_compile_keeps_an_unannotated_function_its_callers_type() {
    let (program, interner) = parse_program(
        r#"
fn fst(p) { p.0 }

fn main() {
    fst((1, "x"))
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
}

/// An inlined call keeps the function's annotations. `parse_int`'s `-> Int`
/// is what chooses the `Parse<Int>` instance for `parse(s)`; substituting the
/// body into the caller dropped it, and the optimizing compile reported E459.
#[test]
fn optimizing_compile_keeps_an_inlined_functions_annotations() {
    let (program, interner) = parse_program(
        r#"
class Parse<a> {
    fn parse(s: String) -> a
}

instance Parse<Int> {
    fn parse(s) { 7 }
}

instance Parse<Bool> {
    fn parse(s) { true }
}

fn parse_int(s: String) -> Int {
    parse(s)
}

fn ignore<a>(x: a) -> Int { 0 }

fn main() {
    ignore(parse_int("z"))
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
}

/// The optimizer matches a call to a function by name alone. Where a local
/// shadows a top-level function, the call is the local's, and inlining the
/// top-level body there put an `Int` function where a `String` one was.
#[test]
fn optimizing_compile_does_not_inline_a_name_a_local_shadows() {
    let (program, interner) = parse_program(
        r#"
fn helper(x: Int) -> Int { x }

fn shadowed() -> String {
    let helper = \(s: String) -> s
    helper("shadowed")
}

fn main() {
    shadowed()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
}

/// An inlined call keeps the function's effects. `run_task` is polymorphic
/// in its row, so the copy must say so too, or the effect its argument
/// performs is one the copy does not allow. And `run`'s handler is not moved
/// at all: inlined into a `with IO` caller, its `handle Logger { print(..) }`
/// stopped discharging `Logger` (E400).
#[test]
fn optimizing_compile_keeps_an_inlined_functions_effects() {
    let (program, interner) = parse_program(
        r#"
effect Logger {
    print: String -> ()
}

fn run_task(f: (() -> Int with |e)) -> Int with |e {
    f()
}

fn task() -> Int with Logger {
    perform Logger.print("ok")
    7
}

fn run() -> Int {
    run_task(task) handle Logger {
        print(resume, _msg) -> resume(())
    }
}

fn main() with IO {
    print(run())
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
}

/// A class over a higher-kinded parameter, used at a partially applied
/// constructor. The `remap` stub's `f<a>` must unify with
/// `Either<String, Int>` as `f = Either<String>`; when it did not, `f` stayed
/// unbound and the predicate was reported as E459.
#[test]
fn a_higher_kinded_method_resolves_at_a_partially_applied_constructor() {
    let (program, interner) = parse_program(
        r#"
class Mappable<f> {
    fn remap<a, b>(x: f<a>, g: (a) -> b) -> f<b>
}

instance Mappable<Either<l>> {
    fn remap(x, g) {
        match x {
            Left(e) -> Left(e),
            Right(v) -> Right(g(v)),
        }
    }
}

fn main() {
    let r: Either<String, Int> = Right(2)
    remap(r, fn(x) { x + 1 })
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    if let Err(diagnostics) = compiler.compile_with_opts(&program, false, false) {
        panic!(
            "the program compiles: {}",
            render_diagnostics(&diagnostics, None, None)
        );
    }
}

/// Under `--optimize`, the lowering paths solve evidence for the program they
/// lower. `0 - 1` folds to `-1`, so the folded program raises no `Num`
/// predicate there. When folding ran after the final inference, the evidence
/// still had the unfolded operator's predicate, at a site lowering never
/// reaches.
#[test]
fn optimizing_dump_solves_evidence_for_the_folded_program() {
    use crate::syntax::expression::Expression;
    let (program, interner) = parse_program(
        r#"
fn main() {
    let x = 0 - 1
    x
}
"#,
    );
    let folded = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { body, .. } => match body.statements.first() {
                Some(Statement::Let {
                    value: infix @ Expression::Infix { .. },
                    ..
                }) => Some(infix.expr_id()),
                _ => None,
            },
            _ => None,
        })
        .expect("`let x = 0 - 1`");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile_with_opts(&program, true, false)
        .expect("the optimizing compile succeeds");
    compiler
        .dump_core_with_opts(
            &program,
            true,
            crate::core::display::CoreDisplayMode::Readable,
        )
        .expect("the optimizing dump succeeds");

    assert_eq!(compiler.evidence_map().raised(folded), 0);
}

/// Lower `program`, already compiled by `compiler`, with dictionaries taken
/// from evidence and no elaboration (0.0.8 plan, step 1e).
fn lower_emitting_evidence(
    compiler: &mut Compiler,
    program: &crate::syntax::program::Program,
) -> crate::core::CoreProgram {
    let program = program_to_lower(compiler, program);
    let (core, errors) = lower_emitting(compiler, &program, &compiler.evidence_map);
    assert!(errors.is_empty(), "lowering reports {errors:?}");
    core
}

/// The program the Core path lowers, its generated instance methods among
/// it, with the final inference's types and evidence applied.
fn program_to_lower(
    compiler: &mut Compiler,
    program: &crate::syntax::program::Program,
) -> crate::syntax::program::Program {
    let prepared = compiler.prepare_program_for_lowering(program);
    compiler.apply_hm_final(&prepared.hm_final);
    let mut program = prepared.effective_program.into_owned();
    compiler.apply_named_field_desugar(&mut program);
    program
}

/// Lower `program` from `evidence` in `Emit` mode, with the internal errors.
fn lower_emitting(
    compiler: &Compiler,
    program: &crate::syntax::program::Program,
    evidence: &crate::types::evidence::EvidenceMap,
) -> (
    crate::core::CoreProgram,
    Vec<crate::diagnostics::Diagnostic>,
) {
    crate::core::lower_ast::lower_program_ast_with_evidence(
        program,
        &compiler.hm_expr_types,
        Some(&compiler.interner),
        Some(&compiler.type_env),
        None,
        Some(&compiler.class_env),
        Some(evidence),
    )
}

fn lambda_param_names<'a>(
    compiler: &'a Compiler,
    expr: &'a crate::core::CoreExpr,
) -> (Vec<&'a str>, &'a [crate::core::CoreBinder]) {
    let crate::core::CoreExpr::Lam { params, .. } = expr else {
        panic!("expected a lambda, got {expr:?}");
    };
    let names = params
        .iter()
        .map(|param| compiler.interner.resolve(param.name))
        .collect();
    (names, params)
}

const SIZED_CLASS: &str = r#"
class Sized<a> {
    fn size(x: a) -> Int
}

instance Sized<Int> {
    fn size(x) { x }
}
"#;

/// 1e: in `Emit` mode a definition takes one parameter per recorded
/// evidence parameter, even where elaboration would pair them differently.
/// Two dictionaries of one class are told apart by their recorded order.
#[test]
fn emitting_evidence_gives_a_definition_each_recorded_parameter() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn both<a: Sized, b: Sized>(x: a, y: b) -> Int {{
    size(x) + size(y)
}}

fn main() {{
    both(1, 2)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = lower_emitting_evidence(&mut compiler, &program);
    let both = core
        .defs
        .iter()
        .find(|def| compiler.interner.resolve(def.name) == "both")
        .expect("`both` is lowered");
    let (names, _) = lambda_param_names(&compiler, &both.expr);
    assert_eq!(names, ["__dict_Sized", "__dict_Sized_1", "x", "y"]);
}

/// 1e: a bounded function nested in a body gets its own parameter, and a
/// method call in it on the enclosing definition's dictionary reads the
/// enclosing binder, though the two share a name.
#[test]
fn emitting_evidence_gives_a_nested_function_its_parameters() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn outer<a: Sized>(x: a) -> Int {{
    fn inner<b: Sized>(y: b) -> Int {{
        size(y) + size(x)
    }}
    inner(x) + inner(4)
}}

fn main() {{
    outer(3)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = lower_emitting_evidence(&mut compiler, &program);
    let outer = core
        .defs
        .iter()
        .find(|def| compiler.interner.resolve(def.name) == "outer")
        .expect("`outer` is lowered");
    let (outer_names, outer_params) = lambda_param_names(&compiler, &outer.expr);
    assert_eq!(outer_names, ["__dict_Sized", "x"]);
    let crate::core::CoreExpr::Lam { body, .. } = &outer.expr else {
        unreachable!()
    };

    let mut inner = None;
    let mut projected = Vec::new();
    collect_inner_and_projections(&compiler, body, &mut inner, &mut projected);
    let inner = inner.expect("`inner` is lowered as a local function");
    let (inner_names, inner_params) = lambda_param_names(&compiler, inner);
    assert_eq!(inner_names, ["__dict_Sized", "y"]);
    assert_ne!(inner_params[0].id, outer_params[0].id);
    projected.sort_by_key(|id| id.0);
    let mut expected = vec![outer_params[0].id, inner_params[0].id];
    expected.sort_by_key(|id| id.0);
    assert_eq!(
        projected, expected,
        "`size(y)` reads `inner`'s dictionary and `size(x)` reads `outer`'s"
    );
}

/// The lambda bound to `inner`, and the binder of every dictionary a method
/// is projected from inside it.
fn collect_inner_and_projections<'a>(
    compiler: &Compiler,
    expr: &'a crate::core::CoreExpr,
    inner: &mut Option<&'a crate::core::CoreExpr>,
    projected: &mut Vec<crate::core::CoreBinderId>,
) {
    use crate::core::CoreExpr;
    match expr {
        CoreExpr::LetRec { var, rhs, body, .. } | CoreExpr::Let { var, rhs, body, .. } => {
            if compiler.interner.try_resolve(var.name) == Some("inner") {
                *inner = Some(rhs);
                collect_projections(rhs, projected);
            }
            collect_inner_and_projections(compiler, body, inner, projected);
        }
        _ => {}
    }
}

fn collect_projections(expr: &crate::core::CoreExpr, out: &mut Vec<crate::core::CoreBinderId>) {
    use crate::core::CoreExpr;
    match expr {
        CoreExpr::TupleField { object, .. } => {
            if let CoreExpr::Var { var, .. } = object.as_ref()
                && let Some(binder) = var.binder
            {
                out.push(binder);
            }
            collect_projections(object, out);
        }
        CoreExpr::Lam { body, .. } => collect_projections(body, out),
        CoreExpr::App { func, args, .. } => {
            collect_projections(func, out);
            args.iter().for_each(|arg| collect_projections(arg, out));
        }
        CoreExpr::PrimOp { args, .. } => args.iter().for_each(|arg| collect_projections(arg, out)),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_projections(rhs, out);
            collect_projections(body, out);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_projections(scrutinee, out);
            alts.iter()
                .for_each(|alt| collect_projections(&alt.rhs, out));
        }
        _ => {}
    }
}

/// Every call to `name` in `expr`, as its arguments' shapes: a variable's
/// name, an applied variable as `f(args)`, anything else `_`.
fn collect_calls(
    compiler: &Compiler,
    expr: &crate::core::CoreExpr,
    name: &str,
    out: &mut Vec<Vec<String>>,
) {
    use crate::core::CoreExpr;
    fn shape(compiler: &Compiler, expr: &CoreExpr) -> String {
        match expr {
            CoreExpr::Var { var, .. } => compiler.interner.resolve(var.name).to_string(),
            CoreExpr::App { func, args, .. } => format!(
                "{}({})",
                shape(compiler, func),
                args.iter()
                    .map(|arg| shape(compiler, arg))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => "_".to_string(),
        }
    }
    match expr {
        CoreExpr::App { func, args, .. } => {
            if matches!(func.as_ref(), CoreExpr::Var { var, .. }
                if compiler.interner.resolve(var.name) == name)
            {
                out.push(args.iter().map(|arg| shape(compiler, arg)).collect());
            }
            collect_calls(compiler, func, name, out);
            args.iter()
                .for_each(|arg| collect_calls(compiler, arg, name, out));
        }
        CoreExpr::Lam { body, .. } => collect_calls(compiler, body, name, out),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_calls(compiler, rhs, name, out);
            collect_calls(compiler, body, name, out);
        }
        CoreExpr::PrimOp { args, .. } => args
            .iter()
            .for_each(|arg| collect_calls(compiler, arg, name, out)),
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_calls(compiler, scrutinee, name, out);
            alts.iter()
                .for_each(|alt| collect_calls(compiler, &alt.rhs, name, out));
        }
        _ => {}
    }
}

/// The calls to `name` in the definition `def`, by [`collect_calls`].
fn calls_in(
    compiler: &Compiler,
    core: &crate::core::CoreProgram,
    def: &str,
    name: &str,
) -> Vec<Vec<String>> {
    let def = core
        .defs
        .iter()
        .find(|candidate| compiler.interner.resolve(candidate.name) == def)
        .unwrap_or_else(|| panic!("`{def}` is lowered"));
    let mut calls = Vec::new();
    collect_calls(compiler, &def.expr, name, &mut calls);
    calls
}

const CONTEXTUAL_SIZED: &str = r#"
class Sized<a> {
    fn size(x: a) -> Int
}

instance Sized<Int> {
    fn size(x) { x }
}

instance Sized<a> => Sized<List<a>> {
    fn size(xs) {
        match xs {
            [h | t] -> size(h) + size(t),
            _ -> 0
        }
    }
}

fn measure<a: Sized>(x: a) -> Int {
    size(x)
}
"#;

/// 1e: a call to a constrained function passes the dictionaries its
/// callee's evidence names, ahead of its own arguments: an instance's
/// global, or a contextual instance's applied to its context.
#[test]
fn emitting_evidence_passes_a_constrained_calls_dictionaries() {
    let (program, interner) = parse_program(&format!(
        r#"{CONTEXTUAL_SIZED}
fn main() {{
    measure(3) + measure([1, 2])
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = lower_emitting_evidence(&mut compiler, &program);
    assert_eq!(
        calls_in(&compiler, &core, "main", "measure"),
        [
            vec!["__dict_Sized_Int", "_"],
            vec!["__dict_Sized_List<a>(__dict_Sized_Int)", "_"],
        ]
    );
}

/// 1e: a method call on a type with an instance calls the instance's method
/// directly, passing the dictionaries the instance's context needs. Inside
/// the contextual instance, the call on the element reads the instance's own
/// context parameter, and the call on the tail passes it on.
#[test]
fn emitting_evidence_calls_an_instances_method_directly() {
    let (program, interner) = parse_program(&format!(
        r#"{CONTEXTUAL_SIZED}
fn main() {{
    size(3) + size([4])
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = lower_emitting_evidence(&mut compiler, &program);
    assert_eq!(
        calls_in(&compiler, &core, "main", "__tc_Sized_Int_size"),
        [vec!["_"]]
    );
    assert_eq!(
        calls_in(&compiler, &core, "main", "__tc_Sized_List<a>_size"),
        [vec!["__dict_Sized_Int", "_"]]
    );

    let list_size = core
        .defs
        .iter()
        .find(|def| compiler.interner.resolve(def.name) == "__tc_Sized_List<a>_size")
        .expect("the list instance's method is lowered");
    let (names, params) = lambda_param_names(&compiler, &list_size.expr);
    assert_eq!(names[0], "__dict_Sized");
    let mut projected = Vec::new();
    collect_projections(&list_size.expr, &mut projected);
    assert_eq!(
        projected,
        [params[0].id],
        "`size(h)` reads the context's dictionary"
    );
    assert_eq!(
        calls_in(
            &compiler,
            &core,
            "__tc_Sized_List<a>_size",
            "__tc_Sized_List<a>_size"
        ),
        [vec!["__dict_Sized", "t"]],
        "`size(t)` calls the list instance again with the same context"
    );
}

/// 1e: a site whose predicate has no answer is an internal error naming it,
/// not a call missing its dictionary.
#[test]
fn emitting_evidence_reports_an_unanswered_site() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn measure<a: Sized>(x: a) -> Int {{
    size(x)
}}

fn main() {{
    measure(3)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let program = program_to_lower(&mut compiler, &program);
    let main_body = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { name, body, .. }
                if compiler.interner.resolve(*name) == "main" =>
            {
                Some(body)
            }
            _ => None,
        })
        .expect("`main` is parsed");
    let Some(Statement::Expression {
        expression: crate::syntax::expression::Expression::Call { function, .. },
        ..
    }) = main_body.statements.last()
    else {
        panic!("`main` ends in a call: {main_body:?}");
    };
    let callee = function.expr_id();
    // A second `Sized` predicate at the callee, which nothing answered.
    let mut evidence = compiler.evidence_map.clone();
    let raised = evidence
        .predicate(&crate::types::evidence::EvidenceSite::new(callee, 0))
        .expect("`measure` raised `Sized`")
        .clone();
    evidence.raise(callee, raised);

    let (_, errors) = lower_emitting(&compiler, &program, &evidence);
    let messages: Vec<_> = errors
        .iter()
        .map(|error| error.message().unwrap_or_default().to_string())
        .collect();
    assert_eq!(messages.len(), 1, "one error, for the site: {messages:?}");
    assert!(
        messages[0].contains("Sized") && messages[0].contains("no answer"),
        "the error names the predicate: {messages:?}"
    );
}

/// A recursive call to a function with a declared signature instantiates the
/// signature, bounds included, so it raises their predicates and the solver
/// answers them from the function's own parameters. Without the bounds the
/// call raised nothing, and evidence had no dictionary for it to pass.
#[test]
fn a_recursive_call_raises_its_signatures_bounds() {
    let (program, interner) = parse_program(
        r#"
fn has<a: Eq>(xs: List<a>, x: a) -> Bool {
    match xs {
        [h | t] -> if h == x { true } else { has(t, x) },
        _ -> false
    }
}

fn main() {
    has([1, 2], 2)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let (has_id, body) = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { name, id, body, .. }
                if compiler.interner.resolve(*name) == "has" =>
            {
                Some((*id, body))
            }
            _ => None,
        })
        .expect("`has` is parsed");
    let mut callees = Vec::new();
    collect_callees_named(&compiler, body, "has", &mut callees);
    let [recursive] = callees.as_slice() else {
        panic!("one recursive call, got {callees:?}");
    };

    let evidence = &compiler.evidence_map;
    assert_eq!(evidence.raised(*recursive), 1, "the call raises `Eq`");
    assert_eq!(
        evidence.dict_args_at(*recursive),
        Some(vec![crate::types::translate::DictArg::Param {
            owner: has_id,
            index: 0,
            path: Vec::new(),
        }]),
        "answered by `has`'s own dictionary"
    );
}

/// The ids of the callee identifiers named `name` in `block`.
fn collect_callees_named(
    compiler: &Compiler,
    block: &crate::syntax::block::Block,
    name: &str,
    out: &mut Vec<crate::syntax::expression::ExprId>,
) {
    use crate::syntax::expression::Expression;
    fn walk(
        compiler: &Compiler,
        expr: &Expression,
        name: &str,
        out: &mut Vec<crate::syntax::expression::ExprId>,
    ) {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if let Expression::Identifier {
                    name: callee, id, ..
                } = function.as_ref()
                    && compiler.interner.resolve(*callee) == name
                {
                    out.push(*id);
                }
                walk(compiler, function, name, out);
                arguments
                    .iter()
                    .for_each(|arg| walk(compiler, arg, name, out));
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                walk(compiler, condition, name, out);
                collect_callees_named(compiler, consequence, name, out);
                if let Some(alternative) = alternative {
                    collect_callees_named(compiler, alternative, name, out);
                }
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                walk(compiler, scrutinee, name, out);
                arms.iter()
                    .for_each(|arm| walk(compiler, &arm.body, name, out));
            }
            _ => {}
        }
    }
    for stmt in &block.statements {
        if let Statement::Expression { expression, .. } = stmt {
            walk(compiler, expression, name, out);
        }
    }
}

/// E0: a call inference classified as a class-method call, but which raised
/// no predicate, is an internal error in `Emit` mode, not a call to the
/// method's dispatch stub.
#[test]
fn emitting_evidence_reports_a_method_call_that_raised_no_predicate() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn main() {{
    size(3)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");
    let program = program_to_lower(&mut compiler, &program);

    let size_call = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { name, body, .. }
                if compiler.interner.resolve(*name) == "main" =>
            {
                match body.statements.last() {
                    Some(Statement::Expression { expression, .. }) => Some(expression.expr_id()),
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("`main` ends in the call");
    assert!(
        compiler.evidence_map.is_method_call(size_call),
        "inference classifies `size(3)` as a method call"
    );
    // Classified, and raised nothing: what a unit compile records for a
    // method its inference could not type.
    let mut evidence = crate::types::evidence::EvidenceMap::new();
    evidence.record_method_calls(&std::collections::HashSet::from([size_call]));
    let (_, errors) = lower_emitting(&compiler, &program, &evidence);
    let messages: Vec<_> = errors
        .iter()
        .map(|error| error.message().unwrap_or_default().to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("class method `size` raised no predicate")),
        "the call to `size` is reported: {messages:?}"
    );
}

/// E0: a parameter named like a class method shadows the method. Calling it
/// is a call to the parameter, so inference does not classify it as a
/// class-method call, though the method's dispatch stub is bound further out.
#[test]
fn a_parameter_named_like_a_method_is_not_a_method_call() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn apply(size, x) {{
    size(x)
}}

fn main() {{
    apply(fn(n) {{ n + 1 }}, 3) + size(4)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let call_in = |function: &str| {
        program
            .statements
            .iter()
            .find_map(|stmt| match stmt {
                Statement::Function { name, body, .. }
                    if compiler.interner.resolve(*name) == function =>
                {
                    match body.statements.last() {
                        Some(Statement::Expression { expression, .. }) => {
                            Some(expression.expr_id())
                        }
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the function ends in an expression")
    };
    assert!(
        !compiler.evidence_map.is_method_call(call_in("apply")),
        "`size(x)` calls the parameter"
    );
    let size_4 = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Function { name, body, .. }
                if compiler.interner.resolve(*name) == "main" =>
            {
                match body.statements.last() {
                    Some(Statement::Expression {
                        expression: crate::syntax::expression::Expression::Infix { right, .. },
                        ..
                    }) => Some(right.expr_id()),
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("`main` ends in a sum");
    assert!(
        compiler.evidence_map.is_method_call(size_4),
        "`size(4)` calls the method"
    );
}

/// A call qualified by the module that declares the class, in the same
/// program, is typed as the method and raises its predicate. The method's
/// dispatch stub is not a public member of the module, so no member scheme
/// types `M.my_eq`; the class env's signature does.
#[test]
fn a_call_qualified_by_the_declaring_module_raises_its_predicate() {
    let (program, interner) = parse_program(
        r#"
module M {
    public class MyEq<a> {
        fn my_eq(x: a, y: a) -> Bool
    }
    public instance MyEq<Int> {
        fn my_eq(x, y) { x == y }
    }
}

M.my_eq(1, 2);
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let call = program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Expression { expression, .. } => Some(expression.expr_id()),
            _ => None,
        })
        .expect("the program ends in the call");
    assert!(
        compiler.evidence_map.is_method_call(call),
        "inference classifies `M.my_eq(1, 2)` as a method call"
    );
    assert_eq!(
        compiler.evidence_map.raised(call),
        1,
        "the call raises its `MyEq` predicate"
    );
}

/// 1e, C2: the Core path lowers from evidence. A bounded function nested in
/// a body gets its own dictionary parameter, which elaboration never gave
/// it, and the program runs its passes with no internal error.
#[test]
fn core_path_gives_a_nested_bounded_function_its_parameter() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn outer<a: Sized>(x: a) -> Int {{
    fn inner<b: Sized>(y: b) -> Int {{
        size(y) + size(x)
    }}
    inner(x) + inner(4)
}}

fn main() {{
    outer(3)
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");

    let core = compiler
        .prepare_core_program(&program, false, true)
        .expect("the program lowers");
    let outer = core
        .defs
        .iter()
        .find(|def| compiler.interner.try_resolve(def.name) == Some("outer"))
        .expect("`outer` is lowered");
    let crate::core::CoreExpr::Lam { body, .. } = &outer.expr else {
        panic!("`outer` lowers to a lambda, got {:?}", outer.expr);
    };
    let mut inner = None;
    let mut projected = Vec::new();
    collect_inner_and_projections(&compiler, body, &mut inner, &mut projected);
    let inner = inner.expect("`inner` is lowered as a local function");
    let (names, _) = lambda_param_names(&compiler, inner);
    assert_eq!(names, ["__dict_Sized", "y"]);
}

/// KI-101: a bounded function without a complete signature gets no declared
/// scheme, so its recursive call raises no predicate. Lowering from evidence
/// would pass it nothing; it is an internal error instead.
#[test]
fn core_path_reports_a_call_that_raised_no_predicate_for_its_dictionaries() {
    let (program, interner) = parse_program(
        r#"
fn has<a: Eq>(xs: List<a>, x: a) {
    match xs {
        [h | t] -> if h == x { true } else { has(t, x) },
        _ -> false
    }
}

fn main() {
    has([1, 2], 2)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    // The VM path reports it too (C3a); the Core path is checked on its own.
    let _ = compiler.compile(&program);

    let error = compiler
        .prepare_core_program(&program, false, true)
        .expect_err("the recursive call has no evidence");
    let message = error.message().unwrap_or_default();
    assert!(
        message.contains("`has`") && message.contains("KI-101"),
        "the error names the callee: {message}"
    );
}

/// 1e, C3a: the VM's CFG path lowers from evidence too. A recursive call that
/// raised no predicate (KI-101) is an internal error there, as on the Core
/// path, rather than a call elaboration re-derives the dictionaries for.
#[test]
fn cfg_path_reports_a_call_that_raised_no_predicate_for_its_dictionaries() {
    let (program, interner) = parse_program(
        r#"
fn has<a: Eq>(xs: List<a>, x: a) {
    match xs {
        [h | t] -> if h == x { true } else { has(t, x) },
        _ -> false
    }
}

fn main() {
    has([1, 2], 2)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let errors = compiler
        .compile(&program)
        .expect_err("the recursive call has no evidence");
    let messages: Vec<_> = errors
        .iter()
        .map(|error| error.message().unwrap_or_default().to_string())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`has`") && message.contains("KI-101")),
        "the error names the callee: {messages:?}"
    );
}

/// A top-level statement is not a definition, but an undetermined class
/// parameter it raises is just as ambiguous: nothing can fix `largest([])`'s
/// element type, so no `Sized` instance can be chosen. It is E459, as inside
/// a function, rather than the `Int` the old lowering guessed.
#[test]
fn an_undetermined_class_parameter_at_top_level_is_reported() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
fn largest<a: Sized>(xs: List<a>) -> Int {{
    0
}}

let n = largest([]);
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let errors = compiler
        .compile(&program)
        .expect_err("the `Sized` instance is undetermined");
    let codes: Vec<_> = errors.iter().filter_map(|error| error.code()).collect();
    assert!(codes.contains(&"E459"), "E459 is reported: {codes:?}");
}

/// A class's default method calls a sibling method bare (`!eq(x, y)`), while
/// a preloaded dependency declares another class with a method of that name.
/// The name has no binding then: the dependency's stub claims it, and two
/// classes declare it, so the imported schemes skip it. The class the name
/// resolves to is still the unit's own, so the call is typed as its method
/// and raises its predicate.
#[test]
fn a_bare_method_call_shadowing_an_imported_method_raises_its_predicate() {
    let (dependency, interner) = parse_program(
        r#"
module Flow.Eq {
    public class Eq<a> {
        fn eq(x: a, y: a) -> Bool
    }
    public instance Eq<Int> {
        fn eq(x, y) { true }
    }
}
"#,
    );
    let (program, interner) = parse_program_with_interner(
        r#"
class LocalEq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool { !eq(x, y) }
}

instance LocalEq<Int> {
    fn eq(x, y) { x == y }
}

fn main() {
    neq(1, 2)
}
"#,
        interner,
    );
    // One compiler for both units, the dependency compiled first: what the
    // driver and the fixture harness do.
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&dependency)
        .expect("the dependency compiles");
    compiler.preload_dependency_program(&dependency);
    compiler
        .compile(&program)
        .expect("every method call raised its predicate");
}

/// A compile that returns early from inside a function's scope must not leave
/// that scope open for the next unit: the compiler is shared across modules,
/// and the next file's first `import` would then be inside a function (E017).
#[test]
fn a_new_compile_starts_at_file_scope() {
    let (program, interner) = parse_program("import Flow.Array as Array\nlet x = 1;\n");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    // What an early return inside `compile_function_statement` leaves behind.
    compiler.enter_scope();
    let result = compiler.compile(&program);
    let codes: Vec<_> = result
        .err()
        .unwrap_or_default()
        .iter()
        .filter_map(|error| error.code())
        .map(str::to_string)
        .collect();
    assert!(
        !codes.iter().any(|code| code == "E017"),
        "the import is at file scope: {codes:?}"
    );
    assert_eq!(compiler.scope_index, 0);
}

/// C3b-2: a private module member's scheme reaches the VM compiler. Neither
/// the qualified `type_env` lookup nor the interface schemes have it, so the
/// compiler read no constraints for it, while its IR takes the dictionary its
/// bound needs.
#[test]
fn a_private_module_member_keeps_its_scheme() {
    let (program, interner) = parse_program(&format!(
        r#"{SIZED_CLASS}
module M {{
    fn total<a: Sized>(x: a) -> Int {{
        size(x)
    }}

    public fn run() -> Int {{
        total(3)
    }}
}}
"#
    ));
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");
    let module = compiler.interner.lookup("M").expect("`M` is interned");
    let member = compiler
        .interner
        .lookup("total")
        .expect("`total` is interned");
    let scheme = compiler
        .private_member_schemes
        .get(&(module, member))
        .expect("the private member's scheme is recorded");
    assert_eq!(
        scheme.constraints.len(),
        1,
        "`total` keeps its `Sized` bound"
    );
}

// ── 0.0.8 plan step 2a: clone per instantiation ────────────────────────

/// Lower `source` on the Core path, clone names pre-interned, and return each
/// definition's name with its Core, as `--dump-core=debug` prints it.
fn lowered_core_defs(source: &str) -> Vec<(String, String)> {
    let (program, interner) = parse_program(source);
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile(&program).expect("the program compiles");
    let core = compiler
        .prepare_core_program(&program, false, true)
        .expect("the program lowers");
    core.defs
        .iter()
        .map(|def| {
            let one = crate::core::CoreProgram {
                defs: vec![def.clone()],
                top_level_items: Vec::new(),
            };
            (
                compiler.interner.resolve(def.name).to_string(),
                crate::core::display::display_program_debug(&one, &compiler.interner),
            )
        })
        .collect()
}

fn core_def<'a>(defs: &'a [(String, String)], name: &str) -> &'a str {
    defs.iter()
        .find(|(def, _)| def == name)
        .map(|(_, core)| core.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no `{name}` among {:?}",
                defs.iter().map(|d| &d.0).collect::<Vec<_>>()
            )
        })
}

const COPY_HEAD: &str = r#"
fn copy_head(xs) {
    match xs {
        [h | t] -> [h | [h | t]],
        _ -> [],
    }
}
"#;

#[test]
fn a_function_used_at_an_unboxed_and_a_boxed_type_gets_one_clone() {
    let defs = lowered_core_defs(&format!(
        "{COPY_HEAD}\nfn main() {{\n    copy_head([1, 2])\n    copy_head([\"a\"])\n}}\n"
    ));
    let clone = core_def(&defs, "copy_head$Int");
    assert!(clone.contains("h#") && clone.contains(":Int,"), "{clone}");
    let generic = core_def(&defs, "copy_head");
    assert!(
        !generic.contains(":Int,"),
        "the generic body stays generic: {generic}"
    );
    let main = core_def(&defs, "main");
    assert!(
        main.contains("copy_head$Int#"),
        "the Int call calls the clone: {main}"
    );
    assert!(
        main.contains("copy_head#"),
        "the String call calls the generic body: {main}"
    );
}

#[test]
fn boxed_instantiations_share_the_generic_body() {
    let defs = lowered_core_defs(&format!(
        "{COPY_HEAD}\nfn main() {{\n    copy_head([\"a\"])\n    copy_head([[1]])\n}}\n"
    ));
    assert!(
        !defs.iter().any(|(name, _)| name.starts_with("copy_head$")),
        "String and List<Int> have one representation, so nothing is cloned"
    );
}

#[test]
fn a_clone_calls_itself() {
    let defs = lowered_core_defs(
        r#"
fn last_or(xs, d) {
    match xs {
        [h | t] -> last_or(t, h),
        _ -> d,
    }
}

fn main() {
    last_or([1, 2], 0)
    last_or([1.5], 0.0)
}
"#,
    );
    for clone in ["last_or$Int", "last_or$Float"] {
        let core = core_def(&defs, clone);
        assert!(core.contains(&format!("{clone}#")), "{core}");
    }
    let generic = core_def(&defs, "last_or");
    assert!(!generic.contains("last_or$"), "{generic}");
}

#[test]
fn a_function_gets_at_most_three_clones() {
    let defs = lowered_core_defs(
        r#"
fn pair(a, b) { (b, a) }

fn main() {
    pair(1, 2)
    pair(1, 2.5)
    pair(1.5, 2)
    pair(1.5, 2.5)
}
"#,
    );
    let clones: Vec<&str> = defs
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("pair$"))
        .collect();
    assert_eq!(clones, ["pair$Int_Int", "pair$Int_Float", "pair$Float_Int"]);
    let main = core_def(&defs, "main");
    assert!(
        main.contains("pair#"),
        "the fourth instantiation calls the generic body: {main}"
    );
}

// ── 0.0.8 plan step 2a-2: re-specialise clones ─────────────────────────

#[test]
fn a_clone_calls_the_clone_its_instantiation_reaches() {
    let defs = lowered_core_defs(&format!(
        "{COPY_HEAD}\nfn twice_head(xs) {{ copy_head(xs) }}\n\nfn main() {{\n    twice_head([1, 2])\n    twice_head([\"a\"])\n}}\n"
    ));
    let clone = core_def(&defs, "twice_head$Int");
    assert!(clone.contains("copy_head$Int#"), "{clone}");
    let generic = core_def(&defs, "twice_head");
    assert!(
        generic.contains("copy_head#") && !generic.contains("copy_head$"),
        "{generic}"
    );
    let reached = core_def(&defs, "copy_head$Int");
    assert!(reached.contains(":Int,"), "{reached}");
}

#[test]
fn an_in_place_body_calls_the_clone_its_instantiation_reaches() {
    let defs = lowered_core_defs(&format!(
        "{COPY_HEAD}\nfn twice_head(xs) {{ copy_head(xs) }}\n\nfn main() {{\n    twice_head([1, 2])\n    copy_head([\"a\"])\n}}\n"
    ));
    assert!(
        !defs.iter().any(|(name, _)| name.starts_with("twice_head$")),
        "one instantiation is specialised in place"
    );
    let in_place = core_def(&defs, "twice_head");
    assert!(in_place.contains("copy_head$Int#"), "{in_place}");
}

/// The group is annotated: unannotated, each member is quantified on its
/// own, `ping` comes out as `forall a, b. (List<a>, b) -> b`, and its call to
/// `pong` names variables its clones do not bind. Step 2c quantifies a group
/// once, and its test covers the unannotated group.
#[test]
fn a_recursive_group_reaches_its_own_clones() {
    let defs = lowered_core_defs(
        r#"
fn ping<a>(xs: List<a>, d: a) -> a {
    match xs {
        [h | t] -> pong(t, h),
        _ -> d,
    }
}

fn pong<a>(xs: List<a>, d: a) -> a {
    match xs {
        [h | t] -> ping(t, h),
        _ -> d,
    }
}

fn main() {
    ping([1, 2], 0)
    ping([1.5], 0.0)
}
"#,
    );
    for (clone, calls) in [
        ("ping$Int", "pong$Int#"),
        ("pong$Int", "ping$Int#"),
        ("ping$Float", "pong$Float#"),
        ("pong$Float", "ping$Float#"),
    ] {
        let core = core_def(&defs, clone);
        assert!(core.contains(calls), "{core}");
    }
}

#[test]
fn a_partly_known_instantiation_is_cloned() {
    let defs = lowered_core_defs(
        r#"
fn pair(a, b) { (b, a) }

fn tag(x) { pair(1, x) }

fn main() {
    tag("a")
    tag([1])
    pair("b", "c")
}
"#,
    );
    let tag = core_def(&defs, "tag");
    assert!(tag.contains("pair$Int_Box#"), "{tag}");
    let main = core_def(&defs, "main");
    assert!(!main.contains("pair$"), "{main}");
}
