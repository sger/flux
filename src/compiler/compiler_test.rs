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
) -> Option<std::rc::Rc<crate::runtime::compiled_function::CompiledFunction>> {
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
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
    let logger_h = compiler.interner.intern("h");
    let log_name = compiler.interner.intern("log");
    let string_name = compiler.interner.intern("String");
    let int_name = compiler.interner.intern("Int");
    let console_name = compiler.interner.intern("Console");

    let class_interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
    };
    let instance_interface = ModuleInterface {
        module_name: "Example.StdLog".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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

    compiler.preload_module_interface(&class_interface);
    compiler.preload_module_interface(&instance_interface);
    let mangled = compiler.interner.intern("__tc_Logger_Int_log");
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
    let logger_h = compiler.interner.intern("h");
    let log_name = compiler.interner.intern("log");
    let string_name = compiler.interner.intern("String");

    let interface = ModuleInterface {
        module_name: "Example.Logger".to_string(),
        source_hash: "hash".to_string(),
        compiler_version: env!("CARGO_PKG_VERSION").to_string(),
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
    };

    compiler.preload_module_interface(&interface);
    compiler
        .compile(&program)
        .expect("program with imported public class instance should compile");

    let prepared = compiler.prepare_program_for_lowering_with_preloaded(&program);
    assert!(
        top_level_has_function(
            &prepared.effective_program.statements,
            "__tc_Logger_StdoutHandle_log",
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
