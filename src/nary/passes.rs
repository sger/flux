/// Core IR optimization passes.
///
/// These passes operate on `CoreExpr` / `CoreProgram` before backend lowering.
/// Passes run after `lower::lower_program` produces a `CoreProgram`.
use super::{CoreExpr, CoreLit, CorePat, CoreProgram};

// ── Pass pipeline ─────────────────────────────────────────────────────────────

/// Run all Core IR passes in order.
///
/// Pass order matters:
/// 1. `beta_reduce`              — eliminate `App(Lam(x, body), arg)` redexes
/// 2. `case_of_known_constructor` — reduce `Case(Con/Lit, alts)` statically
/// 3. `inline_trivial_lets`      — substitute literal/variable let-bindings
///    (COKC creates field-binding lets like `let x = Lit(n)` that this collapses)
/// 4. `elim_dead_let`            — drop unused pure bindings left over
pub fn run_core_passes(program: &mut CoreProgram) {
    let sentinel = CoreExpr::Lit(CoreLit::Unit, Default::default());
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = beta_reduce(e);
        let e = case_of_known_constructor(e);
        let e = inline_trivial_lets(e);
        let e = elim_dead_let(e);
        def.expr = e;
    }
}

// ── Beta reduction ────────────────────────────────────────────────────────────

/// Reduce obvious `App(Lam(x, body), arg)` → `body[x := arg]` at the top level.
///
/// This eliminates the desugaring overhead introduced by lowering
/// (e.g. `|>` pipe always produces `App(f, x)` which may be immediately applied).
pub fn beta_reduce(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            let func = beta_reduce(*func);
            let args: Vec<_> = args.into_iter().map(beta_reduce).collect();
            if let CoreExpr::Lam { params, body, .. } = func {
                if params.len() == args.len() {
                    // Full application: substitute all params
                    let mut body = *body;
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p, &a);
                    }
                    beta_reduce(body)
                } else if args.len() < params.len() {
                    // Partial application: substitute provided args, return Lam with remaining
                    let mut body = *body;
                    let remaining = params[args.len()..].to_vec();
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p, &a);
                    }
                    beta_reduce(CoreExpr::Lam { params: remaining, body: Box::new(body), span })
                } else {
                    // Over-application: apply all params, then apply remaining args
                    let extra_args = args[params.len()..].to_vec();
                    let mut body = *body;
                    for (p, a) in params.into_iter().zip(args.into_iter()) {
                        body = subst(body, p, &a);
                    }
                    let body = beta_reduce(body);
                    beta_reduce(CoreExpr::App { func: Box::new(body), args: extra_args, span })
                }
            } else {
                CoreExpr::App { func: Box::new(func), args, span }
            }
        }
        CoreExpr::Lam { params, body, span } => {
            CoreExpr::Lam { params, body: Box::new(beta_reduce(*body)), span }
        }
        CoreExpr::Let { var, rhs, body, span } => CoreExpr::Let {
            var,
            rhs: Box::new(beta_reduce(*rhs)),
            body: Box::new(beta_reduce(*body)),
            span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var,
            rhs: Box::new(beta_reduce(*rhs)),
            body: Box::new(beta_reduce(*body)),
            span,
        },
        CoreExpr::Case { scrutinee, alts, span } => {
            let scrutinee = beta_reduce(*scrutinee);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = beta_reduce(alt.rhs);
                    alt.guard = alt.guard.map(beta_reduce);
                    alt
                })
                .collect();
            CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(beta_reduce).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(beta_reduce).collect(),
            span,
        },
        CoreExpr::Perform { effect, operation, args, span } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(beta_reduce).collect(),
            span,
        },
        CoreExpr::Handle { body, effect, handlers, span } => CoreExpr::Handle {
            body: Box::new(beta_reduce(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = beta_reduce(h.body);
                    h
                })
                .collect(),
            span,
        },
        // Atoms are already in normal form.
        other => other,
    }
}

// ── Dead let elimination ──────────────────────────────────────────────────────

/// Remove `Let { var, rhs, body }` where `var` does not appear free in `body`
/// and `rhs` is pure (a literal or variable — no observable effects).
pub fn elim_dead_let(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let { var, rhs, body, span } => {
            let rhs = elim_dead_let(*rhs);
            let body = elim_dead_let(*body);
            if is_pure(&rhs) && !appears_free(var, &body) {
                body
            } else {
                CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span }
            }
        }
        CoreExpr::Lam { params, body, span } => {
            CoreExpr::Lam { params, body: Box::new(elim_dead_let(*body)), span }
        }
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(elim_dead_let(*func)),
            args: args.into_iter().map(elim_dead_let).collect(),
            span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var,
            rhs: Box::new(elim_dead_let(*rhs)),
            body: Box::new(elim_dead_let(*body)),
            span,
        },
        CoreExpr::Case { scrutinee, alts, span } => {
            let scrutinee = elim_dead_let(*scrutinee);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = elim_dead_let(alt.rhs);
                    alt
                })
                .collect();
            CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(elim_dead_let).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(elim_dead_let).collect(),
            span,
        },
        other => other,
    }
}

// ── Substitution ──────────────────────────────────────────────────────────────

/// Substitute `replacement` for free occurrences of `var` in `expr`.
///
/// This is capture-avoiding for `Lam` and `Let` binders.
fn subst(expr: CoreExpr, var: crate::syntax::Identifier, replacement: &CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Var(name, span) => {
            if name == var {
                replacement.clone()
            } else {
                CoreExpr::Var(name, span)
            }
        }
        CoreExpr::Lam { params, body, span } => {
            if params.contains(&var) {
                // Shadowed — don't substitute inside.
                CoreExpr::Lam { params, body, span }
            } else {
                CoreExpr::Lam { params, body: Box::new(subst(*body, var, replacement)), span }
            }
        }
        CoreExpr::Let { var: binding, rhs, body, span } => {
            let rhs = subst(*rhs, var, replacement);
            if binding == var {
                CoreExpr::Let { var: binding, rhs: Box::new(rhs), body, span }
            } else {
                CoreExpr::Let {
                    var: binding,
                    rhs: Box::new(rhs),
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(subst(*func, var, replacement)),
            args: args.into_iter().map(|a| subst(a, var, replacement)).collect(),
            span,
        },
        CoreExpr::Case { scrutinee, alts, span } => {
            let scrutinee = subst(*scrutinee, var, replacement);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    if !pat_binds(&alt.pat, var) {
                        alt.guard = alt.guard.map(|g| subst(g, var, replacement));
                        alt.rhs = subst(alt.rhs, var, replacement);
                    }
                    alt
                })
                .collect();
            CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(|f| subst(f, var, replacement)).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(|a| subst(a, var, replacement)).collect(),
            span,
        },
        CoreExpr::Perform { effect, operation, args, span } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(|a| subst(a, var, replacement)).collect(),
            span,
        },
        CoreExpr::LetRec { var: binding, rhs, body, span } => {
            if binding == var {
                CoreExpr::LetRec { var: binding, rhs, body, span }
            } else {
                CoreExpr::LetRec {
                    var: binding,
                    rhs: Box::new(subst(*rhs, var, replacement)),
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::Handle { body, effect, handlers, span } => CoreExpr::Handle {
            body: Box::new(subst(*body, var, replacement)),
            effect,
            handlers,
            span,
        },
        other => other,
    }
}

// ── Case-of-known-constructor ─────────────────────────────────────────────────

/// Reduce `Case(Con(tag, fields), alts)` and `Case(Lit(l), alts)` when the
/// scrutinee is a statically-known value.
///
/// Only unguarded arms are considered.  Nested constructor patterns in field
/// position are left alone (handled by a future pattern-compilation pass).
///
/// Examples:
/// ```text
/// Case(Con(Some, [Lit(42)]), [Con(Some, [Var(x)]) → x])  →  Lit(42)
/// Case(Lit(true), [Lit(true) → a, Wildcard → b])          →  a
/// ```
pub fn case_of_known_constructor(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case { scrutinee, alts, span } => {
            let scrutinee = case_of_known_constructor(*scrutinee);
            let alts: Vec<_> = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = case_of_known_constructor(alt.rhs);
                    alt.guard = alt.guard.map(case_of_known_constructor);
                    alt
                })
                .collect();
            match &scrutinee {
                CoreExpr::Con { tag, fields, .. } => {
                    for alt in &alts {
                        if alt.guard.is_some() {
                            continue;
                        }
                        if let Some(bindings) = match_con_pat(&alt.pat, tag, fields) {
                            let mut body = alt.rhs.clone();
                            for (var, val) in bindings {
                                body = subst(body, var, &val);
                            }
                            return case_of_known_constructor(body);
                        }
                    }
                    CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
                }
                CoreExpr::Lit(lit, lit_span) => {
                    let lit = lit.clone();
                    let lit_span = *lit_span;
                    for alt in &alts {
                        if alt.guard.is_some() {
                            continue;
                        }
                        if let Some(bindings) = match_lit_pat(&alt.pat, &lit, lit_span) {
                            let mut body = alt.rhs.clone();
                            for (var, val) in bindings {
                                body = subst(body, var, &val);
                            }
                            return case_of_known_constructor(body);
                        }
                    }
                    CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
                }
                _ => CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span },
            }
        }
        CoreExpr::Lam { params, body, span } => {
            CoreExpr::Lam { params, body: Box::new(case_of_known_constructor(*body)), span }
        }
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(case_of_known_constructor(*func)),
            args: args.into_iter().map(case_of_known_constructor).collect(),
            span,
        },
        CoreExpr::Let { var, rhs, body, span } => CoreExpr::Let {
            var,
            rhs: Box::new(case_of_known_constructor(*rhs)),
            body: Box::new(case_of_known_constructor(*body)),
            span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var,
            rhs: Box::new(case_of_known_constructor(*rhs)),
            body: Box::new(case_of_known_constructor(*body)),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(case_of_known_constructor).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(case_of_known_constructor).collect(),
            span,
        },
        CoreExpr::Perform { effect, operation, args, span } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(case_of_known_constructor).collect(),
            span,
        },
        CoreExpr::Handle { body, effect, handlers, span } => CoreExpr::Handle {
            body: Box::new(case_of_known_constructor(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = case_of_known_constructor(h.body);
                    h
                })
                .collect(),
            span,
        },
        other => other,
    }
}

/// Try to match `Con { tag, fields }` against `pat`.
///
/// Returns `Some(bindings)` on success or `None` if the pattern cannot match
/// statically (wrong tag, arity mismatch, or nested non-trivial sub-pattern).
fn match_con_pat(
    pat: &CorePat,
    tag: &super::CoreTag,
    fields: &[CoreExpr],
) -> Option<Vec<(crate::syntax::Identifier, CoreExpr)>> {
    use crate::diagnostics::position::Span;
    match pat {
        CorePat::Wildcard => Some(vec![]),
        CorePat::Var(name) => {
            let val = CoreExpr::Con { tag: tag.clone(), fields: fields.to_vec(), span: Span::default() };
            Some(vec![(*name, val)])
        }
        CorePat::Con { tag: pat_tag, fields: pat_fields } => {
            if pat_tag != tag || pat_fields.len() != fields.len() {
                return None;
            }
            let mut bindings = vec![];
            for (pat_field, val) in pat_fields.iter().zip(fields.iter()) {
                match pat_field {
                    CorePat::Wildcard => {}
                    CorePat::Var(name) => bindings.push((*name, val.clone())),
                    // Nested non-trivial pattern — too complex for this pass.
                    _ => return None,
                }
            }
            Some(bindings)
        }
        CorePat::EmptyList => {
            if *tag == super::CoreTag::Nil && fields.is_empty() { Some(vec![]) } else { None }
        }
        _ => None,
    }
}

/// Try to match a literal scrutinee against `pat`.
fn match_lit_pat(
    pat: &CorePat,
    lit: &super::CoreLit,
    lit_span: crate::diagnostics::position::Span,
) -> Option<Vec<(crate::syntax::Identifier, CoreExpr)>> {
    match pat {
        CorePat::Wildcard => Some(vec![]),
        CorePat::Var(name) => Some(vec![(*name, CoreExpr::Lit(lit.clone(), lit_span))]),
        CorePat::Lit(pat_lit) => {
            if pat_lit == lit { Some(vec![]) } else { None }
        }
        _ => None,
    }
}

// ── Trivial let inlining ──────────────────────────────────────────────────────

/// Inline `let x = rhs; body` when `rhs` is a literal or variable.
///
/// This is constant propagation + copy propagation at the Core IR level.
/// It complements `elim_dead_let`: that pass removes unused pure bindings;
/// this pass substitutes trivial values so downstream passes (COKC, dead-let)
/// can see through them.
///
/// Examples:
/// ```text
/// let x = 5; x + x          →  5 + 5
/// let x = y; some_fn(x)     →  some_fn(y)
/// ```
pub fn inline_trivial_lets(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let { var, rhs, body, .. } => {
            let rhs = inline_trivial_lets(*rhs);
            let body = inline_trivial_lets(*body);
            if is_pure(&rhs) {
                // Substitute and continue — may unlock further inlining.
                inline_trivial_lets(subst(body, var, &rhs))
            } else {
                // Keep the binding; rhs has side-effects or is non-trivial.
                let span = rhs.span();
                CoreExpr::Let { var, rhs: Box::new(rhs), body: Box::new(body), span }
            }
        }
        CoreExpr::Lam { params, body, span } => {
            CoreExpr::Lam { params, body: Box::new(inline_trivial_lets(*body)), span }
        }
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(inline_trivial_lets(*func)),
            args: args.into_iter().map(inline_trivial_lets).collect(),
            span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var,
            rhs: Box::new(inline_trivial_lets(*rhs)),
            body: Box::new(inline_trivial_lets(*body)),
            span,
        },
        CoreExpr::Case { scrutinee, alts, span } => {
            let scrutinee = inline_trivial_lets(*scrutinee);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = inline_trivial_lets(alt.rhs);
                    alt.guard = alt.guard.map(inline_trivial_lets);
                    alt
                })
                .collect();
            CoreExpr::Case { scrutinee: Box::new(scrutinee), alts, span }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(inline_trivial_lets).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(inline_trivial_lets).collect(),
            span,
        },
        CoreExpr::Perform { effect, operation, args, span } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(inline_trivial_lets).collect(),
            span,
        },
        CoreExpr::Handle { body, effect, handlers, span } => CoreExpr::Handle {
            body: Box::new(inline_trivial_lets(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = inline_trivial_lets(h.body);
                    h
                })
                .collect(),
            span,
        },
        other => other,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns true when `expr` is guaranteed pure (no effects, no calls).
fn is_pure(expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::Lit(_, _) | CoreExpr::Var(_, _))
}

/// Returns true when `var` appears free in `expr`.
fn appears_free(var: crate::syntax::Identifier, expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var(name, _) => *name == var,
        CoreExpr::Lit(_, _) => false,
        CoreExpr::Lam { params, body, .. } => !params.contains(&var) && appears_free(var, body),
        CoreExpr::App { func, args, .. } => appears_free(var, func) || args.iter().any(|a| appears_free(var, a)),
        CoreExpr::Let { var: binding, rhs, body, .. } => {
            appears_free(var, rhs) || (*binding != var && appears_free(var, body))
        }
        CoreExpr::LetRec { var: binding, rhs, body, .. } => {
            *binding != var && (appears_free(var, rhs) || appears_free(var, body))
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            appears_free(var, scrutinee)
                || alts.iter().any(|alt| {
                    !pat_binds(&alt.pat, var)
                        && (alt.guard.as_ref().is_some_and(|g| appears_free(var, g))
                            || appears_free(var, &alt.rhs))
                })
        }
        CoreExpr::Con { fields, .. } => fields.iter().any(|f| appears_free(var, f)),
        CoreExpr::PrimOp { args, .. } => args.iter().any(|a| appears_free(var, a)),
        CoreExpr::Perform { args, .. } => args.iter().any(|a| appears_free(var, a)),
        CoreExpr::Handle { body, handlers, .. } => {
            appears_free(var, body)
                || handlers.iter().any(|h| {
                    h.resume != var
                        && !h.params.contains(&var)
                        && appears_free(var, &h.body)
                })
        }
    }
}

/// Returns true when pattern `pat` introduces a binding for `var`.
fn pat_binds(pat: &CorePat, var: crate::syntax::Identifier) -> bool {
    match pat {
        CorePat::Var(name) => *name == var,
        CorePat::Con { fields, .. } => fields.iter().any(|f| pat_binds(f, var)),
        CorePat::Tuple(fields) => fields.iter().any(|f| pat_binds(f, var)),
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        nary::{CoreAlt, CoreLit, CoreTag},
        diagnostics::position::Span,
        syntax::interner::Interner,
    };

    fn s() -> Span { Span::default() }

    // ── case_of_known_constructor ─────────────────────────────────────────────

    #[test]
    fn cokc_reduces_some_constructor() {
        // Case(Con(Some, [Lit(42)]), [Con(Some, [Var(x)]) → Var(x), Wildcard → Lit(0)])
        //   → Lit(42)
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Con {
                tag: CoreTag::Some,
                fields: vec![CoreExpr::Lit(CoreLit::Int(42), s())],
                span: s(),
            }),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con { tag: CoreTag::Some, fields: vec![CorePat::Var(x)] },
                    guard: None,
                    rhs: CoreExpr::Var(x, s()),
                    span: s(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                    span: s(),
                },
            ],
            span: s(),
        };

        let result = case_of_known_constructor(expr);
        assert!(
            matches!(result, CoreExpr::Lit(CoreLit::Int(42), _)),
            "expected Lit(42), got {result:?}"
        );
    }

    #[test]
    fn cokc_reduces_bool_literal() {
        // Case(Lit(true), [Lit(true) → Lit(1), Wildcard → Lit(0)])  →  Lit(1)
        let expr = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), s())),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: CoreExpr::Lit(CoreLit::Int(1), s()),
                    span: s(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                    span: s(),
                },
            ],
            span: s(),
        };

        let result = case_of_known_constructor(expr);
        assert!(
            matches!(result, CoreExpr::Lit(CoreLit::Int(1), _)),
            "expected Lit(1), got {result:?}"
        );
    }

    #[test]
    fn cokc_skips_guarded_arm() {
        // A guarded arm must not be selected even if the tag matches.
        // The wildcard fallthrough should be chosen instead.
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let guard = CoreExpr::Lit(CoreLit::Bool(false), s()); // always-false guard
        let expr = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Con {
                tag: CoreTag::Some,
                fields: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
                span: s(),
            }),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con { tag: CoreTag::Some, fields: vec![CorePat::Var(x)] },
                    guard: Some(guard),
                    rhs: CoreExpr::Lit(CoreLit::Int(99), s()),
                    span: s(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                    span: s(),
                },
            ],
            span: s(),
        };

        let result = case_of_known_constructor(expr);
        // Guarded arm is skipped → wildcard arm is selected → Lit(0)
        assert!(
            matches!(result, CoreExpr::Lit(CoreLit::Int(0), _)),
            "expected Lit(0) (guarded arm skipped), got {result:?}"
        );
    }

    #[test]
    fn cokc_leaves_unknown_scrutinee_alone() {
        // Case(Var(x), [...]) — scrutinee not statically known; must not be reduced.
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Var(x, s())),
            alts: vec![CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: CoreExpr::Lit(CoreLit::Int(0), s()),
                span: s(),
            }],
            span: s(),
        };

        let result = case_of_known_constructor(expr);
        assert!(matches!(result, CoreExpr::Case { .. }), "should remain a Case");
    }

    // ── inline_trivial_lets ───────────────────────────────────────────────────

    #[test]
    fn inline_trivial_substitutes_literal() {
        // let x = 5; x  →  5
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let expr = CoreExpr::Let {
            var: x,
            rhs: Box::new(CoreExpr::Lit(CoreLit::Int(5), s())),
            body: Box::new(CoreExpr::Var(x, s())),
            span: s(),
        };

        let result = inline_trivial_lets(expr);
        assert!(
            matches!(result, CoreExpr::Lit(CoreLit::Int(5), _)),
            "expected Lit(5), got {result:?}"
        );
    }

    #[test]
    fn inline_trivial_copy_propagation() {
        // let x = y; x  →  y
        let mut interner = Interner::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let expr = CoreExpr::Let {
            var: x,
            rhs: Box::new(CoreExpr::Var(y, s())),
            body: Box::new(CoreExpr::Var(x, s())),
            span: s(),
        };

        let result = inline_trivial_lets(expr);
        assert!(
            matches!(result, CoreExpr::Var(name, _) if name == y),
            "expected Var(y), got {result:?}"
        );
    }

    #[test]
    fn inline_trivial_multiple_uses() {
        // let x = 3; PrimOp(Add, [x, x])  →  PrimOp(Add, [3, 3])
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let expr = CoreExpr::Let {
            var: x,
            rhs: Box::new(CoreExpr::Lit(CoreLit::Int(3), s())),
            body: Box::new(CoreExpr::PrimOp {
                op: crate::nary::CorePrimOp::IAdd,
                args: vec![CoreExpr::Var(x, s()), CoreExpr::Var(x, s())],
                span: s(),
            }),
            span: s(),
        };

        let result = inline_trivial_lets(expr);
        match result {
            CoreExpr::PrimOp { args, .. } => {
                assert!(matches!(args[0], CoreExpr::Lit(CoreLit::Int(3), _)));
                assert!(matches!(args[1], CoreExpr::Lit(CoreLit::Int(3), _)));
            }
            other => panic!("expected PrimOp, got {other:?}"),
        }
    }

    #[test]
    fn inline_trivial_does_not_inline_non_trivial() {
        // let x = PrimOp(Add, ...); x  — must keep the let.
        let mut interner = Interner::new();
        let x = interner.intern("x");
        let a = interner.intern("a");
        let b = interner.intern("b");

        let expr = CoreExpr::Let {
            var: x,
            rhs: Box::new(CoreExpr::PrimOp {
                op: crate::nary::CorePrimOp::IAdd,
                args: vec![CoreExpr::Var(a, s()), CoreExpr::Var(b, s())],
                span: s(),
            }),
            body: Box::new(CoreExpr::Var(x, s())),
            span: s(),
        };

        let result = inline_trivial_lets(expr);
        assert!(matches!(result, CoreExpr::Let { .. }), "non-trivial rhs must keep the Let");
    }

    // ── combined: COKC + inline_trivial ──────────────────────────────────────

    #[test]
    fn cokc_then_inline_collapses_field_binding() {
        // COKC creates `let x = Lit(7)` from a field binding; inline_trivial then
        // substitutes it so the final result is just Lit(7).
        //
        // Case(Con(Some, [Lit(7)]), [Con(Some, [Var(x)]) → x])
        //   --COKC→  let x = Lit(7); Var(x)   (field binding)
        //   --inline→  Lit(7)
        let mut interner = Interner::new();
        let x = interner.intern("x");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Con {
                tag: CoreTag::Some,
                fields: vec![CoreExpr::Lit(CoreLit::Int(7), s())],
                span: s(),
            }),
            alts: vec![CoreAlt {
                pat: CorePat::Con { tag: CoreTag::Some, fields: vec![CorePat::Var(x)] },
                guard: None,
                rhs: CoreExpr::Var(x, s()),
                span: s(),
            }],
            span: s(),
        };

        // COKC substitutes x := Lit(7) directly (no intermediate let needed
        // since the field count is one and the pattern is Var).
        let result = case_of_known_constructor(expr);
        let result = inline_trivial_lets(result);
        assert!(
            matches!(result, CoreExpr::Lit(CoreLit::Int(7), _)),
            "expected Lit(7), got {result:?}"
        );
    }
}
