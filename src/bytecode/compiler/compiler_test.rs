use crate::{
    bytecode::compiler::Compiler,
    bytecode::op_code::OpCode,
    bytecode::symbol_scope::SymbolScope,
    diagnostics::render_diagnostics,
    runtime::base::BaseModule,
    runtime::value::Value,
    syntax::{interner::Interner, lexer::Lexer, parser::Parser},
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
fn compiler_registers_base_functions_in_registry_order() {
    let (_, interner) = parse_program("");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let base = BaseModule::new();

    for (expected_index, name) in base.names().enumerate() {
        let symbol = compiler.interner.intern(name);
        let binding = compiler
            .symbol_table
            .resolve(symbol)
            .expect("base base should be pre-registered");
        assert_eq!(binding.symbol_scope, SymbolScope::Base);
        assert_eq!(binding.index, expected_index);
    }
}

#[test]
fn base_indices_are_deterministic_across_interner_state() {
    let mut seeded_interner = Interner::new();
    // Pre-seed unrelated symbols to prove base indices do not depend on interner history.
    seeded_interner.intern("zzz");
    seeded_interner.intern("another_symbol");

    let mut compiler_a = Compiler::new_with_interner("<test-a>", Interner::new());
    let mut compiler_b = Compiler::new_with_interner("<test-b>", seeded_interner);

    for name in BaseModule::new().names() {
        let sym_a = compiler_a.interner.intern(name);
        let sym_b = compiler_b.interner.intern(name);
        let binding_a = compiler_a
            .symbol_table
            .resolve(sym_a)
            .expect("base must exist in compiler A");
        let binding_b = compiler_b
            .symbol_table
            .resolve(sym_b)
            .expect("base must exist in compiler B");

        assert_eq!(binding_a.symbol_scope, SymbolScope::Base);
        assert_eq!(binding_b.symbol_scope, SymbolScope::Base);
        assert_eq!(
            binding_a.index, binding_b.index,
            "base index mismatch for `{}`",
            name
        );
    }
}

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
import Base except [flatten]

fn main() {
    fn flatten(x) { x }
    flatten(1)
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("nested function should shadow excluded Base name");
}

#[test]
fn local_let_can_shadow_base_name() {
    let (program, interner) = parse_program(
        r#"
import Base except [len]

fn main() {
    let len = 1
    len
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("local let should shadow excluded Base name");
}

#[test]
fn match_pattern_can_shadow_base_name() {
    let (program, interner) = parse_program(
        r#"
import Base except [len]

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
        .expect("pattern binding should shadow excluded Base name");
}

#[test]
fn parameter_can_shadow_base_name() {
    let (program, interner) = parse_program(
        r#"
import Base except [len]

fn id(len) { len }
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .expect("parameter binding should shadow excluded Base name");
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

#[test]
fn strict_mode_accepts_io_annotation_when_io_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit with IO {
    print("x")
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    compiler
        .compile(&program)
        .expect("strict mode should accept with IO for print");
}

#[test]
fn strict_mode_accepts_time_annotation_when_time_is_declared() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit with Time {
    let _t = now_ms()
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    compiler
        .compile(&program)
        .expect("strict mode should accept with Time for now_ms");
}

#[test]
fn strict_mode_accepts_io_and_time_when_both_are_declared() {
    let (program, interner) = parse_program(
        r#"
fn main() -> Unit with IO, Time {
    print(now_ms())
}
"#,
    );
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.strict_mode = true;
    compiler
        .compile(&program)
        .expect("strict mode should accept with IO, Time when both are required");
}

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
