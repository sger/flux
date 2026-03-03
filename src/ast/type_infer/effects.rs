use super::*;

impl<'a> InferCtx<'a> {
    /// Infer an [`InferEffectRow`] from parsed effect annotations.
    ///
    /// This helper is the AST/type-inference entrypoint for converting a list
    /// of parsed [`EffectExpr`] nodes (for example from `fn ... with ...`) into
    /// the canonical row representation used by HM inference and row unification.
    ///
    /// Behavior:
    /// - Collects and normalizes all concrete effect names from `effects`.
    /// - Resolves symbolic row variables through `row_var_env` so the same
    ///   source-level row variable always maps to the same [`TypeVarId`] within
    ///   this inference context.
    /// - Allocates fresh row-variable ids using `next_row_var_id` when a symbol
    ///   appears for the first time.
    /// - Produces a closed row when no row-variable is present, otherwise an open
    ///   row whose tail is the last row-variable encountered in `effects`.
    ///
    /// Parameters:
    /// - `effects`: parsed effect expressions for the current annotation site.
    /// - `row_var_env`: per-site/per-scope mapping from symbolic row variables
    ///   to canonical row-variable ids.
    /// - `next_row_var_id`: monotonic allocator for fresh row-variable ids.
    ///
    /// This function intentionally delegates construction details to
    /// [`InferEffectRow::from_effect_exprs`] so row-shape policy stays
    /// centralized.
    pub(super) fn infer_effect_row(
        effects: &[EffectExpr],
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
        next_row_var_id: &mut u32,
    ) -> InferEffectRow {
        InferEffectRow::from_effect_exprs(effects, row_var_env, next_row_var_id)
    }

    /// Emit diagnostic `E426` when a referenced Base function has no HM metadata.
    ///
    /// This is raised when inference needs a Base function scheme but cannot
    /// obtain one from the preloaded Base signature registry. The diagnostic
    /// points at the use site (`span`) and includes a fix hint directing
    /// contributors to the Base HM signature definitions.
    ///
    /// Parameters:
    /// - `base_name`: interned symbol of the missing Base function.
    /// - `span`: source location where the missing metadata was required.
    pub(super) fn emit_missing_base_hm_signature(&mut self, base_name: Identifier, span: Span) {
        self.errors.push(
            Diagnostic::make_error_dynamic(
                "E426",
                "BASE HM SIGNATURE MISSING",
                crate::diagnostics::ErrorType::Compiler,
                format!(
                    "Base function `{}` is missing HM metadata and cannot be typed.",
                    self.interner.resolve(base_name)
                ),
                Some(
                    "Add an HM signature for this Base function in src/runtime/base/base_hm_signature.rs."
                        .to_string(),
                ),
                self.file_path.clone(),
                span,
            )
            .with_primary_label(span, "missing Base HM metadata"),
        );
    }

    /// Compute the currently active ambient effect row for the inference point.
    ///
    /// The base row comes from the top of `ambient_effect_rows` (if any),
    /// otherwise it starts as a closed empty row. Effects currently marked as
    /// handled via [`Self::with_handle_effect`] are then merged into the
    /// concrete set so downstream checks treat them as available in scope.
    ///
    /// Returns a cloned row value; internal stacks are not mutated.
    pub(super) fn current_ambient_effect_row(&self) -> InferEffectRow {
        let mut row = self
            .ambient_effect_rows
            .last()
            .cloned()
            .unwrap_or_else(InferEffectRow::closed_empty);
        row.concrete_mut()
            .extend(self.handled_effects.iter().copied());
        row
    }

    /// Execute `f` with `row` pushed as the ambient effect context.
    ///
    /// This provides scoped ambient-effect tracking: the row is pushed before
    /// executing `f` and always popped afterward, then `f`'s result is returned.
    /// It is used for entering bodies (for example function bodies) that carry a
    /// known ambient effect annotation.
    ///
    /// Parameters:
    /// - `row`: ambient row to make active for the duration of `f`.
    /// - `f`: closure executed under that ambient row.
    pub(super) fn with_ambient_effect_row<F, R>(&mut self, row: InferEffectRow, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.ambient_effect_rows.push(row);
        let result = f(self);
        self.ambient_effect_rows.pop();
        result
    }

    /// Execute `f` while marking one effect as handled in the current scope.
    ///
    /// The effect symbol is pushed to `handled_effects` before running `f` and
    /// removed immediately afterward, giving nested handle scopes stack-like
    /// behavior. [`Self::current_ambient_effect_row`] incorporates these handled
    /// effects into the concrete ambient row used by compatibility checks.
    ///
    /// Parameters:
    /// - `effect`: effect symbol treated as handled during `f`.
    /// - `f`: closure executed with that temporary handled-effect binding.
    pub(super) fn with_handle_effect<F, R>(&mut self, effect: Identifier, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.handled_effects.push(effect);
        let result = f(self);
        self.handled_effects.pop();
        result
    }

    /// Resolve an effect operation signature and lower it to inference types.
    ///
    /// Looks up `(effect, operation)` in `effect_op_signatures`, expects the
    /// stored type expression to be function-shaped, and lowers its parameter
    /// and return type expressions into [`InferType`] values.
    ///
    /// Lowering details:
    /// - Uses `TypeEnv::infer_type_from_type_expr_with_row_vars` for each
    ///   parameter and the return type.
    /// - Tracks row-variable symbols through a local `row_var_env` so repeated
    ///   row vars in one signature map to stable row-variable ids.
    /// - Advances `self.env.counter` to reserve any fresh ids consumed during
    ///   lowering.
    ///
    /// Returns:
    /// - `Some((params, ret))` when a well-formed function signature is found
    ///   and fully lowered.
    /// - `None` when the operation is missing, the stored expression is not a
    ///   function type, or any component cannot be lowered.
    ///
    /// Parameters:
    /// - `effect`: effect symbol containing the operation.
    /// - `operation`: operation symbol to resolve within that effect.
    pub(super) fn effect_op_signature_types(
        &mut self,
        effect: Identifier,
        operation: Identifier,
    ) -> Option<(Vec<InferType>, InferType)> {
        let type_expr = self.effect_op_signatures.get(&(effect, operation))?;
        let TypeExpr::Function {
            params,
            ret,
            effects: _,
            span: _,
        } = type_expr
        else {
            return None;
        };
        let tp_map: HashMap<Identifier, TypeVarId> = HashMap::new();
        let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();
        let mut fresh = self.env.counter;
        let param_tys = params
            .iter()
            .map(|p| {
                TypeEnv::infer_type_from_type_expr_with_row_vars(
                    p,
                    &tp_map,
                    self.interner,
                    &mut row_var_env,
                    &mut fresh,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        let ret_ty = TypeEnv::infer_type_from_type_expr_with_row_vars(
            ret,
            &tp_map,
            self.interner,
            &mut row_var_env,
            &mut fresh,
        )?;
        self.env.counter = fresh;
        Some((param_tys, ret_ty))
    }

    pub(super) fn constrain_call_effects(
        &mut self,
        callee_effects: &InferEffectRow,
        ambient_effects: &InferEffectRow,
        span: Span,
    ) {
        let callee = callee_effects.apply_row_subst(&self.subst);
        let ambient = ambient_effects.apply_row_subst(&self.subst);
        let mut missing: Vec<Identifier> = callee
            .concrete()
            .iter()
            .filter(|effect| !ambient.concrete().contains(effect))
            .copied()
            .collect();
        missing.sort_by_key(|s| s.as_u32());

        if missing.is_empty() {
            if let (Some(callee_tail), Some(ambient_tail)) = (callee.tail(), ambient.tail()) {
                let mut subst = TypeSubst::empty();
                subst.insert_row(
                    callee_tail,
                    InferEffectRow::open_from_symbols(
                        std::iter::empty::<Identifier>(),
                        ambient_tail,
                    ),
                );
                self.subst = std::mem::take(&mut self.subst).compose(&subst);
            }
            return;
        }

        if let Some(ambient_tail) = ambient.tail() {
            if let Some(callee_tail) = callee.tail() {
                let mut subst = TypeSubst::empty();
                subst.insert_row(
                    callee_tail,
                    InferEffectRow::open_from_symbols(missing, ambient_tail),
                );
                self.subst = std::mem::take(&mut self.subst).compose(&subst);
            }
            return;
        }

        let actual_effect_ty = InferType::Fun(
            vec![],
            Box::new(InferType::Con(TypeConstructor::Unit)),
            callee,
        );

        let expected_effect_ty = InferType::Fun(
            vec![],
            Box::new(InferType::Con(TypeConstructor::Unit)),
            ambient,
        );

        let _ = self.unify_with_context(
            &expected_effect_ty,
            &actual_effect_ty,
            span,
            ReportContext::Plain,
        );
    }
}
