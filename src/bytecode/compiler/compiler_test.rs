use crate::{
    bytecode::symbol_scope::SymbolScope,
    bytecode::compiler::Compiler,
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
fn compile_with_opts_skips_tail_call_analysis_without_optimization() {
    let (program, interner) = parse_program("fn f(n) { f(n - 1) }");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler.compile_with_opts(&program, false, false).unwrap();
    assert!(compiler.tail_calls.is_empty());
}

#[test]
fn compiler_registers_base_builtins_in_registry_order() {
    let (_, interner) = parse_program("");
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    let base = BaseModule::new();

    for (expected_index, name) in base.names().enumerate() {
        let symbol = compiler.interner.intern(name);
        let binding = compiler
            .symbol_table
            .resolve(symbol)
            .expect("base builtin should be pre-registered");
        assert_eq!(binding.symbol_scope, SymbolScope::Base);
        assert_eq!(binding.index, expected_index);
    }
}

#[test]
fn builtin_indices_are_deterministic_across_interner_state() {
    let mut seeded_interner = Interner::new();
    // Pre-seed unrelated symbols to prove builtin indices do not depend on interner history.
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
            .expect("builtin must exist in compiler A");
        let binding_b = compiler_b
            .symbol_table
            .resolve(sym_b)
            .expect("builtin must exist in compiler B");

        assert_eq!(binding_a.symbol_scope, SymbolScope::Base);
        assert_eq!(binding_b.symbol_scope, SymbolScope::Base);
        assert_eq!(
            binding_a.index, binding_b.index,
            "builtin index mismatch for `{}`",
            name
        );
    }
}
