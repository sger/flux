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
    /// Infer an expression and record its resolved HM type under a stable node id.
    ///
    /// Behavior:
    /// - Allocates/looks up expression node id.
    /// - Routes inference through literal/structured/effect families.
    /// - Applies current substitution to store a resolved result.
    ///
    /// Side effects:
    /// - May mutate environment/substitution/diagnostics through delegated handlers.
    /// - Mutates `self.expr_types` with the resolved type for this node.
    pub(super) fn infer_expression(&mut self, expr: &Expression) -> InferType {
        let node_id = self.node_id_for_expr(expr);
        let inferred = self
            .infer_literal_family_expression(expr)
            .unwrap_or_else(|| {
                self.infer_structured_family_expression(expr)
                    .or_else(|| self.infer_effect_family_expression(expr))
                    .unwrap_or_else(|| self.infer_unknown_expr_fallback())
            });

        let resolved = inferred.apply_type_subst(&self.subst);
        self.expr_types.insert(node_id, resolved.clone());
        resolved
    }

    /// Infer literal-family expressions (`Integer`, `Identifier`, `Some`, ...).
    fn infer_literal_family_expression(&mut self, expr: &Expression) -> Option<InferType> {
        self.infer_literal_expression(expr)
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
            "+" => self.infer_numeric_or_string_infix(&left_ty, &right_ty, span),
            "-" | "*" | "/" | "%" => self.infer_numeric_infix(&left_ty, &right_ty, span),
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
    fn infer_numeric_or_string_infix(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        let resolved = self.unify_reporting(left_ty, right_ty, span);
        match resolved.apply_type_subst(&self.subst) {
            InferType::Con(TypeConstructor::Int)
            | InferType::Con(TypeConstructor::Float)
            | InferType::Con(TypeConstructor::String) => resolved.apply_type_subst(&self.subst),
            InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                InferType::Con(TypeConstructor::Any)
            }
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                InferType::Con(TypeConstructor::Any)
            }
        }
    }

    /// Infer `-`, `*`, `/`, `%` where result must remain numeric.
    fn infer_numeric_infix(
        &mut self,
        left_ty: &InferType,
        right_ty: &InferType,
        span: Span,
    ) -> InferType {
        let resolved = self.unify_reporting(left_ty, right_ty, span);
        match resolved.apply_type_subst(&self.subst) {
            InferType::Con(TypeConstructor::Int) | InferType::Con(TypeConstructor::Float) => {
                resolved.apply_type_subst(&self.subst)
            }
            InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                InferType::Con(TypeConstructor::Any)
            }
            other => {
                let expected_numeric = InferType::Con(TypeConstructor::Int);
                self.unify_reporting(&other, &expected_numeric, span);
                InferType::Con(TypeConstructor::Any)
            }
        }
    }
}
