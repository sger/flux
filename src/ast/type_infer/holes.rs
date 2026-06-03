//! Typed holes — GHC-style `_` / `_name`.
//!
//! A *hole* is an underscore-prefixed identifier with no binding in scope: bare
//! `_` (always a hole) or `_name` (a hole only when unbound). When inference meets
//! one it assigns a fresh ordinary type variable — so the surrounding context fixes
//! the hole's type through normal unification — and records it here. At
//! finalization ([`InferCtx::finalize_holes`](super)), each hole is reported as a
//! `TYPED HOLE` diagnostic: `found hole _ : T`, plus the in-scope bindings whose
//! type fits that position.
//!
//! Because both the REPL and the LSP consume `InferProgramResult.diagnostics`,
//! emitting holes as a diagnostic surfaces them in both with no surface-specific
//! code.

use std::collections::HashSet;
use std::sync::Arc;

use crate::diagnostics::{Diagnostic, DiagnosticBuilder, ErrorType, position::Span};
use crate::syntax::{Identifier, interner::Interner};
use crate::types::{
    TypeVarId, infer_type::InferType, scheme::generalize, type_env::TypeEnv, type_subst::TypeSubst,
    unify::unify_core,
};

use super::{display_infer_type, render_scheme_canonical};

/// Maximum number of fitting bindings listed for a hole.
const MAX_FITS: usize = 10;

/// One recorded typed hole awaiting finalization.
pub(super) struct HoleInfo {
    /// The hole's source name (`_`, or `_name` for a named hole).
    pub name: Identifier,
    pub span: Span,
    /// The fresh inference variable standing in for the hole; resolved through the
    /// final substitution to obtain the hole's type.
    pub var: TypeVarId,
}

/// Whether `name` denotes a typed hole: a single-underscore-prefixed name (`_`,
/// `_foo`) — but never a `__`-prefixed compiler-internal name (e.g. `__repl_type`).
/// Only consulted for names that already failed scope lookup.
pub(crate) fn is_hole_name(name: &str) -> bool {
    name.starts_with('_') && !name.starts_with("__")
}

/// Build the `TYPED HOLE` diagnostic for a single resolved hole.
pub(super) fn hole_diagnostic(
    name: &str,
    hole_ty: &InferType,
    fits: &[String],
    file_path: Arc<str>,
    span: Span,
    interner: &Interner,
) -> Diagnostic {
    let ty = display_hole_type(hole_ty, interner);
    let help = if fits.is_empty() {
        "no in-scope bindings fit this hole".to_string()
    } else {
        let mut text = String::from("relevant bindings in scope:");
        for fit in fits {
            text.push_str("\n    ");
            text.push_str(fit);
        }
        text
    };
    Diagnostic::make_error_dynamic(
        "E469",
        "TYPED HOLE",
        ErrorType::Compiler,
        format!("found hole `{name}` : {ty}"),
        Some(help),
        file_path,
        span,
    )
    .with_primary_label(span, format!("hole : {ty}"))
}

/// The in-scope bindings whose type unifies with `hole_ty`, rendered as
/// `name : type` and capped at [`MAX_FITS`] (with a `…and N more` line).
///
/// Trial-unifies each candidate on a throwaway basis: [`unify_core`] returns its
/// composed substitution by value, which we **discard**, so the inference
/// substitution `subst` is never mutated. Concrete (monomorphic) fits are listed
/// before polymorphic ones, then alphabetically.
pub(super) fn hole_fits(
    env: &mut TypeEnv,
    subst: &TypeSubst,
    hole_ty: &InferType,
    interner: &Interner,
) -> Vec<String> {
    let no_skolems: HashSet<TypeVarId> = HashSet::new();
    // Snapshot the bindings first so the immutable `visible_bindings` borrow is
    // released before we take `&mut env.counter` for instantiation/unification.
    let bindings: Vec<(Identifier, crate::types::scheme::Scheme)> = env
        .visible_bindings()
        .map(|(name, scheme)| (name, scheme.clone()))
        .collect();

    let mut fits: Vec<(bool, String)> = Vec::new();
    for (name, scheme) in bindings {
        let rendered_name = interner.resolve(name);
        // Skip holes/internal-ish names and anything underscore-prefixed.
        if rendered_name.starts_with('_') {
            continue;
        }
        let (inst_ty, _mapping, _constraints) = scheme.instantiate(&mut env.counter);
        if unify_core(
            &inst_ty,
            hole_ty,
            subst,
            Span::default(),
            &mut env.counter,
            &no_skolems,
        )
        .is_ok()
        {
            let is_polymorphic = !scheme.forall.is_empty();
            let ty = strip_forall(render_scheme_canonical(interner, &scheme));
            fits.push((is_polymorphic, format!("{rendered_name} : {ty}")));
        }
    }

    fits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    fits.dedup_by(|a, b| a.1 == b.1);

    let total = fits.len();
    let mut out: Vec<String> = fits.into_iter().take(MAX_FITS).map(|(_, s)| s).collect();
    if total > MAX_FITS {
        out.push(format!("…and {} more", total - MAX_FITS));
    }
    out
}

/// Render a hole's type with named (`a`, `b`, …) variables rather than `_`, by
/// generalizing and reusing the canonical scheme renderer, then dropping the
/// `forall ….` quantifier prefix.
fn display_hole_type(ty: &InferType, interner: &Interner) -> String {
    // A monomorphic type renders fine directly and reads better (`Int`, not via a
    // scheme); only free-variable types need the letter-naming treatment.
    if ty.is_concrete() {
        return display_infer_type(ty, interner);
    }
    let scheme = generalize(ty, &HashSet::new());
    strip_forall(render_scheme_canonical(interner, &scheme))
}

/// Drop a leading `forall <vars>. ` quantifier from a rendered scheme, keeping any
/// constraint context (`Num<a> => …`) intact. Returns the input unchanged when it
/// carries no quantifier (a monomorphic type).
fn strip_forall(rendered: String) -> String {
    rendered
        .strip_prefix("forall ")
        .and_then(|rest| rest.split_once(". "))
        .map(|(_, body)| body.to_string())
        .unwrap_or(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hole_names_exclude_double_underscore() {
        assert!(is_hole_name("_"));
        assert!(is_hole_name("_foo"));
        assert!(!is_hole_name("__repl_type"));
        assert!(!is_hole_name("foo"));
    }

    #[test]
    fn strip_forall_drops_quantifier_keeps_constraints() {
        assert_eq!(strip_forall("forall a. (a) -> a".to_string()), "(a) -> a");
        assert_eq!(
            strip_forall("forall a. Num<a> => a".to_string()),
            "Num<a> => a"
        );
        assert_eq!(strip_forall("(Int) -> Int".to_string()), "(Int) -> Int");
    }
}
