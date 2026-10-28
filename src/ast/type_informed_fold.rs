use std::collections::{HashMap, HashSet};

use crate::shared::opt_bisect;
use crate::{
    ast::{
        Folder, fold_expr,
        shift_expr_ids::fresh_expr_ids,
        visit::{Visitor, walk_expr, walk_pat, walk_stmt},
    },
    diagnostics::position::Span,
    syntax::{
        block::Block,
        effect_expr::EffectExpr,
        expression::Pattern,
        expression::{ExprId, ExprIdGen, Expression},
        interner::Interner,
        program::Program,
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
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
/// - **T1**: Pure function inlining (zero-arg and single-arg, fully annotated)
/// - **T2**: Constant propagation for typed `let` bindings
/// - **T3**: Dead branch elimination on statically-known boolean conditions
struct TypeInformedFoldCtx<'a> {
    type_env: &'a TypeEnv,
    /// Top-level function bodies eligible for inlining (zero-arg, pure, single-expression).
    fn_bodies: HashMap<Symbol, InlinebleBody>,
    /// Ids for the copies inlining makes, past every id in the program.
    ids: ExprIdGen,
    /// Typed literal bindings eligible for constant propagation.
    let_literals: HashMap<Symbol, Expression>,
}

#[derive(Clone)]
struct InlinebleBody {
    /// Each parameter with its annotation.
    params: Vec<(Symbol, TypeExpr)>,
    return_type: TypeExpr,
    effects: Vec<EffectExpr>,
    body_expr: Expression,
}

impl<'a> TypeInformedFoldCtx<'a> {
    fn new(type_env: &'a TypeEnv, ids: ExprIdGen) -> Self {
        Self { type_env, fn_bodies: HashMap::new(), ids, let_literals: HashMap::new() }
    }

    /// Collect top-level functions eligible for inlining.
    ///
    /// A function is inlinable when:
    /// 1. It has a monomorphic scheme in TypeEnv (forall is empty)
    /// 2. Its body is a single expression (no early returns, no do-blocks)
    /// 3. Its inferred effect row is closed-empty (pure)
    /// 4. It is not recursive (its body does not reference its own name)
    /// 5. Every parameter and the result are annotated, and every parameter
    ///    is used
    ///
    /// The annotations are kept on the inlined copy (see
    /// [`Self::try_inline_call`]). Without them, inlining changed what the
    /// program means to inference: an unannotated function typed only by its
    /// callers (`fn fst(p) { p.0 }`) was left with no caller to type it, and
    /// a result annotation that chose an instance (`-> Int` over
    /// `parse(s)`) was dropped.
    fn collect_inlinable_functions(&mut self, program: &Program) {
        // A call is matched to the function by name alone, with no scope in
        // hand, so a name bound locally anywhere could be that local instead.
        let local_names = local_binder_names(program);
        for stmt in &program.statements {
            let Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type: Some(return_type),
                effects,
                body,
                ..
            } = stmt
            else {
                continue;
            };
            if local_names.contains(name) {
                continue;
            }
            let Some(params) = parameters
                .iter()
                .zip(parameter_types)
                .map(|(name, ty)| ty.clone().map(|ty| (*name, ty)))
                .collect::<Option<Vec<_>>>()
                .filter(|params| params.len() == parameters.len())
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
            // A handler or a `perform` depends on the effects of where it
            // runs, and a copy runs somewhere else: `run()`'s `handle Logger
            // { print(..) }` moved into a `with IO` caller stopped discharging
            // `Logger` there.
            if handles_effects(body_expr) {
                continue;
            }
            // An unused parameter would become an unused lambda parameter.
            if !parameters.iter().all(|param| expr_references_name(body_expr, *param)) {
                continue;
            }

            self.fn_bodies.insert(
                *name,
                InlinebleBody {
                    params,
                    return_type: return_type.clone(),
                    effects: effects.clone(),
                    body_expr: body_expr.clone(),
                },
            );
        }
    }

    /// Collect typed `let` bindings with literal values for constant propagation.
    ///
    /// A name some binder below the top level reuses is left out: the
    /// substitution replaces every identifier of that name, and inside
    /// `fn f(x)` the `x` is the parameter, not the top-level `let x`.
    fn collect_let_literals(&mut self, program: &Program) {
        let shadowed = local_binder_names(program);
        for stmt in &program.statements {
            let Statement::Let { name, value, .. } = stmt else {
                continue;
            };
            if shadowed.contains(name) {
                continue;
            }

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
    ///
    /// `f(arg)` becomes `(fn(p: P) -> R { <body> })(arg)`: the argument is
    /// evaluated once, under the parameter's annotation, and the result is
    /// checked against the declared return type, so inference sees what it
    /// saw at the call. A `do` block would not do: it is no scope of its own,
    /// so its `let`s collide with the caller's names. Core's beta reduction
    /// removes the redex.
    ///
    /// A zero-argument function whose body is a literal is replaced by the
    /// literal itself, which is what lets dead-branch elimination (T3) see
    /// it. The body is a copy, so it gets fresh ids.
    fn try_inline_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> Option<Expression> {
        let Expression::Identifier { name, .. } = function else {
            return None;
        };

        let body = self.fn_bodies.get(name)?.clone();

        // Arity must match.
        if arguments.len() != body.params.len() {
            return None;
        }

        let mut copy = fresh_expr_ids(body.body_expr, &mut self.ids);
        if body.params.is_empty() && is_simple_literal(&copy) {
            set_expr_span(&mut copy, span);
            return Some(copy);
        }
        let (parameters, parameter_types) =
            body.params.into_iter().map(|(param, ty)| (param, Some(ty))).unzip();
        let lambda = Expression::Function {
            parameters,
            parameter_types,
            return_type: Some(body.return_type),
            effects: body.effects,
            body: Block {
                statements: vec![Statement::Expression {
                    expression: copy,
                    has_semicolon: false,
                    span,
                }],
                span,
            },
            span,
            id: self.ids.next_id(),
        };
        Some(Expression::Call {
            function: Box::new(lambda),
            arguments: arguments.to_vec(),
            span,
            id: self.ids.next_id(),
        })
    }
}

impl Folder for TypeInformedFoldCtx<'_> {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up).
        let expr = fold_expr(self, expr);

        match expr {
            // T3: Dead branch elimination remove branches on literal booleans.
            Expression::If { ref condition, consequence, alternative, span, id } => {
                match condition.as_ref() {
                    Expression::Boolean { value: true, .. }
                        if opt_bisect::allow("type_informed_fold", || {
                            "drop the else of `if true`".to_string()
                        }) =>
                    {
                        block_to_expr(consequence, span)
                    }
                    Expression::Boolean { value: false, .. }
                        if opt_bisect::allow("type_informed_fold", || {
                            "drop the then of `if false`".to_string()
                        }) =>
                    {
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
                }
            }

            // T2: Constant propagation replace known-literal identifiers.
            Expression::Identifier { name, span, id } => {
                if let Some(literal) = self.let_literals.get(&name)
                    && opt_bisect::allow("type_informed_fold", || {
                        "replace a name with its literal".to_string()
                    })
                {
                    let mut replaced = fresh_expr_ids(literal.clone(), &mut self.ids);
                    set_expr_span(&mut replaced, span);
                    replaced
                } else {
                    Expression::Identifier { name, span, id }
                }
            }

            // T1: Pure function inlining.
            Expression::Call { ref function, ref arguments, span, id } => {
                if let Some(inlined) = self.try_inline_call(function, arguments, span)
                    && opt_bisect::allow("type_informed_fold", || "inline a pure call".to_string())
                {
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

/// Whether `expr` handles or performs an effect anywhere inside it.
fn handles_effects(expr: &Expression) -> bool {
    struct Finder {
        found: bool,
    }

    impl<'ast> Visitor<'ast> for Finder {
        fn visit_expr(&mut self, expr: &'ast Expression) {
            if matches!(expr, Expression::Handle { .. } | Expression::Perform { .. }) {
                self.found = true;
                return;
            }
            walk_expr(self, expr);
        }
    }

    let mut finder = Finder { found: false };
    finder.visit_expr(expr);
    finder.found
}

/// Every name the program binds below the top level: `let` names, pattern
/// variables, function and lambda parameters, and nested function names.
fn local_binder_names(program: &Program) -> HashSet<Symbol> {
    struct Binders {
        names: HashSet<Symbol>,
        depth: usize,
    }

    impl<'ast> Visitor<'ast> for Binders {
        fn visit_stmt(&mut self, stmt: &'ast Statement) {
            match stmt {
                Statement::Let { name, .. } if self.depth > 0 => {
                    self.names.insert(*name);
                }
                Statement::Function { name, parameters, .. } => {
                    if self.depth > 0 {
                        self.names.insert(*name);
                    }
                    self.names.extend(parameters.iter().copied());
                    self.depth += 1;
                    walk_stmt(self, stmt);
                    self.depth -= 1;
                    return;
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }

        fn visit_expr(&mut self, expr: &'ast Expression) {
            if let Expression::Function { parameters, .. } = expr {
                self.names.extend(parameters.iter().copied());
            }
            walk_expr(self, expr);
        }

        fn visit_pat(&mut self, pat: &'ast Pattern) {
            if let Pattern::Identifier { name, .. } = pat {
                self.names.insert(*name);
            }
            walk_pat(self, pat);
        }
    }

    let mut binders = Binders { names: HashSet::new(), depth: 0 };
    binders.visit_program(program);
    binders.names
}

/// Apply type-informed optimization to a program.
///
/// Requires a `TypeEnv` from a prior Phase 1 inference run. The returned
/// program must be re-inferred (Phase 2) before codegen because pointer-keyed
/// expression IDs are invalidated by AST rewrites.
pub fn type_informed_fold(program: &Program, type_env: &TypeEnv, _interner: &Interner) -> Program {
    // Its rewrites belong to no Core definition (0190 §13).
    opt_bisect::set_def(String::new);
    let mut ctx = TypeInformedFoldCtx::new(type_env, ExprIdGen::resuming_past_program(program));
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
        Statement::Expression { expression, has_semicolon: false, .. } => Some(expression),
        Statement::Return { value: Some(expr), .. } => Some(expr),
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
        Expression::Call { function, arguments, .. } => {
            expr_references_name(function, target)
                || arguments.iter().any(|a| expr_references_name(a, target))
        }
        Expression::If { condition, consequence, alternative, .. } => {
            expr_references_name(condition, target)
                || consequence.statements.iter().any(|s| stmt_reference_name(s, target))
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
        Expression::Sealing { expr, .. } => expr_references_name(expr, target),
        Expression::Match { scrutinee, arms, .. } => {
            expr_references_name(scrutinee, target)
                || arms.iter().any(|arm| expr_references_name(&arm.body, target))
        }
        Expression::DoBlock { block, .. } => {
            block.statements.iter().any(|s| stmt_reference_name(s, target))
        }
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
        Statement::Return { value: Some(expr), .. } => expr_references_name(expr, target),
        Statement::Assign { value, .. } => expr_references_name(value, target),
        _ => false,
    }
}

/// Convert a block to an expression. If the block has a single expression
/// statement, unwrap it; otherwise wrap in a DoBlock.
fn block_to_expr(block: Block, span: Span) -> Expression {
    if block.statements.len() == 1
        && let Statement::Expression { expression, has_semicolon: false, .. } = &block.statements[0]
    {
        return expression.clone();
    }
    Expression::DoBlock { block, span, id: ExprId::UNSET }
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
        | Expression::Sealing { span: s, .. }
        | Expression::NamedConstructor { span: s, .. }
        | Expression::Spread { span: s, .. } => *s = span,
    }
}
