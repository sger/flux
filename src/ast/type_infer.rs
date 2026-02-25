use std::collections::HashMap;

use crate::{
    diagnostics::{
        Diagnostic,
        compiler_errors::{occurs_check_failure, type_unification_error},
        position::Span,
    },
    syntax::{
        Identifier,
        block::Block,
        expression::{Expression, Pattern},
        interner::Interner,
        program::Program,
        statement::Statement,
        type_expr::TypeExpr,
    },
    types::{
        TypeVarId,
        infer_type::InferType,
        scheme::{Scheme, generalize},
        type_constructor::TypeConstructor,
        type_env::TypeEnv,
        type_subst::TypeSubst,
        unify_error::{UnifyErrorKind, unify_with_span},
    },
};

// ─────────────────────────────────────────────────────────────────────────────
// Inference context
// ─────────────────────────────────────────────────────────────────────────────

struct InferCtx<'a> {
    env: TypeEnv,
    interner: &'a Interner,
    errors: Vec<Diagnostic>,
    file_path: String,
    /// Accumulated global substitution — grows monotonically as constraints
    /// are solved.  Apply this to any `Ty` retrieved from the env to obtain
    /// its most-resolved form.
    subst: TypeSubst,
}

impl<'a> InferCtx<'a> {
    fn new(interner: &'a Interner, file_path: String) -> Self {
        InferCtx {
            env: TypeEnv::new(),
            interner,
            errors: Vec::new(),
            file_path,
            subst: TypeSubst::empty(),
        }
    }

    // ── Unification with error recovery ──────────────────────────────────────

    /// Join two types for branch contexts (if/else, match arms).
    ///
    /// Unlike `unify_reporting`, this does NOT add substitution constraints —
    /// it only compares the already-resolved types.  When the resolved types
    /// agree exactly, the common type is returned.  When they differ, `Any` is
    /// returned without modifying the substitution.
    ///
    /// This models Flux's gradual type system where different branches may
    /// legitimately produce values of different types (union falls back to Any).
    fn join_types(&mut self, t1: &InferType, t2: &InferType) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        if t1_sub == t2_sub {
            t1_sub
        } else {
            InferType::Con(TypeConstructor::Any)
        }
    }

    /// Unify `t1` with `t2`, composing the result into `self.subst`.
    ///
    /// On success, returns the resolved first type.
    /// On failure, emits a diagnostic and returns `Any` so that inference can
    /// continue without cascading errors.
    fn unify_reporting(&mut self, t1: &InferType, t2: &InferType, span: Span) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        match unify_with_span(&t1_sub, &t2_sub, span) {
            Ok(s) => {
                // Compose the new solution into the global substitution.
                self.subst = std::mem::take(&mut self.subst).compose(&s);
                t1_sub.apply_type_subst(&self.subst)
            }
            Err(e) => {
                // Only emit a diagnostic when both conflicting types are fully
                // concrete (no unresolved type variables) and neither is `Any`.
                //
                // This prevents false positives in gradual / partially-typed code
                // where a fresh variable from an uninferred base-function call
                // collides with a known type — those conflicts resolve to `Any`
                // once the base-function signature is known.
                let should_emit = e.expected.is_concrete()
                    && e.actual.is_concrete()
                    && !e.expected.is_any()
                    && !e.actual.is_any();

                if should_emit {
                    let file = self.file_path.clone();
                    let diag = match e.kind {
                        UnifyErrorKind::OccursCheck(v) => {
                            let v_str = format!("t{v}");
                            let ty_str = e.actual.to_string();
                            occurs_check_failure(file, span, &v_str, &ty_str)
                        }
                        UnifyErrorKind::Mismatch => {
                            let exp_str = e.expected.to_string();
                            let act_str = e.actual.to_string();
                            type_unification_error(file, span, &exp_str, &act_str)
                        }
                    };
                    self.errors.push(diag);
                }
                InferType::Con(TypeConstructor::Any)
            }
        }
    }

    // ── Program / statement inference ─────────────────────────────────────────

    fn infer_program(&mut self, program: &Program) {
        // Phase A: pre-declare all top-level function names with a fresh type
        // variable so that mutually-recursive functions can reference each other.
        for stmt in &program.statements {
            if let Statement::Function { name, .. } = stmt {
                let v = self.env.fresh_infer_type();
                self.env.bind(*name, Scheme::mono(v));
            }
        }

        // Phase B: infer each top-level statement.
        for stmt in &program.statements {
            self.infer_stmt(stmt);
        }
    }

    fn infer_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Function {
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                body,
                ..
            } => {
                self.infer_fn(
                    *name,
                    type_params,
                    parameters,
                    parameter_types,
                    return_type,
                    body,
                );
            }
            Statement::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                self.infer_let(*name, type_annotation.as_ref(), value);
            }
            Statement::LetDestructure {
                pattern,
                value,
                span,
            } => {
                let val_ty = self.infer_expr(value);
                self.bind_pattern(pattern, &val_ty, *span);
            }
            Statement::Expression { expression, .. } => {
                // Evaluate for side effects; type is discarded.
                self.infer_expr(expression);
            }
            Statement::Assign { value, .. } => {
                self.infer_expr(value);
            }
            Statement::Module { body, .. } => {
                self.env.enter_scope();
                for s in &body.statements {
                    self.infer_stmt(s);
                }
                self.env.leave_scope();
            }
            // Import, Data, Return at top-level: no HM inference needed.
            _ => {}
        }
    }

    // ── Function inference ────────────────────────────────────────────────────

    fn infer_fn(
        &mut self,
        name: Identifier,
        type_params: &[Identifier],
        parameters: &[Identifier],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        body: &Block,
    ) {
        // Map explicit type parameters (e.g. `T`, `U`) to fresh type variables.
        let tp_map: HashMap<Identifier, TypeVarId> =
            type_params.iter().map(|s| (*s, self.env.fresh())).collect();

        self.env.enter_scope();

        // Bind each parameter to its annotated type (or a fresh variable).
        let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
        for (i, &param) in parameters.iter().enumerate() {
            let ty = parameter_types
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|te| TypeEnv::infer_type_from_type_expr(te, &tp_map, self.interner))
                .unwrap_or_else(|| self.env.fresh_infer_type());
            param_tys.push(ty.clone());
            self.env.bind(param, Scheme::mono(ty));
        }

        // Infer the body type.
        let body_ty = self.infer_block(body);

        // Unify the body type with the declared return type, if present.
        let ret_ty = match return_type {
            Some(ret_ann) => {
                match TypeEnv::infer_type_from_type_expr(ret_ann, &tp_map, self.interner) {
                    Some(ann_ty) => self.unify_reporting(&body_ty, &ann_ty, ret_ann.span()),
                    None => body_ty.apply_type_subst(&self.subst),
                }
            }
            None => body_ty.apply_type_subst(&self.subst),
        };

        // Resolve parameter types through the accumulated substitution.
        let final_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|t| t.apply_type_subst(&self.subst))
            .collect();
        let fn_ty = InferType::Fun(final_param_tys, Box::new(ret_ty));

        self.env.leave_scope();

        // Generalize: quantify over type variables that are free in `fn_ty`
        // but not in the surrounding environment (the let-generalization step).
        // We only generalize functions with explicit type parameters — for
        // implicitly typed functions, we keep the monomorphic type so that
        // unification constraints across call sites are preserved.
        let env_free = self.env.free_vars();
        let scheme = if !type_params.is_empty() {
            generalize(&fn_ty, &env_free)
        } else {
            Scheme::mono(fn_ty)
        };

        // Update the pre-declared entry (from Phase A).
        self.env.bind(name, scheme);
    }

    fn infer_let(&mut self, name: Identifier, annotation: Option<&TypeExpr>, value: &Expression) {
        let val_ty = self.infer_expr(value);

        let final_ty = match annotation {
            Some(ann) => {
                match TypeEnv::infer_type_from_type_expr(ann, &HashMap::new(), self.interner) {
                    Some(ann_ty) => self.unify_reporting(&val_ty, &ann_ty, ann.span()),
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

    // ── Block inference ───────────────────────────────────────────────────────

    /// Infer the type of a block: the type of the last value-producing
    /// expression (i.e., the last statement without a trailing semicolon).
    /// Returns `Unit` if there is no such expression.
    fn infer_block(&mut self, block: &Block) -> InferType {
        let mut last_ty = InferType::Con(TypeConstructor::Unit);
        for stmt in &block.statements {
            match stmt {
                // The last no-semicolon expression is the block's value.
                Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                } => {
                    last_ty = self.infer_expr(expression);
                }
                // An explicit `return expr` also gives the block's type.
                Statement::Return {
                    value: Some(expr), ..
                } => {
                    last_ty = self.infer_expr(expr);
                }
                _ => {
                    self.infer_stmt(stmt);
                    last_ty = InferType::Con(TypeConstructor::Unit);
                }
            }
        }
        last_ty
    }

    // ── Expression inference ──────────────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expression) -> InferType {
        match expr {
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
                        // In Flux's gradual type system branches may legitimately
                        // return different types — the result is `Any`.  No E300.
                        self.join_types(&then_ty, &else_ty)
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
                self.infer_expr(scrutinee);
                if arms.is_empty() {
                    return InferType::Con(TypeConstructor::Any);
                }

                // Infer the first arm.
                self.env.enter_scope();
                self.bind_pattern(
                    &arms[0].pattern,
                    &InferType::Con(TypeConstructor::Any),
                    *span,
                );
                if let Some(guard) = &arms[0].guard {
                    self.infer_expr(guard);
                }
                let first_ty = self.infer_expr(&arms[0].body);
                self.env.leave_scope();

                // Unify remaining arms against the first.
                let mut result_ty = first_ty;
                for arm in arms.iter().skip(1) {
                    self.env.enter_scope();
                    self.bind_pattern(&arm.pattern, &InferType::Con(TypeConstructor::Any), *span);
                    if let Some(guard) = &arm.guard {
                        self.infer_expr(guard);
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.env.leave_scope();
                    // Same gradual-typing rationale as if/else: arms may differ.
                    result_ty = self.join_types(&result_ty, &arm_ty);
                }
                result_ty
            }

            // ── Lambda ────────────────────────────────────────────────────────
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                body,
                ..
            } => {
                self.env.enter_scope();

                let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
                for (i, &param) in parameters.iter().enumerate() {
                    let ty = parameter_types
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .and_then(|te| {
                            TypeEnv::infer_type_from_type_expr(te, &HashMap::new(), self.interner)
                        })
                        .unwrap_or_else(|| self.env.fresh_infer_type());
                    param_tys.push(ty.clone());
                    self.env.bind(param, Scheme::mono(ty));
                }

                let body_ty = self.infer_block(body);
                let ret_ty = match return_type {
                    Some(ret_ann) => {
                        match TypeEnv::infer_type_from_type_expr(
                            ret_ann,
                            &HashMap::new(),
                            self.interner,
                        ) {
                            Some(ann_ty) => self.unify_reporting(&body_ty, &ann_ty, ret_ann.span()),
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

                InferType::Fun(final_param_tys, Box::new(ret_ty))
            }

            // ── Function call ─────────────────────────────────────────────────
            Expression::Call {
                function,
                arguments,
                span,
            } => self.infer_call(function, arguments, *span),

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

            // ── Member / index access: return Any (gradual) ───────────────────
            Expression::Index { left, index, .. } => {
                self.infer_expr(left);
                self.infer_expr(index);
                InferType::Con(TypeConstructor::Any)
            }
            Expression::MemberAccess { object, .. } => {
                self.infer_expr(object);
                InferType::Con(TypeConstructor::Any)
            }
            Expression::TupleFieldAccess { object, .. } => {
                self.infer_expr(object);
                InferType::Con(TypeConstructor::Any)
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.infer_expr(arg);
                }
                InferType::Con(TypeConstructor::Any)
            }
            Expression::Handle { expr, arms, .. } => {
                self.infer_expr(expr);
                for arm in arms {
                    self.infer_expr(&arm.body);
                }
                InferType::Con(TypeConstructor::Any)
            }
        }
    }

    // ── Infix operator typing ─────────────────────────────────────────────────

    fn infer_infix(
        &mut self,
        left: &Expression,
        op: &str,
        right: &Expression,
        span: Span,
    ) -> InferType {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);
        match op {
            // Arithmetic — Flux allows mixed Int/Float arithmetic (e.g. `1 + 2.5`).
            // We do NOT require operand agreement; instead we propagate the concrete
            // type when both sides agree, and fall back to Any for mixed or unknown.
            "+" | "-" | "*" | "/" | "%" => {
                let lt_r = lt.apply_type_subst(&self.subst);
                let rt_r = rt.apply_type_subst(&self.subst);
                if lt_r == rt_r {
                    lt_r
                } else {
                    InferType::Con(TypeConstructor::Any)
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

    fn infer_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let fn_ty = self.infer_expr(function);

        // Infer argument types left-to-right.
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expr(a)).collect();

        // Build the function type we expect and unify with the callee's type.
        // A fresh return-type variable is solved by unification.
        let ret_var = self.env.fresh_infer_type();
        let expected_fn_ty = InferType::Fun(arg_tys, Box::new(ret_var.clone()));
        self.unify_reporting(&fn_ty, &expected_fn_ty, span);

        // The return variable is now resolved (or remains free → Any at use).
        ret_var.apply_type_subst(&self.subst)
    }

    // ── Pattern variable binding ──────────────────────────────────────────────

    /// Conservatively bind all variables introduced by a pattern to fresh
    /// type variables.  We do not propagate scrutinee type information
    /// downward into patterns at this stage.
    fn bind_pattern(&mut self, pattern: &Pattern, _scrutinee_ty: &InferType, span: Span) {
        match pattern {
            Pattern::Identifier { name, .. } => {
                let v = self.env.fresh_infer_type();
                self.env.bind(*name, Scheme::mono(v));
            }
            Pattern::Wildcard { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. }
            | Pattern::Literal { .. } => {
                // Nothing to bind.
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => {
                self.bind_pattern(pattern, &InferType::Con(TypeConstructor::Any), span);
            }
            Pattern::Cons { head, tail, .. } => {
                self.bind_pattern(head, &InferType::Con(TypeConstructor::Any), span);
                self.bind_pattern(tail, &InferType::Con(TypeConstructor::Any), span);
            }
            Pattern::Tuple { elements, .. } => {
                for elem in elements {
                    self.bind_pattern(elem, &InferType::Con(TypeConstructor::Any), span);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for field in fields {
                    self.bind_pattern(field, &InferType::Con(TypeConstructor::Any), span);
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Run Algorithm W (Hindley-Milner) over the entire program.
///
/// Returns the resulting `TypeEnv` (can be queried for any identifier's
/// inferred scheme) and a list of type-error diagnostics.
///
/// Type errors are **non-fatal**: inference always completes, recovering with
/// `Any` when unification fails.  The compiler can then use the env to enrich
/// its own static type information without gating on type errors.
pub fn infer_program(
    program: &Program,
    interner: &Interner,
    file_path: Option<String>,
) -> (TypeEnv, Vec<Diagnostic>) {
    let file = file_path.unwrap_or_default();
    let mut ctx = InferCtx::new(interner, file);
    ctx.infer_program(program);
    (ctx.env, ctx.errors)
}
