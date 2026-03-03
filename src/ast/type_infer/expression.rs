use super::*;

impl<'a> InferCtx<'a> {
    // ── Expression inference ──────────────────────────────────────────────────

    pub(super) fn infer_expr(&mut self, expr: &Expression) -> InferType {
        let node_id = self.node_id_for_expr(expr);
        let inferred = match expr {
            // ── Literals ──────────────────────────────────────────────────────
            Expression::Integer { .. } => InferType::Con(TypeConstructor::Int),
            Expression::Float { .. } => InferType::Con(TypeConstructor::Float),
            Expression::Boolean { .. } => InferType::Con(TypeConstructor::Bool),
            Expression::String { .. } | Expression::InterpolatedString { .. } => {
                InferType::Con(TypeConstructor::String)
            }

            // ── Option / Either constructors ──────────────────────────────────
            Expression::None { .. } => {
                InferType::App(TypeConstructor::Option, vec![self.env.fresh_infer_type()])
            }
            Expression::Some { value, .. } => {
                let inner = self.infer_expr(value);
                InferType::App(TypeConstructor::Option, vec![inner])
            }
            Expression::Left { value, .. } => {
                let inner = self.infer_expr(value);
                let r = self.env.fresh_infer_type();
                InferType::App(TypeConstructor::Either, vec![inner, r])
            }
            Expression::Right { value, .. } => {
                let inner = self.infer_expr(value);
                let l = self.env.fresh_infer_type();
                InferType::App(TypeConstructor::Either, vec![l, inner])
            }

            // ── Identifier lookup ─────────────────────────────────────────────
            Expression::Identifier { name, .. } => {
                if let Some(scheme) = self.env.lookup(*name).cloned() {
                    // Instantiate the scheme with fresh type variables so each
                    // use of a generic function is independent.
                    let (ty, _) = scheme.instantiate(&mut self.env.counter);
                    ty
                } else {
                    if self.known_base_names.contains(name) {
                        self.emit_missing_base_hm_signature(*name, expr.span());
                    }
                    // Unknown at this stage (may be a built-in / runtime binding).
                    // Gradual typing: treat as Any without an error.
                    InferType::Con(TypeConstructor::Any)
                }
            }

            // ── Operators ─────────────────────────────────────────────────────
            Expression::Prefix { right, .. } => {
                // Best-effort: return the operand type (covers `-x` and `!x`).
                self.infer_expr(right)
            }
            Expression::Infix {
                left,
                operator,
                right,
                span,
            } => self.infer_infix(left, operator, right, *span),

            // ── Control flow ──────────────────────────────────────────────────
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
            } => {
                let cond_ty = self.infer_expr(condition);
                self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), *span);

                let then_ty = self.infer_block(consequence);
                match alternative {
                    Some(alt) => {
                        let else_ty = self.infer_block(alt);
                        let then_value_span = self.block_value_span(consequence);
                        let else_value_span = self.block_value_span(alt);
                        self.unify_with_context(
                            &then_ty,
                            &else_ty,
                            *span,
                            ReportContext::IfBranch {
                                then_span: then_value_span,
                                else_span: else_value_span,
                            },
                        )
                    }
                    None => then_ty,
                }
            }

            Expression::DoBlock { block, .. } => self.infer_block(block),

            Expression::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                if arms.is_empty() {
                    return InferType::Con(TypeConstructor::Any);
                }
                let propagated_scrutinee_ty = self
                    .match_constraint_family(arms)
                    .and_then(|family| self.family_expected_type(&family))
                    .map(|expected| self.unify_reporting(&scrutinee_ty, &expected, *span))
                    .unwrap_or_else(|| scrutinee_ty.clone());

                // Infer the first arm.
                self.env.enter_scope();
                self.bind_pattern(&arms[0].pattern, &propagated_scrutinee_ty, *span);
                if let Some(guard) = &arms[0].guard {
                    self.infer_expr(guard);
                }
                let first_ty = self.infer_expr(&arms[0].body);
                let first_span = arms[0].body.span();
                self.env.leave_scope();

                // Keep historical arm-join unification behavior so unresolved
                // first-arm variables can still be refined by later concrete arms.
                let mut result_ty = first_ty.clone();
                let mut arm_types: Vec<(InferType, Span, usize)> =
                    vec![(first_ty.clone(), first_span, 1)];
                for (i, arm) in arms.iter().skip(1).enumerate() {
                    self.env.enter_scope();
                    self.bind_pattern(&arm.pattern, &propagated_scrutinee_ty, *span);
                    if let Some(guard) = &arm.guard {
                        self.infer_expr(guard);
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.env.leave_scope();
                    result_ty = self.unify_with_context(
                        &first_ty,
                        &arm_ty,
                        arm.span,
                        ReportContext::MatchArm {
                            first_span,
                            arm_span: arm.body.span(),
                            arm_index: i + 2,
                        },
                    );
                    arm_types.push((arm_ty, arm.body.span(), i + 1));
                }

                // Additional concrete-pivot check only when first arm is not
                // concrete, so ordering does not hide concrete conflicts.
                if !Self::is_concrete_non_any(&first_ty) {
                    let pivot = arm_types
                        .iter()
                        .find(|(ty, _, _)| Self::is_concrete_non_any(ty))
                        .cloned();
                    if let Some((pivot_ty, pivot_span, pivot_index)) = pivot {
                        for (arm_ty, arm_span, arm_index) in &arm_types {
                            if *arm_index == pivot_index {
                                continue;
                            }
                            if Self::is_concrete_non_any(arm_ty) {
                                let _ = self.unify_with_context(
                                    &pivot_ty,
                                    arm_ty,
                                    *arm_span,
                                    ReportContext::MatchArm {
                                        first_span: pivot_span,
                                        arm_span: *arm_span,
                                        arm_index: *arm_index,
                                    },
                                );
                            }
                        }
                    }
                }
                result_ty
            }

            // ── Lambda ────────────────────────────────────────────────────────
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                self.env.enter_scope();
                let mut row_var_env: HashMap<Identifier, TypeVarId> = HashMap::new();

                let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
                for (i, &param) in parameters.iter().enumerate() {
                    let ty = parameter_types
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .and_then(|te| {
                            TypeEnv::infer_type_from_type_expr_with_row_vars(
                                te,
                                &HashMap::new(),
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
                    InferEffectRow::open_from_symbols(
                        std::iter::empty::<Identifier>(),
                        self.env.fresh(),
                    )
                } else {
                    Self::infer_effect_row(effects, &mut row_var_env, &mut self.env.counter)
                };

                // Lambdas are effect-transparent: their published row matches
                // the ambient row (open when unannotated). This differs from
                // named functions, which publish closed-empty when unannotated
                // to preserve backward-compatible signatures.
                let declared_effect_row = ambient_effect_row.clone();
                let body_ty =
                    self.with_ambient_effect_row(ambient_effect_row, |ctx| ctx.infer_block(body));
                let ret_ty = match return_type {
                    Some(ret_ann) => {
                        match TypeEnv::infer_type_from_type_expr_with_row_vars(
                            ret_ann,
                            &HashMap::new(),
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

                let final_param_tys: Vec<InferType> = param_tys
                    .iter()
                    .map(|t| t.apply_type_subst(&self.subst))
                    .collect();
                self.env.leave_scope();

                InferType::Fun(
                    final_param_tys,
                    Box::new(ret_ty),
                    declared_effect_row.apply_row_subst(&self.subst),
                )
            }

            // ── Function call ─────────────────────────────────────────────────
            Expression::Call {
                function,
                arguments,
                span,
            } => {
                if let Expression::Identifier { name, .. } = function.as_ref()
                    && self.adt_constructor_types.contains_key(name)
                {
                    self.infer_constructor_call(*name, arguments, *span)
                } else {
                    self.infer_call(function, arguments, *span)
                }
            }

            // ── Collection literals ───────────────────────────────────────────
            Expression::TupleLiteral { elements, .. } => {
                let elem_tys: Vec<InferType> =
                    elements.iter().map(|e| self.infer_expr(e)).collect();
                InferType::Tuple(elem_tys)
            }

            Expression::ListLiteral { elements, span } => {
                if elements.is_empty() {
                    InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()])
                } else {
                    let first = self.infer_expr(&elements[0]);
                    for e in elements.iter().skip(1) {
                        let t = self.infer_expr(e);
                        self.unify_reporting(&first, &t, *span);
                    }
                    InferType::App(
                        TypeConstructor::List,
                        vec![first.apply_type_subst(&self.subst)],
                    )
                }
            }

            Expression::ArrayLiteral { elements, .. } => {
                // Flux arrays (`[| |]`) are heterogeneous at runtime (Array<Any>).
                // Infer each element for substitution side-effects, then return
                // Array<T> when all elements agree on T, or Array<Any> otherwise.
                if elements.is_empty() {
                    InferType::App(TypeConstructor::Array, vec![self.env.fresh_infer_type()])
                } else {
                    let first = self.infer_expr(&elements[0]);
                    let mut homogeneous = true;
                    for e in elements.iter().skip(1) {
                        let t = self.infer_expr(e);
                        let t_r = t.apply_type_subst(&self.subst);
                        let f_r = first.apply_type_subst(&self.subst);
                        if t_r != f_r {
                            homogeneous = false;
                            // Strict-first Any-fallback reduction: emit concrete
                            // mismatches at the literal site when possible so strict
                            // validation doesn't only observe unresolved Array<Any>.
                            self.unify_reporting(&first, &t, e.span());
                        }
                    }
                    let elem_ty = if homogeneous {
                        first.apply_type_subst(&self.subst)
                    } else {
                        InferType::Con(TypeConstructor::Any)
                    };
                    InferType::App(TypeConstructor::Array, vec![elem_ty])
                }
            }

            Expression::EmptyList { .. } => {
                InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()])
            }

            Expression::Hash { pairs, .. } => {
                if pairs.is_empty() {
                    let k = self.env.fresh_infer_type();
                    let v = self.env.fresh_infer_type();
                    InferType::App(TypeConstructor::Map, vec![k, v])
                } else {
                    let kt = self.infer_expr(&pairs[0].0);
                    let vt = self.infer_expr(&pairs[0].1);
                    // Infer remaining pairs for side effects (subst updates).
                    for (k, v) in pairs.iter().skip(1) {
                        self.infer_expr(k);
                        self.infer_expr(v);
                    }
                    InferType::App(TypeConstructor::Map, vec![kt, vt])
                }
            }

            Expression::Cons { head, tail, span } => {
                let elem_ty = self.infer_expr(head);
                let list_ty = InferType::App(TypeConstructor::List, vec![elem_ty]);
                let tail_ty = self.infer_expr(tail);
                self.unify_reporting(&list_ty, &tail_ty, *span);
                list_ty.apply_type_subst(&self.subst)
            }

            // ── Member / index access ─────────────────────────────────────────
            Expression::Index { left, index, .. } => {
                let left_ty = self.infer_expr(left);
                let _index_ty = self.infer_expr(index);
                let left_resolved = left_ty.apply_type_subst(&self.subst);
                match left_resolved {
                    InferType::App(TypeConstructor::Array, args)
                    | InferType::App(TypeConstructor::List, args)
                        if args.len() == 1 =>
                    {
                        InferType::App(TypeConstructor::Option, vec![args[0].clone()])
                    }
                    InferType::App(TypeConstructor::Map, args) if args.len() == 2 => {
                        InferType::App(TypeConstructor::Option, vec![args[1].clone()])
                    }
                    InferType::Tuple(elements) => {
                        if let Expression::Integer { value, .. } = index.as_ref()
                            && *value >= 0
                            && let Some(elem) = elements.get(*value as usize)
                        {
                            InferType::App(
                                TypeConstructor::Option,
                                vec![elem.clone().apply_type_subst(&self.subst)],
                            )
                        } else {
                            let joined = elements.iter().skip(1).fold(
                                elements
                                    .first()
                                    .cloned()
                                    .unwrap_or(InferType::Con(TypeConstructor::Any)),
                                |acc, ty| self.join_types(&acc, ty),
                            );
                            InferType::App(TypeConstructor::Option, vec![joined])
                        }
                    }
                    _ => InferType::Con(TypeConstructor::Any),
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                if let Expression::Identifier {
                    name: module_name, ..
                } = object.as_ref()
                    && let Some(scheme) = self
                        .module_member_schemes
                        .get(&(*module_name, *member))
                        .cloned()
                {
                    let (ty, _) = scheme.instantiate(&mut self.env.counter);
                    ty
                } else {
                    if let Expression::Identifier {
                        name: module_name, ..
                    } = object.as_ref()
                        && *module_name == self.base_module_symbol
                        && self.known_base_names.contains(member)
                    {
                        self.emit_missing_base_hm_signature(*member, expr.span());
                    }
                    self.infer_expr(object);
                    InferType::Con(TypeConstructor::Any)
                }
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                match self.infer_expr(object).apply_type_subst(&self.subst) {
                    InferType::Tuple(elements) => elements
                        .get(*index)
                        .cloned()
                        .unwrap_or(InferType::Con(TypeConstructor::Any)),
                    _ => InferType::Con(TypeConstructor::Any),
                }
            }
            Expression::Perform {
                effect,
                operation,
                args,
                span,
            } => {
                let arg_tys: Vec<InferType> = args.iter().map(|arg| self.infer_expr(arg)).collect();
                if let Some((param_tys, ret_ty)) =
                    self.effect_op_signature_types(*effect, *operation)
                {
                    if arg_tys.len() == param_tys.len() {
                        for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
                            self.unify_reporting(actual, expected, *span);
                        }
                        ret_ty.apply_type_subst(&self.subst)
                    } else {
                        InferType::Con(TypeConstructor::Any)
                    }
                } else {
                    for arg in args {
                        self.infer_expr(arg);
                    }
                    InferType::Con(TypeConstructor::Any)
                }
            }
            Expression::Handle {
                expr,
                effect,
                arms,
                span,
            } => {
                let handled_ty = self.with_handle_effect(*effect, |ctx| ctx.infer_expr(expr));
                let mut arm_result: Option<InferType> = None;
                for arm in arms {
                    self.env.enter_scope();
                    if let Some((param_tys, _ret_ty)) =
                        self.effect_op_signature_types(*effect, arm.operation_name)
                    {
                        for (param_name, param_ty) in arm.params.iter().zip(param_tys.iter()) {
                            self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
                        }
                    }
                    let body_ty =
                        self.with_handle_effect(*effect, |ctx| ctx.infer_expr(&arm.body));
                    self.env.leave_scope();
                    arm_result = Some(match arm_result {
                        Some(prev) => self.join_types(&prev, &body_ty),
                        None => body_ty,
                    });
                }

                let arm_ty = arm_result.unwrap_or(InferType::Con(TypeConstructor::Any));
                let _ = span;
                self.join_types(&handled_ty, &arm_ty)
            }
        };
        let resolved = inferred.apply_type_subst(&self.subst);
        self.expr_types.insert(node_id, resolved.clone());
        resolved
    }

    // ── Infix operator typing ─────────────────────────────────────────────────

    pub(super) fn infer_infix(
        &mut self,
        left: &Expression,
        op: &str,
        right: &Expression,
        span: Span,
    ) -> InferType {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);
        match op {
            // Arithmetic policy:
            // - `+` supports Int, Float, and String with matching operand types.
            // - `-`, `*`, `/`, `%` support Int/Float with matching operand types.
            // Mixed numeric types are rejected in typed contexts.
            "+" => {
                let resolved = self.unify_reporting(&lt, &rt, span);
                match resolved.apply_type_subst(&self.subst) {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float)
                    | InferType::Con(TypeConstructor::String) => {
                        resolved.apply_type_subst(&self.subst)
                    }
                    InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                        InferType::Con(TypeConstructor::Any)
                    }
                    other => {
                        let expected_numeric = InferType::Con(TypeConstructor::Int);
                        self.unify_reporting(&other, &expected_numeric, span);
                        InferType::Con(TypeConstructor::Any)
                    }
                }
            }
            "-" | "*" | "/" | "%" => {
                let resolved = self.unify_reporting(&lt, &rt, span);
                match resolved.apply_type_subst(&self.subst) {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float) => {
                        resolved.apply_type_subst(&self.subst)
                    }
                    InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                        InferType::Con(TypeConstructor::Any)
                    }
                    other => {
                        let expected_numeric = InferType::Con(TypeConstructor::Int);
                        self.unify_reporting(&other, &expected_numeric, span);
                        InferType::Con(TypeConstructor::Any)
                    }
                }
            }
            // Comparisons — operands must agree; result is Bool.
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                self.unify_reporting(&lt, &rt, span);
                InferType::Con(TypeConstructor::Bool)
            }
            // Logical — operands must be Bool.
            "&&" | "||" => {
                let bool_ty = InferType::Con(TypeConstructor::Bool);
                self.unify_reporting(&lt, &bool_ty, span);
                self.unify_reporting(&rt, &bool_ty, span);
                InferType::Con(TypeConstructor::Bool)
            }
            // Concatenation (strings or lists).
            "++" => {
                self.unify_reporting(&lt, &rt, span);
                lt.apply_type_subst(&self.subst)
            }
            // Pipe `|>`: right side is a function applied to left side.
            "|>" => rt,
            _ => InferType::Con(TypeConstructor::Any),
        }
    }

    // ── Call inference ────────────────────────────────────────────────────────

    pub(super) fn infer_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let fn_ty = self.infer_expr(function);
        let fn_ty_resolved = fn_ty.apply_type_subst(&self.subst);
        let ambient_effect_row = self
            .current_ambient_effect_row()
            .apply_row_subst(&self.subst);

        let (fn_name, fn_def_span) = match function {
            Expression::Identifier { name, .. } => {
                let fn_name = self.interner.resolve(*name).to_string();
                (Some(fn_name), self.env.lookup_span(*name))
            }
            _ => (None, None),
        };

        if let InferType::Fun(param_tys, ret_ty, fn_effects) = fn_ty_resolved.clone() {
            self.constrain_call_effects(&fn_effects, &ambient_effect_row, span);

            let has_higher_order_params = param_tys
                .iter()
                .map(|t| t.apply_type_subst(&self.subst))
                .any(|t| matches!(t, InferType::Fun(..)));
            if has_higher_order_params {
                // Preserve pre-058 higher-order behavior for now: keep the
                // original whole-function unification path, which is less eager
                // for function-typed argument diagnostics.
                let arg_tys: Vec<InferType> =
                    arguments.iter().map(|a| self.infer_expr(a)).collect();
                let ret_var = self.env.fresh_infer_type();
                let expected_fn_ty = InferType::Fun(
                    arg_tys,
                    Box::new(ret_var.clone()),
                    fn_effects.apply_row_subst(&self.subst),
                );
                self.unify_reporting(&fn_ty, &expected_fn_ty, span);
                return ret_var.apply_type_subst(&self.subst);
            }

            if param_tys.len() != arguments.len() {
                // Keep arity diagnostics in compile pass (E056); do not emit HM arity
                // diagnostics from call inference.
                return ret_ty.apply_type_subst(&self.subst);
            }

            for (index, (arg_expr, expected_param_ty)) in
                arguments.iter().zip(param_tys.iter()).enumerate()
            {
                let arg_ty = self.infer_expr(arg_expr);
                let expected_resolved = expected_param_ty.apply_type_subst(&self.subst);
                let actual_resolved = arg_ty.apply_type_subst(&self.subst);
                let should_emit = expected_resolved.is_concrete()
                    && actual_resolved.is_concrete()
                    && !expected_resolved.contains_any()
                    && !actual_resolved.contains_any();

                match unify_with_span_and_row_var_counter(
                    &expected_resolved,
                    &actual_resolved,
                    arg_expr.span(),
                    &mut self.env.counter,
                ) {
                    Ok(s) => {
                        self.subst = std::mem::take(&mut self.subst).compose(&s);
                    }
                    Err(_) => {
                        if should_emit {
                            let exp_str = self.display_type(&expected_resolved);
                            let act_str = self.display_type(&actual_resolved);
                            self.errors.push(call_arg_type_mismatch(
                                self.file_path.clone(),
                                arg_expr.span(),
                                fn_name.as_deref(),
                                index + 1,
                                fn_def_span,
                                &exp_str,
                                &act_str,
                            ));
                        }
                    }
                }
            }

            return ret_ty.apply_type_subst(&self.subst);
        }

        // Fallback for dynamic/unknown callees keeps prior behavior.
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expr(a)).collect();
        let ret_var = self.env.fresh_infer_type();
        let expected_fn_ty = InferType::Fun(arg_tys, Box::new(ret_var.clone()), ambient_effect_row);
        self.unify_with_context(
            &fn_ty,
            &expected_fn_ty,
            span,
            ReportContext::CallArg {
                fn_name,
                fn_def_span,
            },
        );
        ret_var.apply_type_subst(&self.subst)
    }

    // ── Pattern variable binding ──────────────────────────────────────────────

    /// Bind variables introduced by a pattern, propagating scrutinee type
    /// information when available.
    pub(super) fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        scrutinee_ty: &InferType,
        span: Span,
    ) {
        let resolved_scrutinee = scrutinee_ty.apply_type_subst(&self.subst);
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.env.bind(*name, Scheme::mono(resolved_scrutinee));
            }
            Pattern::Wildcard { .. } => {
                // Nothing to bind.
            }
            Pattern::Literal { expression, .. } => {
                let literal_ty = self.infer_expr(expression);
                self.unify_reporting(&resolved_scrutinee, &literal_ty, span);
            }
            Pattern::None { .. } => {
                let inner = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Option, vec![inner]);
                self.unify_reporting(&resolved_scrutinee, &expected, span);
            }
            Pattern::Some { pattern, .. } => {
                let inner = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Option, vec![inner.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let inner_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Option, args) if args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => inner.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &inner_ty, span);
            }
            Pattern::Left { pattern, .. } => {
                let left = self.env.fresh_infer_type();
                let right = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Either, vec![left.clone(), right]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let left_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[0].clone()
                    }
                    _ => left.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &left_ty, span);
            }
            Pattern::Right { pattern, .. } => {
                let left = self.env.fresh_infer_type();
                let right = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Either, vec![left, right.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let right_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[1].clone()
                    }
                    _ => right.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &right_ty, span);
            }
            Pattern::EmptyList { .. } => {
                let elem = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::List, vec![elem]);
                self.unify_reporting(&resolved_scrutinee, &expected, span);
            }
            Pattern::Cons { head, tail, .. } => {
                let elem = self.env.fresh_infer_type();
                let list_ty = InferType::App(TypeConstructor::List, vec![elem.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &list_ty, span);
                let element_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::List, args) if args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => elem.apply_type_subst(&self.subst),
                };
                self.bind_pattern(head, &element_ty, span);
                self.bind_pattern(tail, &list_ty, span);
            }
            Pattern::Tuple { elements, .. } => {
                let tuple_shape = InferType::Tuple(
                    elements
                        .iter()
                        .map(|_| self.env.fresh_infer_type())
                        .collect(),
                );
                let unified = self.unify_reporting(&resolved_scrutinee, &tuple_shape, span);
                if let InferType::Tuple(component_types) = unified.apply_type_subst(&self.subst) {
                    for (elem, elem_ty) in elements.iter().zip(component_types.iter()) {
                        self.bind_pattern(elem, elem_ty, span);
                    }
                } else {
                    if Self::is_concrete_non_any(&resolved_scrutinee) {
                        let expected =
                            self.display_type(&tuple_shape.apply_type_subst(&self.subst));
                        let actual = self.display_type(&resolved_scrutinee);
                        self.errors.push(type_unification_error(
                            self.file_path.clone(),
                            span,
                            &expected,
                            &actual,
                        ));
                    }
                    for elem in elements {
                        let fallback = self.env.fresh_infer_type();
                        self.bind_pattern(elem, &fallback, span);
                    }
                }
            }
            Pattern::Constructor { fields, .. } => {
                if let Pattern::Constructor { name, .. } = pattern
                    && let Some((field_tys, result_ty)) = self.instantiate_constructor_parts(*name)
                {
                    self.unify_reporting(&resolved_scrutinee, &result_ty, span);
                    if field_tys.len() == fields.len() {
                        for (field, field_ty) in fields.iter().zip(field_tys.iter()) {
                            self.bind_pattern(field, field_ty, span);
                        }
                    } else {
                        for field in fields {
                            self.bind_pattern(field, &InferType::Con(TypeConstructor::Any), span);
                        }
                    }
                } else {
                    for field in fields {
                        self.bind_pattern(field, &InferType::Con(TypeConstructor::Any), span);
                    }
                }
            }
        }
    }

    // ── Pattern family helpers ────────────────────────────────────────────────

    pub(super) fn pattern_family(&self, pattern: &Pattern) -> PatternFamily {
        match pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } | Pattern::Literal { .. } => {
                PatternFamily::NonConstraining
            }
            Pattern::None { .. } | Pattern::Some { .. } => PatternFamily::Option,
            Pattern::Left { .. } | Pattern::Right { .. } => PatternFamily::Either,
            Pattern::EmptyList { .. } | Pattern::Cons { .. } => PatternFamily::List,
            Pattern::Tuple { elements, .. } => PatternFamily::Tuple(elements.len()),
            Pattern::Constructor { name, .. } => self
                .adt_constructor_types
                .get(name)
                .map(|info| PatternFamily::Adt(info.adt_name))
                .unwrap_or(PatternFamily::UnknownOrMixed),
        }
    }

    pub(super) fn match_constraint_family(
        &self,
        arms: &[crate::syntax::expression::MatchArm],
    ) -> Option<PatternFamily> {
        let mut family: Option<PatternFamily> = None;
        for arm in arms {
            let arm_family = self.pattern_family(&arm.pattern);
            match arm_family {
                PatternFamily::NonConstraining => {}
                PatternFamily::UnknownOrMixed => return None,
                _ => match &family {
                    None => family = Some(arm_family),
                    Some(existing) if *existing == arm_family => {}
                    Some(_) => return None,
                },
            }
        }
        family
    }

    pub(super) fn family_expected_type(&mut self, family: &PatternFamily) -> Option<InferType> {
        match family {
            PatternFamily::Option => Some(InferType::App(
                TypeConstructor::Option,
                vec![self.env.fresh_infer_type()],
            )),
            PatternFamily::Either => Some(InferType::App(
                TypeConstructor::Either,
                vec![self.env.fresh_infer_type(), self.env.fresh_infer_type()],
            )),
            PatternFamily::List => Some(InferType::App(
                TypeConstructor::List,
                vec![self.env.fresh_infer_type()],
            )),
            PatternFamily::Tuple(arity) => Some(InferType::Tuple(
                (0..*arity).map(|_| self.env.fresh_infer_type()).collect(),
            )),
            PatternFamily::Adt(adt_name) => {
                let info = self
                    .adt_constructor_types
                    .values()
                    .find(|info| info.adt_name == *adt_name)?;
                if info.type_params.is_empty() {
                    Some(InferType::Con(TypeConstructor::Adt(*adt_name)))
                } else {
                    Some(InferType::App(
                        TypeConstructor::Adt(*adt_name),
                        info.type_params
                            .iter()
                            .map(|_| self.env.fresh_infer_type())
                            .collect(),
                    ))
                }
            }
            PatternFamily::NonConstraining | PatternFamily::UnknownOrMixed => None,
        }
    }
}
