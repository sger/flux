use super::*;

/// Grouped inputs for [`InferCtx::infer_call_typed_callee`].
struct CallTypedCalleeSpec<'a> {
    fn_ty: &'a InferType,
    param_tys: &'a [InferType],
    ret_ty: &'a InferType,
    fn_effects: &'a InferEffectRow,
    input: CallInferInput<'a>,
    fn_name: Option<String>,
    fn_def_span: Option<Span>,
    ambient_effect_row: InferEffectRow,
}

impl<'a> InferCtx<'a> {
    /// Infer a call expression, routing constructor calls to ADT-specific inference.
    ///
    /// Behavior:
    /// - Detects constructor names first and uses constructor arity/type paths.
    /// - Falls back to regular function call inference otherwise.
    ///
    /// Side effects:
    /// - May mutate substitutions and diagnostics through delegated paths.
    pub(super) fn infer_call_expression(&mut self, input: CallInferInput<'_>) -> InferType {
        if let Expression::Identifier { name, .. } = input.function
            && self.adt_constructor_types.contains_key(name)
        {
            self.infer_constructor_call(*name, input.arguments, input.span)
        } else {
            self.infer_function_call(input)
        }
    }

    /// Infer a non-constructor function call under HM typing rules.
    ///
    /// Behavior:
    /// - Infers callee and arguments.
    /// - Applies ambient effect constraints.
    /// - Uses typed-callee path when callee resolves to `Fun`.
    /// - Uses dynamic fallback otherwise.
    ///
    /// Diagnostics:
    /// - Emits per-argument mismatch diagnostics only on fixed-arity typed paths.
    ///
    /// Returns:
    /// - Inferred call return type or `Any` via fallback paths.
    pub(super) fn infer_function_call(&mut self, input: CallInferInput<'_>) -> InferType {
        let fn_ty = self.infer_expression(input.function);
        let fn_ty_resolved = fn_ty.apply_type_subst(&self.subst);
        let ambient_effect_row = self
            .current_ambient_effect_row()
            .apply_row_subst(&self.subst);

        let (fn_name, fn_def_span) = match input.function {
            Expression::Identifier { name, .. } => {
                let fn_name = self.interner.resolve(*name).to_string();
                (Some(fn_name), self.env.lookup_span(*name))
            }
            _ => (None, None),
        };

        // Check if callee is a class method (for post-inference constraint emission).
        let class_method_info = if let Expression::Identifier { name, .. } = input.function {
            if self.env.lookup_span(*name).is_none() {
                self.lookup_class_method(*name).map(|c| (c, input.span))
            } else {
                None
            }
        } else {
            None
        };

        if let InferType::Fun(param_tys, ret_ty, fn_effects) = fn_ty_resolved {
            let result = self.infer_call_typed_callee(CallTypedCalleeSpec {
                fn_ty: &fn_ty,
                param_tys: &param_tys,
                ret_ty: &ret_ty,
                fn_effects: &fn_effects,
                input,
                fn_name,
                fn_def_span,
                ambient_effect_row,
            });
            // Emit class constraint after inference resolves argument types.
            if let Some((class_name, span)) = class_method_info {
                let constrained_ty = param_tys
                    .first()
                    .map(|t| t.apply_type_subst(&self.subst))
                    .unwrap_or(result.apply_type_subst(&self.subst));
                self.emit_class_constraint(
                    class_name,
                    constrained_ty,
                    span,
                    constraint::WantedClassConstraintOrigin::MethodCall,
                );
            }
            return result;
        }

        self.infer_call_unresolved_callee(&fn_ty, input, fn_name, fn_def_span, ambient_effect_row)
    }

    /// Infer calls where callee type resolves to `Fun`.
    fn infer_call_typed_callee(&mut self, spec: CallTypedCalleeSpec<'_>) -> InferType {
        self.constrain_call_effects(spec.fn_effects, &spec.ambient_effect_row, spec.input.span);

        let has_higher_order_params = spec
            .param_tys
            .iter()
            .map(|t| t.apply_type_subst(&self.subst))
            .any(|t| matches!(t, InferType::Fun(..)));

        if has_higher_order_params {
            return self.infer_call_higher_order_path(
                spec.fn_ty,
                spec.input.arguments,
                spec.fn_effects,
                spec.input.span,
            );
        }

        if spec.param_tys.len() != spec.input.arguments.len() {
            return spec.ret_ty.apply_type_subst(&self.subst);
        }

        self.infer_call_fixed_arity_path(
            spec.param_tys,
            spec.input.arguments,
            spec.fn_name.as_deref(),
            spec.fn_def_span,
        );
        spec.ret_ty.apply_type_subst(&self.subst)
    }

    /// Infer higher-order calls by unifying callee type with an expected function shape.
    ///
    /// Uses inferred argument types as parameters, preserves callee effects, and
    /// returns the resolved fresh return variable.
    fn infer_call_higher_order_path(
        &mut self,
        fn_ty: &InferType,
        arguments: &[Expression],
        fn_effects: &InferEffectRow,
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = arguments
            .iter()
            .map(|arg| self.infer_expression(arg))
            .collect();
        let ret_var = self.env.alloc_infer_type_var();
        let expected_fn_ty = InferType::Fun(
            arg_tys,
            Box::new(ret_var.clone()),
            fn_effects.apply_row_subst(&self.subst),
        );
        self.unify_reporting(fn_ty, &expected_fn_ty, span);
        ret_var.apply_type_subst(&self.subst)
    }

    /// Infer fixed-arity call arguments and emit per-argument mismatch diagnostics.
    fn infer_call_fixed_arity_path(
        &mut self,
        param_tys: &[InferType],
        arguments: &[Expression],
        fn_name: Option<&str>,
        fn_def_span: Option<Span>,
    ) {
        for (index, (arg_expr, expected_param_ty)) in
            arguments.iter().zip(param_tys.iter()).enumerate()
        {
            let arg_ty = self.infer_expression(arg_expr);

            // Lazy substitution: pass &self.subst for on-demand variable
            // resolution instead of pre-resolving both types.
            match unify_core(
                expected_param_ty,
                &arg_ty,
                &self.subst,
                arg_expr.span(),
                &mut self.env.counter,
            ) {
                Ok(subst) => {
                    self.subst = std::mem::take(&mut self.subst).compose(&subst);
                }
                Err(_) => {
                    // Resolve only in the error path for the diagnostic check.
                    let expected_resolved = expected_param_ty.apply_type_subst(&self.subst);
                    let actual_resolved = arg_ty.apply_type_subst(&self.subst);
                    if expected_resolved.is_concrete()
                        && actual_resolved.is_concrete()
                        && !expected_resolved.contains_any()
                        && !actual_resolved.contains_any()
                    {
                        let exp_str = self.display_type(&expected_resolved);
                        let act_str = self.display_type(&actual_resolved);
                        self.errors.push(call_arg_type_mismatch(
                            self.file_path.clone(),
                            arg_expr.span(),
                            fn_name,
                            index + 1,
                            fn_def_span,
                            &exp_str,
                            &act_str,
                        ));
                    }
                }
            }
        }
    }

    /// Fallback inference when callee type is unresolved.
    fn infer_call_unresolved_callee(
        &mut self,
        fn_ty: &InferType,
        input: CallInferInput<'_>,
        fn_name: Option<String>,
        fn_def_span: Option<Span>,
        ambient_effect_row: InferEffectRow,
    ) -> InferType {
        let arg_tys: Vec<InferType> = input
            .arguments
            .iter()
            .map(|arg| self.infer_expression(arg))
            .collect();

        let ret_var = self.env.alloc_infer_type_var();
        let expected_fn_ty = InferType::Fun(arg_tys, Box::new(ret_var.clone()), ambient_effect_row);
        self.unify_with_context(
            fn_ty,
            &expected_fn_ty,
            input.span,
            ReportContext::CallArg {
                fn_name,
                fn_def_span,
            },
        );
        ret_var.apply_type_subst(&self.subst)
    }
}
