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
                InferType::Con(TypeConstructor::Any)
            }
        } else {
            InferType::Con(TypeConstructor::Any)
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
        for arm in arms {
            self.env.enter_scope();
            if let Some((param_tys, _ret_ty)) =
                self.effect_op_signature_types(effect, arm.operation_name)
            {
                for (param_name, param_ty) in arm.params.iter().zip(param_tys.iter()) {
                    self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
                }
            }
            let body_ty = self.with_handle_effect(effect, |ctx| ctx.infer_expression(&arm.body));
            self.env.leave_scope();
            arm_result = Some(match arm_result {
                Some(prev) => self.join_types(&prev, &body_ty),
                None => body_ty,
            });
        }

        let arm_ty = arm_result.unwrap_or(InferType::Con(TypeConstructor::Any));
        self.join_types(&handled_ty, &arm_ty)
    }
}
