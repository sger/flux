use super::*;

#[test]
fn import_top_level_ok() {
    compile_ok_in("examples/test.flx", "import Math module Main { fn main() { 1; } }");
}

#[test]
fn source_annotations_report_unknown_type_names_generically() {
    let code = compile_err("fn id(x: Mystery) -> Int { 0 }");
    assert_eq!(code, "E423");
}

#[test]
fn strict_types_report_unknown_type_names_generically() {
    let code = compile_err_static_typing("fn id(x: Mystery) -> Int { 0 }");
    assert_eq!(code, "E423");
}

#[test]
fn source_return_annotations_report_unknown_type_names_generically() {
    let code = compile_err("fn id(x: Int) -> Mystery { x }");
    assert_eq!(code, "E423");
}

#[test]
fn source_let_annotations_report_unknown_type_names_generically() {
    let code = compile_err("let x: Mystery = 1");
    assert_eq!(code, "E423");
}

#[test]
fn import_in_function_error() {
    let code = compile_err("module Main { fn main() { import Math } }");
    assert_eq!(code, "E017");
}

#[test]
fn import_name_collision_error() {
    let code = compile_err_in("examples/test.flx", "let Math = 1; import Math");
    assert_eq!(code, "E029");
}

#[test]
fn private_member_access_error() {
    let code = compile_err(
        "module Math { fn _private() { 1; } } module Main { fn main() { Math._private(); } }",
    );
    assert_eq!(code, "E011");
}

#[test]
fn non_public_module_function_access_error() {
    let code = compile_err(
        "module Math { fn hidden() { 1; } } module Main { fn main() { Math.hidden(); } }",
    );
    assert_eq!(code, "E011");
}

#[test]
fn module_adt_constructor_access_strict_uses_e086() {
    let code = compile_err_strict(
        "module M { type MaybeInt = SomeInt(Int) | NoneInt } fn main() { M.SomeInt(1); }",
    );
    assert_eq!(code, "E086");
}

#[test]
fn module_adt_constructor_access_non_strict_emits_w201_warning() {
    let warnings = compile_ok_with_warnings_in(
        "examples/test.flx",
        "module M { type MaybeInt = SomeInt(Int) | NoneInt } fn main() { M.SomeInt(1); }",
        false,
    );
    assert!(
        warnings.iter().any(|d| d.code() == Some("W201")),
        "expected W201 warning, got: {:?}",
        warnings.iter().map(|d| (d.code(), d.title().to_string())).collect::<Vec<_>>()
    );
}

#[test]
fn fip_annotation_emits_semantic_warning_for_fresh_allocation() {
    let warnings = compile_ok_with_warnings_in(
        "examples/test.flx",
        "@fip fn alloc(x) { Some(x) } fn main() { alloc(1) }",
        false,
    );
    assert!(
        warnings.iter().any(|d| d.message().is_some_and(|m| m.contains("fresh heap allocation"))),
        "expected semantic @fip warning, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn fbip_annotation_is_hard_error_when_proof_fails() {
    let diagnostics = compile_err_diagnostics(
        "@fbip fn bounded(f, x) { f(x) } fn main() { bounded(\\y -> y, 1) }",
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message().is_some_and(|m| m.contains("indirect or opaque callee `f`"))),
        "expected semantic @fbip error, got: {:?}",
        diagnostics
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn fbip_annotation_without_constructors_emits_advisory_warning() {
    let warnings = compile_ok_with_warnings_in(
        "examples/test.flx",
        "@fip fn no_alloc(x) { x } fn main() { no_alloc(1) }",
        false,
    );
    assert!(
        warnings.iter().any(|d| {
            d.title() == "FBIP Annotation Has No Effect"
                && d.message().is_some_and(|m| m.contains("no heap constructor sites"))
        }),
        "expected no-constructors advisory warning, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn direct_array_len_emits_legacy_helper_warning() {
    let warnings =
        compile_ok_with_warnings_in("<unknown>", "fn main() { array_len([|1, 2, 3|]) }", false);
    assert!(
        warnings.iter().any(|d| {
            d.title() == "Legacy Builtin Helper"
                && d.message().is_some_and(|m| m.contains("Flow.Array.length"))
        }),
        "expected legacy builtin warning for array_len, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn direct_string_len_emits_legacy_helper_warning() {
    let warnings =
        compile_ok_with_warnings_in("<unknown>", "fn main() { string_len(\"abc\") }", false);
    assert!(
        warnings.iter().any(|d| {
            d.title() == "Legacy Builtin Helper"
                && d.message().is_some_and(|m| m.contains("Flow.String.string_len"))
        }),
        "expected legacy builtin warning for string_len, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn direct_array_len_is_rejected_in_normal_example_files() {
    let rendered =
        compile_err_rendered_in("examples/guide/test.flx", "fn main() { array_len([|1, 2, 3|]) }");
    assert!(rendered.contains("error[E034]"));
    assert!(rendered.contains("Flow.Array.length"));
}

#[test]
fn primop_example_fixtures_still_allow_legacy_helper_spellings() {
    let warnings = compile_ok_with_warnings_in(
        "examples/primop/demo.flx",
        "fn main() { array_len([|1, 2, 3|]) }",
        false,
    );
    assert!(
        !warnings.iter().any(|d| d.title() == "Legacy Builtin Helper"),
        "did not expect legacy builtin warning inside primop fixtures, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flow_stdlib_modules_do_not_warn_on_legacy_helper_spellings() {
    let warnings = compile_ok_with_warnings_in(
        "lib/Flow/Array.flx",
        "module Flow.Array { public fn demo(arr: Array<Int>) -> Int { array_len(arr) } }",
        false,
    );
    assert!(
        !warnings.iter().any(|d| d.title() == "Legacy Builtin Helper"),
        "did not expect legacy builtin warning inside lib/Flow, got: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flow_array_surface_wrappers_compile_without_legacy_warning() {
    let source =
        std::fs::read_to_string("lib/Flow/Array.flx").expect("expected to read lib/Flow/Array.flx");
    let warnings = compile_ok_with_warnings_in("lib/Flow/Array.flx", &source, false);
    assert!(
        !warnings.iter().any(|d| d.title() == "Legacy Builtin Helper"),
        "Flow.Array surface wrappers should not emit legacy helper warnings: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flow_map_surface_wrappers_compile_without_legacy_warning() {
    let source =
        std::fs::read_to_string("lib/Flow/Map.flx").expect("expected to read lib/Flow/Map.flx");
    let warnings = compile_ok_with_warnings_in("lib/Flow/Map.flx", &source, false);
    assert!(
        !warnings.iter().any(|d| d.title() == "Legacy Builtin Helper"),
        "Flow.Map surface wrappers should not emit legacy helper warnings: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flow_string_surface_wrappers_compile_without_legacy_warning() {
    // After Proposal 0164 Phase 7, `starts_with` etc. are pure-Flux
    // functions living in Flow.String, not primop-backed helpers. A file
    // under lib/Flow/ that defines its own such functions must not trip the
    // "Legacy Builtin Helper" warning that fires for bare primop spellings.
    let warnings = compile_ok_with_warnings_in(
        "lib/Flow/String.flx",
        r#"module Flow.String {
            public fn starts_with(s: String, prefix: String) -> Bool {
                substring(s, 0, 1) == prefix
            }
        }"#,
        false,
    );
    assert!(
        !warnings.iter().any(|d| d.title() == "Legacy Builtin Helper"),
        "Flow.String surface wrappers should not emit legacy helper warnings: {:?}",
        warnings
            .iter()
            .map(|d| (d.title().to_string(), d.message().unwrap_or("").to_string()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn intrinsic_array_surface_wrapper_compiles() {
    compile_ok_in(
        "lib/Flow/Array.flx",
        "module Flow.Array { public intrinsic fn length<a>(arr: Array<a>) -> Int = primop ArrayLen }",
    );
}

#[test]
fn flow_primops_surface_compiles() {
    let source =
        std::fs::read_to_string("lib/Flow/Primops.flx").expect("expected Flow.Primops fixture");
    compile_ok_in("lib/Flow/Primops.flx", &source);
}

#[test]
fn effectful_prelude_call_routes_to_perform_before_inference() {
    let lexer = Lexer::new("fn main() { println(1) }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let mut interner = parser.take_interner();
    let no_user_effects = std::collections::HashSet::new();
    let routing =
        flux::ast::route_effectful_primops::route_effectful_primops_and_synthesize_handlers(
            &program,
            &mut interner,
            &no_user_effects,
            &std::collections::HashMap::new(),
        );
    assert!(routing.changed, "expected 0165 routing to change the AST");
    let routed = routing.program;
    let Statement::Function { body, .. } = &routed.statements[0] else {
        panic!("expected main function");
    };
    let Statement::Expression { expression, .. } = &body.statements[0] else {
        panic!("expected synthesized handler expression");
    };
    assert!(
        expression_contains_perform(expression, "Console", "println", &interner),
        "expected routed AST to contain perform Console.println, got {expression:#?}"
    );
}

#[test]
fn reserved_internal_primop_call_is_rejected_in_user_source() {
    let rendered = compile_err_rendered("fn main() { __primop_println(\"nope\") }");
    assert!(rendered.contains("Reserved Primop Name"), "unexpected diagnostics:\n{rendered}");
    assert!(rendered.contains("__primop_println"), "unexpected diagnostics:\n{rendered}");
}

#[test]
fn parameterized_handle_compiles_through_vm_runtime() {
    compile_ok_in(
        "test.flx",
        r#"
effect State {
    get: () -> Int
}

fn program() -> Int with State {
    perform State.get()
}

fn main() {
    let _ = program() handle State(0) {
        get(resume, state) -> resume(state, state)
    }
}
"#,
    );
}

#[test]
fn intrinsic_self_named_primop_wrapper_compiles() {
    compile_ok_in(
        "lib/Flow/Primops.flx",
        "module Flow.Primops { public intrinsic fn println<a>(x: a) -> Unit with Console = primop Println }",
    );
}

#[test]
fn intrinsic_declarations_are_rejected_outside_flow_stdlib() {
    let rendered = compile_err_rendered_in(
        "examples/test.flx",
        "module Demo { public intrinsic fn length<a>(arr: Array<a>) -> Int = primop ArrayLen }",
    );
    assert!(rendered.contains("outside the Flow stdlib surface"));
    assert!(rendered.contains("Move this intrinsic declaration into `lib/Flow/*`"));
}

#[test]
fn constructor_pattern_arity_mismatch_uses_e085() {
    let code = compile_err(
        "type BoxI = BoxI(Int) | EmptyI fn main() -> Unit { let _x = match BoxI(1) { BoxI(a, b) -> a, EmptyI -> 0 } }",
    );
    assert_eq!(code, "E085");
}

#[test]
fn constructor_call_arity_mismatch_uses_diagnostic_not_panic() {
    let rendered = compile_err_rendered(
        r#"
type BoxI = BoxI(Int) | EmptyI
fn main() -> Unit {
    let _x = BoxI(1, 2)
}
"#,
    );
    assert!(
        rendered.contains("error[E082]"),
        "expected E082 constructor call arity diagnostic, got:\n{}",
        rendered
    );
}

fn expression_contains_perform(
    expression: &Expression,
    effect: &str,
    operation: &str,
    interner: &flux::syntax::interner::Interner,
) -> bool {
    match expression {
        Expression::Perform { effect: e, operation: op, args, .. } => {
            (interner.resolve(*e) == effect && interner.resolve(*op) == operation)
                || args
                    .iter()
                    .any(|arg| expression_contains_perform(arg, effect, operation, interner))
        }
        Expression::Handle { expr, arms, .. } => {
            expression_contains_perform(expr, effect, operation, interner)
                || arms
                    .iter()
                    .any(|arm| expression_contains_perform(&arm.body, effect, operation, interner))
        }
        Expression::DoBlock { block, .. } => block.statements.iter().any(|stmt| match stmt {
            Statement::Expression { expression, .. }
            | Statement::Let { value: expression, .. }
            | Statement::LetDestructure { value: expression, .. }
            | Statement::Assign { value: expression, .. } => {
                expression_contains_perform(expression, effect, operation, interner)
            }
            Statement::Return { value: Some(v), .. } => {
                expression_contains_perform(v, effect, operation, interner)
            }
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                body.statements.iter().any(|stmt| match stmt {
                    Statement::Expression { expression, .. } => {
                        expression_contains_perform(expression, effect, operation, interner)
                    }
                    _ => false,
                })
            }
            _ => false,
        }),
        Expression::Call { function, arguments, .. } => {
            expression_contains_perform(function, effect, operation, interner)
                || arguments
                    .iter()
                    .any(|arg| expression_contains_perform(arg, effect, operation, interner))
        }
        Expression::If { condition, consequence, alternative, .. } => {
            expression_contains_perform(condition, effect, operation, interner)
                || consequence.statements.iter().any(|stmt| match stmt {
                    Statement::Expression { expression, .. } => {
                        expression_contains_perform(expression, effect, operation, interner)
                    }
                    _ => false,
                })
                || alternative.as_ref().is_some_and(|block| {
                    block.statements.iter().any(|stmt| match stmt {
                        Statement::Expression { expression, .. } => {
                            expression_contains_perform(expression, effect, operation, interner)
                        }
                        _ => false,
                    })
                })
        }
        _ => false,
    }
}
