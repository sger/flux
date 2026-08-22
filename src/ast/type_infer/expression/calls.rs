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

#[derive(Debug, Clone, Copy)]
struct ResolvedClassMethodCall {
    class_name: Identifier,
    method_name: Identifier,
    first_arg_id: ExprId,
    /// `ExprId` of the **function-position** sub-expression of the call
    /// (`show` in `show(42)`, or `member` in `Foldable.fold(xs, ...)`'s
    /// `MemberAccess`). The LSP keys
    /// `InferProgramResult::class_method_dispatch` by this id because
    /// the locator's `NodeRef::Expr(Expression::Identifier { expr_id })`
    /// for a cursor on the method name carries exactly this value.
    function_expr_id: ExprId,
    span: Span,
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
    /// - Inferred call return type or a fresh inference variable via fallback paths.
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
        let class_method_info =
            self.class_method_call_info(input.function, input.arguments, input.span);

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
            if let Some(info) = class_method_info {
                self.emit_typed_class_method_constraint(info, &param_tys, &result);
            }
            return result;
        }

        let result = self.infer_call_unresolved_callee(
            &fn_ty,
            input,
            fn_name,
            fn_def_span,
            ambient_effect_row,
        );
        // The unresolved-callee path is what fires for buffer-declared
        // class methods (their short name isn't pre-bound in HM env, so
        // `fn_ty_resolved` is a fresh var rather than `Fun`). Re-run the
        // dispatch resolution here so the LSP still gets a
        // `ClassDispatch` entry — without emitting class constraints,
        // since those are the typed-path's responsibility.
        if let Some(info) = class_method_info {
            let _ = self.propagate_resolved_class_call_effects(info);
        }
        result
    }

    /// Emit the class constraint for a resolved class-method call on the
    /// typed-callee path, once argument types are known.
    ///
    /// Uses the concrete head type arguments when the first argument selects a
    /// unique instance; otherwise falls back to constraining the first
    /// parameter type (or the call result when there are no parameters).
    fn emit_typed_class_method_constraint(
        &mut self,
        info: ResolvedClassMethodCall,
        param_tys: &[InferType],
        result: &InferType,
    ) {
        if let Some(type_args) = self.propagate_resolved_class_call_effects(info) {
            self.emit_class_constraint_args(
                info.class_name,
                type_args,
                info.span,
                constraint::WantedClassConstraintOrigin::MethodCall,
            );
        } else {
            let constrained_ty = param_tys
                .first()
                .map(|t| t.apply_type_subst(&self.subst))
                .unwrap_or(result.apply_type_subst(&self.subst));
            self.emit_class_constraint(
                info.class_name,
                constrained_ty,
                info.span,
                constraint::WantedClassConstraintOrigin::MethodCall,
            );
        }
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
                spec.param_tys,
                spec.input.arguments,
                spec.input.function,
                spec.fn_effects,
                spec.input.span,
            );
        }

        if spec.param_tys.len() != spec.input.arguments.len() {
            if spec.fn_name.as_deref() == Some("resume") {
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E428",
                        "PARAMETERIZED HANDLER SHAPE ERROR",
                        crate::diagnostics::ErrorType::Compiler,
                        format!(
                            "Handler resume expects {} argument(s), got {}.",
                            spec.param_tys.len(),
                            spec.input.arguments.len()
                        ),
                        Some(
                            "Use the resume arity required by the enclosing handler arm."
                                .to_string(),
                        ),
                        self.file_path.clone(),
                        spec.input.span,
                    )
                    .with_primary_label(spec.input.span, "wrong resume arity in handler arm"),
                );
            }
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
        param_tys: &[InferType],
        arguments: &[Expression],
        function: &Expression,
        fn_effects: &InferEffectRow,
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = arguments
            .iter()
            .enumerate()
            .map(|(i, arg)| self.infer_higher_order_call_arg(param_tys.get(i), arg))
            .collect();
        let ret_var = self.env.alloc_infer_type_var();
        let expected_fn_ty = InferType::Fun(
            arg_tys,
            Box::new(ret_var.clone()),
            fn_effects.apply_row_subst(&self.subst),
        );
        self.unify_reporting(fn_ty, &expected_fn_ty, span);
        self.emit_task_spawn_capture_constraints(function, arguments, param_tys, span);
        ret_var.apply_type_subst(&self.subst)
    }

    /// `Task.spawn` moves the action closure to a worker, so each captured
    /// runtime value must be independently Sendable even though function
    /// values remain non-Sendable in the general type-class lattice.
    fn emit_task_spawn_capture_constraints(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        param_tys: &[InferType],
        span: Span,
    ) {
        if !self.is_task_spawn_callee(function, param_tys) {
            return;
        }
        let Some(Expression::Function {
            parameters, body, ..
        }) = arguments.first()
        else {
            return;
        };
        let Some(sendable) = self.interner.lookup("Sendable") else {
            return;
        };

        let mut captures = collect_free_vars_in_function_body(parameters, body)
            .into_iter()
            .filter(|name| self.env.lookup_level(*name).is_some_and(|level| level > 0))
            .collect::<Vec<_>>();
        captures.sort_unstable_by_key(|name| name.as_u32());
        captures.dedup();

        for capture in captures {
            let Some(scheme) = self.env.lookup(capture) else {
                continue;
            };
            let capture_ty = scheme.infer_type.apply_type_subst(&self.subst);
            self.emit_class_constraint(
                sendable,
                capture_ty,
                span,
                constraint::WantedClassConstraintOrigin::TaskSpawnCapture {
                    capture_name: capture,
                },
            );
        }
    }

    /// Return whether a call resolves to the `Flow.Task.spawn` surface.
    ///
    /// This uses import-origin metadata instead of matching every function named
    /// `spawn`, so unrelated module members and local shadows do not receive
    /// Task-specific closure-capture constraints.
    fn is_task_spawn_callee(&self, function: &Expression, param_tys: &[InferType]) -> bool {
        let has_spawn_shape = || {
            param_tys
                .first()
                .map(|ty| ty.apply_type_subst(&self.subst))
                .is_some_and(|ty| matches!(ty, InferType::Fun(params, _, _) if params.is_empty()))
        };

        match function {
            Expression::Identifier { name, .. } => {
                self.interner.resolve(*name) == "spawn"
                    && self.env.lookup_span(*name).is_none()
                    && self.task_spawn_exposed
                    && has_spawn_shape()
            }
            Expression::MemberAccess { object, member, .. } => {
                self.interner.resolve(*member) == "spawn"
                    && matches!(object.as_ref(), Expression::Identifier { name, .. }
                        if self.task_module_bindings.contains(name)
                            && self.module_member_schemes.contains_key(&(*name, *member)))
                    && has_spawn_shape()
            }
            _ => false,
        }
    }

    /// Infer one higher-order call argument with bidirectional propagation
    /// for lambdas against concrete expected types (Proposal 0159, Phase 3).
    /// Non-lambda args silently unify against the expected parameter type so
    /// later propagatable args see resolved callee type variables, without
    /// shadowing downstream effect-row diagnostics.
    fn infer_higher_order_call_arg(
        &mut self,
        expected: Option<&InferType>,
        arg: &Expression,
    ) -> InferType {
        let Some(expected) = expected else {
            return self.infer_expression(arg);
        };
        let expected_resolved = expected.apply_type_subst(&self.subst);
        if is_propagatable_call_arg(arg) && lambda_param_types_concrete(&expected_resolved) {
            self.check_expression(arg, &expected_resolved);
            return expected_resolved.apply_type_subst(&self.subst);
        }
        let arg_ty = self.infer_expression(arg);
        if !is_propagatable_call_arg(arg) {
            self.unify_silent(&expected_resolved, &arg_ty);
        }
        arg_ty.apply_type_subst(&self.subst)
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
            // Propagate the expected parameter type into propagatable
            // arguments (Proposal 0159, Phase 3 follow-up) so per-sub-
            // expression mismatches report at the offending span. The
            // subsequent unify_core + call_arg_type_mismatch emission
            // remains as the canonical argument-level diagnostic.
            if is_propagatable_call_arg(arg_expr) {
                let expected_resolved = expected_param_ty.apply_type_subst(&self.subst);
                if expected_resolved.is_concrete() {
                    self.check_expression(arg_expr, &expected_resolved);
                }
            }
            let arg_ty = self.infer_expression(arg_expr);

            // Lazy substitution: pass &self.subst for on-demand variable
            // resolution instead of pre-resolving both types.
            match unify_core(
                expected_param_ty,
                &arg_ty,
                &self.subst,
                arg_expr.span(),
                &mut self.env.counter,
                &self.skolem_vars,
            ) {
                Ok(subst) => {
                    self.subst = std::mem::take(&mut self.subst).compose(&subst);
                }
                Err(_) => {
                    // Resolve only in the error path for the diagnostic check.
                    let expected_resolved = expected_param_ty.apply_type_subst(&self.subst);
                    let actual_resolved = arg_ty.apply_type_subst(&self.subst);
                    if expected_resolved.is_concrete() && actual_resolved.is_concrete() {
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

    /// Recognize a direct class-method call candidate.
    ///
    /// Returns the class/method identity plus the first argument expression id,
    /// which is later used to resolve the concrete instance selected at the
    /// call site. Supports both bare calls (`eq(x, y)`) and imported
    /// module-qualified calls (`Foldable.fold(xs, init, step)`).
    fn class_method_call_info(
        &self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> Option<ResolvedClassMethodCall> {
        let first_arg_id = arguments.first()?.expr_id();
        match function {
            Expression::Identifier { name, .. } => {
                if self
                    .env
                    .lookup_span(*name)
                    .is_some_and(|def_span| def_span != Span::default())
                {
                    return None;
                }
                let class_name = self.lookup_class_method(*name)?;
                Some(ResolvedClassMethodCall {
                    class_name,
                    method_name: *name,
                    first_arg_id,
                    function_expr_id: function.expr_id(),
                    span,
                })
            }
            Expression::MemberAccess { object, member, .. } => {
                self.qualified_class_method_call(object, *member, function, first_arg_id, span)
            }
            _ => None,
        }
    }

    /// Recognize a *qualified* class-method call, `Module.method(..)`.
    ///
    /// A qualified call dispatches as a class method only when the qualifier
    /// names that class. `Foldable.fold` and `Comparable.same` do;
    /// `Stream.append` does not — there the qualifier is a module that happens
    /// to export a function sharing a name with the built-in `Semigroup`
    /// method, and the module's own function must win.
    ///
    /// Matching on the class's declaring path does not work as a rule: an
    /// instance may live in a different module from its class, and the
    /// qualifier at the call site is an import alias rather than a path.
    fn qualified_class_method_call(
        &self,
        object: &Expression,
        member: Identifier,
        function: &Expression,
        first_arg_id: ExprId,
        span: Span,
    ) -> Option<ResolvedClassMethodCall> {
        let Expression::Identifier {
            name: module_name, ..
        } = object
        else {
            return None;
        };
        if !self
            .module_member_schemes
            .contains_key(&(*module_name, member))
        {
            return None;
        }
        let class_name = self.lookup_class_method(member)?;
        if member != class_name && *module_name != class_name {
            return None;
        }
        Some(ResolvedClassMethodCall {
            class_name,
            method_name: member,
            first_arg_id,
            function_expr_id: function.expr_id(),
            span,
        })
    }

    /// Resolve a class-method call's first-argument type to a concrete instance.
    ///
    /// Records the F12 dispatch entry (so the LSP can route `show(42)` to the
    /// matching `instance Show<Int>` arm) and returns the concrete head type
    /// arguments plus the instance's generated mangled `__tc_*` scheme. `None`
    /// when no unique instance is selected or its scheme is absent.
    fn resolve_class_method_instance(
        &mut self,
        info: ResolvedClassMethodCall,
        first_arg_ty: &InferType,
    ) -> Option<(Vec<InferType>, Scheme)> {
        let class_env = self.class_env.as_ref()?;
        let (instance, concrete_type_args) = class_env
            .resolve_method_call_instance_from_first_arg(
                info.class_name,
                first_arg_ty,
                self.interner,
            )?;

        // Record the dispatch so the LSP can route F12 from `show(42)` to the
        // matching `instance Show<Int>` arm. Reads the head identifier directly
        // off the instance's syntactic `type_args[0]` — same shape
        // `ClassEnv::head_type_name` uses (src/types/class_env.rs:684).
        if let Some(TypeExpr::Named { name: head, .. }) = instance.type_args.first() {
            self.class_method_dispatch.insert(
                info.function_expr_id,
                ClassDispatch {
                    class_name: info.class_name,
                    head_type_ctor: *head,
                    method_name: info.method_name,
                },
            );
        }

        let type_key = instance
            .type_args
            .iter()
            .map(|arg| arg.display_with(self.interner))
            .collect::<Vec<_>>()
            .join("_");
        let class_str = self.interner.resolve(info.class_name);
        let method_str = self.interner.resolve(info.method_name);
        let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
        let mangled_sym = self.interner.lookup(&mangled)?;
        let scheme = self.env.lookup(mangled_sym).cloned()?;
        Some((concrete_type_args, scheme))
    }

    /// Resolve a direct class-method call to its concrete instance effects.
    ///
    /// When the first argument type selects a unique instance, this looks up
    /// the generated mangled `__tc_*` function scheme, constrains the caller's
    /// ambient effect row against that function's effect row, and returns the
    /// full concrete class head for method-call constraint emission.
    fn propagate_resolved_class_call_effects(
        &mut self,
        info: ResolvedClassMethodCall,
    ) -> Option<Vec<InferType>> {
        let first_arg_ty = self
            .expr_types
            .get(&info.first_arg_id)
            .map(|ty| ty.apply_type_subst(&self.subst))?;

        let (resolved_type_args, scheme) =
            self.resolve_class_method_instance(info, &first_arg_ty)?;

        let (resolved_fn_ty, mapping, constraints) = scheme.instantiate(&mut self.env.counter);
        let fresh_vars = mapping.values().copied().collect::<Vec<_>>();
        for &fresh in &fresh_vars {
            self.env.record_var_level(fresh);
        }
        self.record_instantiated_expr_vars(fresh_vars);
        self.emit_scheme_constraints(&constraints, info.span);

        if let InferType::Fun(_, _, effect_row) = resolved_fn_ty.apply_type_subst(&self.subst) {
            let ambient_effect_row = self
                .current_ambient_effect_row()
                .apply_row_subst(&self.subst);
            self.constrain_call_effects(&effect_row, &ambient_effect_row, info.span);
        }
        Some(resolved_type_args)
    }
}

/// Return true when a call argument benefits from expected-type propagation
/// (Proposal 0159, Phase 3 + follow-up). Covers lambda expressions plus
/// control-flow and collection / wrapper literals that `check_expression`
/// has specialised rules for. Non-propagatable shapes fall back to plain
/// inference so the existing call_arg_type_mismatch diagnostic keeps its
/// canonical form.
fn is_propagatable_call_arg(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Function { .. }
            | Expression::If { .. }
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
    )
}

/// Return true when the expected type for a lambda argument has its parameter
/// list fully resolved — the return type may remain flexible since checking a
/// lambda body against a flexible expected return is a no-op.
fn lambda_param_types_concrete(expected: &InferType) -> bool {
    match expected {
        InferType::Fun(params, _, _) => params.iter().all(InferType::is_concrete),
        _ => expected.is_concrete(),
    }
}
