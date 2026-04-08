use crate::ast::{Visitor, walk_block, walk_expr, walk_stmt};

use super::*;

impl<'a> InferCtx<'a> {
    // ── Function inference ────────────────────────────────────────────────────

    /// Infer a named function declaration and update its predeclared binding.
    ///
    /// Behavior:
    /// - Enters function local scope.
    /// - Infers parameter, effect, body, and return types.
    /// - Applies optional self-recursive return refinement for unannotated functions.
    /// - Finalizes and binds the resulting scheme in outer scope.
    ///
    /// Side effects:
    /// - Mutates environment scopes and substitution state.
    /// - May emit diagnostics indirectly through delegated inference/unification.
    ///
    /// Invariants:
    /// - Preserves existing function inference ordering and refinement conditions.
    pub(super) fn infer_function_declaration(&mut self, input: FnInferInput<'_>) {
        // Map explicit type parameters (e.g. `T`, `U`) to fresh type variables.
        let tp_map = self.allocate_type_parameter_vars(input.type_params);
        let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();
        self.emit_declared_type_param_constraints(input.type_params, &tp_map, input.fn_span);

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
        let mut ret_ty = self.infer_return_type_with_optional_annotation(
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
        type_params: &[crate::syntax::statement::FunctionTypeParam],
    ) -> HashMap<Identifier, TypeVarId> {
        type_params
            .iter()
            .map(|param| (param.name, self.env.alloc_type_var_id()))
            .collect()
    }

    /// Emit class constraints declared inline on function generic parameters.
    ///
    /// This reuses the normal wanted-constraint path so explicit bounds like
    /// `fn f<a: Eq + Show>(...)` flow through solving and scheme generation
    /// the same way as constraints inferred from operators or method calls.
    fn emit_declared_type_param_constraints(
        &mut self,
        type_params: &[crate::syntax::statement::FunctionTypeParam],
        tp_map: &HashMap<Identifier, TypeVarId>,
        span: Span,
    ) {
        for type_param in type_params {
            let Some(type_var) = tp_map.get(&type_param.name).copied() else {
                continue;
            };
            for &constraint in &type_param.constraints {
                self.emit_class_constraint(constraint, InferType::Var(type_var), span);
            }
        }
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
                .unwrap_or_else(|| self.env.alloc_infer_type_var());
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
            InferEffectRow::open_from_symbols(
                std::iter::empty::<Identifier>(),
                self.env.alloc_type_var_id(),
            )
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
    pub(super) fn infer_return_type_with_optional_annotation(
        &mut self,
        type_params: &HashMap<Identifier, TypeVarId>,
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        return_type: &Option<TypeExpr>,
        body_ty: &InferType,
    ) -> InferType {
        match return_type {
            Some(ret_ann) => {
                match self.infer_type_from_annotation(ret_ann, type_params, row_var_env) {
                    Some(ann_ty) => self.unify_silent(body_ty, &ann_ty),
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
        TypeEnv::convert_type_expr_rec(
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
        type_params: &[crate::syntax::statement::FunctionTypeParam],
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
            let constraints = self.collect_scheme_constraints(&fn_ty);
            if constraints.is_empty() {
                generalize(&fn_ty, &self.env.free_vars())
            } else {
                generalize_with_constraints(&fn_ty, &self.env.free_vars(), constraints)
            }
        } else {
            Scheme::mono(fn_ty)
        };

        self.env.bind_with_span(name, scheme, Some(fn_span));
    }

    /// Run a second pass for unannotated self recursive functions to refine type.
    ///
    /// This preserves existing T11 behavior by feeding recursive call result
    /// constraints back into the function return slot.
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
        let ret_slot = self.env.alloc_infer_type_var();
        let self_fn_ty = InferType::Fun(
            refined_param_tys,
            Box::new(ret_slot.clone()),
            effect_row.apply_row_subst(&self.subst),
        );
        self.env.bind(name, Scheme::mono(self_fn_ty));
        let second_body_ty =
            self.with_ambient_effect_row(effect_row.clone(), |ctx| ctx.infer_block(body));
        let refined_ret = self.unify_silent(&second_body_ty, &ret_slot);
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
            self.unify_silent(&current_resolved, &refined_resolved)
                .apply_type_subst(&self.subst)
        }
    }

    /// Return `true` when any statement in `block` contains a self call to `name`.
    pub(super) fn block_contains_self_call(&self, block: &Block, name: Identifier) -> bool {
        let mut search = SelfCallSearch::new(name);
        search.visit_block(block);
        search.found
    }
}

/// Read-only AST search for direct self-calls to a named function.
///
/// Short-circuits on first match. Does not descend into nested function literals.
struct SelfCallSearch {
    target_name: Identifier,
    found: bool,
}

impl SelfCallSearch {
    /// Create a new self call search instance for `target_name`.
    fn new(target_name: Identifier) -> Self {
        Self {
            target_name,
            found: false,
        }
    }
}

impl<'ast> Visitor<'ast> for SelfCallSearch {
    /// Walk child statements, short-circuiting if already found.
    fn visit_block(&mut self, block: &'ast Block) {
        if self.found {
            return;
        }
        walk_block(self, block);
    }

    /// Walk child expressions, short-circuiting if already found.
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if self.found {
            return;
        }
        walk_stmt(self, stmt);
    }

    /// Mark found on direct `target_name(...)` call; skip nested function bodies.
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if self.found {
            return;
        }
        if let Expression::Call { function, .. } = expr
            && let Expression::Identifier { name, .. } = function.as_ref()
            && *name == self.target_name
        {
            self.found = true;
            return;
        }
        if matches!(expr, Expression::Function { .. }) {
            return;
        }
        walk_expr(self, expr);
    }
}
