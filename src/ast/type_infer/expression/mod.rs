use super::*;

mod access;
mod calls;
mod collections;
mod control_flow;
mod effects_nodes;
mod lambda;
mod literals;
mod operators;
mod patterns;

impl<'a> InferCtx<'a> {
    /// Infer an expression and record its resolved HM type under a stable node id.
    ///
    /// Uses a single flat match over all expression variants — each variant is
    /// dispatched exactly once with no intermediate Option layers.
    pub(super) fn infer_expression(&mut self, expr: &Expression) -> InferType {
       let expr_id = expr.expr_id();

       let inferred = match expr {
           // Literals
           Expression::Integer { .. } => InferType::Con(TypeConstructor::Int),
           Expression::Float { .. } => InferType::Con(TypeConstructor::Float),
           Expression::Boolean { .. } => InferType::Con(TypeConstructor::Bool),
           Expression::String { .. } | Expression::InterpolatedString { .. } => InferType::Con(TypeConstructor::String),
           Expression::None { .. } => InferType::App(TypeConstructor::Option, vec![self.env.alloc_infer_type_var()]),
           // Wrappers
           Expression::Some { value, .. } => {
               let inner = self.infer_expression(value);
               InferType::App(TypeConstructor::Option, vec![inner])
           }
           Expression::Left { value, .. } => {
               let inner = self.infer_expression(expr);
               let right = self.env.alloc_infer_type_var();
               InferType::App(TypeConstructor::Either, vec![inner, right])
           }
           Expression::Right { value, .. } => {
               let inner = self.infer_expression(expr);
               let left = self.env.alloc_infer_type_var();
               InferType::App(TypeConstructor::Either, vec![left, inner])
           }
           // Identifiers
           Expression::Identifier { name, .. } => {
               if let Some(scheme) = self.env.lookup(*name).cloned()  {
                   let(ty, mapping) = scheme.instantiate(&mut self.env.counter);
                   for &fresh in mapping.values()  {
                       self.env.record_var_level(fresh);
                   }
                   ty
               }
           }
       }
    }

    /// Infer structured expressions (control-flow, lambdas, calls, data access).
    fn infer_structured_family_expression(&mut self, expr: &Expression) -> Option<InferType> {
        self.infer_control_or_callable_expression(expr)
            .or_else(|| self.infer_data_expression(expr))
    }

    /// Infer control-flow and callable structured expressions.
    fn infer_control_or_callable_expression(&mut self, expr: &Expression) -> Option<InferType> {
        let inferred = match expr {
            Expression::Prefix { right, .. } => self.infer_expression(right),
            Expression::Infix {
                left,
                operator,
                right,
                span,
            } => self.infer_infix_expression(left, operator, right, *span),
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
            } => self.infer_if_expression(condition, consequence, alternative, *span),
            Expression::DoBlock { block, .. } => self.infer_block(block),
            Expression::Match {
                scrutinee,
                arms,
                span,
            } => self.infer_match_expression(MatchInferInput {
                scrutinee,
                arms,
                span: *span,
            }),
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => self.infer_lambda_expression(LambdaInferInput {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
            }),
            Expression::Call {
                function,
                arguments,
                span,
            } => self.infer_call_expression(CallInferInput {
                function,
                arguments,
                span: *span,
            }),
            _ => return None,
        };
        Some(inferred)
    }

    /// Infer collection and access structured expressions.
    fn infer_data_expression(&mut self, expr: &Expression) -> Option<InferType> {
        if let Some(inferred) = self.infer_collection_expression(expr) {
            return Some(inferred);
        }
        self.infer_access_expression(expr)
    }

    /// Infer effect-family expressions (`perform`, `handle`).
    fn infer_effect_family_expression(&mut self, expr: &Expression) -> Option<InferType> {
        let inferred = match expr {
            Expression::Perform {
                effect,
                operation,
                args,
                span,
            } => self.infer_perform_expression(*effect, *operation, args, *span),
            Expression::Handle {
                expr,
                effect,
                arms,
                span: _,
            } => self.infer_handle_expression(expr, *effect, arms),
            _ => return None,
        };
        Some(inferred)
    }

    /// Fallback type for expression forms not handled by HM inference paths.
    fn infer_unknown_expr_fallback(&self) -> InferType {
        InferType::Con(TypeConstructor::Any)
    }
}
