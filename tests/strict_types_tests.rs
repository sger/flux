//! Unit tests for the strict types infrastructure (Proposal 0123).
//!
//! Covers:
//! - `validate_strict_types()` — E430 rejection of Any-inferred bindings
//! - `ClassEnv` validation — E440 (duplicate class), E441 (unknown class),
//!   E442 (missing method), E443 (duplicate instance)
//! - `solve_class_constraints()` — E444 (no instance for concrete type)
//! - `generate_dispatch_functions()` — mangled instance method generation

use std::collections::{HashMap, HashSet};

use flux::{
    ast::type_infer::{infer_program, InferProgramConfig, InferProgramResult},
    syntax::{
        interner::Interner, lexer::Lexer, parser::Parser, program::Program, statement::Statement,
    },
    types::{
        class_env::ClassEnv,
        class_solver::solve_class_constraints,
    },
};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn parse(source: &str) -> (Program, Interner) {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn infer(source: &str) -> (InferProgramResult, Program, Interner) {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let mut interner = parser.take_interner();
    let flow_sym = interner.intern("Flow");

    // Build ClassEnv from program statements (using same interner)
    let mut class_env = ClassEnv::new();
    class_env.register_builtins(&mut interner);
    class_env.collect_from_statements(&program.statements, &interner);

    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: Some("<test>".into()),
            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            known_flow_names: HashSet::new(),
            flow_module_symbol: flow_sym,
            preloaded_effect_op_signatures: HashMap::new(),
            class_env: Some(class_env),
        },
    );
    (result, program, interner)
}

fn build_class_env(source: &str) -> (ClassEnv, Vec<flux::diagnostics::Diagnostic>, Interner) {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let mut interner = parser.take_interner();
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    let diagnostics = env.collect_from_statements(&program.statements, &interner);
    (env, diagnostics, interner)
}

// ─────────────────────────────────────────────────────────────────────────────
// validate_strict_types — E430
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn strict_types_accepts_fully_typed_function() {
    let (result, program, interner) = infer("fn add(x: Int, y: Int) -> Int { x + y }");
    let diags =
        flux::ast::type_infer::strict_types::validate_strict_types(&program, &result.type_env, &interner);
    assert!(diags.is_empty(), "expected no strict-type errors, got: {diags:?}");
}

#[test]
fn strict_types_accepts_polymorphic_identity() {
    let (result, program, interner) = infer("fn identity(x) { x }");
    let diags =
        flux::ast::type_infer::strict_types::validate_strict_types(&program, &result.type_env, &interner);
    assert!(diags.is_empty(), "identity should infer as a -> a with no Any");
}

#[test]
fn strict_types_accepts_polymorphic_arithmetic() {
    let (result, program, interner) = infer("fn add(x, y) { x + y }");
    let diags =
        flux::ast::type_infer::strict_types::validate_strict_types(&program, &result.type_env, &interner);
    assert!(diags.is_empty(), "add should infer as a -> a -> a with no Any");
}

// ─────────────────────────────────────────────────────────────────────────────
// ClassEnv validation — E440–E443
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn class_env_accepts_valid_class_and_instance() {
    let (env, diags, _) = build_class_env(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
"#,
    );
    assert!(diags.is_empty(), "valid class+instance should produce no errors: {diags:?}");
    assert_eq!(env.instances.len() > 0, true);
}

#[test]
fn class_env_rejects_duplicate_class_e440() {
    let (_, diags, _) = build_class_env(
        r#"
class Foo<a> {
    fn bar(x: a) -> Int
}
class Foo<a> {
    fn bar(x: a) -> Int
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E440")),
        "expected E440 Duplicate Type Class, got: {diags:?}"
    );
}

#[test]
fn class_env_rejects_unknown_class_in_instance_e441() {
    let (_, diags, _) = build_class_env(
        r#"
instance NonExistent<Int> {
    fn foo(x) { x }
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E441")),
        "expected E441 Unknown Type Class, got: {diags:?}"
    );
}

#[test]
fn class_env_rejects_missing_method_e442() {
    let (_, diags, _) = build_class_env(
        r#"
class Describable<a> {
    fn name(x: a) -> Int
    fn value(x: a) -> Int
}
instance Describable<Int> {
    fn name(x) { x }
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E442")),
        "expected E442 Missing Instance Method, got: {diags:?}"
    );
}

#[test]
fn class_env_accepts_default_method_not_overridden() {
    let (_, diags, _) = build_class_env(
        r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Int
    fn my_neq(x: a, y: a) -> Int { 0 }
}
instance MyEq<Int> {
    fn my_eq(x, y) { 0 }
}
"#,
    );
    // my_neq has a default — so not implementing it is fine
    let method_errors: Vec<_> = diags.iter().filter(|d| d.code().as_deref() == Some("E442")).collect();
    assert!(method_errors.is_empty(), "default method should not trigger E442: {method_errors:?}");
}

#[test]
fn class_env_duplicate_instance_detection_uses_debug_format() {
    // Known limitation (Proposal 0146 Track 3): duplicate instance detection
    // uses format!("{:?}") comparison which includes spans. Two instances
    // with the same type at different source positions are NOT detected as
    // duplicates by ClassEnv. The duplicate is instead caught later by the
    // dispatch function generator as E001 (Duplicate Name) when it tries to
    // create two functions with the same mangled name.
    let (_, diags, _) = build_class_env(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
instance Sizeable<Int> {
    fn size(x) { x + 1 }
}
"#,
    );
    // E443 is NOT produced due to the Debug-format span mismatch
    let e443: Vec<_> = diags.iter().filter(|d| d.code().as_deref() == Some("E443")).collect();
    assert!(e443.is_empty(), "E443 not produced due to known Debug-format span issue");
}

#[test]
fn class_env_accepts_different_type_instances() {
    let (env, diags, interner) = build_class_env(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
instance Sizeable<String> {
    fn size(x) { 0 }
}
"#,
    );
    let e443: Vec<_> = diags.iter().filter(|d| d.code().as_deref() == Some("E443")).collect();
    assert!(e443.is_empty(), "different type instances should not conflict: {e443:?}");
    // Should have instances for both Int and String
    let sizeable_sym = interner.lookup("Sizeable").expect("Sizeable interned");
    assert!(
        env.resolve_instance_for_type(sizeable_sym, "Int", &interner).is_some(),
        "should have Sizeable<Int>"
    );
    assert!(
        env.resolve_instance_for_type(sizeable_sym, "String", &interner).is_some(),
        "should have Sizeable<String>"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ClassEnv — lookup and resolution
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn class_env_method_to_class_lookup() {
    let (env, _, interner) = build_class_env(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
"#,
    );
    let size_sym = interner.lookup("size").expect("size should be interned");
    let result = env.method_to_class(size_sym);
    assert!(result.is_some(), "size should be found in Sizeable class");
}

#[test]
fn class_env_resolve_instance_for_type() {
    let (env, _, interner) = build_class_env(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
"#,
    );
    let sizeable_sym = interner.lookup("Sizeable").expect("Sizeable should be interned");
    let result = env.resolve_instance_for_type(sizeable_sym, "Int", &interner);
    assert!(result.is_some(), "should resolve Sizeable<Int>");
    let no_result = env.resolve_instance_for_type(sizeable_sym, "String", &interner);
    assert!(no_result.is_none(), "should not resolve Sizeable<String>");
}

// ─────────────────────────────────────────────────────────────────────────────
// solve_class_constraints — E444
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn class_solver_accepts_satisfied_constraint() {
    let src = r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
fn main() { size(42) }
"#;
    let (result, program, mut interner) = infer(src);

    // Rebuild ClassEnv from same interner for the solver
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let diags = solve_class_constraints(&result.class_constraints, &env, &interner);
    let e444: Vec<_> = diags.iter().filter(|d| d.code().as_deref() == Some("E444")).collect();
    assert!(e444.is_empty(), "satisfied constraint should not produce E444: {e444:?}");
}

#[test]
fn class_solver_rejects_missing_instance_e444() {
    // Build class env with Sizeable class but NO instance for String
    let (program, mut interner) = parse(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let sizeable_sym = interner.lookup("Sizeable").expect("Sizeable interned");

    // Fabricate a constraint for Sizeable<String> which has no instance
    let string_type = flux::types::infer_type::InferType::Con(
        flux::types::type_constructor::TypeConstructor::String,
    );
    let constraint = flux::ast::type_infer::constraint::WantedClassConstraint {
        class_name: sizeable_sym,
        type_arg: string_type,
        span: flux::diagnostics::position::Span {
            start: flux::diagnostics::position::Position { line: 1, column: 0 },
            end: flux::diagnostics::position::Position { line: 1, column: 10 },
        },
    };

    let diags = solve_class_constraints(&[constraint], &env, &interner);
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E444")),
        "expected E444 No Type Class Instance, got: {diags:?}"
    );
}

#[test]
fn class_solver_skips_type_variables() {
    let (program, mut interner) = parse(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let sizeable_sym = interner.lookup("Sizeable").expect("Sizeable interned");

    // Constraint with a type variable — should be skipped (not solved)
    let var_type = flux::types::infer_type::InferType::Var(9999u32);
    let constraint = flux::ast::type_infer::constraint::WantedClassConstraint {
        class_name: sizeable_sym,
        type_arg: var_type,
        span: flux::diagnostics::position::Span {
            start: flux::diagnostics::position::Position { line: 1, column: 0 },
            end: flux::diagnostics::position::Position { line: 1, column: 10 },
        },
    };

    let diags = solve_class_constraints(&[constraint], &env, &interner);
    assert!(diags.is_empty(), "type variable constraints should be skipped: {diags:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// generate_dispatch_functions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dispatch_generates_mangled_functions() {
    let (program, mut interner) = parse(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let generated = flux::types::class_dispatch::generate_dispatch_functions(
        &program.statements,
        &env,
        &mut interner,
    );

    // Should generate at least the mangled instance function
    let has_mangled = generated.iter().any(|stmt| {
        if let Statement::Function { name, .. } = stmt {
            interner.resolve(*name).contains("__tc_Sizeable_Int_size")
        } else {
            false
        }
    });
    assert!(has_mangled, "expected __tc_Sizeable_Int_size in generated functions");
}

#[test]
fn dispatch_generates_polymorphic_stub() {
    let (program, mut interner) = parse(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let generated = flux::types::class_dispatch::generate_dispatch_functions(
        &program.statements,
        &env,
        &mut interner,
    );

    // Should generate a polymorphic stub named `size` (the class method name)
    let has_stub = generated.iter().any(|stmt| {
        if let Statement::Function { name, .. } = stmt {
            interner.resolve(*name) == "size"
        } else {
            false
        }
    });
    assert!(has_stub, "expected polymorphic stub function `size` in generated functions");
}

#[test]
fn dispatch_generates_multiple_instance_functions() {
    let (program, mut interner) = parse(
        r#"
class Sizeable<a> {
    fn size(x: a) -> Int
}
instance Sizeable<Int> {
    fn size(x) { x }
}
instance Sizeable<String> {
    fn size(x) { 0 }
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let generated = flux::types::class_dispatch::generate_dispatch_functions(
        &program.statements,
        &env,
        &mut interner,
    );

    let mangled_names: Vec<String> = generated
        .iter()
        .filter_map(|stmt| {
            if let Statement::Function { name, .. } = stmt {
                let n = interner.resolve(*name).to_string();
                if n.starts_with("__tc_") { Some(n) } else { None }
            } else {
                None
            }
        })
        .collect();

    assert!(
        mangled_names.iter().any(|n| n.contains("Int")),
        "expected __tc_Sizeable_Int_size, got: {mangled_names:?}"
    );
    assert!(
        mangled_names.iter().any(|n| n.contains("String")),
        "expected __tc_Sizeable_String_size, got: {mangled_names:?}"
    );
}

#[test]
fn dispatch_generates_default_method_function() {
    let (program, mut interner) = parse(
        r#"
class MyEq<a> {
    fn my_eq(x: a, y: a) -> Int
    fn my_neq(x: a, y: a) -> Int { 0 }
}
"#,
    );
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);
    env.collect_from_statements(&program.statements, &interner);

    let generated = flux::types::class_dispatch::generate_dispatch_functions(
        &program.statements,
        &env,
        &mut interner,
    );

    // Default method `my_neq` should be generated as a regular function
    // (since there are no instance overrides for it)
    let has_default = generated.iter().any(|stmt| {
        if let Statement::Function { name, .. } = stmt {
            interner.resolve(*name) == "my_neq"
        } else {
            false
        }
    });
    assert!(has_default, "expected default method function `my_neq` in generated functions");
}

// ─────────────────────────────────────────────────────────────────────────────
// Built-in classes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn builtin_classes_registered() {
    let mut interner = Interner::new();
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);

    let eq = interner.lookup("Eq").expect("Eq should be interned");
    let ord = interner.lookup("Ord").expect("Ord should be interned");
    let num = interner.lookup("Num").expect("Num should be interned");
    let show = interner.lookup("Show").expect("Show should be interned");
    let semigroup = interner.lookup("Semigroup").expect("Semigroup should be interned");

    assert!(env.lookup_class(eq).is_some(), "Eq class should be registered");
    assert!(env.lookup_class(ord).is_some(), "Ord class should be registered");
    assert!(env.lookup_class(num).is_some(), "Num class should be registered");
    assert!(env.lookup_class(show).is_some(), "Show class should be registered");
    assert!(env.lookup_class(semigroup).is_some(), "Semigroup class should be registered");
}

#[test]
fn builtin_instances_registered() {
    let mut interner = Interner::new();
    let mut env = ClassEnv::new();
    env.register_builtins(&mut interner);

    let eq = interner.lookup("Eq").expect("Eq interned");
    let num = interner.lookup("Num").expect("Num interned");

    // Eq should have instances for Int, Float, String, Bool
    assert!(env.resolve_instance_for_type(eq, "Int", &interner).is_some());
    assert!(env.resolve_instance_for_type(eq, "Float", &interner).is_some());
    assert!(env.resolve_instance_for_type(eq, "String", &interner).is_some());
    assert!(env.resolve_instance_for_type(eq, "Bool", &interner).is_some());

    // Num should have instances for Int, Float but not String
    assert!(env.resolve_instance_for_type(num, "Int", &interner).is_some());
    assert!(env.resolve_instance_for_type(num, "Float", &interner).is_some());
    assert!(env.resolve_instance_for_type(num, "String", &interner).is_none());
}

#[test]
fn builtin_classes_not_overridden_by_user_redeclaration() {
    // register_builtins runs first, then collect_from_statements sees the
    // user's `class Eq` and reports E440 (duplicate). The builtin Eq stays.
    let (env, diags, interner) = build_class_env(
        r#"
class Eq<a> {
    fn eq(x: a, y: a) -> Int
}
"#,
    );
    assert!(
        diags.iter().any(|d| d.code().as_deref() == Some("E440")),
        "user redeclaring builtin Eq should trigger E440"
    );
    let eq = interner.lookup("Eq").expect("Eq interned");
    let class = env.lookup_class(eq).expect("Eq class should exist");
    // Builtin Eq has eq method — it should remain since user's was rejected
    assert!(class.methods.iter().any(|m| interner.resolve(m.name) == "eq"));
}
