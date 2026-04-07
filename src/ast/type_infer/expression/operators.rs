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
            "==" | "!=" => {
                self.unify_reporting(&left_ty, &right_ty, span);
                // Emit Eq constraint if the Eq class is declared.
                if let Some(eq_sym) = self.class_sym_eq {
                    let resolved = left_ty.apply_type_subst(&self.subst);
                    self.emit_class_constraint(eq_sym, resolved, span);
                }
                InferType::Con(TypeConstructor::Bool)
            }
            "<" | "<=" | ">" | ">=" => {
                self.unify_reporting(&left_ty, &right_ty, span);
                // Emit Ord constraint if the Ord class is declared.
                if let Some(ord_sym) = self.class_sym_ord {
                    let resolved = left_ty.apply_type_subst(&self.subst);
                    self.emit_class_constraint(ord_sym, resolved, span);
                }
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
                let resolved = left_ty.apply_type_subst(&self.subst);
                // Emit Semigroup constraint if declared.
                if let Some(sg_sym) = self.class_sym_semigroup {
                    self.emit_class_constraint(sg_sym, resolved.clone(), span);
                }
                resolved
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
        // Emit Num constraint for numeric + usage.
        if let Some(num_sym) = self.class_sym_num {
            self.emit_class_constraint(num_sym, substituted.clone(), span);
        }
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
        // Emit Num constraint for arithmetic usage.
        if let Some(num_sym) = self.class_sym_num {
            self.emit_class_constraint(num_sym, substituted.clone(), span);
        }
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
