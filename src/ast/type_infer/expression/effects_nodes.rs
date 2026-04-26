use crate::{
    diagnostics::{DiagnosticBuilder, position::Span},
    syntax::{
        Identifier,
        expression::{Expression, HandleArm},
    },
    types::{infer_type::InferType, scheme::Scheme},
};

use super::super::{InferCtx, ReportContext};

impl InferCtx<'_> {
    /// Emit E428 for parameterized handler shape mistakes discovered during HM.
    fn emit_parameterized_handler_shape_error(
        &mut self,
        span: Span,
        message: impl Into<String>,
        label: impl Into<String>,
    ) {
        self.errors.push(
            crate::diagnostics::Diagnostic::make_error_dynamic(
                "E428",
                "PARAMETERIZED HANDLER SHAPE ERROR",
                crate::diagnostics::ErrorType::Compiler,
                message.into(),
                Some(
                    "Parameterized handlers use `op(resume, args..., state) -> resume(result, next_state)`."
                        .to_string(),
                ),
                self.file_path.clone(),
                span,
            )
            .with_primary_label(span, label.into()),
        );
    }

    /// Map operation return types to the source value shape expected by `resume`.
    ///
    /// Flux source writes the unit value as `()`, which HM currently infers as
    /// an empty tuple. Effect operation signatures normally spell that return
    /// as `Unit`, so resume parameter checking accepts the source value shape.
    fn resume_argument_type_for_operation_return(ret_ty: &InferType) -> InferType {
        if matches!(
            ret_ty,
            InferType::Con(crate::types::type_constructor::TypeConstructor::Unit)
        ) {
            InferType::Tuple(Vec::new())
        } else {
            ret_ty.clone()
        }
    }

    /// Bind handler operation parameters and optional trailing state parameter.
    fn bind_handle_arm_params(
        &mut self,
        arm: &HandleArm,
        param_tys: &[InferType],
        parameter_ty: Option<&InferType>,
    ) {
        let expected_param_count = param_tys.len() + usize::from(parameter_ty.is_some());
        if parameter_ty.is_some() && arm.params.len() != expected_param_count {
            self.emit_parameterized_handler_shape_error(
                arm.span,
                format!(
                    "Handle arm `{}` expects {} parameter(s), got {}.",
                    self.interner.resolve(arm.operation_name),
                    expected_param_count,
                    arm.params.len()
                ),
                "handler arm parameter mismatch",
            );
        }

        for (param_name, param_ty) in arm.params.iter().zip(param_tys.iter()) {
            self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
        }
        if let Some(parameter_ty) = parameter_ty
            && let Some(state_name) = arm.params.get(param_tys.len())
        {
            self.env
                .bind(*state_name, Scheme::mono(parameter_ty.clone()));
        }
    }

    /// Bind the arm-local `resume` continuation with ordinary or stateful arity.
    fn bind_handle_arm_resume(
        &mut self,
        arm: &HandleArm,
        op_ret_ty: &InferType,
        parameter_ty: Option<&InferType>,
        handled_ty: &InferType,
    ) {
        let mut resume_params = vec![Self::resume_argument_type_for_operation_return(op_ret_ty)];
        if let Some(parameter_ty) = parameter_ty {
            resume_params.push(parameter_ty.clone());
        }
        self.env.bind(
            arm.resume_param,
            Scheme::mono(InferType::Fun(
                resume_params,
                Box::new(handled_ty.clone()),
                crate::types::infer_effect_row::InferEffectRow::closed_empty(),
            )),
        );
    }

    /// Infer one handler arm under an arm-local scope.
    fn infer_handle_arm_body(
        &mut self,
        effect: Identifier,
        arm: &HandleArm,
        parameter_ty: Option<&InferType>,
        handled_ty: &InferType,
    ) -> InferType {
        self.env.enter_scope();
        if let Some((param_tys, op_ret_ty)) =
            self.effect_op_signature_types(effect, arm.operation_name, arm.span)
        {
            self.bind_handle_arm_params(arm, &param_tys, parameter_ty);
            self.bind_handle_arm_resume(arm, &op_ret_ty, parameter_ty, handled_ty);
        }
        let body_ty = self.with_handle_effect(effect, |ctx| ctx.infer_expression(&arm.body));
        self.env.leave_scope();
        body_ty
    }

    /// Merge one inferred handler arm result into the accumulated arm type.
    fn merge_handle_arm_result(
        &mut self,
        arms: &[HandleArm],
        previous: Option<InferType>,
        body_ty: InferType,
        arm_index: usize,
    ) -> InferType {
        match previous {
            Some(prev) => self.unify_with_context(
                &prev,
                &body_ty,
                arms[arm_index].body.span(),
                ReportContext::MatchArm {
                    first_span: arms[0].body.span(),
                    arm_span: arms[arm_index].body.span(),
                    arm_index: arm_index + 1,
                },
            ),
            None => body_ty,
        }
    }

    /// Infer `perform` expression nodes.
    pub(super) fn infer_perform_expression(
        &mut self,
        effect: Identifier,
        operation: Identifier,
        args: &[Expression],
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = args.iter().map(|a| self.infer_expression(a)).collect();
        if let Some((param_tys, ret_ty)) = self.effect_op_signature_types(effect, operation, span) {
            if arg_tys.len() == param_tys.len() {
                for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
                    self.unify_reporting(actual, expected, span);
                }
                ret_ty.apply_type_subst(&self.subst)
            } else {
                self.alloc_fallback_var()
            }
        } else {
            self.alloc_fallback_var()
        }
    }

    /// Infer `handle` expression nodes.
    pub(super) fn infer_handle_expression(
        &mut self,
        expr: &Expression,
        effect: Identifier,
        parameter: Option<&Expression>,
        arms: &[HandleArm],
    ) -> InferType {
        let parameter_ty = parameter.map(|parameter| self.infer_expression(parameter));
        let handled_ty = self.with_handle_effect(effect, |ctx| ctx.infer_expression(expr));
        let mut arm_result: Option<InferType> = None;
        for (arm_index, arm) in arms.iter().enumerate() {
            let body_ty =
                self.infer_handle_arm_body(effect, arm, parameter_ty.as_ref(), &handled_ty);
            arm_result = Some(self.merge_handle_arm_result(arms, arm_result, body_ty, arm_index));
        }

        let arm_ty = arm_result.unwrap_or_else(|| self.alloc_fallback_var());
        self.unify_with_context(&handled_ty, &arm_ty, expr.span(), ReportContext::Plain)
    }
}
