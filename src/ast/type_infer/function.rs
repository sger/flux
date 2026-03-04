use serde_json::map;

use super::*;

impl<'a> InferCtx<'a> {
    // ── Function inference ────────────────────────────────────────────────────

    pub(super) fn infer_function_declaration(&mut self, input: FnInferInput<'_>) {
        // Map explicit type parameters (e.g. `T`, `U`) to fresh type variables.
        let tp_map = self.allocate_type_parameter_vars(input.type_params);
        let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();

        self.env.enter_scope();

        // Bind each parameter to its annotated type (or a fresh variable).
        let param_tys = self.infer_and_bind_parameter_types(
            &tp_map,
            &mut row_var_env,
            input.parameters,
            input.parameter_types,
        );
        let (declared_effect_row, ambient_effect_row) =
            self.infer_declared_and_ambient_effect_rows(input.effects, &mut row_var_env);

        let body_ty = self.with_ambient_effect_row(ambient_effect_row.clone(), |ctx| {
            ctx.infer_block(input.body)
        });
        let mut ret_ty = self.infern_return_type_with_optional_annotation(
            &tp_map,
            &mut row_var_env,
            input.return_type,
            &body_ty,
        );

        // T11 (self-only): run one extra refinement pass for unannotated
        // self-recursive functions so recursive call result types can feed
        // back into the function return slot.
        if input.return_type.is_none()
            && input.type_params.is_empty()
            && self.block_contains_self_call(input.body, input.name)
        {
            ret_ty = self.refine_unannotated_self_recursive_return(
                input.name,
                input.parameters,
                &param_tys,
                &ambient_effect_row,
                input.body,
                &ret_ty,
            );
        }

        self.finalize_and_bind_function_scheme(
            input.name,
            input.fn_span,
            input.type_params,
            &param_tys,
            &ret_ty,
            &declared_effect_row,
        )
    }

    /// Allocate fresh HM type variables for explicit generic type parameters.
    fn allocate_type_parameter_vars(
        &mut self,
        type_params: &[Identifier],
    ) -> HashMap<Identifier, TypeVarId> {
        type_params
            .iter()
            .map(|symbol| (*symbol, self.env.fresh()))
            .collect()
    }

    /// Infer and bind function parameters in the current scope.
    ///
    /// Each parameter uses its annotation when provided, otherwise a fresh type
    /// variable. Bound parameter schemes are monomorphic.
    pub(super) fn infer_and_bind_parameter_types(
        &mut self,
        type_params: &HashMap<Identifier, TypeVarId>,
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        parameters: &[Identifier],
        parameter_types: &[Option<TypeExpr>],
    ) -> Vec<InferType> {
        let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
        for (index, &param) in parameters.iter().enumerate() {
            let ty = parameter_types
                .get(index)
                .and_then(|opt| opt.as_ref())
                .and_then(|type_expr| {
                    self.infer_type_from_annotation(type_expr, type_params, row_var_env)
                })
                .unwrap_or_else(|| self.env.fresh_infer_type());
            param_tys.push(ty.clone());
            self.env.bind(param, Scheme::mono(ty));
        }
        param_tys
    }

    /// Infer the function effect rows used for signature publication and body checks.
    ///
    /// Returns `(declared, ambient)` where:
    /// - `declared` is written into the function type scheme.
    /// - `ambient` is pushed while inferring the body.
    fn infer_declared_and_ambient_effect_rows(
        &mut self,
        effects: &[EffectExpr],
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
    ) -> (InferEffectRow, InferEffectRow) {
        let ambient_effect_row = if effects.is_empty() {
            InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), self.env.fresh())
        } else {
            Self::infer_effect_row(effects, row_var_env, &mut self.env.counter)
        };
        let declared_effect_row = if effects.is_empty() {
            InferEffectRow::closed_empty()
        } else {
            ambient_effect_row.clone()
        };
        (declared_effect_row, ambient_effect_row)
    }

    /// Infer the function return type, applying annotation constraints silently.
    ///
    /// Annotation mismatches are propagated via substitutions diagnostics are
    /// emitted by compiler boundary checks, not by this HM helper.
    fn infern_return_type_with_optional_annotation(
        &mut self,
        type_params: &HashMap<Identifier, TypeVarId>,
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        return_type: &Option<TypeExpr>,
        body_ty: &InferType,
    ) -> InferType {
        match return_type {
            Some(ret_ann) => {
                match self.infer_type_from_annotation(ret_ann, type_params, row_var_env) {
                    Some(ann_ty) => self.unify_propagate(body_ty, &ann_ty),
                    None => body_ty.apply_type_subst(&self.subst),
                }
            }
            None => body_ty.apply_type_subst(&self.subst),
        }
    }

    /// Infer one type annotation in the current type/row parameter context.
    ///
    /// Behavior:
    /// - Lowers a syntax level annotation into an HM type using the provided
    ///   type parameter map and row variable environment.
    ///
    /// Side effects:
    /// - May allocate fresh row/type variables by mutating `row_var_env` and
    ///   the type environment counter.
    ///
    /// Diagnostics:
    /// - Emits no diagnostics directly; callers choose fallback/error behavior.
    ///
    /// Returns:
    /// - `Some(InferType)` when lowering succeeds otherwise `None`.
    pub(super) fn infer_type_from_annotation(
        &mut self,
        annotation: &TypeExpr,
        type_params: &HashMap<Identifier, TypeVarId>,
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
    ) -> Option<InferType> {
        TypeEnv::infer_type_from_type_expr_with_row_vars(
            annotation,
            type_params,
            self.interner,
            row_var_env,
            &mut self.env.counter,
        )
    }

    /// Finalize and bind the inferred function scheme in the outer scope.
    fn finalize_and_bind_function_scheme(
        &mut self,
        name: Identifier,
        fn_span: Span,
        type_params: &[Identifier],
        param_tys: &[InferType],
        ret_ty: &InferType,
        declared_effect_row: &InferEffectRow,
    ) {
        let final_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|ty| ty.apply_type_subst(&self.subst))
            .collect();
        let effect_row = declared_effect_row.apply_row_subst(&self.subst);
        let fn_ty = InferType::Fun(final_param_tys, Box::new(ret_ty.clone()), effect_row);

        self.env.leave_scope();

        let scheme = if !type_params.is_empty() {
            generalize(&fn_ty, &self.env.free_vars())
        } else {
            Scheme::mono(fn_ty)
        };

        self.env.bind_with_span(name, scheme, Some(fn_span));
    }

    /// Run a second pass for unannotated self recursive functions to refine type.
    ///
    /// This preserves existing T11 behavior by feeding recursive call result
    /// constraints back into the fucntion return slot.
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

    /// Return `true` when any statement in `block` contains a self call to `name`.
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
