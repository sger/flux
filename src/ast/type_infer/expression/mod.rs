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
            Expression::String { .. } | Expression::InterpolatedString { .. } => {
                InferType::Con(TypeConstructor::String)
            }
            Expression::None { .. } => InferType::App(
                TypeConstructor::Option,
                vec![self.env.alloc_infer_type_var()],
            ),
            // Wrappers
            Expression::Some { value, .. } => {
                let inner = self.infer_expression(value);
                InferType::App(TypeConstructor::Option, vec![inner])
            }
            Expression::Left { value, .. } => {
                let inner = self.infer_expression(value);
                let right = self.env.alloc_infer_type_var();
                InferType::App(TypeConstructor::Either, vec![inner, right])
            }
            Expression::Right { value, .. } => {
                let inner = self.infer_expression(value);
                let left = self.env.alloc_infer_type_var();
                InferType::App(TypeConstructor::Either, vec![left, inner])
            }
            // Identifiers
            Expression::Identifier { name, .. } => {
                if let Some(scheme) = self.env.lookup(*name).cloned() {
                    let (ty, mapping, _constraints) = scheme.instantiate(&mut self.env.counter);
                    for &fresh in mapping.values() {
                        self.env.record_var_level(fresh);
                    }
                    ty
                } else {
                    if self.known_flow_names.contains(name) {
                        self.emit_missing_flow_hm_signature(*name, expr.span());
                    }
                    InferType::Con(TypeConstructor::Any)
                }
            }
            // Operators
            Expression::Prefix { right, .. } => self.infer_expression(right),
            Expression::Infix {
                left,
                operator,
                right,
                span,
                ..
            } => self.infer_infix_expression(left, operator, right, *span),
            // Control flow
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
                ..
            } => self.infer_if_expression(condition, consequence, alternative, *span),
            Expression::DoBlock { block, .. } => self.infer_block(block),
            Expression::Match {
                scrutinee,
                arms,
                span,
                ..
            } => self.infer_match_expression(MatchInferInput {
                scrutinee,
                arms,
                span: *span,
            }),
            // Functions
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
                ..
            } => self.infer_call_expression(CallInferInput {
                function,
                arguments,
                span: *span,
            }),
            // Collections
            Expression::TupleLiteral { elements, .. } => {
                self.infer_tuple_literal_expression(elements)
            }
            Expression::ListLiteral { elements, span, .. } => {
                self.infer_list_literal_expression(elements, *span)
            }
            Expression::ArrayLiteral { elements, .. } => {
                self.infer_array_literal_expression(elements)
            }
            Expression::EmptyList { .. } => {
                InferType::App(TypeConstructor::List, vec![self.env.alloc_infer_type_var()])
            }
            Expression::Hash { pairs, .. } => self.infer_hash_literal_expression(pairs),
            Expression::Cons {
                head, tail, span, ..
            } => self.infer_cons_expression(head, tail, *span),
            // Access
            Expression::Index { left, index, .. } => self.infer_index_expression(left, index),
            Expression::MemberAccess { object, member, .. } => {
                self.infer_member_access_expression(expr, object, *member)
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                self.infer_tuple_field_access_expression(object, *index)
            }
            // Effects
            Expression::Perform {
                effect,
                operation,
                args,
                span,
                ..
            } => self.infer_perform_expression(*effect, *operation, args, *span),
            Expression::Handle {
                expr, effect, arms, ..
            } => self.infer_handle_expression(expr, *effect, arms),
            // Fallback guards against future Expression variants
            #[allow(unreachable_patterns)]
            _ => InferType::Con(TypeConstructor::Any),
        };

        let resolved = inferred.apply_type_subst(&self.subst);
        self.expr_types.insert(expr_id, resolved.clone());
        resolved
    }
}
