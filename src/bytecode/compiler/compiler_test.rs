use crate::{
    bytecode::compiler::Compiler,
    bytecode::op_code::{OpCode, disassemble},
    diagnostics::render_diagnostics,
    runtime::value::Value,
    syntax::{interner::Interner, lexer::Lexer, parser::Parser},
    types::{
        infer_effect_row::InferEffectRow, infer_type::InferType, module_interface::ModuleInterface,
        scheme::Scheme, type_constructor::TypeConstructor,
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
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::from([(old_adt_id, "Color".to_string())]),
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
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::from([(old_effect_id, "IO".to_string())]),
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

    let io_sym = compiler.interner.intern("IO");
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
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
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
        dependency_fingerprints: Vec::new(),
        symbol_table: std::collections::HashMap::new(),
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
fn main() -> Unit {
    print("x")
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
            && rendered
                .contains("Effectful function `main` must declare `with IO` in strict mode."),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_requires_time_annotation_for_non_public_effectful_function() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit {
    let _t = now_ms()
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
            && rendered
                .contains("Effectful function `main` must declare `with Time` in strict mode."),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_reports_missing_time_when_only_io_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit with IO {
    let _t = now_ms()
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
        rendered.contains("Effectful function `main` must declare `with Time` in strict mode."),
        "unexpected diagnostics:\n{}",
        rendered
    );
}

#[test]
fn strict_mode_reports_missing_io_when_only_time_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit with Time {
    print("x")
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
        rendered.contains("Effectful function `main` must declare `with IO` in strict mode."),
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
