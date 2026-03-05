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
                let v = self.env.fresh_infer_type();
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

        // Propagate the let annotation constraint silently — the compiler's
        // boundary checker in statement.rs is the authoritative reporter for
        // typed-let initializer mismatches.
        let final_ty = match annotation {
            Some(ann) => {
                let mut row_var_env = HashMap::new();
                match TypeEnv::infer_type_from_type_expr_with_row_vars(
                    ann,
                    &HashMap::new(),
                    self.interner,
                    &mut row_var_env,
                    &mut self.env.counter,
                ) {
                    Some(ann_ty) => self.unify_propagate(&val_ty, &ann_ty),
                    None => val_ty.apply_type_subst(&self.subst),
                }
            }
            None => val_ty.apply_type_subst(&self.subst),
        };

        // Generalize the let binding (Hindley-Milner let-polymorphism).
        let env_free = self.env.free_vars();
        let scheme = generalize(&final_ty, &env_free);
        self.env.bind(name, scheme);
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

    /// Infer the type of a block from its last value-producing statement.
    ///
    /// Returns `Unit` when the block has no value expression.
    pub(super) fn infer_block(&mut self, block: &Block) -> InferType {
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
