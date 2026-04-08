use crate::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, compiler_errors::STRICT_TYPES_ANY_INFERRED, diagnostic_for,
        position::Span,
    },
    syntax::{Identifier, interner::Interner, program::Program, statement::Statement},
    types::type_env::TypeEnv,
};

use super::display::display_infer_type;

// ─────────────────────────────────────────────────────────────────────────────
// Strict-types validation pass
// ─────────────────────────────────────────────────────────────────────────────

/// Post-inference validation: rejects any binding whose inferred type contains
/// `Any`. Called only when `--strict-types` is active.
///
/// This runs **after** HM inference completes — it does not change inference
/// behaviour. It walks the program's top-level statements and checks each
/// named binding's inferred scheme for residual `Any` types.
pub fn validate_strict_types(
    program: &Program,
    type_env: &TypeEnv,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_statements(&program.statements, type_env, interner, &mut diagnostics);
    diagnostics
}

/// Walk a list of statements, checking each named binding for residual `Any`.
fn validate_statements(
    statements: &[Statement],
    type_env: &TypeEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for stmt in statements {
        match stmt {
            Statement::Function { name, span, .. } => {
                check_binding(*name, *span, type_env, interner, diagnostics);
            }
            Statement::Let { name, span, .. } => {
                check_binding(*name, *span, type_env, interner, diagnostics);
            }
            Statement::Module { body, .. } => {
                validate_statements(&body.statements, type_env, interner, diagnostics);
            }
            _ => {}
        }
    }
}

/// Look up a single binding in the type environment and emit an error if its
/// inferred type contains `Any`.
fn check_binding(
    name: Identifier,
    span: Span,
    type_env: &TypeEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(scheme) = type_env.lookup(name) else {
        return;
    };
    if !scheme.infer_type.contains_any() {
        return;
    }
    let display_name = interner.resolve(name);
    let inferred = display_infer_type(&scheme.infer_type, interner);
    diagnostics.push(build_any_diagnostic(display_name, &inferred, span));
}

/// Build a diagnostic for a binding whose inferred type contains `Any`.
fn build_any_diagnostic(name: &str, inferred_type: &str, span: Span) -> Diagnostic {
    diagnostic_for(&STRICT_TYPES_ANY_INFERRED)
        .with_span(span)
        .with_message(format!(
            "Could not determine a concrete type for `{name}`. \
             Inferred type: `{inferred_type}`."
        ))
        .with_hint_text(format!(
            "Add a type annotation: e.g. `fn {name}(x: Int, y: Int): Int`"
        ))
}
