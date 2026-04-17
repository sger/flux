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

        // Phase A: pre-declare all top-level function names with a fresh type
        // variable so that mutually-recursive functions can reference each other.
        for stmt in &program.statements {
            if let Statement::Function { name, span, .. } = stmt {
                let v = self.env.alloc_infer_type_var();
                self.env.bind_with_span(*name, Scheme::mono(v), Some(*span));
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
                ..
            } => {
                self.infer_let_binding(*name, type_annotation.as_ref(), value);
            }
            Statement::LetDestructure {
                pattern,
                value,
                span,
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
        annotation: Option<&TypeExpr>,
        value: &Expression,
    ) {
        let val_ty = self.infer_expression(value);

        // Enforce the let annotation: unify val_ty with ann_ty and emit E300
        // on mismatch. HM is authoritative; downstream boundary checks in the
        // bytecode compiler remain a fallback for non-HM paths, and the
        // diagnostic aggregator dedupes overlapping E300s.
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
                    Some(ann_ty) => self.check_let_annotation(name, ann, &ann_ty, value, &val_ty),
                    None => val_ty.apply_type_subst(&self.subst),
                }
            }
            None => val_ty.apply_type_subst(&self.subst),
        };

        // Generalize the let binding (Hindley-Milner let-polymorphism).
        let env_free = self.env.free_vars();
        let constraints = self.collect_scheme_constraints(&final_ty);
        let scheme = if constraints.is_empty() {
            generalize(&final_ty, &env_free)
        } else {
            generalize_with_constraints(&final_ty, &env_free, constraints)
        };
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
        for stmt in &body.statements {
            self.infer_statement(stmt);
            if let Statement::Function {
                is_public: true,
                name,
                ..
            } = stmt
                && let Some(scheme) = self.env.lookup(*name).cloned()
            {
                self.module_member_schemes
                    .insert((module_name, *name), scheme);
            }
        }
        self.env.leave_scope();
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
