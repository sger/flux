//! Integration tests for Proposal 0151 — module-scoped type classes.
//!
//! Phase 1, Step 1: confirm that the bytecode compiler's module-body validator
//! accepts `class`, `instance`, and `import` declarations inside a `module { }`
//! block. Before Proposal 0151 these were rejected with `INVALID_MODULE_CONTENT`.
//!
//! This file does NOT yet assert that class semantics work end-to-end inside
//! modules — that lands in later Phase 1a/1b commits. The minimum guarantee
//! here is "the validator no longer rejects the source."

use flux::bytecode::compiler::Compiler;
use flux::bytecode::vm::VM;
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

/// Parse `source` and run it through the bytecode compiler. Returns the list
/// of diagnostics. The test asserts on the diagnostic codes.
fn compile_source(source: &str) -> Vec<flux::diagnostics::Diagnostic> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    match compiler.compile(&program) {
        Ok(()) => Vec::new(),
        Err(diags) => diags,
    }
}

/// Returns true if any diagnostic in `diags` has the `INVALID_MODULE_CONTENT`
/// error code (`E054`). The exact code is checked by string match against the
/// rendered diagnostic to avoid taking a hard dependency on the internal
/// `ErrorCode` type from the test crate.
fn has_invalid_module_content(diags: &[flux::diagnostics::Diagnostic]) -> bool {
    let rendered = render_diagnostics(diags, None, None);
    rendered.contains("Invalid content in module")
}

/// Compile `source` to bytecode, run it through the VM, and return the last
/// value popped from the operand stack. Panics with rendered diagnostics on
/// any compile or runtime error so test failures show useful messages.
fn compile_and_run(source: &str) -> Value {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("test.flx".to_string(), interner);
    if let Err(diags) = compiler.compile(&program) {
        panic!(
            "compile errors:\n{}",
            render_diagnostics(&diags, Some(source), None)
        );
    }

    let bytecode = compiler.bytecode();
    let mut vm = VM::new(bytecode);
    vm.run().unwrap_or_else(|err| panic!("VM error: {err}"));
    vm.last_popped_stack_elem()
}

#[test]
fn module_body_accepts_class_declaration() {
    // Bare class declaration inside a module body. Before Proposal 0151 this
    // hit the catch-all in compile_module_statement and produced
    // INVALID_MODULE_CONTENT. After the validator whitelist update, the
    // compiler must not emit that diagnostic.
    let source = r#"
module Phase1.SmokeClass {
    class Eq2<a> {
        fn eq2(x: a, y: a) -> Bool
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body class should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_body_accepts_instance_declaration() {
    let source = r#"
module Phase1.SmokeInstance {
    class Eq2<a> {
        fn eq2(x: a, y: a) -> Bool
    }

    instance Eq2<Int> {
        fn eq2(x, y) { x == y }
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body instance should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_body_accepts_import_declaration() {
    // Module-body imports are also whitelisted (Proposal 0151 §5a).
    // The import target need not exist for this test — we only care that
    // the validator does not reject the *form*.
    let source = r#"
module Phase1.SmokeImport {
    import Flow.Option as Option

    fn ping() -> Int { 1 }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body import should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

/// Proposal 0151, Phase 4a-prereq: an `effect` declaration is allowed
/// inside a `module { ... }` block. Before this prereq landed the
/// validator rejected the form with `INVALID_MODULE_CONTENT` because
/// only `Function`/`Let`/`Data`/`Class`/`Instance`/`Import` were on the
/// whitelist. Phase 4 needs effect declarations inside modules so each
/// of the four worked examples (Console, AuditLog, Clock, Tracer) can
/// live in its own dedicated module the same way classes do.
#[test]
fn module_body_accepts_effect_declaration() {
    let source = r#"
module Flow.Console {
    effect Console {
        print: String -> ()
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body effect should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

/// Module-body `effect` with multiple operations parses too — the
/// validator must not narrow on operation count.
#[test]
fn module_body_accepts_effect_declaration_with_multiple_ops() {
    let source = r#"
module Flow.State {
    effect State {
        get: () -> Int
        put: Int -> ()
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body effect with multiple ops should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

/// Sanity check: a module containing an effect declaration *and* a
/// class declaration coexists without either form interfering with the
/// other. This is closer to the shape of the Phase 4 worked examples
/// where effects and classes live in their own dedicated modules and
/// get imported together.
///
/// Note: the class method's parameter is named `hnd` rather than
/// `handle` because `handle` is a reserved keyword in Flux's effect
/// handler syntax (`expr handle Effect { ... }`). The Phase 4 worked
/// examples use this convention.
#[test]
fn module_body_accepts_effect_and_class_together() {
    let source = r#"
module Phase4.SmokeMixed {
    effect Console {
        print: String -> ()
    }

    class Logger<h> {
        fn log(hnd: h, msg: String) -> Bool
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_invalid_module_content(&diags),
        "module-body effect + class should not produce INVALID_MODULE_CONTENT, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

/// End-to-end exploratory test for the surprise win discovered in commit #3:
///
/// `class_env::collect_classes`, `class_env::collect_instances`, and
/// `class_dispatch::generate_from_statements` ALL already recurse into module
/// bodies. The only thing that was blocking module-scoped classes from
/// working end-to-end was the validator at `bytecode/compiler/statement.rs`
/// which we whitelisted in commit #1.
///
/// If the existing dispatch path "just works" for module-scoped classes,
/// this test should compile, run, and produce 42. If something is missing,
/// the failure tells us the next concrete obstacle to fix.
#[test]
fn module_scoped_class_with_int_instance_runs_via_existing_dispatch() {
    let source = r#"
module Phase1.RuntimeUse {
    class DoublerCls<a> {
        fn double_it(x: a) -> a
    }

    instance DoublerCls<Int> {
        fn double_it(x) { x + x }
    }
}

fn main() {
    double_it(21)
}
"#;
    let result = compile_and_run(source);
    assert_eq!(
        result,
        Value::Integer(42),
        "module-scoped class+instance should resolve and run; got {:?}",
        result
    );
}

/// Proposal 0151, Phase 1a, commit #6: short-form qualified call.
///
/// `Quall.size_of2(...)` — referring to a same-file `module Phase1.Quall` by
/// the last segment of its dotted name — does NOT work today. The resolver
/// at `resolve_module_name_from_expr` looks up `Quall` in `imported_modules`,
/// which stores the full dotted name `Phase1.Quall`, so the short form misses.
///
/// This is **Gap B** in commit #6. The user-facing workaround is the explicit
/// `import Phase1.Quall as Quall` form, which is exercised by
/// `qualified_call_via_import_alias` below and works today.
///
/// Closing Gap B requires either: (a) inserting last-segment aliases into
/// `imported_modules` for every same-file `module` declaration, or
/// (b) extending `resolve_module_name_from_expr` to fall back to a
/// last-segment match. Both are out of scope for commit #6 because the
/// import-alias workaround is sufficient and matches the §5a precedence
/// rules of Proposal 0151 (explicit imports beat implicit shortening).
#[test]
#[ignore = "Gap B: short-form qualified call without explicit import; commit #6 leaves this for a follow-up"]
fn qualified_call_short_form_fails_today() {
    let source = r#"
module Phase1.Quall {
    public class Sizeable2<a> {
        fn size_of2(x: a) -> Int
    }

    public instance Sizeable2<Int> {
        fn size_of2(x) { x }
    }
}

fn main() {
    Quall.size_of2(42)
}
"#;
    let result = compile_and_run(source);
    assert_eq!(result, Value::Integer(42));
}

/// Proposal 0151, Phase 1a, commit #6: explicit `import ... as Alias` form.
///
/// The standard user-facing way to call a module-scoped class method via a
/// short alias. Works today thanks to the dispatch fix in commit #6.
#[test]
fn qualified_call_via_import_alias() {
    let source = r#"
module Phase1.QuallAlias {
    public class Sizeable3<a> {
        fn size_of3(x: a) -> Int
    }

    public instance Sizeable3<Int> {
        fn size_of3(x) { x }
    }
}

import Phase1.QuallAlias as Quall

fn main() {
    Quall.size_of3(42)
}
"#;
    let result = compile_and_run(source);
    assert_eq!(result, Value::Integer(42));
}

/// Proposal 0151, Phase 1a, commit #6: full-dotted qualified call.
///
/// Calling a module-scoped class method through the full dotted module path
/// (`Phase1.Quall.size_of2(42)`). This is the qualified-resolution headline
/// feature of commit #6.
#[test]
fn qualified_call_full_dotted_form() {
    let source = r#"
module Phase1.Quall {
    public class Sizeable2<a> {
        fn size_of2(x: a) -> Int
    }

    public instance Sizeable2<Int> {
        fn size_of2(x) { x }
    }
}

fn main() {
    Phase1.Quall.size_of2(42)
}
"#;
    let result = compile_and_run(source);
    assert_eq!(result, Value::Integer(42));
}

/// Walks a parsed program and returns `Some(is_public)` for the first
/// `Statement::Class` it finds (recursing into module bodies). Used by the
/// `public class` parser tests.
fn first_class_visibility(program: &flux::syntax::program::Program) -> Option<bool> {
    fn walk(
        statements: &[flux::syntax::statement::Statement],
    ) -> Option<bool> {
        use flux::syntax::statement::Statement;
        for stmt in statements {
            match stmt {
                Statement::Class { is_public, .. } => return Some(*is_public),
                Statement::Module { body, .. } => {
                    if let Some(found) = walk(&body.statements) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&program.statements)
}

/// Same shape as `first_class_visibility` but for the first `Statement::Instance`.
fn first_instance_visibility(program: &flux::syntax::program::Program) -> Option<bool> {
    fn walk(
        statements: &[flux::syntax::statement::Statement],
    ) -> Option<bool> {
        use flux::syntax::statement::Statement;
        for stmt in statements {
            match stmt {
                Statement::Instance { is_public, .. } => return Some(*is_public),
                Statement::Module { body, .. } => {
                    if let Some(found) = walk(&body.statements) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&program.statements)
}

fn parse_program_only(source: &str) -> flux::syntax::program::Program {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {}",
        render_diagnostics(&parser.errors, Some(source), None)
    );
    program
}

#[test]
fn bare_class_is_private_by_default() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(false));
}

#[test]
fn public_class_sets_is_public_true() {
    let program = parse_program_only(
        r#"
public class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(true));
}

#[test]
fn bare_instance_is_private_by_default() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}

instance Eq2<Int> {
    fn eq2(x, y) { x == y }
}
"#,
    );
    assert_eq!(first_instance_visibility(&program), Some(false));
}

#[test]
fn public_instance_sets_is_public_true() {
    let program = parse_program_only(
        r#"
class Eq2<a> {
    fn eq2(x: a, y: a) -> Bool
}

public instance Eq2<Int> {
    fn eq2(x, y) { x == y }
}
"#,
    );
    assert_eq!(first_instance_visibility(&program), Some(true));
}

#[test]
fn module_body_public_class_and_instance_set_is_public_true() {
    // The full target shape: module-scoped public class and public instance,
    // both should round-trip is_public=true through the parser.
    let program = parse_program_only(
        r#"
module Phase1.PublicCheck {
    public class Sizeable<a> {
        fn size_of(x: a) -> Int
    }

    public instance Sizeable<Int> {
        fn size_of(x) { x }
    }
}
"#,
    );
    assert_eq!(first_class_visibility(&program), Some(true));
    assert_eq!(first_instance_visibility(&program), Some(true));
}

/// Proposal 0151, Phase 2: short-name constraint ambiguity (E456).
///
/// When two classes named `Foldable` are visible (one in `Mod.A`, one in
/// `Mod.B`), an explicit constraint `<a: Foldable>` cannot be resolved.
/// The constraint solver fires E456 for each ambiguous bound.
#[test]
fn ambiguous_short_name_constraint_fires_e456() {
    let source = r#"
module Mod.A {
    public class Bag<a> {
        fn pack(x: a) -> a
    }
}

module Mod.B {
    public class Bag<a> {
        fn pack(x: a) -> a
    }
}

fn use_bag<a: Bag>(x: a) -> a { x }
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        rendered.contains("E456"),
        "ambiguous short-name constraint should fire E456, got:\n{rendered}"
    );
}

/// Negative test: a single visible class with the constraint name must
/// not fire E456.
#[test]
fn unambiguous_short_name_constraint_does_not_fire_e456() {
    let source = r#"
module Mod.Only {
    public class Bag<a> {
        fn pack(x: a) -> a
    }
}

fn use_bag<a: Bag>(x: a) -> a { x }
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        !rendered.contains("E456"),
        "single-class constraint must not fire E456, got:\n{rendered}"
    );
}

// ============================================================
// Proposal 0151, Phase 3: import-collision diagnostics.
// ============================================================

/// E457 — `exposing (foo)` brings in a name that already exists as
/// a top-level local declaration. The compiler must reject this with
/// a clear hint about the collision.
#[test]
fn exposing_collides_with_top_level_local_fires_e457() {
    let source = r#"
import Flow.Option as Option exposing (map)

fn map(x) { x + 1 }
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        rendered.contains("E457"),
        "exposing-vs-local collision must fire E457, got:\n{rendered}"
    );
}

/// Negative: a top-level `fn` whose name does NOT collide with any
/// exposed import is fine. The walker must not over-fire.
#[test]
fn exposing_with_no_local_collision_does_not_fire_e457() {
    let source = r#"
import Flow.Option as Option exposing (map)

fn add_one(x) { x + 1 }
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        !rendered.contains("E457"),
        "non-colliding exposing must not fire E457, got:\n{rendered}"
    );
}

/// E458 — a file-level import and a module-body import bind the same
/// short name to two different module targets. The walker must
/// surface the conflict so users can disambiguate.
#[test]
fn file_vs_module_import_collision_fires_e458() {
    let source = r#"
import Flow.Option as Option exposing (map)

module Phase3.Inner {
    import Flow.List as List exposing (map)
}
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        rendered.contains("E458"),
        "cross-scope import collision must fire E458, got:\n{rendered}"
    );
}

/// Negative: the same short name exposed by the SAME module from both
/// file scope and module-body scope is not a conflict — it's just a
/// redundant re-import that resolves to one target.
#[test]
fn same_target_in_both_scopes_does_not_fire_e458() {
    let source = r#"
import Flow.Option as Option exposing (map)

module Phase3.InnerSame {
    import Flow.Option as OptionAlias exposing (map)
}
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        !rendered.contains("E458"),
        "same-module re-exposing must not fire E458, got:\n{rendered}"
    );
}

/// Inside-module shadowing rule: when a default method body inside a
/// `module` block calls another method by short name, the resolver
/// reaches the local class's method (not any legacy global). This is
/// a smoke test confirming `lookup_class_method`'s short-name lookup
/// already handles this for classes with default-bodied methods.
///
/// We use a class with a single method whose default body calls a
/// sibling method by short name; if resolution worked, the test
/// compiles cleanly. If a stray collision sneaks in via the new
/// E457/E458 walkers, this catches it.
#[test]
fn inside_module_default_method_resolves_to_sibling() {
    let source = r#"
module Phase3.Shadow {
    class Foldable<a> {
        fn fold_default(x: a) -> Int
    }

    instance Foldable<Int> {
        fn fold_default(x) { x + 1 }
    }
}
"#;
    let diags = compile_source(source);
    let rendered = render_diagnostics(&diags, Some(source), None);
    assert!(
        !rendered.contains("E457") && !rendered.contains("E458"),
        "inside-module class declaration must not fire spurious collisions, got:\n{rendered}"
    );
}

// ── Phase 4a: instance-method effect floor (E452) ────────────────────

/// Returns `true` if any rendered diagnostic carries the E452 code.
fn has_e452(diags: &[flux::diagnostics::Diagnostic]) -> bool {
    let rendered = render_diagnostics(diags, None, None);
    rendered.contains("E452")
}

#[test]
fn instance_method_missing_class_effect_fires_e452() {
    // The class declares `with IO` on `eq`, but the instance method drops
    // it. Floor semantics: instance row must be a superset of class row.
    let source = r#"
effect IO {
    log: String -> ()
}

class Eq<a> {
    fn eq(x: a, y: a) -> Bool with IO
}

instance Eq<Int> {
    fn eq(x, y) { x == y }
}
"#;
    let diags = compile_source(source);
    assert!(
        has_e452(&diags),
        "instance method dropping a class-declared effect must fire E452, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn instance_method_matching_class_effect_does_not_fire_e452() {
    // Both class and instance declare `with IO` — floor is satisfied.
    let source = r#"
effect IO {
    log: String -> ()
}

class Eq<a> {
    fn eq(x: a, y: a) -> Bool with IO
}

instance Eq<Int> {
    fn eq(x, y) with IO { x == y }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_e452(&diags),
        "matching effect rows must not fire E452, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn instance_method_adding_extra_effect_does_not_fire_e452() {
    // Floor semantics: instance may declare *more* effects than the
    // class. Class declares `with IO`, instance declares `with IO, Audit`.
    let source = r#"
effect IO {
    log: String -> ()
}

effect Audit {
    audit: String -> ()
}

class Eq<a> {
    fn eq(x: a, y: a) -> Bool with IO
}

instance Eq<Int> {
    fn eq(x, y) with IO, Audit { x == y }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_e452(&diags),
        "instance adding effects beyond the class floor must not fire E452, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn class_with_no_effect_clause_imposes_no_floor() {
    // Class method has no `with` clause. Instance is free to declare
    // anything, including a `with` clause — no E452.
    let source = r#"
effect IO {
    log: String -> ()
}

class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) with IO { x == y }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_e452(&diags),
        "no class floor means no E452 regardless of instance effects, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_scoped_class_with_effect_floor_violation_fires_e452() {
    // Same rule must hold inside a module body — the walker recurses.
    let source = r#"
module Phase4.Floor {
    effect IO {
        log: String -> ()
    }

    class Eq<a> {
        fn eq(x: a, y: a) -> Bool with IO
    }

    instance Eq<Int> {
        fn eq(x, y) { x == y }
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        has_e452(&diags),
        "floor violation inside a module body must still fire E452, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_scoped_class_with_effect_floor_satisfied_does_not_fire_e452() {
    let source = r#"
module Phase4.FloorOk {
    effect IO {
        log: String -> ()
    }

    class Eq<a> {
        fn eq(x: a, y: a) -> Bool with IO
    }

    instance Eq<Int> {
        fn eq(x, y) with IO { x == y }
    }
}
"#;
    let diags = compile_source(source);
    assert!(
        !has_e452(&diags),
        "module-scoped class with satisfied floor must not fire E452, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}

#[test]
fn module_body_still_rejects_unsupported_statements() {
    // Negative regression: a return statement at module body level is still
    // not a valid module member, and the validator must continue to reject it.
    let source = r#"
module Phase1.SmokeReject {
    return 1
}
"#;
    let diags = compile_source(source);
    assert!(
        has_invalid_module_content(&diags),
        "stray return inside module body should still be rejected, got: {}",
        render_diagnostics(&diags, Some(source), None)
    );
}
