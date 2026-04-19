use super::*;

impl<'a> InferCtx<'a> {
    // ── Program / statement inference ─────────────────────────────────────────

    /// Infer all top-level statements in evaluation order.
    ///
    /// Phase A predeclares constructors and function names for recursive
    /// references; Phase B performs statement-by-statement inference.
    pub(super) fn infer_program(&mut self, program: &Program) {
        // Phase A0: predeclare top-level ADT constructors so functions can
        // reference constructors defined later in the file.
        self.predeclare_data_constructors_in_statements(&program.statements);

        // Phase A: pre-declare all top-level function names. Functions with a
        // complete explicit signature (all parameters annotated + return type)
        // get their declared polymorphic scheme so recursive call sites
        // instantiate at fresh types (Proposal 0159, Phase 3). Functions
        // lacking a complete signature fall back to the `Scheme::mono` slot
        // used for mutual-recursion forward references.
        for stmt in &program.statements {
            if let Statement::Function {
                name,
                span,
                type_params,
                parameter_types,
                return_type,
                effects,
                ..
            } = stmt
            {
                let scheme = self
                    .declared_fn_scheme(type_params, parameter_types, return_type, effects)
                    .unwrap_or_else(|| {
                        let v = self.env.alloc_infer_type_var();
                        Scheme::mono(v)
                    });
                self.env.bind_with_span(*name, scheme, Some(*span));
            }
        }

        // Phase B: infer each top-level statement.
        for stmt in &program.statements {
            self.infer_statement(stmt);
        }
    }

    /// Infer one statement and apply any side effects to environment/substitution state.
    ///
    /// Behavior:
    /// - Routes each statement variant to a focused inference helper.
    /// - Preserves top-level inference order.
    pub(super) fn infer_statement(&mut self, stmt: &Statement) {
        if self.infer_function_statement(stmt) {
            return;
        }
        match stmt {
            Statement::Let {
                name,
                type_annotation,
                value,
                span,
                ..
            } => {
                self.infer_let_binding(*name, *span, type_annotation.as_ref(), value);
            }
            Statement::LetDestructure {
                pattern,
                value,
                span,
                ..
            } => {
                let val_ty = self.infer_expression(value);
                self.bind_pattern_variables(pattern, &val_ty, *span);
            }
            Statement::Expression { expression, .. } => {
                // Evaluate for side effects; type is discarded.
                self.infer_expression(expression);
            }
            Statement::Assign { value, .. } => {
                self.infer_expression(value);
            }
            Statement::Module { name, body, .. } => self.infer_module(*name, body),
            Statement::Data {
                name,
                type_params,
                variants,
                ..
            } => {
                self.register_data_constructors(*name, type_params, variants);
            }
            // Import, Return at top-level: no HM inference needed.
            _ => {}
        }
    }

    /// Infer function declarations from a statement and return `true` when handled.
    fn infer_function_statement(&mut self, stmt: &Statement) -> bool {
        match stmt {
            Statement::Function {
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } => {
                self.infer_function_declaration(FnInferInput {
                    name: *name,
                    fn_span: *span,
                    type_params,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                });
                true
            }
            _ => false,
        }
    }

    /// Infer a `let` binding, apply optional annotation constraint, and generalize.
    ///
    /// Annotation mismatches are propagated through substitutions; primary
    /// diagnostics remain owned by compiler boundary checks.
    pub(super) fn infer_let_binding(
        &mut self,
        name: Identifier,
        let_span: Span,
        annotation: Option<&TypeExpr>,
        value: &Expression,
    ) {
        let constraint_start = self.class_constraints.len();
        // Propagation order (Proposal 0159): when the initializer benefits
        // from expected-type propagation, run check_expression BEFORE the
        // canonical annotation unify. Check mode emits per-sub-expression
        // diagnostics at precise spans; running it first primes the
        // `seen_error_keys` dedup so later broader diagnostics (list-level
        // unify, canonical annotation E300) don't duplicate the same
        // (expected, actual) pair at a vaguer span.
        let final_ty = match annotation {
            Some(ann) => {
                let mut row_var_env = HashMap::new();
                match TypeEnv::convert_type_expr_rec(
                    ann,
                    &HashMap::new(),
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                ) {
                    Some(ann_ty) => {
                        if benefits_from_check_propagation(value) {
                            self.check_expression(value, &ann_ty);
                        }
                        let val_ty = self.infer_expression(value);
                        self.check_let_annotation(name, ann, &ann_ty, value, &val_ty)
                    }
                    None => self.infer_expression(value).apply_type_subst(&self.subst),
                }
            }
            None => self.infer_expression(value).apply_type_subst(&self.subst),
        };

        // Generalize the let binding (Hindley-Milner let-polymorphism).
        let env_free = self.env.free_vars();
        let relevant_constraints = self.class_constraints[constraint_start..].to_vec();
        let scheme = self.finalize_binding_scheme(&final_ty, &relevant_constraints, &env_free);
        self.binding_schemes_by_span
            .insert(binding_span_key(let_span), scheme.clone());
        self.env.bind(name, scheme);
    }

    /// Unify a `let` binding's value type against its annotation, emitting
    /// E300 (Annotation Type Mismatch) on failure and recovering with the
    /// annotation type so downstream inference remains consistent.
    fn check_let_annotation(
        &mut self,
        name: Identifier,
        ann: &TypeExpr,
        ann_ty: &InferType,
        value: &Expression,
        val_ty: &InferType,
    ) -> InferType {
        match crate::types::unify::unify_core(
            val_ty,
            ann_ty,
            &self.subst,
            Span::default(),
            &mut self.env.counter,
            &self.skolem_vars,
        ) {
            Ok(s) => {
                self.subst = std::mem::take(&mut self.subst).compose(&s);
                val_ty.apply_type_subst(&self.subst)
            }
            Err(_) => {
                let val_resolved = val_ty.apply_type_subst(&self.subst);
                let ann_resolved = ann_ty.apply_type_subst(&self.subst);
                let name_str = self.interner.resolve(name).to_string();
                let ann_str = display_infer_type(&ann_resolved, self.interner);
                let val_str = display_infer_type(&val_resolved, self.interner);
                let diag = crate::diagnostics::compiler_errors::let_annotation_type_mismatch(
                    self.file_path.clone(),
                    ann.span(),
                    value.span(),
                    &name_str,
                    &ann_str,
                    &val_str,
                );
                self.errors.push(diag);
                ann_resolved
            }
        }
    }

    /// Infer a module body in an inner scope and export public function schemes.
    pub(super) fn infer_module(&mut self, module_name: Identifier, body: &Block) {
        self.env.enter_scope();
        self.predeclare_data_constructors_in_statements(&body.statements);
        self.predeclare_module_members(&body.statements);
        for stmt in &body.statements {
            self.infer_statement(stmt);
            self.capture_public_module_member_scheme(module_name, stmt);
        }
        self.env.leave_scope();
    }

    /// Predeclare module-scope `let` and explicitly-annotated function members.
    ///
    /// This mirrors the top-level Phase A pass, but runs inside the module's
    /// inner scope so recursive and forward references can resolve while
    /// preserving Proposal 0159's annotation-gated polymorphic predeclaration.
    fn predeclare_module_members(&mut self, statements: &[Statement]) {
        // Annotation-gated Phase A for modules (Proposal 0159, Phase 3):
        // only predeclare functions with a complete explicit signature,
        // using their declared polymorphic scheme so each call site
        // instantiates at fresh types. Unannotated helpers are left out —
        // the old `Scheme::mono(fresh)` predeclare caused distinct callers
        // to collapse their polymorphic parameters through a shared var
        // (see docs/internals/proposal_0159_investigation.md).
        for stmt in statements {
            self.predeclare_module_let(stmt);
            self.predeclare_module_function(stmt);
        }
    }

    /// Reserve a module-scope `let` binding before body inference.
    ///
    /// Annotated lets use their declared type as the monomorphic placeholder;
    /// unannotated lets receive a fresh inference variable to be refined once
    /// the initializer is visited.
    fn predeclare_module_let(&mut self, stmt: &Statement) {
        let Statement::Let {
            name,
            type_annotation,
            span,
            ..
        } = stmt
        else {
            return;
        };

        let scheme = match type_annotation {
            Some(annotation) => {
                let mut row_var_env = HashMap::new();
                TypeEnv::convert_type_expr_rec(
                    annotation,
                    &HashMap::new(),
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                )
                .map(Scheme::mono)
                .unwrap_or_else(|| {
                    let v = self.env.alloc_infer_type_var();
                    Scheme::mono(v)
                })
            }
            None => {
                let v = self.env.alloc_infer_type_var();
                Scheme::mono(v)
            }
        };
        self.env.bind_with_span(*name, scheme, Some(*span));
    }

    /// Bind a module function's declared polymorphic scheme when available.
    ///
    /// Functions without a complete explicit signature are left alone here and
    /// inferred normally during the statement pass.
    fn predeclare_module_function(&mut self, stmt: &Statement) {
        let Statement::Function {
            name,
            span,
            type_params,
            parameter_types,
            return_type,
            effects,
            ..
        } = stmt
        else {
            return;
        };

        if let Some(scheme) =
            self.declared_fn_scheme(type_params, parameter_types, return_type, effects)
        {
            self.env.bind_with_span(*name, scheme, Some(*span));
        }
    }

    /// Export the inferred scheme for a public module member after inference.
    ///
    /// Module consumers read these cached schemes for qualified lookups and
    /// cross-module typechecking.
    fn capture_public_module_member_scheme(&mut self, module_name: Identifier, stmt: &Statement) {
        let name = match stmt {
            Statement::Function {
                is_public: true,
                name,
                ..
            }
            | Statement::Let {
                is_public: true,
                name,
                ..
            } => *name,
            _ => return,
        };

        if let Some(scheme) = self.env.lookup(name).cloned() {
            self.module_member_schemes
                .insert((module_name, name), scheme);
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

    /// Infer the type of a block from its last value-producing statement.
    ///
    /// Returns `Unit` when the block has no value expression.
    pub(super) fn infer_block(&mut self, block: &Block) -> InferType {
        // Predeclare nested function names so forward references and mutual
        // recursion work inside function bodies (mirrors top-level Phase A).
        for stmt in &block.statements {
            if let Statement::Function { name, span, .. } = stmt
                && self.env.lookup(*name).is_none()
            {
                let v = self.env.alloc_infer_type_var();
                self.env.bind_with_span(*name, Scheme::mono(v), Some(*span));
            }
        }

        let mut last_ty = InferType::Con(TypeConstructor::Unit);
        for stmt in &block.statements {
            match stmt {
                // The last no-semicolon expression is the block's value.
                Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                } => {
                    last_ty = self.infer_expression(expression);
                }
                // An explicit `return expr` also gives the block's type.
                Statement::Return {
                    value: Some(expr), ..
                } => {
                    last_ty = self.infer_expression(expr);
                }
                _ => {
                    self.infer_statement(stmt);
                    last_ty = InferType::Con(TypeConstructor::Unit);
                }
            }
        }
        last_ty
    }
}

/// Return true when a typed-`let` initializer benefits from expected-type
/// propagation into sub-expressions (per Proposal 0159, Phase 1 + follow-up).
fn benefits_from_check_propagation(expr: &Expression) -> bool {
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
    )
}
