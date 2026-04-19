use super::*;
use crate::diagnostics::{
    DiagnosticCategory, compiler_errors::UNKNOWN_INFIX_OPERATOR, diagnostic_for,
};

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
            "==" | "!=" => self.infer_eq_operator(&left_ty, &right_ty, span),
            "<" | "<=" | ">" | ">=" => self.infer_ord_operator(&left_ty, &right_ty, span),
            "&&" | "||" => self.infer_bool_operator(&left_ty, &right_ty, span),
            "++" => self.infer_semigroup_operator(&left_ty, &right_ty, span),
            "|>" => right_ty,
            _ => {
                self.errors.push(
                    diagnostic_for(&UNKNOWN_INFIX_OPERATOR)
                        .with_file(self.file_path.clone())
                        .with_span(span)
                        .with_message(format!("Unknown infix operator: `{op}`."))
                        .with_category(DiagnosticCategory::TypeInference),
                );
                self.alloc_fallback_var()
            }
        }
    }

    /// Infer equality/inequality operators and emit `Eq` obligations when declared.
    fn infer_eq_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        self.unify_reporting(left_ty, right_ty, span);
        self.emit_operator_constraint(self.class_sym_eq, left_ty, span);
        InferType::Con(TypeConstructor::Bool)
    }

    /// Infer ordering operators and emit `Ord` obligations when declared.
    fn infer_ord_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        self.unify_reporting(left_ty, right_ty, span);
        self.emit_operator_constraint(self.class_sym_ord, left_ty, span);
        InferType::Con(TypeConstructor::Bool)
    }

    /// Infer boolean operators by constraining both operands to `Bool`.
    fn infer_bool_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        let bool_ty = InferType::Con(TypeConstructor::Bool);
        self.unify_reporting(left_ty, &bool_ty, span);
        self.unify_reporting(right_ty, &bool_ty, span);
        InferType::Con(TypeConstructor::Bool)
    }

    /// Infer append and emit the corresponding `Semigroup` obligation.
    fn infer_semigroup_operator(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        self.unify_reporting(left_ty, right_ty, span);
        let resolved = left_ty.apply_type_subst(&self.subst);
        self.emit_operator_constraint_for_type(self.class_sym_semigroup, resolved.clone(), span);
        resolved
    }

    /// Resolve the operand type and emit an inferred operator class constraint.
    fn emit_operator_constraint(
        &mut self,
        class_sym: Option<Identifier>,
        ty: &InferType,
        span: Span,
    ) {
        let resolved = ty.apply_type_subst(&self.subst);
        self.emit_operator_constraint_for_type(class_sym, resolved, span);
    }

    /// Emit an inferred operator class constraint for an already-resolved type.
    fn emit_operator_constraint_for_type(
        &mut self,
        class_sym: Option<Identifier>,
        ty: InferType,
        span: Span,
    ) {
        if let Some(class_sym) = class_sym {
            self.emit_class_constraint(
                class_sym,
                ty,
                span,
                constraint::WantedClassConstraintOrigin::InferredOperator,
            );
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
            | InferType::Var(_) => {
                self.emit_operator_constraint_for_type(
                    self.class_sym_num,
                    substituted.clone(),
                    span,
                );
                substituted
            }
            InferType::Con(TypeConstructor::String) => substituted,
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                self.alloc_fallback_var()
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
        self.emit_operator_constraint_for_type(self.class_sym_num, substituted.clone(), span);
        match substituted {
            InferType::Con(TypeConstructor::Int) | InferType::Con(TypeConstructor::Float) => {
                substituted
            }
            InferType::Var(_) => substituted,
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                self.alloc_fallback_var()
            }
        }
    }
}
