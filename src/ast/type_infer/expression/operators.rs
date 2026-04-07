use super::*;

impl<'a> InferCtx<'a> {
    /// Infer infix operators with gradual fallback semantics.
    pub(super) fn infer_infix_expression(
        &mut self,
        left: &Expression,
        op: &str,
        right: &Expression,
        span: Span,
    ) -> InferType {
        let left_ty = self.infer_expression(left);
        let right_ty = self.infer_expression(right);
        match op {
            "+" => self.infer_add_operator(&left_ty, &right_ty, span),
            "-" | "*" | "/" | "%" => self.infer_arithmetic_operator(&left_ty, &right_ty, span),
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                self.unify_reporting(&left_ty, &right_ty, span);
                InferType::Con(TypeConstructor::Bool)
            }
            "&&" | "||" => {
                let bool_ty = InferType::Con(TypeConstructor::Bool);
                self.unify_reporting(&left_ty, &bool_ty, span);
                self.unify_reporting(&right_ty, &bool_ty, span);
                InferType::Con(TypeConstructor::Bool)
            }
            "++" => {
                self.unify_reporting(&left_ty, &right_ty, span);
                left_ty.apply_type_subst(&self.subst)
            }
            "|>" => right_ty,
            _ => InferType::Con(TypeConstructor::Any),
        }
    }

    /// Infer `+` where result may be numeric or string.
    fn infer_add_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        let resolved = self.unify_reporting(left_ty, right_ty, span);
        let substituted = resolved.apply_type_subst(&self.subst);
        match substituted {
            InferType::Con(TypeConstructor::Int)
            | InferType::Con(TypeConstructor::Float)
            | InferType::Con(TypeConstructor::String) => substituted,
            // Unresolved variable: preserve it — let call-site unification resolve.
            InferType::Var(_) => substituted,
            InferType::Con(TypeConstructor::Any) => InferType::Con(TypeConstructor::Any),
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                InferType::Con(TypeConstructor::Any)
            }
        }
    }

    /// Infer `-`, `*`, `/`, `%` where result must remain numeric.
    fn infer_arithmetic_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        let resolved = self.unify_reporting(left_ty, right_ty, span);
        let substituted = resolved.apply_type_subst(&self.subst);
        match substituted {
            InferType::Con(TypeConstructor::Int) | InferType::Con(TypeConstructor::Float) => {
                substituted
            }
            // Unresolved variable: preserve it — let call-site unification resolve.
            InferType::Var(_) => substituted,
            InferType::Con(TypeConstructor::Any) => InferType::Con(TypeConstructor::Any),
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                InferType::Con(TypeConstructor::Any)
            }
        }
    }
}
