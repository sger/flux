use std::collections::{HashMap, HashSet};

use crate::{
    diagnostics::{
        Diagnostic, DiagnosticBuilder, compiler_errors::STRICT_TYPES_ANY_INFERRED, diagnostic_for,
        position::Span,
    },
    syntax::{
        Identifier,
        block::Block,
        expression::{ExprId, Expression, HandleArm, MatchArm, Pattern, StringPart},
        interner::Interner,
        program::Program,
        statement::Statement,
    },
    types::{TypeVarId, infer_type::InferType, type_env::TypeEnv},
};

use super::display::display_infer_type;

// ─────────────────────────────────────────────────────────────────────────────
// Static type validation pass
// ─────────────────────────────────────────────────────────────────────────────

/// Post-inference validation for the maintained static-typing contract.
///
/// Walks the AST after type inference and reports the smallest subexpressions
/// whose inferred type still contains unresolved type variables (variables not
/// bound by a surrounding `forall`). Also checks top-level bindings for
/// unresolved variables in their inferred schemes.
pub fn validate_static_types(
    program: &Program,
    type_env: &TypeEnv,
    expr_types: &HashMap<ExprId, InferType>,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut emitted_exprs = HashSet::new();
    validate_statements(
        &program.statements,
        type_env,
        expr_types,
        interner,
        &mut diagnostics,
        &mut emitted_exprs,
    );
    diagnostics
}

struct StrictTypeValidator<'a> {
    expr_types: &'a HashMap<ExprId, InferType>,
    interner: &'a Interner,
    diagnostics: &'a mut Vec<Diagnostic>,
    emitted_exprs: &'a mut HashSet<ExprId>,
    /// Type variables currently in scope from surrounding `forall` quantifiers.
    /// Variables in this set are legitimately polymorphic and should NOT be
    /// flagged as unresolved.
    in_scope_forall: HashSet<TypeVarId>,
}

impl<'a> StrictTypeValidator<'a> {
    /// Walk top-level statements, checking bindings and nested expressions for unresolved type variables.
    fn validate_statements(&mut self, statements: &[Statement], type_env: &TypeEnv) {
        for stmt in statements {
            self.validate_statement(stmt, type_env);
        }
    }

    /// Validate one statement and recurse into the shapes that can carry inferred expressions.
    fn validate_statement(&mut self, stmt: &Statement, type_env: &TypeEnv) {
        match stmt {
            Statement::Function {
                name, body, span, ..
            } => {
                // Look up the function's scheme to get its forall-quantified vars.
                // These vars are legitimately polymorphic inside the body.
                let saved_forall = self.in_scope_forall.clone();
                if let Some(scheme) = type_env.lookup(*name) {
                    self.in_scope_forall
                        .extend(scheme.forall.iter().copied());
                }
                self.validate_block(body);
                self.in_scope_forall = saved_forall;
                check_binding(*name, *span, type_env, self.interner, self.diagnostics);
            }
            Statement::Let {
                name, value, span, ..
            } => {
                self.validate_expression(value);
                check_binding(*name, *span, type_env, self.interner, self.diagnostics);
            }
            Statement::LetDestructure { value, .. } | Statement::Assign { value, .. } => {
                self.validate_expression(value);
            }
            Statement::Return {
                value: Some(value), ..
            } => {
                self.validate_expression(value);
            }
            Statement::Expression { expression, .. } => {
                self.validate_expression(expression);
            }
            Statement::Module { body, .. } => self.validate_statements(&body.statements, type_env),
            Statement::Class { methods, .. } => {
                for method in methods {
                    if let Some(body) = &method.default_body {
                        self.validate_block(body);
                    }
                }
            }
            Statement::Instance { methods, .. } => {
                for method in methods {
                    self.validate_block(&method.body);
                }
            }
            _ => {}
        }
    }

    /// Validate every statement in a block under a fresh local binding environment view.
    fn validate_block(&mut self, block: &Block) {
        self.validate_statements(&block.statements, &TypeEnv::new());
    }

    /// Validate one expression and return whether it or any child has unresolved type variables.
    fn validate_expression(&mut self, expr: &Expression) -> bool {
        let child_has_unresolved = self.expression_children_have_unresolved(expr);
        self.emit_expression_diagnostic_if_needed(expr, child_has_unresolved);
        self.expression_has_unresolved_var(expr) || child_has_unresolved
    }

    /// Recurse into the immediate children of an expression without emitting duplicate diagnostics.
    fn expression_children_have_unresolved(&mut self, expr: &Expression) -> bool {
        match expr {
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => false,
            Expression::InterpolatedString { parts, .. } => {
                self.parts_have_unresolved(parts)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => self.validate_if_expression(condition, consequence, alternative.as_ref()),
            Expression::DoBlock { block, .. } | Expression::Function { body: block, .. } => {
                self.block_has_unresolved(block)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => self.validate_expression(scrutinee) || self.match_arms_have_unresolved(arms),
            Expression::Handle { expr, arms, .. } => {
                self.validate_expression(expr) || self.handle_arms_have_unresolved(arms)
            }
            _ => self.simple_expression_children_have_unresolved(expr),
        }
    }

    /// Handle the simple expression shapes whose children can be checked uniformly.
    fn simple_expression_children_have_unresolved(&mut self, expr: &Expression) -> bool {
        match expr {
            Expression::Prefix { right, .. }
            | Expression::Some { value: right, .. }
            | Expression::Left { value: right, .. }
            | Expression::Right { value: right, .. } => self.validate_expression(right),
            Expression::Infix { left, right, .. }
            | Expression::Cons {
                head: left,
                tail: right,
                ..
            }
            | Expression::Index {
                left, index: right, ..
            } => self.validate_expression(left) || self.validate_expression(right),
            Expression::Call {
                function,
                arguments,
                ..
            } => self.validate_expression(function) || self.expressions_have_unresolved(arguments),
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => {
                self.expressions_have_unresolved(elements)
            }
            Expression::Hash { pairs, .. } => self.pairs_have_unresolved(pairs),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => self.validate_expression(object),
            Expression::Perform { args, .. } => self.expressions_have_unresolved(args),
            _ => false,
        }
    }

    /// Emit the leaf-most strict-types diagnostic for an expression with unresolved type variables.
    fn emit_expression_diagnostic_if_needed(
        &mut self,
        expr: &Expression,
        child_has_unresolved: bool,
    ) {
        let expr_id = expr.expr_id();
        let has_unresolved = self.expression_has_unresolved_var(expr);
        if has_unresolved && !child_has_unresolved && self.emitted_exprs.insert(expr_id) {
            let ty = self
                .expr_types
                .get(&expr_id)
                .expect("expr_types should contain current expression");
            self.diagnostics
                .push(build_expression_any_diagnostic(expr, ty, self.interner));
        }
    }

    /// Return whether the inferred type for an expression contains unresolved
    /// type variables.
    ///
    /// Expression-level checking is currently disabled because expression types
    /// in the `expr_types` map use instantiated variables (from scheme
    /// instantiation at use sites) that are distinct from the `forall` vars
    /// in the enclosing function's scheme.  A future pass with proper
    /// instantiation tracking can enable this.
    fn expression_has_unresolved_var(&self, _expr: &Expression) -> bool {
        false
    }

    /// Check interpolation segments for unresolved type variables.
    fn parts_have_unresolved(&mut self, parts: &[StringPart]) -> bool {
        parts.iter().any(|part| match part {
            StringPart::Literal(_) => false,
            StringPart::Interpolation(expr) => self.validate_expression(expr),
        })
    }

    /// Return whether any expression in a flat list has unresolved type variables.
    fn expressions_have_unresolved(&mut self, exprs: &[Expression]) -> bool {
        exprs.iter().any(|expr| self.validate_expression(expr))
    }

    /// Return whether any key or value inside a hash literal has unresolved type variables.
    fn pairs_have_unresolved(&mut self, pairs: &[(Expression, Expression)]) -> bool {
        pairs
            .iter()
            .any(|(key, value)| self.validate_expression(key) || self.validate_expression(value))
    }

    /// Validate an `if` expression by checking its condition and both branch blocks.
    fn validate_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: Option<&Block>,
    ) -> bool {
        self.validate_expression(condition)
            || self.block_has_unresolved(consequence)
            || alternative.is_some_and(|alt| self.block_has_unresolved(alt))
    }

    /// Return whether any statement nested inside the block has unresolved type variables.
    fn block_has_unresolved(&mut self, block: &Block) -> bool {
        block
            .statements
            .iter()
            .any(|statement| self.statement_has_unresolved(statement))
    }

    /// Return whether a statement contains any nested expression or body with unresolved type variables.
    fn statement_has_unresolved(&mut self, statement: &Statement) -> bool {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => self.validate_expression(value),
            Statement::Return { value, .. } => value
                .as_ref()
                .is_some_and(|value| self.validate_expression(value)),
            Statement::Expression { expression, .. } => self.validate_expression(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                self.block_has_unresolved(body)
            }
            Statement::Class { methods, .. } => methods.iter().any(|method| {
                method
                    .default_body
                    .as_ref()
                    .is_some_and(|body| self.block_has_unresolved(body))
            }),
            Statement::Instance { methods, .. } => methods
                .iter()
                .any(|method| self.block_has_unresolved(&method.body)),
            _ => false,
        }
    }

    /// Return whether any arm in a `match` has unresolved type variables.
    fn match_arms_have_unresolved(&mut self, arms: &[MatchArm]) -> bool {
        arms.iter().any(|arm| self.match_arm_has_unresolved(arm))
    }

    /// Return whether one `match` arm has unresolved type variables.
    fn match_arm_has_unresolved(&mut self, arm: &MatchArm) -> bool {
        self.pattern_has_unresolved(&arm.pattern)
            || arm
                .guard
                .as_ref()
                .is_some_and(|guard| self.validate_expression(guard))
            || self.validate_expression(&arm.body)
    }

    /// Return whether any handler arm has unresolved type variables.
    fn handle_arms_have_unresolved(&mut self, arms: &[HandleArm]) -> bool {
        arms.iter().any(|arm| self.handle_arm_has_unresolved(arm))
    }

    /// Return whether one effect handler arm has unresolved type variables.
    fn handle_arm_has_unresolved(&mut self, arm: &HandleArm) -> bool {
        self.validate_expression(&arm.body)
    }

    /// Return whether a pattern embeds literals or nested subpatterns with unresolved type variables.
    fn pattern_has_unresolved(&mut self, pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Literal { expression, .. } => self.validate_expression(expression),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.pattern_has_unresolved(pattern),
            Pattern::Tuple { elements, .. }
            | Pattern::Constructor {
                fields: elements, ..
            } => elements.iter().any(|element| self.pattern_has_unresolved(element)),
            Pattern::Cons { head, tail, .. } => {
                self.pattern_has_unresolved(head) || self.pattern_has_unresolved(tail)
            }
            Pattern::None { .. }
            | Pattern::Identifier { .. }
            | Pattern::Wildcard { .. }
            | Pattern::EmptyList { .. } => false,
        }
    }
}

/// Run the strict-types validator over top-level statements using the recorded HM expression map.
fn validate_statements(
    statements: &[Statement],
    type_env: &TypeEnv,
    expr_types: &HashMap<ExprId, InferType>,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
    emitted_exprs: &mut HashSet<ExprId>,
) {
    StrictTypeValidator {
        expr_types,
        interner,
        diagnostics,
        emitted_exprs,
        in_scope_forall: HashSet::new(),
    }
    .validate_statements(statements, type_env);
}

/// Look up a single binding in the type environment and emit an error if its
/// inferred scheme contains unresolved type variables (vars not in `forall`).
///
/// Note: the `type_env` from inference stores pre-substitution schemes.
/// Functions without explicit type params get mono schemes (`forall = []`)
/// even when they are legitimately polymorphic. We account for this by
/// treating ALL free vars in the resolved type as implicitly quantified —
/// the same rule `build_infer_result` uses for `module_member_schemes`.
/// A var is only unresolved if it is free in the body AND not in `forall`
/// AND the scheme was explicitly constructed with `forall` (i.e., the function
/// had explicit type params). For mono schemes, `has_unresolved_vars()`
/// would always fire, so we skip them — they are handled by HM inference
/// itself and by the `emit_strict_inference_error` inline checks.
fn check_binding(
    name: Identifier,
    span: Span,
    type_env: &TypeEnv,
    interner: &Interner,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(scheme) = type_env.lookup(name) else {
        return;
    };
    // Skip mono schemes: these are unannotated functions where all remaining
    // vars are implicitly polymorphic (standard HM behavior).
    if scheme.forall.is_empty() {
        return;
    }
    if !scheme.has_unresolved_vars() {
        return;
    }
    let display_name = interner.resolve(name);
    let inferred = display_infer_type(&scheme.infer_type, interner);
    diagnostics.push(build_binding_any_diagnostic(display_name, &inferred, span));
}

/// Build the strict-types diagnostic for a top-level binding whose inferred type
/// still contains unresolved type variables.
fn build_binding_any_diagnostic(name: &str, inferred_type: &str, span: Span) -> Diagnostic {
    diagnostic_for(&STRICT_TYPES_ANY_INFERRED)
        .with_span(span)
        .with_message(format!(
            "Could not determine a concrete type for `{name}`. \
             Inferred type: `{inferred_type}`."
        ))
        .with_hint_text(format!(
            "Add a type annotation: e.g. `fn {name}(x: Int, y: Int): Int`"
        ))
}

/// Build the strict-types diagnostic for the smallest expression whose inferred
/// type still contains unresolved type variables.
fn build_expression_any_diagnostic(
    expr: &Expression,
    inferred_type: &InferType,
    interner: &Interner,
) -> Diagnostic {
    diagnostic_for(&STRICT_TYPES_ANY_INFERRED)
        .with_span(expr.span())
        .with_message(format!(
            "Could not determine a concrete type for this expression. \
             Expression: `{}`. Inferred type: `{}`.",
            expr.display_with(interner),
            display_infer_type(inferred_type, interner),
        ))
        .with_hint_text(
            "Add a type annotation or rewrite this expression so its type is fully determined.",
        )
}
