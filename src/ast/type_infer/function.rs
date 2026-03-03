use super::*;

impl<'a> InferCtx<'a> {
    // ── Function inference ────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub(super) fn infer_fn(
        &mut self,
        name: Identifier,
        fn_span: Span,
        type_params: &[Identifier],
        parameters: &[Identifier],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &Block,
    ) {
        // Map explicit type parameters (e.g. `T`, `U`) to fresh type variables.
        let tp_map: HashMap<Identifier, TypeVarId> =
            type_params.iter().map(|s| (*s, self.env.fresh())).collect();
        let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();

        self.env.enter_scope();

        // Bind each parameter to its annotated type (or a fresh variable).
        let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
        for (i, &param) in parameters.iter().enumerate() {
            let ty = parameter_types
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|te| {
                    TypeEnv::infer_type_from_type_expr_with_row_vars(
                        te,
                        &tp_map,
                        self.interner,
                        &mut row_var_env,
                        &mut self.env.counter,
                    )
                })
                .unwrap_or_else(|| self.env.fresh_infer_type());
            param_tys.push(ty.clone());
            self.env.bind(param, Scheme::mono(ty));
        }

        let ambient_effect_row = if effects.is_empty() {
            InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), self.env.fresh())
        } else {
            Self::infer_effect_row(effects, &mut row_var_env, &mut self.env.counter)
        };

        // The *declared* row is what appears in the function's published scheme.
        // Unannotated functions get a closed-empty row so they don't suddenly
        // acquire inferred effect types in their signatures (gradual compat).
        // The *ambient* row (open) is used only inside the body for permissive
        // call-site checking.
        let declared_effect_row = if effects.is_empty() {
            InferEffectRow::closed_empty()
        } else {
            ambient_effect_row.clone()
        };

        // Infer the body type.
        let body_ty =
            self.with_ambient_effect_row(ambient_effect_row.clone(), |ctx| ctx.infer_block(body));

        // Propagate the return type annotation constraint silently — the
        // compiler's boundary checker in statement.rs is the authoritative
        // reporter for return type mismatches.
        let mut ret_ty = match return_type {
            Some(ret_ann) => {
                match TypeEnv::infer_type_from_type_expr_with_row_vars(
                    ret_ann,
                    &tp_map,
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                ) {
                    Some(ann_ty) => self.unify_propagate(&body_ty, &ann_ty),
                    None => body_ty.apply_type_subst(&self.subst),
                }
            }
            None => body_ty.apply_type_subst(&self.subst),
        };

        // T11 (self-only): run one extra refinement pass for unannotated
        // self-recursive functions so recursive call result types can feed
        // back into the function return slot.
        if return_type.is_none()
            && type_params.is_empty()
            && self.block_contains_self_call(body, name)
        {
            ret_ty = self.refine_unannotated_self_recursive_return(
                name,
                parameters,
                &param_tys,
                &ambient_effect_row,
                body,
                &ret_ty,
            );
        }

        // Resolve parameter types through the accumulated substitution.
        let final_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|t| t.apply_type_subst(&self.subst))
            .collect();
        let effect_row = declared_effect_row.apply_row_subst(&self.subst);
        let fn_ty = InferType::Fun(final_param_tys, Box::new(ret_ty), effect_row);

        self.env.leave_scope();

        // Generalize: quantify over type variables that are free in `fn_ty`
        // but not in the surrounding environment (the let-generalization step).
        // We only generalize functions with explicit type parameters — for
        // implicitly typed functions, we keep the monomorphic type so that
        // unification constraints across call sites are preserved.
        let env_free = self.env.free_vars();
        let scheme = if !type_params.is_empty() {
            generalize(&fn_ty, &env_free)
        } else {
            Scheme::mono(fn_ty)
        };

        // Update the pre-declared entry (from Phase A).
        self.env.bind_with_span(name, scheme, Some(fn_span));
    }

    pub(super) fn refine_unannotated_self_recursive_return(
        &mut self,
        name: Identifier,
        parameters: &[Identifier],
        param_tys: &[InferType],
        effect_row: &InferEffectRow,
        body: &Block,
        current_ret: &InferType,
    ) -> InferType {
        self.env.enter_scope();
        let refined_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|ty| ty.apply_type_subst(&self.subst))
            .collect();
        for (param_name, param_ty) in parameters.iter().zip(refined_param_tys.iter()) {
            self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
        }
        let ret_slot = self.env.fresh_infer_type();
        let self_fn_ty = InferType::Fun(
            refined_param_tys,
            Box::new(ret_slot.clone()),
            effect_row.apply_row_subst(&self.subst),
        );
        self.env.bind(name, Scheme::mono(self_fn_ty));
        let second_body_ty =
            self.with_ambient_effect_row(effect_row.clone(), |ctx| ctx.infer_block(body));
        let refined_ret = self.unify_propagate(&second_body_ty, &ret_slot);
        self.env.leave_scope();
        let refined_resolved = refined_ret.apply_type_subst(&self.subst);
        let current_resolved = current_ret.apply_type_subst(&self.subst);
        let current_concrete = Self::is_concrete_non_any(&current_resolved);
        let refined_concrete = Self::is_concrete_non_any(&refined_resolved);

        if current_concrete && !refined_concrete {
            current_resolved
        } else if (refined_concrete && !current_concrete) || current_ret.contains_any() {
            refined_resolved
        } else if refined_resolved.contains_any() {
            // Keep the prior concrete inference when the refinement pass did not
            // increase precision and would otherwise fall back to Any.
            current_resolved
        } else {
            self.unify_propagate(&current_resolved, &refined_resolved)
                .apply_type_subst(&self.subst)
        }
    }

    pub(super) fn block_contains_self_call(&self, block: &Block, name: Identifier) -> bool {
        block
            .statements
            .iter()
            .any(|stmt| self.statement_contains_self_call(stmt, name))
    }

    pub(super) fn statement_contains_self_call(&self, stmt: &Statement, name: Identifier) -> bool {
        match stmt {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => self.expression_contains_self_call(value, name),
            Statement::Return {
                value: Some(expr), ..
            }
            | Statement::Expression {
                expression: expr, ..
            } => self.expression_contains_self_call(expr, name),
            Statement::Module { body, .. } => self.block_contains_self_call(body, name),
            _ => false,
        }
    }

    pub(super) fn expression_contains_self_call(
        &self,
        expr: &Expression,
        name: Identifier,
    ) -> bool {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if let Expression::Identifier { name: callee, .. } = function.as_ref()
                    && *callee == name
                {
                    return true;
                }
                self.expression_contains_self_call(function, name)
                    || arguments
                        .iter()
                        .any(|arg| self.expression_contains_self_call(arg, name))
            }
            Expression::Prefix { right, .. } => self.expression_contains_self_call(right, name),
            Expression::Infix { left, right, .. } => {
                self.expression_contains_self_call(left, name)
                    || self.expression_contains_self_call(right, name)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expression_contains_self_call(condition, name)
                    || self.block_contains_self_call(consequence, name)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_contains_self_call(b, name))
            }
            Expression::DoBlock { block, .. } => self.block_contains_self_call(block, name),
            Expression::Function { .. } => false,
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements
                .iter()
                .any(|element| self.expression_contains_self_call(element, name)),
            Expression::Hash { pairs, .. } => pairs.iter().any(|(k, v)| {
                self.expression_contains_self_call(k, name)
                    || self.expression_contains_self_call(v, name)
            }),
            Expression::Cons { head, tail, .. } => {
                self.expression_contains_self_call(head, name)
                    || self.expression_contains_self_call(tail, name)
            }
            Expression::Index { left, index, .. } => {
                self.expression_contains_self_call(left, name)
                    || self.expression_contains_self_call(index, name)
            }
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => {
                self.expression_contains_self_call(object, name)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.expression_contains_self_call(scrutinee, name)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(|g| self.expression_contains_self_call(g, name))
                            || self.expression_contains_self_call(&arm.body, name)
                    })
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.expression_contains_self_call(value, name),
            Expression::Perform { args, .. } => args
                .iter()
                .any(|arg| self.expression_contains_self_call(arg, name)),
            Expression::Handle { expr, arms, .. } => {
                self.expression_contains_self_call(expr, name)
                    || arms
                        .iter()
                        .any(|arm| self.expression_contains_self_call(&arm.body, name))
            }
            _ => false,
        }
    }

    /// Span of the expression that determines a block's value in HM inference.
    /// Falls back to the full block span when the block has no value expression.
    pub(super) fn block_value_span(&self, block: &Block) -> Span {
        let mut value_span = block.span;
        for stmt in &block.statements {
            match stmt {
                Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                } => {
                    value_span = expression.span();
                }
                Statement::Return {
                    value: Some(expr), ..
                } => {
                    value_span = expr.span();
                }
                _ => {
                    value_span = block.span;
                }
            }
        }
        value_span
    }
}
