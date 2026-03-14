/// Direct AST → Core IR lowering (runs immediately after HM type inference).
///
/// This is the primary Core IR entry point. By operating directly on the typed
/// AST rather than the intermediate `IrTopLevelItem` representation, it:
///
/// 1. Puts Core IR in the right pipeline position:
///    `HM inference → Core IR → passes → backend lowering`
///
/// 2. Gives every binder access to its HM-inferred type (via `hm_expr_types`),
///    enabling type-driven passes such as:
///    - Emitting `IAdd`/`IDiv` instead of generic `Add`/`Div`
///    - Deciding which values need reference-count operations
///    - Detecting integer vs float arithmetic
///
/// All surface Flux constructs (sugar, n-ary functions, multi-argument calls,
/// pattern matching, effects) are desugared into the ~12-variant `CoreExpr`.
use std::collections::HashMap;

use crate::{
    diagnostics::position::Span,
    syntax::{
        block::Block,
        expression::{ExprId, Expression, HandleArm, MatchArm, Pattern, StringPart},
        program::Program,
        statement::Statement,
    },
    types::{infer_type::InferType, type_constructor::TypeConstructor},
};

use super::{
    CoreAlt, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreProgram, CoreTag,
};

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a typed Flux `Program` into a `CoreProgram`.
///
/// `hm_expr_types` maps every `ExprId` (assigned by the parser) to its
/// HM-inferred type. The lowering uses this to guide type-directed decisions.
pub fn lower_program_ast(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
) -> CoreProgram {
    let mut lowerer = AstLowerer::new(hm_expr_types);
    let mut defs = Vec::new();
    for stmt in &program.statements {
        lowerer.lower_top_level(stmt, &mut defs);
    }
    CoreProgram { defs }
}

// ── Lowerer ───────────────────────────────────────────────────────────────────

struct AstLowerer<'a> {
    /// HM-inferred types keyed by ExprId — used for type-directed code selection.
    hm_expr_types: &'a HashMap<ExprId, InferType>,
    /// Counter for synthesizing fresh binding names.
    fresh: u32,
}

impl<'a> AstLowerer<'a> {
    fn new(hm_expr_types: &'a HashMap<ExprId, InferType>) -> Self {
        Self { hm_expr_types, fresh: 0 }
    }

    /// Allocate a fresh synthetic `Identifier` for compiler-generated bindings.
    /// Used by future passes that need to introduce new named temporaries.
    #[allow(dead_code)]
    fn fresh_name(&mut self, interner: &mut crate::syntax::interner::Interner) -> crate::syntax::Identifier {
        let id = self.fresh;
        self.fresh += 1;
        interner.intern(&format!("$tmp{id}"))
    }

    /// Look up the HM-inferred type for an expression, if available.
    /// Intended for type-driven passes (e.g. emitting `IAdd` vs `Add`).
    #[allow(dead_code)]
    pub fn expr_type(&self, id: ExprId) -> Option<&InferType> {
        self.hm_expr_types.get(&id)
    }

    // ── Top-level statements ─────────────────────────────────────────────────

    fn lower_top_level(&mut self, stmt: &Statement, out: &mut Vec<CoreDef>) {
        match stmt {
            // Named function definition → CoreDef wrapping a curried Lam.
            Statement::Function { name, parameters, body, span, .. } => {
                let body_expr = self.lower_block(body);
                // Always wrap in Lam, even for parameterless functions — the
                // Core→IR lowerer uses the Lam marker to distinguish function
                // definitions from value bindings.  We construct Lam directly
                // instead of using CoreExpr::lambda() which elides empty params.
                let expr = CoreExpr::Lam {
                    params: parameters.clone(),
                    body: Box::new(body_expr),
                    span: *span,
                };
                out.push(CoreDef {
                    name: *name,
                    expr,
                    is_recursive: true, // functions may reference themselves
                    span: *span,
                });
            }

            // Value binding → CoreDef for the RHS expression.
            Statement::Let { name, value, span, .. } => {
                out.push(CoreDef {
                    name: *name,
                    expr: self.lower_expr(value),
                    is_recursive: false,
                    span: *span,
                });
            }

            // Assignment (mutable rebind) → treat as a new CoreDef.
            Statement::Assign { name, value, span } => {
                out.push(CoreDef {
                    name: *name,
                    expr: self.lower_expr(value),
                    is_recursive: false,
                    span: *span,
                });
            }

            // Expression statement → anonymous CoreDef (evaluated for effects).
            Statement::Expression { expression, span, .. } => {
                out.push(CoreDef {
                    name: crate::syntax::symbol::Symbol::new(0), // anonymous sentinel
                    expr: self.lower_expr(expression),
                    is_recursive: false,
                    span: *span,
                });
            }

            // Destructuring let → synthetic tmp var + Case alt for the pattern.
            Statement::LetDestructure { pattern, value, span } => {
                let rhs = self.lower_expr(value);
                let core_pat = lower_pattern(pattern);
                // Wrap: let $destructure = value in case $destructure of { pat -> () }
                // At the top level we emit this as one def per bound variable by
                // expanding the pattern into field accesses where possible.
                self.expand_destructure_top_level(core_pat, rhs, *span, out);
            }

            // Return at top level is unusual but syntactically valid in some contexts.
            Statement::Return { value, span } => {
                if let Some(val) = value {
                    out.push(CoreDef {
                        name: crate::syntax::symbol::Symbol::new(0),
                        expr: self.lower_expr(val),
                        is_recursive: false,
                            span: *span,
                    });
                }
            }

            // Declarations that don't produce runtime values — skip.
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::Module { .. } => {}
        }
    }

    // ── Block lowering ───────────────────────────────────────────────────────

    /// Lower a `Block` (sequence of statements) into a single `CoreExpr`.
    ///
    /// Each statement is desugared:
    /// - `let x = e; rest` → `Let(x, e, rest)`
    /// - `fn f(p) { b }; rest` → `LetRec(f, Lam(p, b), rest)`
    /// - `e; rest` → `Let($_, e, rest)`   (sequence — result is discarded)
    /// - last expression → the block's return value
    pub fn lower_block(&mut self, block: &Block) -> CoreExpr {
        self.lower_stmts(&block.statements, block.span)
    }

    fn lower_stmts(&mut self, stmts: &[Statement], span: Span) -> CoreExpr {
        // Find where the "value" of the block comes from.
        // It's the last statement if it's a non-semicolon expression,
        // otherwise the block returns unit.
        match stmts {
            [] => CoreExpr::Lit(CoreLit::Unit, span),

            [single] => self.lower_stmt_as_expr(single, span),

            [rest @ .., last] => {
                let tail = self.lower_stmts(std::slice::from_ref(last), span);
                self.prepend_stmt(&rest[rest.len() - 1..], rest, tail, span)
            }
        }
    }

    /// Lower the full statement slice by prepending one statement at a time.
    fn prepend_stmts(&mut self, stmts: &[Statement], body: CoreExpr, span: Span) -> CoreExpr {
        stmts.iter().rev().fold(body, |acc, stmt| {
            self.prepend_one_stmt(stmt, acc, span)
        })
    }

    fn prepend_stmt(&mut self, _one: &[Statement], all_but_last: &[Statement], tail: CoreExpr, span: Span) -> CoreExpr {
        self.prepend_stmts(all_but_last, tail, span)
    }

    /// Wrap `tail` with the binding/effect introduced by a single statement.
    fn prepend_one_stmt(&mut self, stmt: &Statement, tail: CoreExpr, _span: Span) -> CoreExpr {
        match stmt {
            Statement::Let { name, value, span: s, .. } => CoreExpr::Let {
                var: *name,
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::Function { name, parameters, body, span: s, .. } => {
                let fn_body = self.lower_block(body);
                let fn_expr = if parameters.is_empty() {
                    fn_body
                } else {
                    CoreExpr::lambda(parameters.clone(), fn_body, *s)
                };
                CoreExpr::LetRec {
                    var: *name,
                    rhs: Box::new(fn_expr),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Assign { name, value, span: s } => CoreExpr::Let {
                var: *name,
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::LetDestructure { pattern, value, span: s } => {
                // Bind the scrutinee to a tmp var and use Case to extract fields.
                let rhs = self.lower_expr(value);
                let core_pat = lower_pattern(pattern);
                // Build: Let($tmp, rhs, Case($tmp, [core_pat → tail]))
                // We use a special sentinel name for the tmp var.
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 1_000_000);
                self.fresh += 1;
                let tmp_var = CoreExpr::Var(tmp_name, *s);
                let alt = CoreAlt { pat: core_pat, guard: None, rhs: tail, span: *s };
                CoreExpr::Let {
                    var: tmp_name,
                    rhs: Box::new(rhs),
                    body: Box::new(CoreExpr::Case {
                        scrutinee: Box::new(tmp_var),
                        alts: vec![alt],
                        span: *s,
                    }),
                    span: *s,
                }
            }

            // Expression statement: evaluate for its effect, discard result.
            Statement::Expression { expression, has_semicolon: true, span: s } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: tmp_name,
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            // Expression without semicolon in a non-last position is unusual.
            // Treat as sequencing (result discarded).
            Statement::Expression { expression, has_semicolon: false, span: s } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: tmp_name,
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Return { value, span: s } => {
                // Early return — the tail is dead code, but we still need a well-typed
                // expression. Emit the return value and discard the tail.
                match value {
                    Some(v) => self.lower_expr(v),
                    None => CoreExpr::Lit(CoreLit::Unit, *s),
                }
            }

            // Declarations don't contribute to block value.
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::Module { .. } => tail,
        }
    }

    /// Lower the last (or only) statement in a block as an expression.
    fn lower_stmt_as_expr(&mut self, stmt: &Statement, span: Span) -> CoreExpr {
        match stmt {
            Statement::Expression { expression, has_semicolon: false, .. } => {
                self.lower_expr(expression)
            }
            // Semicolon-terminated expression discards its value — evaluate
            // for effect, then return unit.
            Statement::Expression { expression, has_semicolon: true, span: s } => {
                let tmp = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: tmp,
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(CoreExpr::Lit(CoreLit::Unit, *s)),
                    span: *s,
                }
            }
            Statement::Return { value, span: s } => match value {
                Some(v) => self.lower_expr(v),
                None => CoreExpr::Lit(CoreLit::Unit, *s),
            },
            // A `let` or `fn` as the last statement returns unit.
            other => {
                let unit = CoreExpr::Lit(CoreLit::Unit, span);
                self.prepend_one_stmt(other, unit, span)
            }
        }
    }

    // ── Expression lowering ──────────────────────────────────────────────────

    pub fn lower_expr(&mut self, expr: &Expression) -> CoreExpr {
        match expr {
            Expression::Identifier { name, span, .. } => {
                CoreExpr::Var(*name, *span)
            }

            Expression::Integer { value, span, .. } => {
                CoreExpr::Lit(CoreLit::Int(*value), *span)
            }

            Expression::Float { value, span, .. } => {
                CoreExpr::Lit(CoreLit::Float(*value), *span)
            }

            Expression::String { value, span, .. } => {
                CoreExpr::Lit(CoreLit::String(value.clone()), *span)
            }

            Expression::Boolean { value, span, .. } => {
                CoreExpr::Lit(CoreLit::Bool(*value), *span)
            }

            Expression::InterpolatedString { parts, span, .. } => {
                let args: Vec<CoreExpr> = parts.iter().map(|p| match p {
                    StringPart::Literal(s) => CoreExpr::Lit(CoreLit::String(s.clone()), *span),
                    StringPart::Interpolation(e) => self.lower_expr(e),
                }).collect();
                CoreExpr::PrimOp { op: CorePrimOp::Interpolate, args, span: *span }
            }

            Expression::Prefix { operator, right, span, .. } => {
                let arg = self.lower_expr(right);
                let op = match operator.as_str() {
                    "-" => CorePrimOp::Neg,
                    "!" => CorePrimOp::Not,
                    _ => CorePrimOp::Neg, // fallback
                };
                CoreExpr::PrimOp { op, args: vec![arg], span: *span }
            }

            Expression::Infix { left, operator, right, span, id } => {
                self.lower_infix(left, operator, right, *span, *id)
            }

            Expression::If { condition, consequence, alternative, span, .. } => {
                let cond = self.lower_expr(condition);
                let true_branch = self.lower_block(consequence);
                let false_branch = alternative
                    .as_ref()
                    .map(|b| self.lower_block(b))
                    .unwrap_or(CoreExpr::Lit(CoreLit::Unit, *span));

                CoreExpr::Case {
                    scrutinee: Box::new(cond),
                    alts: vec![
                        CoreAlt {
                            pat: CorePat::Lit(CoreLit::Bool(true)),
                            guard: None,
                            rhs: true_branch,
                            span: *span,
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: false_branch,
                            span: *span,
                        },
                    ],
                    span: *span,
                }
            }

            Expression::DoBlock { block, span, .. } => {
                // DoBlock is a sequencing block — lower it as a regular block.
                let inner = self.lower_block(block);
                // Preserve the span wrapper.
                match inner {
                    CoreExpr::Let { .. } | CoreExpr::LetRec { .. } | CoreExpr::Case { .. } => inner,
                    other => CoreExpr::Let {
                        var: crate::syntax::symbol::Symbol::new(3_000_000 + self.fresh),
                        rhs: Box::new(CoreExpr::Lit(CoreLit::Unit, *span)),
                        body: Box::new(other),
                        span: *span,
                    },
                }
            }

            Expression::Function { parameters, body, span, .. } => {
                let body_expr = self.lower_block(body);
                if parameters.is_empty() {
                    // Nullary lambda — keep the Lam wrapper so the Core→IR
                    // lowerer recognises it as a closure, but with empty params
                    // so the resulting IR function has arity 0.
                    CoreExpr::Lam {
                        params: vec![],
                        body: Box::new(body_expr),
                        span: *span,
                    }
                } else {
                    CoreExpr::lambda(parameters.clone(), body_expr, *span)
                }
            }

            Expression::Call { function, arguments, span, .. } => {
                let func = self.lower_expr(function);
                let args: Vec<CoreExpr> = arguments.iter().map(|a| self.lower_expr(a)).collect();
                // Always emit App, even for zero-arg calls — Flux functions
                // must be invoked explicitly (they can have side effects).
                CoreExpr::App { func: Box::new(func), args, span: *span }
            }

            Expression::ListLiteral { elements, span, .. } => {
                // [a, b, c] → PrimOp(MakeList, [a, b, c])
                let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                CoreExpr::PrimOp { op: CorePrimOp::MakeList, args, span: *span }
            }

            Expression::ArrayLiteral { elements, span, .. } => {
                let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                CoreExpr::PrimOp { op: CorePrimOp::MakeArray, args, span: *span }
            }

            Expression::TupleLiteral { elements, span, .. } => {
                if elements.is_empty() {
                    // `()` is Unit, not a zero-element tuple.
                    CoreExpr::Lit(CoreLit::Unit, *span)
                } else {
                    let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                    CoreExpr::PrimOp { op: CorePrimOp::MakeTuple, args, span: *span }
                }
            }

            Expression::EmptyList { span, .. } => {
                CoreExpr::Con { tag: CoreTag::Nil, fields: Vec::new(), span: *span }
            }

            Expression::Hash { pairs, span, .. } => {
                // Flatten pairs: [k1, v1, k2, v2, ...] for MakeHash.
                let args: Vec<CoreExpr> = pairs.iter().flat_map(|(k, v)| {
                    [self.lower_expr(k), self.lower_expr(v)]
                }).collect();
                CoreExpr::PrimOp { op: CorePrimOp::MakeHash, args, span: *span }
            }

            Expression::Index { left, index, span, .. } => {
                let l = self.lower_expr(left);
                let i = self.lower_expr(index);
                CoreExpr::PrimOp { op: CorePrimOp::Index, args: vec![l, i], span: *span }
            }

            Expression::MemberAccess { object, member, span, .. } => {
                let obj = self.lower_expr(object);
                CoreExpr::PrimOp {
                    op: CorePrimOp::MemberAccess(*member),
                    args: vec![obj],
                    span: *span,
                }
            }

            Expression::TupleFieldAccess { object, index, span, .. } => {
                let obj = self.lower_expr(object);
                CoreExpr::PrimOp {
                    op: CorePrimOp::TupleField(*index),
                    args: vec![obj],
                    span: *span,
                }
            }

            Expression::Match { scrutinee, arms, span, .. } => {
                let scrut = self.lower_expr(scrutinee);
                let alts: Vec<CoreAlt> = arms.iter().map(|arm| self.lower_match_arm(arm)).collect();
                CoreExpr::Case { scrutinee: Box::new(scrut), alts, span: *span }
            }

            Expression::None { span, .. } => {
                CoreExpr::Con { tag: CoreTag::None, fields: Vec::new(), span: *span }
            }

            Expression::Some { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con { tag: CoreTag::Some, fields: vec![v], span: *span }
            }

            Expression::Left { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con { tag: CoreTag::Left, fields: vec![v], span: *span }
            }

            Expression::Right { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con { tag: CoreTag::Right, fields: vec![v], span: *span }
            }

            Expression::Cons { head, tail, span, .. } => {
                let h = self.lower_expr(head);
                let t = self.lower_expr(tail);
                CoreExpr::Con { tag: CoreTag::Cons, fields: vec![h, t], span: *span }
            }

            Expression::Perform { effect, operation, args, span, .. } => {
                let arg_exprs: Vec<CoreExpr> = args.iter().map(|a| self.lower_expr(a)).collect();
                CoreExpr::Perform {
                    effect: *effect,
                    operation: *operation,
                    args: arg_exprs,
                    span: *span,
                }
            }

            Expression::Handle { expr, effect, arms, span, .. } => {
                let body = self.lower_expr(expr);
                let handlers: Vec<CoreHandler> = arms.iter()
                    .map(|arm| self.lower_handle_arm(arm))
                    .collect();
                CoreExpr::Handle {
                    body: Box::new(body),
                    effect: *effect,
                    handlers,
                    span: *span,
                }
            }
        }
    }

    // ── Infix lowering ───────────────────────────────────────────────────────

    fn lower_infix(
        &mut self,
        left: &Expression,
        operator: &str,
        right: &Expression,
        span: Span,
        id: ExprId,
    ) -> CoreExpr {
        // Pipe operator: `a |> f` → `App(f, a)`
        if operator == "|>" {
            let func = self.lower_expr(right);
            let arg = self.lower_expr(left);
            return CoreExpr::App { func: Box::new(func), args: vec![arg], span };
        }

        // Determine the concrete result type from HM inference.
        // For arithmetic ops (+, -, *, /, %), the result type is the operand type,
        // so we can select `IAdd`/`FAdd` directly rather than leaving it generic.
        let result_ty = self.hm_expr_types.get(&id);
        let is_int = matches!(result_ty, Some(InferType::Con(TypeConstructor::Int)));
        let is_float = matches!(result_ty, Some(InferType::Con(TypeConstructor::Float)));

        let op = match operator {
            // Arithmetic — specialized by result type when known.
            "+" if is_int   => CorePrimOp::IAdd,
            "+" if is_float => CorePrimOp::FAdd,
            "+" => CorePrimOp::Add,
            "-" if is_int   => CorePrimOp::ISub,
            "-" if is_float => CorePrimOp::FSub,
            "-" => CorePrimOp::Sub,
            "*" if is_int   => CorePrimOp::IMul,
            "*" if is_float => CorePrimOp::FMul,
            "*" => CorePrimOp::Mul,
            "/" if is_int   => CorePrimOp::IDiv,
            "/" if is_float => CorePrimOp::FDiv,
            "/" => CorePrimOp::Div,
            "%" if is_int   => CorePrimOp::IMod,
            "%" => CorePrimOp::Mod,
            // Comparisons and logical — always generic (result is Bool).
            "==" => CorePrimOp::Eq,
            "!=" => CorePrimOp::NEq,
            "<"  => CorePrimOp::Lt,
            "<=" => CorePrimOp::Le,
            ">"  => CorePrimOp::Gt,
            ">=" => CorePrimOp::Ge,
            "&&" => CorePrimOp::And,
            "||" => CorePrimOp::Or,
            "++" => CorePrimOp::Concat,
            _ => {
                // Unknown operator — emit as generic Add (fallback placeholder).
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                return CoreExpr::PrimOp { op: CorePrimOp::Add, args: vec![l, r], span };
            }
        };

        let l = self.lower_expr(left);
        let r = self.lower_expr(right);
        CoreExpr::PrimOp { op, args: vec![l, r], span }
    }

    // ── Pattern lowering ─────────────────────────────────────────────────────

    fn lower_match_arm(&mut self, arm: &MatchArm) -> CoreAlt {
        CoreAlt {
            pat: lower_pattern(&arm.pattern),
            guard: arm.guard.as_ref().map(|g| self.lower_expr(g)),
            rhs: self.lower_expr(&arm.body),
            span: arm.span,
        }
    }

    fn lower_handle_arm(&mut self, arm: &HandleArm) -> CoreHandler {
        CoreHandler {
            operation: arm.operation_name,
            params: arm.params.clone(),
            resume: arm.resume_param,
            body: self.lower_expr(&arm.body),
            span: arm.span,
        }
    }

    // ── Destructuring at top level ───────────────────────────────────────────

    /// Expand a top-level `LetDestructure` into individual `CoreDef`s.
    ///
    /// For simple tuple patterns `(x, y) = expr` this emits:
    ///   `x = TupleField(expr, 0)`,  `y = TupleField(expr, 1)`
    ///
    /// For more complex patterns we emit a single `$destructure` def and
    /// subsequent field projections.
    fn expand_destructure_top_level(
        &mut self,
        pat: CorePat,
        rhs: CoreExpr,
        span: Span,
        out: &mut Vec<CoreDef>,
    ) {
        match pat {
            CorePat::Tuple(fields) => {
                // Bind to a tmp first so rhs is evaluated once.
                let tmp = crate::syntax::symbol::Symbol::new(5_000_000 + self.fresh);
                self.fresh += 1;
                out.push(CoreDef {
                    name: tmp,
                    expr: rhs,
                    is_recursive: false,
                    span,
                });
                for (i, field_pat) in fields.into_iter().enumerate() {
                    if let CorePat::Var(name) = field_pat {
                        out.push(CoreDef {
                            name,
                            expr: CoreExpr::PrimOp {
                                op: CorePrimOp::TupleField(i),
                                args: vec![CoreExpr::Var(tmp, span)],
                                span,
                            },
                            is_recursive: false,
                                    span,
                        });
                    }
                    // Nested non-variable patterns are skipped for now.
                }
            }
            CorePat::Var(name) => {
                out.push(CoreDef { name, expr: rhs, is_recursive: false, span });
            }
            _ => {
                // General case: bind to a tmp.
                let tmp = crate::syntax::symbol::Symbol::new(5_000_000 + self.fresh);
                self.fresh += 1;
                out.push(CoreDef { name: tmp, expr: rhs, is_recursive: false, span });
            }
        }
    }
}

// ── Pure pattern lowering (no side effects) ───────────────────────────────────

fn lower_pattern(pat: &Pattern) -> CorePat {
    match pat {
        Pattern::Wildcard { .. } => CorePat::Wildcard,
        Pattern::Identifier { name, .. } => CorePat::Var(*name),
        Pattern::Literal { expression, .. } => {
            // Only simple literal patterns are supported.
            match expression {
                Expression::Integer { value, .. } => CorePat::Lit(CoreLit::Int(*value)),
                Expression::Float { value, .. } => CorePat::Lit(CoreLit::Float(*value)),
                Expression::String { value, .. } => CorePat::Lit(CoreLit::String(value.clone())),
                Expression::Boolean { value, .. } => CorePat::Lit(CoreLit::Bool(*value)),
                _ => CorePat::Wildcard, // complex expression patterns → wildcard
            }
        }
        Pattern::None { .. } => CorePat::Con { tag: CoreTag::None, fields: Vec::new() },
        Pattern::Some { pattern, .. } => CorePat::Con {
            tag: CoreTag::Some,
            fields: vec![lower_pattern(pattern)],
        },
        Pattern::Left { pattern, .. } => CorePat::Con {
            tag: CoreTag::Left,
            fields: vec![lower_pattern(pattern)],
        },
        Pattern::Right { pattern, .. } => CorePat::Con {
            tag: CoreTag::Right,
            fields: vec![lower_pattern(pattern)],
        },
        Pattern::Cons { head, tail, .. } => CorePat::Con {
            tag: CoreTag::Cons,
            fields: vec![lower_pattern(head), lower_pattern(tail)],
        },
        Pattern::EmptyList { .. } => CorePat::EmptyList,
        Pattern::Tuple { elements, .. } => {
            CorePat::Tuple(elements.iter().map(lower_pattern).collect())
        }
        Pattern::Constructor { name, fields, .. } => CorePat::Con {
            tag: CoreTag::Named(*name),
            fields: fields.iter().map(lower_pattern).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::type_infer::{InferProgramConfig, infer_program},
        nary::CorePrimOp,
        syntax::{interner::Interner, lexer::Lexer, parser::Parser},
    };

    fn parse_and_infer(src: &str) -> (crate::syntax::program::Program, HashMap<ExprId, InferType>, Interner) {
        let mut parser = Parser::new(Lexer::new(src));
        let program = parser.parse_program();
        let mut interner = parser.take_interner();
        let base_sym = interner.intern("base");
        let hm = infer_program(&program, &interner, InferProgramConfig {
            file_path: None,
            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            known_base_names: std::collections::HashSet::new(),
            base_module_symbol: base_sym,
            preloaded_effect_op_signatures: HashMap::new(),
        });
        let types = hm.expr_types;
        (program, types, interner)
    }

    fn collect_primops(program: &crate::syntax::program::Program, types: &HashMap<ExprId, InferType>) -> Vec<CorePrimOp> {
        let core = lower_program_ast(program, types);
        let mut ops = Vec::new();
        for def in &core.defs {
            collect_ops_in_expr(&def.expr, &mut ops);
        }
        ops
    }

    fn collect_ops_in_expr(expr: &crate::nary::CoreExpr, out: &mut Vec<CorePrimOp>) {
        use crate::nary::CoreExpr;
        match expr {
            CoreExpr::PrimOp { op, args, .. } => {
                out.push(op.clone());
                for a in args { collect_ops_in_expr(a, out); }
            }
            CoreExpr::Lam { body, .. } => collect_ops_in_expr(body, out),
            CoreExpr::App { func, args, .. } => {
                collect_ops_in_expr(func, out);
                for a in args { collect_ops_in_expr(a, out); }
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_ops_in_expr(rhs, out);
                collect_ops_in_expr(body, out);
            }
            CoreExpr::Case { scrutinee, alts, .. } => {
                collect_ops_in_expr(scrutinee, out);
                for alt in alts { collect_ops_in_expr(&alt.rhs, out); }
            }
            _ => {}
        }
    }

    #[test]
    fn integer_add_emits_iadd() {
        let src = "fn add(x: Int, y: Int) -> Int { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(ops.contains(&CorePrimOp::IAdd),
            "expected IAdd for Int addition, got: {:?}", ops);
        assert!(!ops.contains(&CorePrimOp::Add),
            "should not emit generic Add for typed Int addition");
    }

    #[test]
    fn integer_mul_emits_imul() {
        let src = "fn mul(x: Int, y: Int) -> Int { x * y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(ops.contains(&CorePrimOp::IMul), "expected IMul, got: {:?}", ops);
    }

    #[test]
    fn float_add_emits_fadd() {
        let src = "fn fadd(x: Float, y: Float) -> Float { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(ops.contains(&CorePrimOp::FAdd), "expected FAdd, got: {:?}", ops);
    }

    #[test]
    fn generic_add_stays_generic_without_type_info() {
        // Unannotated polymorphic add — HM resolves to Int here, so we get IAdd.
        // This test verifies that untyped (Any) additions stay as generic Add.
        // We use a String + String expression so HM infers String, not Int/Float,
        // which means no typed variant should be emitted — just generic Add.
        let src = r#"fn cat(a: String, b: String) -> String { a + b }"#;
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        // String + String should emit generic Add (no IAdd/FAdd)
        assert!(ops.contains(&CorePrimOp::Add), "expected generic Add for String addition, got: {:?}", ops);
        assert!(!ops.contains(&CorePrimOp::IAdd), "should not emit IAdd for String addition");
        assert!(!ops.contains(&CorePrimOp::FAdd), "should not emit FAdd for String addition");
    }
}
