use crate::syntax::expression::HandleArm;

use super::*;

impl<'a> InferCtx<'a> {
    /// Infer `perform` expression nodes.
    pub(super) fn infer_perform_expression(
        &mut self,
        effect: Identifier,
        operation: Identifier,
        args: &[Expression],
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = args.iter().map(|arg| self.infer_expression(arg)).collect();
        if let Some((param_tys, ret_ty)) = self.effect_op_signature_types(effect, operation) {
            if arg_tys.len() == param_tys.len() {
                for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
                    self.unify_reporting(actual, expected, span);
                }
                ret_ty.apply_type_subst(&self.subst)
            } else {
                if self.strict_mode_enabled() {
                    self.emit_strict_inference_error(
                        span,
                        format!(
                            "Effect operation `{}` was called with the wrong number of arguments in strict mode.",
                            self.interner.resolve(operation)
                        ),
                        "Match the declared operation signature exactly.",
                    );
                }
                self.alloc_fallback_var()
            }
        } else {
            if self.strict_mode_enabled() {
                self.emit_strict_inference_error(
                    span,
                    format!(
                        "Could not resolve the effect signature for `{}.{}` in strict mode.",
                        self.interner.resolve(effect),
                        self.interner.resolve(operation)
                    ),
                    "Declare the effect operation before using `perform`, and make sure it is in scope.",
                );
            }
            self.alloc_fallback_var()
        }
    }

    /// Infer `handle` expression nodes.
    pub(super) fn infer_handle_expression(
        &mut self,
        expr: &Expression,
        effect: Identifier,
        arms: &[HandleArm],
    ) -> InferType {
        let handled_ty = self.with_handle_effect(effect, |ctx| ctx.infer_expression(expr));
        let mut arm_result: Option<InferType> = None;
        for (arm_index, arm) in arms.iter().enumerate() {
            self.env.enter_scope();
            if let Some((param_tys, _ret_ty)) =
                self.effect_op_signature_types(effect, arm.operation_name)
            {
                for (param_name, param_ty) in arm.params.iter().zip(param_tys.iter()) {
                    self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
                }
            } else if self.strict_mode_enabled() {
                self.emit_strict_inference_error(
                    arm.span,
                    format!(
                        "Could not resolve handler arm `{}.{}` in strict mode.",
                        self.interner.resolve(effect),
                        self.interner.resolve(arm.operation_name)
                    ),
                    "Declare the handled operation before using it in a handler arm.",
                );
            }
            let body_ty = self.with_handle_effect(effect, |ctx| ctx.infer_expression(&arm.body));
            self.env.leave_scope();
            arm_result = Some(match arm_result {
                Some(prev) => self.unify_with_context(
                    &prev,
                    &body_ty,
                    arm.body.span(),
                    ReportContext::MatchArm {
                        first_span: arms[0].body.span(),
                        arm_span: arm.body.span(),
                        arm_index: arm_index + 1,
                    },
                ),
                None => body_ty,
            });
        }

        let arm_ty = arm_result.unwrap_or_else(|| {
            if self.strict_mode_enabled() {
                self.emit_strict_inference_error(
                    expr.span(),
                    "Handle expressions must include at least one typed handler arm in strict mode.",
                    "Add at least one handler arm, or use a handled expression with a declared effect operation.",
                );
            }
            self.alloc_fallback_var()
        });
        self.unify_with_context(&handled_ty, &arm_ty, expr.span(), ReportContext::Plain)
    }
}
