use crate::ast::free_vars::collect_free_vars;

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
            if self.is_task_spawn_call(input.function) {
                self.emit_sendable_capture_constraints(input.arguments, 0);
            }
            // Proposal 0174 follow-up: Channel.send with a closure argument
            // requires every captured value to be Sendable. The wire scheme's
            // `<a: Sendable>` only checks the message type itself; capture-set
            // checking is the same shape as Task.spawn but on argument index 1.
            if self.is_channel_send_call(input.function) {
                self.emit_sendable_capture_constraints(input.arguments, 1);
            }
            // Emit class constraint after inference resolves argument types.
            if let Some(info) = class_method_info {
                let resolved_type_args = self
                    .propagate_resolved_class_call_effects(info)
                    .or_else(|| {
                        self.propagate_class_call_from_return_type(info, &result)
                    });
                if let Some(type_args) = resolved_type_args {
                    self.emit_class_constraint_args(
                        info.class_name,
                        type_args,
                        info.span,
                        constraint::WantedClassConstraintOrigin::MethodCall,
                    );
                } else if let Some(type_args) = self.class_constraint_args_from_method_sig(
                    info,
                    &param_tys,
                    &result,
                ) {
                    // Fallback: derive each class type-parameter binding from
                    // the matching slot in the method signature. For Encode
                    // this picks up `a` from the first param. For Decode
                    // (whose first param is `Json`) it picks up `a` from
                    // the return type's `Either<JsonError, a>` pattern. The
                    // emitted type args are still type variables; the
                    // class_solver runs after substitution so it sees the
                    // concrete type.
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
            return result;
        }

        self.infer_call_unresolved_callee(&fn_ty, input, fn_name, fn_def_span, ambient_effect_row)
    }

    fn is_task_spawn_call(&self, function: &Expression) -> bool {
        match function {
            Expression::Identifier { name, .. } => self.interner.resolve(*name) == "spawn",
            Expression::MemberAccess { object, member, .. } => {
                matches!(object.as_ref(), Expression::Identifier { .. })
                    && self.interner.resolve(*member) == "spawn"
            }
            _ => false,
        }
    }

    fn is_channel_send_call(&self, function: &Expression) -> bool {
        match function {
            Expression::MemberAccess { object, member, .. } => {
                if self.interner.resolve(*member) != "send" {
                    return false;
                }
                match object.as_ref() {
                    Expression::Identifier { name, .. } => {
                        let qualifier = self.interner.resolve(*name);
                        qualifier == "Channel" || qualifier == "Flow.Channel"
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn emit_sendable_capture_constraints(
        &mut self,
        arguments: &[Expression],
        index: usize,
    ) {
        let Some(action) = arguments.get(index) else {
            return;
        };
        if !matches!(action, Expression::Function { .. }) {
            return;
        }
        let Some(sendable) = self.interner.lookup("Sendable") else {
            return;
        };
        let mut free_vars = collect_free_vars(action).into_iter().collect::<Vec<_>>();
        free_vars.sort_by_key(|symbol| symbol.as_u32());
        for symbol in free_vars {
            if self.env.lookup_level(symbol).unwrap_or(0) == 0 {
                continue;
            }
            let Some(scheme) = self.env.lookup(symbol).cloned() else {
                continue;
            };
            self.emit_class_constraint(
                sendable,
                scheme.infer_type.apply_type_subst(&self.subst),
                action.span(),
                constraint::WantedClassConstraintOrigin::SchemeUse,
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
        ret_var.apply_type_subst(&self.subst)
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
                    span,
                })
            }
            Expression::MemberAccess { object, member, .. } => {
                let Expression::Identifier {
                    name: module_name, ..
                } = object.as_ref()
                else {
                    return None;
                };
                if !self
                    .module_member_schemes
                    .contains_key(&(*module_name, *member))
                {
                    return None;
                }
                let class_name = self.lookup_class_method(*member)?;
                Some(ResolvedClassMethodCall {
                    class_name,
                    method_name: *member,
                    first_arg_id,
                    span,
                })
            }
            _ => None,
        }
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

        let (resolved_type_args, scheme) = {
            let class_env = self.class_env.as_ref()?;
            let (instance, concrete_type_args) = class_env
                .resolve_method_call_instance_from_first_arg(
                    info.class_name,
                    &first_arg_ty,
                    self.interner,
                )?;

            let type_key = instance
                .type_args
                .iter()
                .map(|arg| arg.display_with(self.interner))
                .collect::<Vec<_>>()
                .join("_");
            let class_str = self.interner.resolve(info.class_name);
            let method_str = self.interner.resolve(info.method_name);
            let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
            // Prefer the qualified `<Module>.__tc_*` form when the
            // instance lives inside a module — that's where the bytecode
            // global is bound. Fall back to bare for built-in / top-level
            // instances.
            let mangled_sym = instance
                .instance_module
                .as_identifier()
                .and_then(|module_sym| self.interner.try_resolve(module_sym))
                .and_then(|module_str| {
                    self.interner.lookup(&format!("{module_str}.{mangled}"))
                })
                .or_else(|| self.interner.lookup(&mangled))?;
            let scheme = self.env.lookup(mangled_sym).cloned()?;
            Some((concrete_type_args, scheme))
        }?;

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

    /// Walk the class method's declared signature and recover each class
    /// type parameter's binding by matching the signature pattern against
    /// the instantiated argument / return types at the call site. This is
    /// the constraint-emission fallback for cases where the first argument
    /// alone doesn't determine the instance — most importantly `Decode<a>`
    /// where the first parameter is `Json` and the return type carries `a`.
    ///
    /// Returns `Some(args)` only when *every* class type parameter shows up
    /// in some position of the method signature. Otherwise the caller falls
    /// back to the legacy single-arg constraint shape.
    fn class_constraint_args_from_method_sig(
        &self,
        info: ResolvedClassMethodCall,
        param_tys: &[InferType],
        result: &InferType,
    ) -> Option<Vec<InferType>> {
        let class_env = self.class_env.as_ref()?;
        let class_def = class_env.lookup_class(info.class_name)?;
        let method = class_def
            .methods
            .iter()
            .find(|m| m.name == info.method_name)?;
        if method.param_types.len() != param_tys.len() {
            return None;
        }

        let mut subst: std::collections::HashMap<Identifier, InferType> = Default::default();
        for (pat, actual) in method.param_types.iter().zip(param_tys.iter()) {
            let actual_resolved = actual.apply_type_subst(&self.subst);
            collect_type_var_bindings(pat, &actual_resolved, self.interner, &mut subst);
        }
        let actual_ret = result.apply_type_subst(&self.subst);
        collect_type_var_bindings(&method.return_type, &actual_ret, self.interner, &mut subst);

        class_def
            .type_params
            .iter()
            .map(|tp| subst.get(tp).cloned())
            .collect::<Option<Vec<_>>>()
    }

    /// Return-type-driven dispatch fallback. When first-argument-driven
    /// resolution fails (because the class method's first parameter has a
    /// fixed type, e.g. `Decode.decode(j: Json) -> Either<JsonError, a>`),
    /// match the class method's declared return type pattern against the
    /// call's resolved HM return type to recover the class type arguments,
    /// then look up the matching instance and instantiate its scheme so
    /// effect rows are constrained correctly.
    fn propagate_class_call_from_return_type(
        &mut self,
        info: ResolvedClassMethodCall,
        call_result_ty: &InferType,
    ) -> Option<Vec<InferType>> {
        let actual_return = call_result_ty.apply_type_subst(&self.subst);
        let (resolved_type_args, scheme) = {
            let class_env = self.class_env.as_ref()?;
            let (instance, concrete_type_args) = class_env
                .resolve_method_call_instance_from_return_type(
                    info.class_name,
                    info.method_name,
                    &actual_return,
                    self.interner,
                )?;

            let type_key = instance
                .type_args
                .iter()
                .map(|arg| arg.display_with(self.interner))
                .collect::<Vec<_>>()
                .join("_");
            let class_str = self.interner.resolve(info.class_name);
            let method_str = self.interner.resolve(info.method_name);
            let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
            let mangled_sym = instance
                .instance_module
                .as_identifier()
                .and_then(|module_sym| self.interner.try_resolve(module_sym))
                .and_then(|module_str| self.interner.lookup(&format!("{module_str}.{mangled}")))
                .or_else(|| self.interner.lookup(&mangled))?;
            let scheme = self.env.lookup(mangled_sym).cloned()?;
            Some((concrete_type_args, scheme))
        }?;

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

/// Walk a `TypeExpr` pattern and an `InferType` actual in lock-step, recording
/// each lowercase-named type variable's binding into `subst`. Used by the
/// return-type-driven dispatch fallback to recover class type-parameter
/// bindings from method signatures whose first argument doesn't depend on
/// the class type variable.
fn collect_type_var_bindings(
    pattern: &TypeExpr,
    actual: &InferType,
    interner: &Interner,
    subst: &mut std::collections::HashMap<Identifier, InferType>,
) {
    match pattern {
        TypeExpr::Named { name, args, .. } => {
            if args.is_empty() && is_type_var_name(*name, interner) {
                subst.entry(*name).or_insert_with(|| actual.clone());
                return;
            }
            match actual {
                InferType::App(_tc, actual_args) if args.len() == actual_args.len() => {
                    for (p, a) in args.iter().zip(actual_args.iter()) {
                        collect_type_var_bindings(p, a, interner, subst);
                    }
                }
                InferType::HktApp(_, actual_args) if args.len() == actual_args.len() => {
                    for (p, a) in args.iter().zip(actual_args.iter()) {
                        collect_type_var_bindings(p, a, interner, subst);
                    }
                }
                _ => {}
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            if let InferType::Tuple(actuals) = actual
                && elements.len() == actuals.len()
            {
                for (p, a) in elements.iter().zip(actuals.iter()) {
                    collect_type_var_bindings(p, a, interner, subst);
                }
            }
        }
        TypeExpr::Function { params, ret, .. } => {
            if let InferType::Fun(actual_params, actual_ret, _) = actual {
                if params.len() == actual_params.len() {
                    for (p, a) in params.iter().zip(actual_params.iter()) {
                        collect_type_var_bindings(p, a, interner, subst);
                    }
                }
                collect_type_var_bindings(ret, actual_ret, interner, subst);
            }
        }
    }
}

fn is_type_var_name(name: Identifier, interner: &Interner) -> bool {
    interner
        .resolve(name)
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase())
}
