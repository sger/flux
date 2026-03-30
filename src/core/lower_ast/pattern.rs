use crate::{
    diagnostics::position::Span,
    syntax::expression::{Expression, HandleArm, MatchArm, Pattern},
};

use crate::core::{CoreAlt, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat, CoreTag};

impl<'a> super::AstLowerer<'a> {
    // ── Pattern lowering ─────────────────────────────────────────────────────

    pub(super) fn lower_match_arm(&mut self, arm: &MatchArm) -> CoreAlt {
        CoreAlt {
            pat: self.lower_pattern(&arm.pattern),
            guard: arm.guard.as_ref().map(|g| self.lower_expr(g)),
            rhs: self.lower_expr(&arm.body),
            span: arm.span,
        }
    }

    pub(super) fn lower_handle_arm(&mut self, arm: &HandleArm) -> CoreHandler {
        CoreHandler {
            operation: arm.operation_name,
            params: arm.params.iter().map(|&p| self.bind_name(p)).collect(),
            resume: self.bind_name(arm.resume_param),
            body: self.lower_expr(&arm.body),
            span: arm.span,
        }
    }

    // ── Destructuring at top level ───────────────────────────────────────────

    /// Expand a top-level `LetDestructure` into individual `CoreDef`s.
    ///
    /// For simple tuple patterns `(x, y) = expr` this emits:
    ///   `x = TupleField(expr, 0)`,  `y = TupleField(expr, 1)`
    ///
    /// For more complex patterns we emit a single `$destructure` def and
    /// subsequent field projections.
    pub(super) fn expand_destructure_top_level(
        &mut self,
        pat: CorePat,
        rhs: CoreExpr,
        span: Span,
        out: &mut Vec<CoreDef>,
    ) {
        match pat {
            CorePat::Tuple(fields) => {
                // Bind to a tmp first so rhs is evaluated once.
                let tmp = crate::syntax::symbol::Symbol::new(5_000_000 + self.fresh);
                self.fresh += 1;
                let tmp_binder = self.fresh_binder(tmp);
                out.push(CoreDef::new(tmp_binder, rhs, false, span));
                for (i, field_pat) in fields.into_iter().enumerate() {
                    if let CorePat::Var(binder) = field_pat {
                        out.push(CoreDef::new(
                            binder,
                            CoreExpr::TupleField {
                                object: Box::new(CoreExpr::bound_var(tmp_binder, span)),
                                index: i,
                                span,
                            },
                            false,
                            span,
                        ));
                    }
                    // Nested non-variable patterns are skipped for now.
                }
            }
            CorePat::Var(binder) => {
                out.push(CoreDef::new(binder, rhs, false, span));
            }
            _ => {
                // General case: bind to a tmp.
                let tmp = crate::syntax::symbol::Symbol::new(5_000_000 + self.fresh);
                self.fresh += 1;
                out.push(CoreDef::new(self.fresh_binder(tmp), rhs, false, span));
            }
        }
    }

    // ── Pure pattern lowering (no side effects) ─────────────────────────────

    pub(super) fn lower_pattern(&mut self, pat: &Pattern) -> CorePat {
        match pat {
            Pattern::Wildcard { .. } => CorePat::Wildcard,
            Pattern::Identifier { name, .. } => CorePat::Var(self.bind_name(*name)),
            Pattern::Literal { expression, .. } => {
                // Only simple literal patterns are supported.
                match expression {
                    Expression::Integer { value, .. } => CorePat::Lit(CoreLit::Int(*value)),
                    Expression::Float { value, .. } => CorePat::Lit(CoreLit::Float(*value)),
                    Expression::String { value, .. } => {
                        CorePat::Lit(CoreLit::String(value.clone()))
                    }
                    Expression::Boolean { value, .. } => CorePat::Lit(CoreLit::Bool(*value)),
                    _ => CorePat::Wildcard, // complex expression patterns → wildcard
                }
            }
            Pattern::None { .. } => CorePat::Con {
                tag: CoreTag::None,
                fields: Vec::new(),
            },
            Pattern::Some { pattern, .. } => CorePat::Con {
                tag: CoreTag::Some,
                fields: vec![self.lower_pattern(pattern)],
            },
            Pattern::Left { pattern, .. } => CorePat::Con {
                tag: CoreTag::Left,
                fields: vec![self.lower_pattern(pattern)],
            },
            Pattern::Right { pattern, .. } => CorePat::Con {
                tag: CoreTag::Right,
                fields: vec![self.lower_pattern(pattern)],
            },
            Pattern::Cons { head, tail, .. } => CorePat::Con {
                tag: CoreTag::Cons,
                fields: vec![self.lower_pattern(head), self.lower_pattern(tail)],
            },
            Pattern::EmptyList { .. } => CorePat::EmptyList,
            Pattern::Tuple { elements, .. } => {
                CorePat::Tuple(elements.iter().map(|p| self.lower_pattern(p)).collect())
            }
            Pattern::Constructor { name, fields, .. } => CorePat::Con {
                tag: CoreTag::Named(*name),
                fields: fields.iter().map(|p| self.lower_pattern(p)).collect(),
            },
        }
    }
}
