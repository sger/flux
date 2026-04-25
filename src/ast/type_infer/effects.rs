use crate::ast::type_infer::constraint::Constraint;

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
    /// - Allocates fresh row-variable ids using `row_var_counter` when a symbol
    ///   appears for the first time.
    /// - Produces a closed row when no row-variable is present, otherwise an open
    ///   row whose tail is the last row-variable encountered in `effects`.
    ///
    /// Parameters:
    /// - `effects`: parsed effect expressions for the current annotation site.
    /// - `row_var_env`: per-site/per-scope mapping from symbolic row variables
    ///   to canonical row-variable ids.
    /// - `row_var_counter`: monotonic allocator for fresh row-variable ids.
    ///
    /// This function intentionally delegates construction details to
    /// [`InferEffectRow::from_effect_exprs`] so row-shape policy stays
    /// centralized.
    ///
    /// # Examples
    ///
    /// ```text
    /// // In a function annotation like: fn f() with io, e
    /// // `effects` contains parsed nodes for `io` and row variable `e`.
    /// let mut row_var_env = HashMap::new();
    /// let mut row_var_counter = 100;
    /// let row = InferCtx::infer_effect_row(&effects, &mut row_var_env, &mut row_var_counter);
    ///
    /// assert!(row.concrete().contains(&io_symbol));
    /// assert!(row.tail().is_some()); // open row due to `e`
    /// ```
    pub(super) fn infer_effect_row(
        &mut self,
        effects: &[EffectExpr],
        row_var_env: &mut HashMap<Identifier, TypeVarId>,
    ) -> InferEffectRow {
        let mut row_var_counter = self.env.counter;
        let expanded_effects: Vec<EffectExpr> = effects
            .iter()
            .map(|effect| effect.expand_aliases(&self.effect_row_aliases))
            .collect();
        let result =
            InferEffectRow::from_effect_exprs(&expanded_effects, row_var_env, &mut row_var_counter);
        self.env.counter = row_var_counter;
        match result {
            Ok(row) => row,
            Err(err) => {
                let span = effects.first().map(|e| e.span()).unwrap_or_default();
                let first = self.interner.resolve(err.first).to_string();
                let second = self.interner.resolve(err.second).to_string();
                let diag = Diagnostic::make_error(
                    &crate::diagnostics::compiler_errors::INVALID_EFFECT_ROW,
                    &[&first, &second],
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "conflicting row variables in this effect row");
                self.errors.push(diag);
                InferEffectRow::closed_empty()
            }
        }
    }

    /// Emit diagnostic `E426` when a referenced Flow function has no HM metadata.
    ///
    /// This is raised when inference needs a Flow function scheme but cannot
    /// obtain one from the preloaded Flow signature registry. The diagnostic
    /// points at the use site (`span`) and includes a fix hint directing
    /// contributors to the Flow HM signature definitions.
    ///
    /// Parameters:
    /// - `flow_name`: interned symbol of the missing Flow function.
    /// - `span`: source location where the missing metadata was required.
    pub(super) fn emit_missing_flow_hm_signature(&mut self, flow_name: Identifier, span: Span) {
        self.errors.push(
            Diagnostic::make_error_dynamic(
                "E426",
                "FLOW HM SIGNATURE MISSING",
                crate::diagnostics::ErrorType::Compiler,
                format!(
                    "Flow function `{}` is missing HM metadata and cannot be typed.",
                    self.interner.resolve(flow_name)
                ),
                Some("Add an HM signature for this Flow function in the Flow library.".to_string()),
                self.file_path.clone(),
                span,
            )
            .with_primary_label(span, "missing Flow HM metadata"),
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
    ///
    /// # Examples
    ///
    /// ```text
    /// // If the ambient stack has {io | e} and `time` is currently handled,
    /// // the resulting row contains both `io` and `time` plus tail `e`.
    /// let row = ctx.current_ambient_effect_row();
    /// assert!(row.concrete().contains(&io_symbol));
    /// assert!(row.concrete().contains(&time_symbol));
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```text
    /// let row = InferEffectRow::closed_from_symbols([io_symbol]);
    /// ctx.with_ambient_effect_row(row, |ctx| {
    ///     let active = ctx.current_ambient_effect_row();
    ///     assert!(active.concrete().contains(&io_symbol));
    /// });
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```text
    /// ctx.with_handle_effect(io_symbol, |ctx| {
    ///     // `io` is considered available while inside this closure.
    ///     let active = ctx.current_ambient_effect_row();
    ///     assert!(active.concrete().contains(&io_symbol));
    /// });
    /// ```
    pub(super) fn with_handle_effect<F, R>(&mut self, effect: Identifier, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.handled_effects.push(effect);
        let result = f(self);
        self.handled_effects.pop();
        result
    }

    /// Resolve an effect operation signature and instantiate it to inference types.
    ///
    /// Looks up `(effect, operation)` in `effect_op_signatures`, instantiates
    /// the stored [`Scheme`], and returns the function parameter and return
    /// types from that fresh monotype.
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
    ///
    /// # Examples
    ///
    /// ```text
    /// // For signature: effect IO { readLine : () -> String }
    /// let sig = ctx.effect_op_signature_types(io_symbol, read_line_symbol);
    /// let (params, ret) = sig.expect("operation exists");
    /// assert!(params.is_empty());
    /// assert_eq!(ctx.display_type(&ret), "String");
    /// ```
    pub(super) fn effect_op_signature_types(
        &mut self,
        effect: Identifier,
        operation: Identifier,
        span: Span,
    ) -> Option<(Vec<InferType>, InferType)> {
        let scheme = self.effect_op_signatures.get(&(effect, operation))?.clone();
        let (ty, mapping, constraints) = scheme.instantiate(&mut self.env.counter);
        let fresh_vars = mapping.values().copied().collect::<Vec<_>>();
        for &fresh in &fresh_vars {
            self.env.record_var_level(fresh);
        }
        self.record_instantiated_expr_vars(fresh_vars);
        self.emit_scheme_constraints(&constraints, span);
        let InferType::Fun(params, ret, _effects) = ty else {
            return None;
        };
        Some((params, *ret))
    }

    /// Constrain callee effects against currently ambient effects at a call-site.
    ///
    /// Behaviour:
    /// - Resolves both rows through current substitution.
    /// - Computes concrete effects missing from ambient scope.
    /// - Rewrites row tails when ambient/callee openness allows absorption.
    /// - Emits mismatch diagnostics only when ambient row is closed and missing
    ///   effects remain incompatible.
    ///
    /// Side effects:
    /// - May compose row substitutions into `self.subst`.
    /// - May append diagnostics through `unify_with_context`.
    ///
    /// # Examples
    ///
    /// ```text
    /// // Callee requires {io, time}, ambient allows only {io} (closed).
    /// // This emits a type/effect mismatch diagnostic at the call span.
    /// let callee = InferEffectRow::closed_from_symbols([io_symbol, time_symbol]);
    /// let ambient = InferEffectRow::closed_from_symbols([io_symbol]);
    /// ctx.constrain_call_effects(&callee, &ambient, call_span);
    /// assert!(!ctx.errors.is_empty());
    /// ```
    pub(super) fn constrain_call_effects(
        &mut self,
        callee_effects: &InferEffectRow,
        ambient_effects: &InferEffectRow,
        span: Span,
    ) {
        self.record_constraint(Constraint::EffectSubset {
            required: callee_effects.clone(),
            available: ambient_effects.clone(),
            span,
        });
        let callee = self.expand_effect_row_aliases(callee_effects.apply_row_subst(&self.subst));
        let ambient = self.expand_effect_row_aliases(ambient_effects.apply_row_subst(&self.subst));
        let mut missing: Vec<Identifier> = callee
            .concrete()
            .iter()
            .filter(|effect| !ambient.concrete().contains(effect))
            .copied()
            .collect();
        missing.sort_by_key(|s| s.as_u32());

        if self.link_row_tails_when_compatible(&missing, &callee, &ambient) {
            return;
        }

        if self.absorb_missing_into_open_ambient(&missing, &callee, &ambient) {
            return;
        }

        self.report_effect_mismatch(&callee, &ambient, span);
    }

    fn expand_effect_row_aliases(&self, row: InferEffectRow) -> InferEffectRow {
        let mut concrete = std::collections::HashSet::new();
        for effect in row.concrete() {
            if let Some(expansion) = self.effect_row_aliases.get(effect) {
                concrete.extend(
                    expansion
                        .expand_aliases(&self.effect_row_aliases)
                        .normalized_concrete_names(),
                );
            } else {
                concrete.insert(*effect);
            }
        }
        match row.tail() {
            Some(tail) => InferEffectRow::open_from_symbols(concrete, tail),
            None => InferEffectRow::closed_from_symbols(concrete),
        }
    }

    /// Link row tails when no concrete effects are missing.
    ///
    /// Returns `true` when handled and no further checks are required.
    fn link_row_tails_when_compatible(
        &mut self,
        missing: &[Identifier],
        callee: &InferEffectRow,
        ambient: &InferEffectRow,
    ) -> bool {
        if !missing.is_empty() {
            return false;
        }
        if let (Some(callee_tail), Some(ambient_tail)) = (callee.tail(), ambient.tail()) {
            let mut subst = TypeSubst::empty();
            subst.insert_row(
                callee_tail,
                InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), ambient_tail),
            );
            self.subst = std::mem::take(&mut self.subst).compose(&subst);
        }
        true
    }

    /// Absorb missing effects into callee tail when ambient row is open.
    ///
    /// Returns `true` when handled and no mismatch diagnostic is needed.
    fn absorb_missing_into_open_ambient(
        &mut self,
        missing: &[Identifier],
        callee: &InferEffectRow,
        ambient: &InferEffectRow,
    ) -> bool {
        let Some(ambient_tail) = ambient.tail() else {
            return false;
        };

        if let Some(callee_tail) = callee.tail() {
            let mut subst = TypeSubst::empty();
            subst.insert_row(
                callee_tail,
                InferEffectRow::open_from_symbols(missing.iter().copied(), ambient_tail),
            );
            self.subst = std::mem::take(&mut self.subst).compose(&subst);
        }
        true
    }

    /// Report a dedicated E400 diagnostic naming the first missing effect.
    ///
    /// Computes the concrete effects required by the callee but absent from
    /// the ambient row, picks the first one (sorted for determinism), and emits
    /// a focused "MISSING EFFECT" diagnostic rather than routing through the
    /// generic unification path.
    fn report_effect_mismatch(
        &mut self,
        callee: &InferEffectRow,
        ambient: &InferEffectRow,
        span: Span,
    ) {
        let mut missing: Vec<Identifier> = callee
            .concrete()
            .iter()
            .filter(|e| !ambient.concrete().contains(e))
            .copied()
            .collect();
        missing.sort_by_key(|s| s.as_u32());

        let first_missing = match missing.first() {
            Some(e) => self.interner.resolve(*e).to_string(),
            // No concrete effects missing — only a row-tail mismatch; nothing to report here.
            None => return,
        };

        let hint = if ambient.tail().is_none() {
            format!("annotate the enclosing function with `with {first_missing}`")
        } else {
            format!("the row variable in the ambient context must include `{first_missing}`")
        };

        self.errors.push(
            Diagnostic::make_error_dynamic(
                "E400",
                "MISSING EFFECT",
                crate::diagnostics::ErrorType::Compiler,
                format!(
                    "requires effect `{first_missing}` but the enclosing context does not provide it"
                ),
                Some(hint),
                self.file_path.clone(),
                span,
            )
            .with_primary_label(span, format!("requires `{first_missing}`")),
        );
    }
}
