use sha2::digest::typenum::Le;

use super::*;

mod access;
mod calls;
mod collections;
mod control_flow;
mod effects_nodes;
mod lambda;
mod literals;
mod patterns;

impl<'a> InferCtx<'a> {
    pub(super) fn infer_expression(&mut self, expr: &Expression) -> InferType {}

    fn infer_literal_family_expression(&mut self, expr: &Expression) -> Option<InferType> {
        self.infer_literal_expression(expr)
    }

    fn infer_structured_family_expression(&mut self, expr: &Expression) -> Option<InferType> {}

    fn infer_control_or_callable_expression(&mut self, expr: &Expression) -> Option<InferType> {}

    fn infer_data_expression(&mut self, expr: &Expression) -> Option<InferType> {}

    fn infer_effect_family_expression(&mut self, expr: &Expression) -> Option<InferType> {

    }

    fn infer_unknown_expr_fallback(&self) -> InferType {

    }

    pub(super) fn infer_infix_expression(&mut self, left: &Expression, op: &str, right: &Expression, span: Span) -> InferType {}\

    fn infer_numeric_or_string_infix(&mut self, left_ty: &InferType, right_ty: &InferType, span: Span) -> InferType {}

    fn infer_numeric_infix(&mut self, left_ty: &InferType, right_ty: &InferType, span: Span) -> InferType {}


}
