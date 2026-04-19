use std::collections::HashMap;

use crate::{
    ast::{Folder, fold_expr},
    diagnostics::position::Span,
    syntax::{
        block::Block,
        expression::{ExprId, Expression},
        interner::Interner,
        program::Program,
        statement::Statement,
        symbol::Symbol,
    },
    types::{
        infer_effect_row::InferEffectRow, infer_type::InferType, type_constructor::TypeConstructor,
        type_env::TypeEnv,
    },
};

/// Type-informed AST optimization pass.
///
/// Runs after Phase 1 HM inference, consuming the `TypeEnv` to guide
/// transformations that require type information:
///
/// - **T1**: Pure function inlining (zero-arg and single-arg)
/// - **T2**: Constant propagation for typed `let` bindings
/// - **T3**: Dead branch elimination on statically-known boolean conditions
struct TypeInformedFoldCtx<'a> {
    type_env: &'a TypeEnv,
    #[allow(dead_code)]
    interner: &'a Interner,
    /// Top-level function bodies eligible for inlining (zero-arg, pure, single-expression).
    fn_bodies: HashMap<Symbol, InlinebleBody>,
    /// Typed literal bindings eligible for constant propagation.
    let_literals: HashMap<Symbol, Expression>,
}

#[derive(Clone)]
struct InlinebleBody {
    params: Vec<Symbol>,
    body_expr: Expression,
}

impl<'a> TypeInformedFoldCtx<'a> {
    fn new(type_env: &'a TypeEnv, interner: &'a Interner) -> Self {
        Self {
            type_env,
            interner,
            fn_bodies: HashMap::new(),
            let_literals: HashMap::new(),
        }
    }

    /// Collect top-level functions eligible for inlining.
    ///
    /// A function is inlinable when:
    /// 1. It has a monomorphic scheme in TypeEnv (forall is empty)
    /// 2. Its body is a single expression (no early returns, no do-blocks)
    /// 3. Its inferred effect row is closed-empty (pure)
    /// 4. It is not recursive (its body does not reference its own name)
    fn collect_inlinable_functions(&mut self, program: &Program) {
        for stmt in &program.statements {
            let Statement::Function {
                name,
                parameters,
                body,
                ..
            } = stmt
            else {
                continue;
            };

            // Must be a single-expression body.
            let Some(body_expr) = single_expression_body(body) else {
                continue;
            };

            // Limit to 0 or 1 parameters.
            if parameters.len() > 1 {
                continue;
            }

            // Must have a monomorphic pure scheme in TypeEnv.
            let Some(scheme) = self.type_env.lookup(*name) else {
                continue;
            };
            if !scheme.forall.is_empty() {
                continue;
            }
            if !is_pure_function_type(&scheme.infer_type) {
                continue;
            }

            // Must not be self-recursive.
            if expr_references_name(body_expr, *name) {
                continue;
            }

            self.fn_bodies.insert(
                *name,
                InlinebleBody {
                    params: parameters.clone(),
                    body_expr: body_expr.clone(),
                },
            );
        }
    }

    /// Collect typed `let` bindings with literal values for constant propagation.
    fn collect_let_literals(&mut self, program: &Program) {
        for stmt in &program.statements {
            let Statement::Let { name, value, .. } = stmt else {
                continue;
            };

            // Only propagate simple literals.
            if !is_simple_literal(value) {
                continue;
            }

            // Must have a monomorphic concrete type in TypeEnv.
            let Some(scheme) = self.type_env.lookup(*name) else {
                continue;
            };
            if !scheme.forall.is_empty() {
                continue;
            }
            if !is_propagatable_type(&scheme.infer_type) {
                continue;
            }

            self.let_literals.insert(*name, value.clone());
        }
    }

    /// Try to inline a function call. Returns `Some(inlined_expr)` on success.
    fn try_inline_call(
        &self,
        function: &Expression,
        arguments: &[Expression],
    ) -> Option<Expression> {
        let Expression::Identifier { name, .. } = function else {
            return None;
        };

        let body = self.fn_bodies.get(name)?;

        // Arity must match.
        if arguments.len() != body.params.len() {
            return None;
        }

        let mut result = body.body_expr.clone();

        // For single-arg: substitute the parameter with the argument via rename.
        if body.params.len() == 1 {
            result = substitute_identifier(result, body.params[0], &arguments[0]);
        }

        Some(result)
    }
}

impl Folder for TypeInformedFoldCtx<'_> {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up).
        let expr = fold_expr(self, expr);

        match expr {
            // T3: Dead branch elimination remove branches on literal booleans.
            Expression::If {
                ref condition,
                consequence,
                alternative,
                span,
                id,
            } => match condition.as_ref() {
                Expression::Boolean { value: true, .. } => block_to_expr(consequence, span),
                Expression::Boolean { value: false, .. } => {
                    if let Some(alt) = alternative {
                        block_to_expr(alt, span)
                    } else {
                        Expression::None { span, id }
                    }
                }
                _ => Expression::If {
                    condition: condition.clone(),
                    consequence,
                    alternative,
                    span,
                    id,
                },
            },

            // T2: Constant propagation replace known-literal identifiers.
            Expression::Identifier { name, span, id } => {
                if let Some(literal) = self.let_literals.get(&name) {
                    let mut replaced = literal.clone();
                    set_expr_span(&mut replaced, span);
                    replaced
                } else {
                    Expression::Identifier { name, span, id }
                }
            }

            // T1: Pure function inlining.
            Expression::Call {
                ref function,
                ref arguments,
                span,
                id,
            } => {
                if let Some(mut inlined) = self.try_inline_call(function, arguments) {
                    set_expr_span(&mut inlined, span);
                    inlined
                } else {
                    Expression::Call {
                        function: function.clone(),
                        arguments: arguments.clone(),
                        span,
                        id,
                    }
                }
            }

            other => other,
        }
    }
}

/// Apply type-informed optimization to a program.
///
/// Requires a `TypeEnv` from a prior Phase 1 inference run. The returned
/// program must be re-inferred (Phase 2) before codegen because pointer-keyed
/// expression IDs are invalidated by AST rewrites.
pub fn type_informed_fold(program: &Program, type_env: &TypeEnv, interner: &Interner) -> Program {
    let mut ctx = TypeInformedFoldCtx::new(type_env, interner);
    ctx.collect_inlinable_functions(program);
    ctx.collect_let_literals(program);
    ctx.fold_program(program.clone())
}

/// Extract the single expression from a block body, if it has exactly one
/// expression statement (no semicolon) or a return statement.
fn single_expression_body(block: &Block) -> Option<&Expression> {
    if block.statements.len() != 1 {
        return None;
    }
    match &block.statements[0] {
        Statement::Expression {
            expression,
            has_semicolon: false,
            ..
        } => Some(expression),
        Statement::Return {
            value: Some(expr), ..
        } => Some(expr),
        _ => None,
    }
}

/// Check if an inferred function type has a closed-empty effect row (pure).
fn is_pure_function_type(ty: &InferType) -> bool {
    match ty {
        InferType::Fun(_, _, effects) => *effects == InferEffectRow::closed_empty(),
        _ => false,
    }
}

/// Check if an expression is a simple literal suitable for propagation.
fn is_simple_literal(expr: &Expression) -> bool {
    matches!(
        expr,
        Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
    )
}

/// Check if a type is suitable for constant propagation (simple scalar type).
fn is_propagatable_type(ty: &InferType) -> bool {
    matches!(
        ty,
        InferType::Con(TypeConstructor::Int)
            | InferType::Con(TypeConstructor::Float)
            | InferType::Con(TypeConstructor::String)
            | InferType::Con(TypeConstructor::Bool)
    )
}

/// Check if an expression references a specific name (shallow — no descent into
/// nested function bodies, since those introduce new scopes).
fn expr_references_name(expr: &Expression, target: Symbol) -> bool {
    match expr {
        Expression::Identifier { name, .. } => *name == target,
        Expression::Infix { left, right, .. } => {
            expr_references_name(left, target) || expr_references_name(right, target)
        }
        Expression::Prefix { right, .. } => expr_references_name(right, target),
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            expr_references_name(function, target)
                || arguments.iter().any(|a| expr_references_name(a, target))
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            expr_references_name(condition, target)
                || consequence
                    .statements
                    .iter()
                    .any(|s| stmt_reference_name(s, target))
                || alternative
                    .as_ref()
                    .is_some_and(|a| a.statements.iter().any(|s| stmt_reference_name(s, target)))
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => expr_references_name(value, target),
        Expression::Cons { head, tail, .. } => {
            expr_references_name(head, target) || expr_references_name(tail, target)
        }
        Expression::TupleLiteral { elements, .. }
        | Expression::ListLiteral { elements, .. }
        | Expression::ArrayLiteral { elements, .. } => {
            elements.iter().any(|e| expr_references_name(e, target))
        }
        Expression::Index { left, index, .. } => {
            expr_references_name(left, target) || expr_references_name(index, target)
        }
        Expression::MemberAccess { object, .. } | Expression::TupleFieldAccess { object, .. } => {
            expr_references_name(object, target)
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            expr_references_name(scrutinee, target)
                || arms
                    .iter()
                    .any(|arm| expr_references_name(&arm.body, target))
        }
        Expression::DoBlock { block, .. } => block
            .statements
            .iter()
            .any(|s| stmt_reference_name(s, target)),
        Expression::Hash { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_references_name(k, target) || expr_references_name(v, target)),
        Expression::InterpolatedString { parts, .. } => parts.iter().any(|p| match p {
            crate::syntax::expression::StringPart::Interpolation(e) => {
                expr_references_name(e, target)
            }
            _ => false,
        }),
        // Don't descend into nested function bodies they create new scopes.
        Expression::Function { .. } => false,
        _ => false,
    }
}

fn stmt_reference_name(stmt: &Statement, target: Symbol) -> bool {
    match stmt {
        Statement::Expression { expression, .. } => expr_references_name(expression, target),
        Statement::Let { value, .. } => expr_references_name(value, target),
        Statement::Return {
            value: Some(expr), ..
        } => expr_references_name(expr, target),
        Statement::Assign { value, .. } => expr_references_name(value, target),
        _ => false,
    }
}

/// Substitute all occurrences of `param` with `arg` in an expression.
/// Simple capture-avoiding substitution for single-argument inlining.
fn substitute_identifier(expr: Expression, param: Symbol, arg: &Expression) -> Expression {
    struct SubstFolder<'a> {
        param: Symbol,
        arg: &'a Expression,
    }

    impl Folder for SubstFolder<'_> {
        fn fold_expr(&mut self, expr: Expression) -> Expression {
            match expr {
                Expression::Identifier { name, .. } if name == self.param => self.arg.clone(),
                // Don't substitute into nested functions that shadow the parameter.
                Expression::Function { ref parameters, .. } if parameters.contains(&self.param) => {
                    expr
                }
                other => fold_expr(self, other),
            }
        }
    }

    let mut folder = SubstFolder { param, arg };
    folder.fold_expr(expr)
}

/// Convert a block to an expression. If the block has a single expression
/// statement, unwrap it; otherwise wrap in a DoBlock.
fn block_to_expr(block: Block, span: Span) -> Expression {
    if block.statements.len() == 1
        && let Statement::Expression {
            expression,
            has_semicolon: false,
            ..
        } = &block.statements[0]
    {
        return expression.clone();
    }
    Expression::DoBlock {
        block,
        span,
        id: ExprId::UNSET,
    }
}

/// Set the span on an expression top-level only.
fn set_expr_span(expr: &mut Expression, span: Span) {
    match expr {
        Expression::Integer { span: s, .. }
        | Expression::Float { span: s, .. }
        | Expression::String { span: s, .. }
        | Expression::Boolean { span: s, .. }
        | Expression::Identifier { span: s, .. }
        | Expression::None { span: s, .. }
        | Expression::Some { span: s, .. }
        | Expression::Left { span: s, .. }
        | Expression::Right { span: s, .. }
        | Expression::Infix { span: s, .. }
        | Expression::Prefix { span: s, .. }
        | Expression::Call { span: s, .. }
        | Expression::If { span: s, .. }
        | Expression::DoBlock { span: s, .. }
        | Expression::Function { span: s, .. }
        | Expression::ListLiteral { span: s, .. }
        | Expression::ArrayLiteral { span: s, .. }
        | Expression::TupleLiteral { span: s, .. }
        | Expression::EmptyList { span: s, .. }
        | Expression::Index { span: s, .. }
        | Expression::Hash { span: s, .. }
        | Expression::MemberAccess { span: s, .. }
        | Expression::TupleFieldAccess { span: s, .. }
        | Expression::Match { span: s, .. }
        | Expression::Cons { span: s, .. }
        | Expression::InterpolatedString { span: s, .. }
        | Expression::Perform { span: s, .. }
        | Expression::Handle { span: s, .. }
        | Expression::NamedConstructor { span: s, .. }
        | Expression::Spread { span: s, .. } => *s = span,
    }
}
