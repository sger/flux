use super::*;

impl<'a> InferCtx<'a> {
    /// Bidirectional check-mode entry point (Proposal 0159, Phase 1).
    ///
    /// Walks `expr` against `expected`, propagating the expected type into
    /// sub-expressions where doing so yields more precise diagnostics. For
    /// nodes without a specialised rule, falls back to `infer + unify_reporting`
    /// so the default rule preserves the existing diagnostic shape.
    ///
    /// Note: this intentionally does not overwrite `self.expr_types[expr]` —
    /// check_expression runs *after* a prior `infer_expression` pass in the
    /// typed-let driver, and that pass has already recorded the authoritative
    /// inferred type for the expression. Overwriting with `expected` would
    /// mask the actual inferred shape from downstream consumers.
    pub(in crate::ast::type_infer) fn check_expression(
        &mut self,
        expr: &Expression,
        expected: &InferType,
    ) -> InferType {
        let resolved = match expr {
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
                ..
            } => self.check_if_expression(condition, consequence, alternative.as_ref(), expected, *span),
            Expression::Match { scrutinee, arms, span, .. } => {
                self.check_match_expression(scrutinee, arms, expected, *span)
            }
            Expression::DoBlock { block, .. } => self.check_block_value(block, expected),
            _ => {
                let actual = self.infer_expression(expr);
                self.unify_reporting(expected, &actual, expr.span())
            }
        };
        resolved.apply_type_subst(&self.subst)
    }

    /// Check mode for `if` expressions: each branch is checked against the
    /// expected type so branch-level mismatches report at the branch span.
    fn check_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: Option<&Block>,
        expected: &InferType,
        span: Span,
    ) -> InferType {
        let cond_ty = self.infer_expression(condition);
        self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), span);
        self.check_block_value(consequence, expected);
        if let Some(alt) = alternative {
            self.check_block_value(alt, expected);
        }
        expected.apply_type_subst(&self.subst)
    }

    /// Check mode for `match` expressions: each arm body is checked against
    /// the expected type so arm mismatches report at the arm body span.
    fn check_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        let scrutinee_ty = self.infer_expression(scrutinee);
        if arms.is_empty() {
            return self.infer_match_expression(MatchInferInput { scrutinee, arms, span });
        }
        for arm in arms {
            self.env.enter_scope();
            self.bind_pattern_variables(&arm.pattern, &scrutinee_ty, span);
            if let Some(guard) = &arm.guard {
                self.infer_expression(guard);
            }
            self.check_expression(&arm.body, expected);
            self.env.leave_scope();
        }
        expected.apply_type_subst(&self.subst)
    }

    /// Check the block's final value expression against `expected`.
    /// Non-value statements are inferred for side-effects as usual.
    fn check_block_value(&mut self, block: &Block, expected: &InferType) -> InferType {
        let stmts = &block.statements;
        let value_index = stmts.iter().rposition(|stmt| {
            matches!(
                stmt,
                Statement::Expression { has_semicolon: false, .. } | Statement::Return { value: Some(_), .. }
            )
        });
        for (i, stmt) in stmts.iter().enumerate() {
            if Some(i) == value_index {
                match stmt {
                    Statement::Expression { expression, .. } => {
                        self.check_expression(expression, expected);
                    }
                    Statement::Return { value: Some(expr), .. } => {
                        self.check_expression(expr, expected);
                    }
                    _ => unreachable!(),
                }
            } else {
                self.infer_statement(stmt);
            }
        }
        expected.apply_type_subst(&self.subst)
    }
}
