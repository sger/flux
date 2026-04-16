use super::*;
use crate::diagnostics::{DiagnosticCategory, compiler_errors::EMPTY_MATCH, diagnostic_for};

impl<'a> InferCtx<'a> {
    /// Infer `if` expressions, constraining condition to `Bool` and joining branch results.
    pub(super) fn infer_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
        span: Span,
    ) -> InferType {
        let cond_ty = self.infer_expression(condition);
        self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), span);

        let then_ty = self.infer_block(consequence);
        match alternative {
            Some(alt) => {
                let else_ty = self.infer_block(alt);
                let then_value_span = self.block_value_span(consequence);
                let else_value_span = self.block_value_span(alt);
                self.unify_with_context(
                    &then_ty,
                    &else_ty,
                    span,
                    ReportContext::IfBranch {
                        then_span: then_value_span,
                        else_span: else_value_span,
                    },
                )
            }
            None => then_ty,
        }
    }

    /// Infer `match` expressions while preserving existing arm-order diagnostics.
    pub(super) fn infer_match_expression(&mut self, input: MatchInferInput<'_>) -> InferType {
        let scrutinee_ty = self.infer_expression(input.scrutinee);
        if input.arms.is_empty() {
            if self.strict_mode_enabled() {
                self.errors.push(
                    diagnostic_for(&EMPTY_MATCH)
                        .with_file(self.file_path.clone())
                        .with_span(input.span)
                        .with_category(DiagnosticCategory::TypeInference),
                );
            }
            return self.env.alloc_infer_type_var();
        }
        let propagated_scrutinee = self.propagate_match_scrutinee_constraint(&scrutinee_ty, &input);
        let (first_ty, first_span) =
            self.infer_first_match_arm(input.arms, &propagated_scrutinee, input.span);
        let (result_ty, arm_types) = self.infer_remaining_match_arms(
            input.arms,
            &propagated_scrutinee,
            input.span,
            &first_ty,
            first_span,
        );
        self.infer_match_concrete_pivot_conflicts(&first_ty, &arm_types);
        result_ty
    }

    /// Infer optional family constraint for a match scrutinee.
    fn propagate_match_scrutinee_constraint(
        &mut self,
        scrutinee_ty: &InferType,
        input: &MatchInferInput<'_>,
    ) -> InferType {
        self.shared_pattern_family(input.arms)
            .and_then(|family| self.expected_type_for_pattern_family(&family))
            .map(|expected| self.unify_reporting(scrutinee_ty, &expected, input.span))
            .unwrap_or_else(|| scrutinee_ty.clone())
    }

    /// Infer the first arm of a match expression under pattern bindings.
    fn infer_first_match_arm(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &InferType,
        span: Span,
    ) -> (InferType, Span) {
        self.env.enter_scope();
        self.bind_pattern_variables(&arms[0].pattern, scrutinee_ty, span);
        if let Some(guard) = &arms[0].guard {
            self.infer_expression(guard);
        }
        let first_ty = self.infer_expression(&arms[0].body);
        let first_span = arms[0].body.span();
        self.env.leave_scope();
        (first_ty, first_span)
    }

    /// Infer all remaining match arms and collect arm type metadata.
    fn infer_remaining_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &InferType,
        span: Span,
        first_ty: &InferType,
        first_span: Span,
    ) -> (InferType, Vec<(InferType, Span, usize)>) {
        let mut result_ty = first_ty.clone();
        let mut arm_types: Vec<(InferType, Span, usize)> = vec![(first_ty.clone(), first_span, 1)];
        for (i, arm) in arms.iter().skip(1).enumerate() {
            self.env.enter_scope();
            self.bind_pattern_variables(&arm.pattern, scrutinee_ty, span);
            if let Some(guard) = &arm.guard {
                self.infer_expression(guard);
            }
            let arm_ty = self.infer_expression(&arm.body);
            self.env.leave_scope();
            result_ty = self.unify_with_context(
                first_ty,
                &arm_ty,
                arm.span,
                ReportContext::MatchArm {
                    first_span,
                    arm_span: arm.body.span(),
                    arm_index: i + 2,
                },
            );
            arm_types.push((arm_ty, arm.body.span(), i + 1));
        }
        (result_ty, arm_types)
    }

    /// Emit additional conflicts between concrete match arms when first arm is unresolved.
    fn infer_match_concrete_pivot_conflicts(
        &mut self,
        first_ty: &InferType,
        arm_types: &[(InferType, Span, usize)],
    ) {
        if Self::is_fully_concrete(first_ty) {
            return;
        }
        let pivot = arm_types
            .iter()
            .find(|(ty, _, _)| Self::is_fully_concrete(ty))
            .cloned();
        let Some((pivot_ty, pivot_span, pivot_index)) = pivot else {
            return;
        };
        for (arm_ty, arm_span, arm_index) in arm_types {
            if *arm_index == pivot_index {
                continue;
            }
            if Self::is_fully_concrete(arm_ty) {
                let _ = self.unify_with_context(
                    &pivot_ty,
                    arm_ty,
                    *arm_span,
                    ReportContext::MatchArm {
                        first_span: pivot_span,
                        arm_span: *arm_span,
                        arm_index: *arm_index,
                    },
                );
            }
        }
    }
}
