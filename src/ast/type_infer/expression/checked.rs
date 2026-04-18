use super::*;

impl<'a> InferCtx<'a> {
    /// Bidirectional check-mode entry point (Proposal 0159, Phase 1).
    ///
    /// Walks `expr` against `expected`, propagating the expected type into
    /// sub-expressions where doing so yields more precise diagnostics. For
    /// nodes without a specialised rule, falls back to `infer + unify_reporting`
    /// so the default rule preserves the existing diagnostic shape.
    ///
    /// Note: this intentionally does not overwrite `self.expr_types[expr]` —
    /// check_expression runs *after* a prior `infer_expression` pass in the
    /// typed-let driver, and that pass has already recorded the authoritative
    /// inferred type for the expression. Overwriting with `expected` would
    /// mask the actual inferred shape from downstream consumers.
    pub(in crate::ast::type_infer) fn check_expression(
        &mut self,
        expr: &Expression,
        expected: &InferType,
    ) -> InferType {
        let resolved = self
            .dispatch_check_expression(expr, expected)
            .unwrap_or_else(|| {
                let actual = self.infer_expression(expr);
                self.unify_reporting(expected, &actual, expr.span())
            });
        resolved.apply_type_subst(&self.subst)
    }

    /// Dispatch to a specialised check rule when one applies, otherwise return
    /// `None` so the caller falls back to `infer + unify_reporting` default.
    fn dispatch_check_expression(
        &mut self,
        expr: &Expression,
        expected: &InferType,
    ) -> Option<InferType> {
        match expr {
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
                ..
            } => Some(self.check_if_expression(
                condition,
                consequence,
                alternative.as_ref(),
                expected,
                *span,
            )),
            Expression::Match {
                scrutinee,
                arms,
                span,
                ..
            } => Some(self.check_match_expression(scrutinee, arms, expected, *span)),
            Expression::DoBlock { block, .. } => Some(self.check_block_value(block, expected)),
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => Some(self.check_lambda_expression(
                LambdaInferInput {
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                },
                expected,
                expr.span(),
            )),
            _ => self.dispatch_check_collection(expr, expected),
        }
    }

    /// Dispatcher for collection / wrapper literals — split out from the
    /// main dispatch to keep each helper under the complexity budget.
    fn dispatch_check_collection(
        &mut self,
        expr: &Expression,
        expected: &InferType,
    ) -> Option<InferType> {
        match expr {
            Expression::TupleLiteral { elements, .. } => {
                Some(self.check_tuple_literal(elements, expected, expr.span()))
            }
            Expression::ListLiteral { elements, span, .. } => {
                Some(self.check_list_literal(elements, expected, *span))
            }
            Expression::ArrayLiteral { elements, .. } => {
                Some(self.check_array_literal(elements, expected, expr.span()))
            }
            Expression::Hash { pairs, .. } => {
                Some(self.check_hash_literal(pairs, expected, expr.span()))
            }
            Expression::Cons {
                head, tail, span, ..
            } => Some(self.check_cons_expression(head, tail, expected, *span)),
            Expression::Some { value, .. } => {
                Some(self.check_wrapper(value, expected, TypeConstructor::Option, 0, expr.span()))
            }
            Expression::Left { value, .. } => {
                Some(self.check_wrapper(value, expected, TypeConstructor::Either, 0, expr.span()))
            }
            Expression::Right { value, .. } => {
                Some(self.check_wrapper(value, expected, TypeConstructor::Either, 1, expr.span()))
            }
            _ => None,
        }
    }

    /// Check a lambda against an expected function type, propagating declared
    /// parameter types into the body so body-level mismatches report at the
    /// offending sub-expression rather than at the lambda span after-the-fact.
    /// Arity mismatch falls back to plain inference + unify so the existing
    /// function-arity diagnostic still fires.
    fn check_lambda_expression(
        &mut self,
        input: LambdaInferInput<'_>,
        expected: &InferType,
        lambda_span: Span,
    ) -> InferType {
        let InferType::Fun(expected_params, expected_ret, expected_effects) = expected else {
            let actual = self.infer_lambda_expression(input);
            return self.unify_reporting(expected, &actual, lambda_span);
        };
        if expected_params.len() != input.parameters.len() {
            let actual = self.infer_lambda_expression(input);
            return self.unify_reporting(expected, &actual, lambda_span);
        }
        self.env.enter_scope();
        for ((name, expected_ty), annotation) in input
            .parameters
            .iter()
            .zip(expected_params.iter())
            .zip(input.parameter_types.iter())
        {
            let ty = match annotation {
                Some(ann) => {
                    let mut row_env = HashMap::new();
                    let ann_ty = self
                        .infer_type_from_annotation(ann, &HashMap::new(), &mut row_env)
                        .unwrap_or_else(|| expected_ty.clone());
                    self.unify_reporting(expected_ty, &ann_ty, ann.span())
                }
                None => expected_ty.clone(),
            };
            self.env.bind(*name, Scheme::mono(ty));
        }
        let ambient = if input.effects.is_empty() {
            expected_effects.clone()
        } else {
            let mut row_env = HashMap::new();
            self.infer_effect_row(input.effects, &mut row_env)
        };
        self.with_ambient_effect_row(ambient, |ctx| {
            ctx.check_block_value(input.body, expected_ret);
        });
        self.env.leave_scope();
        expected.apply_type_subst(&self.subst)
    }

    /// Check mode for `if` expressions: each branch is checked against the
    /// expected type so branch-level mismatches report at the branch span.
    fn check_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: Option<&Block>,
        expected: &InferType,
        span: Span,
    ) -> InferType {
        let cond_ty = self.infer_expression(condition);
        self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), span);
        self.check_block_value(consequence, expected);
        if let Some(alt) = alternative {
            self.check_block_value(alt, expected);
        }
        expected.apply_type_subst(&self.subst)
    }

    /// Check mode for `match` expressions: each arm body is checked against
    /// the expected type so arm mismatches report at the arm body span.
    fn check_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        let scrutinee_ty = self.infer_expression(scrutinee);
        if arms.is_empty() {
            return self.infer_match_expression(MatchInferInput {
                scrutinee,
                arms,
                span,
            });
        }
        let first_span = arms[0].body.span();
        for (i, arm) in arms.iter().enumerate() {
            self.env.enter_scope();
            self.bind_pattern_variables(&arm.pattern, &scrutinee_ty, span);
            if let Some(guard) = &arm.guard {
                self.infer_expression(guard);
            }
            // Preserve MatchArm report context so snapshot tests that pin the
            // specialised "arms of this `match` expression do not agree"
            // diagnostic keep their shape. For propagatable arm bodies, still
            // dispatch via check_expression first to enable sub-expression
            // precision; otherwise emit MatchArm-context mismatch directly.
            if is_check_propagatable_body(&arm.body) {
                self.check_expression(&arm.body, expected);
            } else {
                let arm_ty = self.infer_expression(&arm.body);
                self.unify_with_context(
                    expected,
                    &arm_ty,
                    arm.body.span(),
                    ReportContext::MatchArm {
                        first_span,
                        arm_span: arm.body.span(),
                        arm_index: i + 1,
                    },
                );
            }
            self.env.leave_scope();
        }
        expected.apply_type_subst(&self.subst)
    }

    /// Check the block's final value expression against `expected`.
    /// Non-value statements are inferred for side-effects as usual.
    fn check_block_value(&mut self, block: &Block, expected: &InferType) -> InferType {
        let stmts = &block.statements;
        let value_index = stmts.iter().rposition(|stmt| {
            matches!(
                stmt,
                Statement::Expression {
                    has_semicolon: false,
                    ..
                } | Statement::Return { value: Some(_), .. }
            )
        });
        for (i, stmt) in stmts.iter().enumerate() {
            if Some(i) == value_index {
                match stmt {
                    Statement::Expression { expression, .. } => {
                        self.check_expression(expression, expected);
                    }
                    Statement::Return {
                        value: Some(expr), ..
                    } => {
                        self.check_expression(expr, expected);
                    }
                    _ => unreachable!(),
                }
            } else {
                self.infer_statement(stmt);
            }
        }
        expected.apply_type_subst(&self.subst)
    }

    /// Check a tuple literal against an expected `Tuple` type by recursing
    /// into each element with the corresponding expected type. Arity or kind
    /// mismatches fall back to plain infer + unify so the outer annotation
    /// diagnostic remains.
    fn check_tuple_literal(
        &mut self,
        elements: &[Expression],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        if let InferType::Tuple(expected_elems) = expected
            && expected_elems.len() == elements.len()
        {
            for (elem, exp) in elements.iter().zip(expected_elems.iter()) {
                self.check_expression(elem, exp);
            }
            return expected.apply_type_subst(&self.subst);
        }
        let actual = self.infer_tuple_literal_expression(elements);
        self.unify_reporting(expected, &actual, span)
    }

    /// Check a list literal against an expected `List<T>` type by recursing
    /// into each element with `T` as the expected type.
    fn check_list_literal(
        &mut self,
        elements: &[Expression],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        if let Some(elem_ty) = unwrap_app1(expected, TypeConstructor::List) {
            for element in elements {
                self.check_expression(element, elem_ty);
            }
            return expected.apply_type_subst(&self.subst);
        }
        let actual = self.infer_list_literal_expression(elements, span);
        self.unify_reporting(expected, &actual, span)
    }

    /// Check an array literal against an expected `Array<T>` type.
    fn check_array_literal(
        &mut self,
        elements: &[Expression],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        if let Some(elem_ty) = unwrap_app1(expected, TypeConstructor::Array) {
            for element in elements {
                self.check_expression(element, elem_ty);
            }
            return expected.apply_type_subst(&self.subst);
        }
        let actual = self.infer_array_literal_expression(elements);
        self.unify_reporting(expected, &actual, span)
    }

    /// Check a hash literal against an expected `Map<K, V>` type by checking
    /// each pair's key against `K` and value against `V`.
    fn check_hash_literal(
        &mut self,
        pairs: &[(Expression, Expression)],
        expected: &InferType,
        span: Span,
    ) -> InferType {
        if let InferType::App(TypeConstructor::Map, args) = expected
            && args.len() == 2
        {
            let (key_ty, val_ty) = (&args[0], &args[1]);
            for (key, value) in pairs {
                self.check_expression(key, key_ty);
                self.check_expression(value, val_ty);
            }
            return expected.apply_type_subst(&self.subst);
        }
        let actual = self.infer_hash_literal_expression(pairs);
        self.unify_reporting(expected, &actual, span)
    }

    /// Check a cons expression (`[h | t]`) against an expected `List<T>`
    /// type. Head is checked against `T`, tail against `List<T>`.
    fn check_cons_expression(
        &mut self,
        head: &Expression,
        tail: &Expression,
        expected: &InferType,
        span: Span,
    ) -> InferType {
        if let Some(elem_ty) = unwrap_app1(expected, TypeConstructor::List) {
            self.check_expression(head, elem_ty);
            self.check_expression(tail, expected);
            return expected.apply_type_subst(&self.subst);
        }
        let actual = self.infer_cons_expression(head, tail, span);
        self.unify_reporting(expected, &actual, span)
    }

    /// Check a single-argument wrapper (`Some`, `Left`, `Right`) against an
    /// expected `App(ctor, args)` type by checking the inner value against
    /// `args[arg_index]`.
    fn check_wrapper(
        &mut self,
        value: &Expression,
        expected: &InferType,
        ctor: TypeConstructor,
        arg_index: usize,
        span: Span,
    ) -> InferType {
        if let InferType::App(c, args) = expected
            && *c == ctor
            && arg_index < args.len()
        {
            self.check_expression(value, &args[arg_index]);
            return expected.apply_type_subst(&self.subst);
        }
        let inner_ty = self.infer_expression(value);
        let actual_ty = match ctor {
            TypeConstructor::Option => InferType::App(TypeConstructor::Option, vec![inner_ty]),
            TypeConstructor::Either => {
                let other = self.env.alloc_infer_type_var();
                if arg_index == 0 {
                    InferType::App(TypeConstructor::Either, vec![inner_ty, other])
                } else {
                    InferType::App(TypeConstructor::Either, vec![other, inner_ty])
                }
            }
            _ => inner_ty,
        };
        self.unify_reporting(expected, &actual_ty, span)
    }
}

/// Unwrap `InferType::App(ctor, [arg])` when the constructor matches, returning
/// a reference to the single type argument. Used by list/array/option check
/// rules to pull out the element type from the expected `App`.
fn unwrap_app1(ty: &InferType, ctor: TypeConstructor) -> Option<&InferType> {
    match ty {
        InferType::App(c, args) if *c == ctor && args.len() == 1 => Some(&args[0]),
        _ => None,
    }
}

/// Return true when an expression has a specialised check-mode rule that
/// should own its mismatch diagnostic — used by `check_match_expression` to
/// decide whether to route through `check_expression` or emit a direct
/// MatchArm-context mismatch.
fn is_check_propagatable_body(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::If { .. }
            | Expression::Match { .. }
            | Expression::DoBlock { .. }
            | Expression::TupleLiteral { .. }
            | Expression::ListLiteral { .. }
            | Expression::ArrayLiteral { .. }
            | Expression::Hash { .. }
            | Expression::Cons { .. }
            | Expression::Some { .. }
            | Expression::Left { .. }
            | Expression::Right { .. }
            | Expression::Function { .. }
    )
}
