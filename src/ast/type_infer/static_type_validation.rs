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
    types::{TypeVarId, infer_type::InferType, scheme::Scheme},
};

use super::display::display_infer_type;

// ─────────────────────────────────────────────────────────────────────────────
// Static type validation pass
// ─────────────────────────────────────────────────────────────────────────────

/// Inputs for [`validate_static_types`]. Bundled into a single spec struct
/// so the entry point stays within the repo-wide 6-positional-parameter
/// ceiling (same pattern as [`super::FnSpec`] and [`super::InferProgramConfig`]).
///
/// All fields are borrows held for the duration of one validation pass;
/// nothing is consumed.
pub struct StaticTypeValidationCtx<'a> {
    pub resolved_schemes: &'a HashMap<Identifier, Scheme>,
    pub resolved_binding_schemes_by_span: &'a HashMap<super::BindingSpanKey, Scheme>,
    pub expr_types: &'a HashMap<ExprId, InferType>,
    pub module_member_schemes: &'a HashMap<(Identifier, Identifier), Scheme>,
    pub fallback_vars: &'a HashSet<TypeVarId>,
    pub instantiated_expr_vars: &'a HashSet<TypeVarId>,
    pub existing_diagnostics: &'a [Diagnostic],
    pub interner: &'a Interner,
}

/// Post-inference validation for the maintained static-typing contract.
///
/// This is the **authoritative gate** for static typing. It operates on
/// fully-resolved types (after the final substitution) and checks for
/// residual unresolved type variables.
///
/// `ctx.resolved_schemes` maps each top-level binding name to its resolved
/// `Scheme` where `forall` contains only legitimately polymorphic vars
/// (fallback vars from inference failures are excluded). A binding whose
/// resolved scheme has `has_unresolved_vars() == true` is flagged.
pub fn validate_static_types(program: &Program, ctx: &StaticTypeValidationCtx<'_>) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut emitted_exprs = HashSet::new();
    StrictTypeValidator {
        resolved_schemes: ctx.resolved_schemes,
        resolved_binding_schemes_by_span: ctx.resolved_binding_schemes_by_span,
        expr_types: ctx.expr_types,
        module_member_schemes: ctx.module_member_schemes,
        fallback_vars: ctx.fallback_vars,
        instantiated_expr_vars: ctx.instantiated_expr_vars,
        existing_diagnostics: ctx.existing_diagnostics,
        interner: ctx.interner,
        diagnostics: &mut diagnostics,
        emitted_exprs: &mut emitted_exprs,
        allowed_generalized_vars: HashSet::new(),
        current_module: None,
    }
    .validate_statements(&program.statements);
    diagnostics
}

struct StrictTypeValidator<'a> {
    resolved_schemes: &'a HashMap<Identifier, Scheme>,
    resolved_binding_schemes_by_span: &'a HashMap<super::BindingSpanKey, Scheme>,
    expr_types: &'a HashMap<ExprId, InferType>,
    module_member_schemes: &'a HashMap<(Identifier, Identifier), Scheme>,
    fallback_vars: &'a HashSet<TypeVarId>,
    instantiated_expr_vars: &'a HashSet<TypeVarId>,
    existing_diagnostics: &'a [Diagnostic],
    interner: &'a Interner,
    diagnostics: &'a mut Vec<Diagnostic>,
    emitted_exprs: &'a mut HashSet<ExprId>,
    allowed_generalized_vars: HashSet<TypeVarId>,
    current_module: Option<Identifier>,
}

impl<'a> StrictTypeValidator<'a> {
    /// Walk top-level statements, checking bindings and nested expressions.
    fn validate_statements(&mut self, statements: &[Statement]) {
        for stmt in statements {
            self.validate_statement(stmt);
        }
    }

    /// Validate one statement and recurse into nested shapes.
    fn validate_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Function {
                name,
                body,
                span,
                ..
            } => {
                self.with_binding_allowance(*span, *name, |validator| validator.validate_block(body));
                self.emit_binding_diagnostic(*name, *span);
            }
            Statement::Let {
                name,
                value,
                span,
                ..
            } => {
                self.with_binding_allowance(*span, *name, |validator| {
                    validator.validate_expression(value)
                });
                self.emit_binding_diagnostic(*name, *span);
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
            Statement::Module { name, body, .. } => {
                self.with_module(*name, |validator| {
                    validator.validate_statements(&body.statements)
                });
            }
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

    /// Emit an E430 diagnostic if the binding's resolved scheme still contains
    /// unresolved type variables (vars not in `forall`).
    fn emit_binding_diagnostic(&mut self, name: Identifier, span: Span) {
        let Some(scheme) = self.resolved_schemes.get(&name) else {
            return;
        };
        if !scheme.has_unresolved_vars() {
            return;
        }
        let display_name = self.interner.resolve(name);
        let inferred = display_infer_type(&scheme.infer_type, self.interner);
        self.diagnostics.push(
            diagnostic_for(&STRICT_TYPES_ANY_INFERRED)
                .with_span(span)
                .with_message(format!(
                    "Could not determine a concrete type for `{display_name}`. \
                     Inferred type: `{inferred}`."
                ))
                .with_hint_text(format!(
                    "Add a type annotation: e.g. `fn {display_name}(x: Int, y: Int): Int`"
                )),
        );
    }

    /// Validate every statement in a block.
    fn validate_block(&mut self, block: &Block) {
        self.validate_statements(&block.statements);
    }

    /// Run a validation subpass with the enclosing binding's generalized vars.
    fn with_binding_allowance<R>(
        &mut self,
        span: Span,
        name: Identifier,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = std::mem::take(&mut self.allowed_generalized_vars);
        if let Some(scheme) = self.lookup_binding_scheme(span, name) {
            self.allowed_generalized_vars = scheme.forall.iter().copied().collect();
        } else {
            self.allowed_generalized_vars = previous.clone();
        }
        let result = f(self);
        self.allowed_generalized_vars = previous;
        result
    }

    /// Run a validation subpass inside a module-member scope.
    fn with_module<R>(&mut self, module_name: Identifier, f: impl FnOnce(&mut Self) -> R) -> R {
        let previous = self.current_module.replace(module_name);
        let result = f(self);
        self.current_module = previous;
        result
    }

    /// Resolve the current binding scheme from either top-level or module-member maps.
    fn lookup_binding_scheme(&self, span: Span, name: Identifier) -> Option<&Scheme> {
        self.resolved_binding_schemes_by_span
            .get(&super::binding_span_key(span))
            .or_else(|| {
                self.current_module
            .and_then(|module_name| self.module_member_schemes.get(&(module_name, name)))
            .or_else(|| self.resolved_schemes.get(&name))
            })
    }

    /// Validate one expression and return whether it or any child has unresolved type variables.
    fn validate_expression(&mut self, expr: &Expression) -> bool {
        let child_has_unresolved = self.expression_children_have_unresolved(expr);
        self.emit_expression_diagnostic_if_needed(expr, child_has_unresolved);
        self.expression_has_unresolved_var(expr) || child_has_unresolved
    }

    /// Recurse into the immediate children of an expression.
    fn expression_children_have_unresolved(&mut self, expr: &Expression) -> bool {
        match expr {
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => false,
            Expression::InterpolatedString { parts, .. } => self.parts_have_unresolved(parts),
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

    /// Handle simple expression shapes whose children can be checked uniformly.
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
            Expression::MemberAccess { object, member, .. } => {
                self.member_access_children_have_unresolved(object, *member)
            }
            Expression::TupleFieldAccess { object, .. } => self.validate_expression(object),
            Expression::Perform { args, .. } => self.expressions_have_unresolved(args),
            _ => false,
        }
    }

    /// Skip validating the module name operand for resolved module-member access.
    fn member_access_children_have_unresolved(
        &mut self,
        object: &Expression,
        member: Identifier,
    ) -> bool {
        if let Expression::Identifier { .. } = object {
            return false;
        }
        let _ = member;
        self.validate_expression(object)
    }

    /// Emit the leaf-most strict-types diagnostic for an expression with unresolved type variables.
    fn emit_expression_diagnostic_if_needed(
        &mut self,
        expr: &Expression,
        child_has_unresolved: bool,
    ) {
        if matches!(expr, Expression::TupleFieldAccess { .. }) {
            return;
        }
        let expr_id = expr.expr_id();
        let has_unresolved = self.expression_has_unresolved_var(expr);
        if has_unresolved
            && !child_has_unresolved
            && !self.has_stronger_diagnostic_at(expr.span())
            && self.emitted_exprs.insert(expr_id)
        {
            let ty = self
                .expr_types
                .get(&expr_id)
                .expect("expr_types should contain current expression");
            self.diagnostics.push(
                diagnostic_for(&STRICT_TYPES_ANY_INFERRED)
                    .with_span(expr.span())
                    .with_message(format!(
                        "Could not determine a concrete type for this expression. \
                         Expression: `{}`. Inferred type: `{}`.",
                        expr.display_with(self.interner),
                        display_infer_type(ty, self.interner),
                    ))
                    .with_hint_text(
                        "Add a type annotation or rewrite this expression so its type is fully determined.",
                    ),
            );
        }
    }

    /// Return whether the inferred type for an expression contains unresolved
    /// type variables.
    ///
    /// Three-way predicate. A var is "unresolved" here iff **all** hold:
    ///
    /// - `!allowed_generalized_vars.contains(var)` — the var is not a
    ///   legitimately polymorphic `forall` binder in scope (those are fine).
    /// - `!instantiated_expr_vars.contains(var)` — the var was not introduced
    ///   by scheme instantiation at a call site (those are expected to be
    ///   resolved by downstream unification at the caller's context).
    /// - `fallback_vars.contains(var)` — the var was allocated as a fallback
    ///   after an inference failure. Only fallback vars count as real
    ///   residue; fresh unification vars that happen to remain free are not
    ///   inherently broken.
    ///
    /// This is the load-bearing invariant of the whole pass. If any of the
    /// three sets shifts, E430 either over-reports or misses real bugs.
    fn expression_has_unresolved_var(&self, expr: &Expression) -> bool {
        let Some(ty) = self.expr_types.get(&expr.expr_id()) else {
            return false;
        };
        ty.free_vars().into_iter().any(|var| {
            !self.allowed_generalized_vars.contains(&var)
                && !self.instantiated_expr_vars.contains(&var)
                && self.fallback_vars.contains(&var)
        })
    }

    /// Return whether a stronger existing diagnostic is already anchored at the same span.
    ///
    /// This suppresses follow-on E430 noise for expressions that already have a
    /// primary user-facing error such as E004 at the same location.
    fn has_stronger_diagnostic_at(&self, span: Span) -> bool {
        self.existing_diagnostics
            .iter()
            .any(|diag| diag.code() != Some("E430") && diag.span() == Some(span))
    }

    /// Check interpolation segments for unresolved type variables.
    fn parts_have_unresolved(&mut self, parts: &[StringPart]) -> bool {
        parts.iter().any(|part| match part {
            StringPart::Literal(_) => false,
            StringPart::Interpolation(expr) => self.validate_expression(expr),
        })
    }

    /// Return whether any expression in a list has unresolved type variables.
    fn expressions_have_unresolved(&mut self, exprs: &[Expression]) -> bool {
        exprs.iter().any(|expr| self.validate_expression(expr))
    }

    /// Return whether any key or value in a hash literal has unresolved type variables.
    fn pairs_have_unresolved(&mut self, pairs: &[(Expression, Expression)]) -> bool {
        pairs
            .iter()
            .any(|(key, value)| self.validate_expression(key) || self.validate_expression(value))
    }

    /// Validate an `if` expression by checking condition and both branches.
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

    /// Return whether any statement in a block has unresolved type variables.
    fn block_has_unresolved(&mut self, block: &Block) -> bool {
        block
            .statements
            .iter()
            .any(|statement| self.statement_has_unresolved(statement))
    }

    /// Return whether a statement contains nested expressions with unresolved type variables.
    fn statement_has_unresolved(&mut self, statement: &Statement) -> bool {
        match statement {
            Statement::Let { name, value, .. } => {
                self.with_binding_allowance(statement.span(), *name, |validator| {
                    validator.validate_expression(value)
                })
            }
            Statement::LetDestructure { value, .. } | Statement::Assign { value, .. } => {
                self.validate_expression(value)
            }
            Statement::Return { value, .. } => value
                .as_ref()
                .is_some_and(|value| self.validate_expression(value)),
            Statement::Expression { expression, .. } => self.validate_expression(expression),
            Statement::Function { name, body, .. } => {
                self.with_binding_allowance(statement.span(), *name, |validator| {
                    validator.block_has_unresolved(body)
                })
            }
            Statement::Module { name, body, .. } => {
                self.with_module(*name, |validator| validator.block_has_unresolved(body))
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

    /// Return whether any match arm has unresolved type variables.
    fn match_arms_have_unresolved(&mut self, arms: &[MatchArm]) -> bool {
        arms.iter().any(|arm| self.match_arm_has_unresolved(arm))
    }

    /// Return whether one match arm has unresolved type variables.
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

    /// Return whether one handler arm has unresolved type variables.
    fn handle_arm_has_unresolved(&mut self, arm: &HandleArm) -> bool {
        self.validate_expression(&arm.body)
    }

    /// Return whether a pattern has nested expressions with unresolved type variables.
    fn pattern_has_unresolved(&mut self, pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Literal { expression, .. } => self.validate_expression(expression),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.pattern_has_unresolved(pattern),
            Pattern::Tuple { elements, .. }
            | Pattern::Constructor {
                fields: elements, ..
            } => elements
                .iter()
                .any(|element| self.pattern_has_unresolved(element)),
            Pattern::Cons { head, tail, .. } => {
                self.pattern_has_unresolved(head) || self.pattern_has_unresolved(tail)
            }
            Pattern::None { .. }
            | Pattern::Identifier { .. }
            | Pattern::Wildcard { .. }
            | Pattern::EmptyList { .. } => false,
            Pattern::NamedConstructor { fields, .. } => fields.iter().any(|f| match &f.pattern {
                Some(sub) => self.pattern_has_unresolved(sub),
                None => false,
            }),
        }
    }
}
