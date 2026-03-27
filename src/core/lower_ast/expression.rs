use crate::{
    diagnostics::position::Span,
    syntax::expression::{ExprId, Expression, StringPart},
    types::{infer_type::InferType, type_constructor::TypeConstructor},
};

use crate::core::{CoreAlt, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreTag};

impl<'a> super::AstLowerer<'a> {
    // ── Expression lowering ──────────────────────────────────────────────────

    pub(super) fn lower_expr(&mut self, expr: &Expression) -> CoreExpr {
        match expr {
            Expression::Identifier { name, span, .. } => CoreExpr::external_var(*name, *span),

            Expression::Integer { value, span, .. } => CoreExpr::Lit(CoreLit::Int(*value), *span),

            Expression::Float { value, span, .. } => CoreExpr::Lit(CoreLit::Float(*value), *span),

            Expression::String { value, span, .. } => {
                CoreExpr::Lit(CoreLit::String(value.clone()), *span)
            }

            Expression::Boolean { value, span, .. } => CoreExpr::Lit(CoreLit::Bool(*value), *span),

            Expression::InterpolatedString { parts, span, .. } => {
                let args: Vec<CoreExpr> = parts
                    .iter()
                    .map(|p| match p {
                        StringPart::Literal(s) => CoreExpr::Lit(CoreLit::String(s.clone()), *span),
                        StringPart::Interpolation(e) => self.lower_expr(e),
                    })
                    .collect();
                CoreExpr::PrimOp {
                    op: CorePrimOp::Interpolate,
                    args,
                    span: *span,
                }
            }

            Expression::Prefix {
                operator,
                right,
                span,
                ..
            } => {
                let arg = self.lower_expr(right);
                let op = match operator.as_str() {
                    "-" => CorePrimOp::Neg,
                    "!" => CorePrimOp::Not,
                    _ => CorePrimOp::Neg, // fallback
                };
                CoreExpr::PrimOp {
                    op,
                    args: vec![arg],
                    span: *span,
                }
            }

            Expression::Infix {
                left,
                operator,
                right,
                span,
                id,
            } => self.lower_infix(left, operator, right, *span, *id),

            Expression::If {
                condition,
                consequence,
                alternative,
                span,
                ..
            } => {
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
                        var: self.fresh_binder(crate::syntax::symbol::Symbol::new(
                            3_000_000 + self.fresh,
                        )),
                        rhs: Box::new(CoreExpr::Lit(CoreLit::Unit, *span)),
                        body: Box::new(other),
                        span: *span,
                    },
                }
            }

            Expression::Function {
                parameters,
                body,
                span,
                ..
            } => {
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let body_expr = self.lower_block(body);
                if parameters.is_empty() {
                    // Nullary lambda — keep the Lam wrapper so the Core→IR
                    // lowerer recognises it as a closure, but with empty params
                    // so the resulting IR function has arity 0.
                    CoreExpr::Lam {
                        params,
                        body: Box::new(body_expr),
                        span: *span,
                    }
                } else {
                    CoreExpr::lambda(params, body_expr, *span)
                }
            }

            Expression::Call {
                function,
                arguments,
                span,
                ..
            } => {
                let func = self.lower_expr(function);
                let args: Vec<CoreExpr> = arguments.iter().map(|a| self.lower_expr(a)).collect();
                // Always emit App, even for zero-arg calls — Flux functions
                // must be invoked explicitly (they can have side effects).
                CoreExpr::App {
                    func: Box::new(func),
                    args,
                    span: *span,
                }
            }

            Expression::ListLiteral { elements, span, .. } => {
                // [a, b, c] → PrimOp(MakeList, [a, b, c])
                let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                CoreExpr::PrimOp {
                    op: CorePrimOp::MakeList,
                    args,
                    span: *span,
                }
            }

            Expression::ArrayLiteral { elements, span, .. } => {
                let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                CoreExpr::PrimOp {
                    op: CorePrimOp::MakeArray,
                    args,
                    span: *span,
                }
            }

            Expression::TupleLiteral { elements, span, .. } => {
                let args: Vec<CoreExpr> = elements.iter().map(|e| self.lower_expr(e)).collect();
                CoreExpr::PrimOp {
                    op: CorePrimOp::MakeTuple,
                    args,
                    span: *span,
                }
            }

            Expression::EmptyList { span, .. } => CoreExpr::Con {
                tag: CoreTag::Nil,
                fields: Vec::new(),
                span: *span,
            },

            Expression::Hash { pairs, span, .. } => {
                // Flatten pairs: [k1, v1, k2, v2, ...] for MakeHash.
                let args: Vec<CoreExpr> = pairs
                    .iter()
                    .flat_map(|(k, v)| [self.lower_expr(k), self.lower_expr(v)])
                    .collect();
                CoreExpr::PrimOp {
                    op: CorePrimOp::MakeHash,
                    args,
                    span: *span,
                }
            }

            Expression::Index {
                left, index, span, ..
            } => {
                let l = self.lower_expr(left);
                let i = self.lower_expr(index);
                CoreExpr::PrimOp {
                    op: CorePrimOp::Index,
                    args: vec![l, i],
                    span: *span,
                }
            }

            Expression::MemberAccess {
                object,
                member,
                span,
                ..
            } => {
                let obj = self.lower_expr(object);
                CoreExpr::MemberAccess {
                    object: Box::new(obj),
                    member: *member,
                    span: *span,
                }
            }

            Expression::TupleFieldAccess {
                object,
                index,
                span,
                ..
            } => {
                let obj = self.lower_expr(object);
                CoreExpr::TupleField {
                    object: Box::new(obj),
                    index: *index,
                    span: *span,
                }
            }

            Expression::Match {
                scrutinee,
                arms,
                span,
                ..
            } => {
                let scrut = self.lower_expr(scrutinee);
                let alts: Vec<CoreAlt> = arms.iter().map(|arm| self.lower_match_arm(arm)).collect();
                CoreExpr::Case {
                    scrutinee: Box::new(scrut),
                    alts,
                    span: *span,
                }
            }

            Expression::None { span, .. } => CoreExpr::Con {
                tag: CoreTag::None,
                fields: Vec::new(),
                span: *span,
            },

            Expression::Some { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![v],
                    span: *span,
                }
            }

            Expression::Left { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con {
                    tag: CoreTag::Left,
                    fields: vec![v],
                    span: *span,
                }
            }

            Expression::Right { value, span, .. } => {
                let v = self.lower_expr(value);
                CoreExpr::Con {
                    tag: CoreTag::Right,
                    fields: vec![v],
                    span: *span,
                }
            }

            Expression::Cons {
                head, tail, span, ..
            } => {
                let h = self.lower_expr(head);
                let t = self.lower_expr(tail);
                CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![h, t],
                    span: *span,
                }
            }

            Expression::Perform {
                effect,
                operation,
                args,
                span,
                ..
            } => {
                let arg_exprs: Vec<CoreExpr> = args.iter().map(|a| self.lower_expr(a)).collect();
                CoreExpr::Perform {
                    effect: *effect,
                    operation: *operation,
                    args: arg_exprs,
                    span: *span,
                }
            }

            Expression::Handle {
                expr,
                effect,
                arms,
                span,
                ..
            } => {
                let body = self.lower_expr(expr);
                let handlers: Vec<CoreHandler> =
                    arms.iter().map(|arm| self.lower_handle_arm(arm)).collect();
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
            return CoreExpr::App {
                func: Box::new(func),
                args: vec![arg],
                span,
            };
        }

        // Determine the concrete result type from HM inference.
        // For arithmetic ops (+, -, *, /, %), only emit typed variants (IAdd/FAdd)
        // when both operands AND the result are provably the correct type.
        // Under gradual typing, a type mismatch (e.g. 1 + None) may infer the
        // result as Int while one operand is Any/None — emitting IAdd would skip
        // the runtime type check and produce garbage.
        let result_ty = self.hm_expr_types.get(&id);
        let left_ty = self.hm_expr_types.get(&left.expr_id());
        let right_ty = self.hm_expr_types.get(&right.expr_id());
        let is_int = matches!(result_ty, Some(InferType::Con(TypeConstructor::Int)))
            && matches!(left_ty, Some(InferType::Con(TypeConstructor::Int)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Int)));
        let is_float = matches!(result_ty, Some(InferType::Con(TypeConstructor::Float)))
            && matches!(left_ty, Some(InferType::Con(TypeConstructor::Float)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Float)));

        let op = match operator {
            // Arithmetic — specialized by result type when known.
            "+" if is_int => CorePrimOp::IAdd,
            "+" if is_float => CorePrimOp::FAdd,
            "+" => CorePrimOp::Add,
            "-" if is_int => CorePrimOp::ISub,
            "-" if is_float => CorePrimOp::FSub,
            "-" => CorePrimOp::Sub,
            "*" if is_int => CorePrimOp::IMul,
            "*" if is_float => CorePrimOp::FMul,
            "*" => CorePrimOp::Mul,
            "/" if is_int => CorePrimOp::IDiv,
            "/" if is_float => CorePrimOp::FDiv,
            "/" => CorePrimOp::Div,
            "%" if is_int => CorePrimOp::IMod,
            "%" => CorePrimOp::Mod,
            // Comparisons and logical — always generic (result is Bool).
            "==" => CorePrimOp::Eq,
            "!=" => CorePrimOp::NEq,
            "<" => CorePrimOp::Lt,
            "<=" => CorePrimOp::Le,
            ">" => CorePrimOp::Gt,
            ">=" => CorePrimOp::Ge,
            "&&" => CorePrimOp::And,
            "||" => CorePrimOp::Or,
            "++" => CorePrimOp::Concat,
            _ => {
                // Unknown operator — emit as generic Add (fallback placeholder).
                let l = self.lower_expr(left);
                let r = self.lower_expr(right);
                return CoreExpr::PrimOp {
                    op: CorePrimOp::Add,
                    args: vec![l, r],
                    span,
                };
            }
        };

        let l = self.lower_expr(left);
        let r = self.lower_expr(right);
        CoreExpr::PrimOp {
            op,
            args: vec![l, r],
            span,
        }
    }
}
