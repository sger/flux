use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier,
        expression::{ExprId, Expression, NamedFieldInit, StringPart},
    },
    types::{infer_type::InferType, type_constructor::TypeConstructor},
};

use crate::core::{
    CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp,
    CoreTag, FluxRep,
};

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
                id,
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
                    join_ty: self.infer_core_type(*id),
                    span: *span,
                }
            }

            Expression::DoBlock { block, span, .. } => {
                // DoBlock is a sequencing block — lower it as a regular block.
                let inner = self.lower_block(block);
                // Preserve the span wrapper.
                match inner {
                    CoreExpr::Let { .. }
                    | CoreExpr::LetRec { .. }
                    | CoreExpr::LetRecGroup { .. }
                    | CoreExpr::Case { .. } => inner,
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
                id,
                ..
            } => {
                let params: Vec<_> = self.bind_lambda_params(parameters, *id);
                let (mut param_types, result_ty) = self.lambda_signature_from_expr_id(*id);
                if !param_types.is_empty() && param_types.len() != params.len() {
                    param_types = Vec::new();
                }
                let body_expr = self.lower_block(body);
                if parameters.is_empty() {
                    // Nullary lambda — keep the Lam wrapper so the Core→IR
                    // lowerer recognises it as a closure, but with empty params
                    // so the resulting IR function has arity 0.
                    CoreExpr::Lam {
                        params,
                        param_types,
                        result_ty,
                        body: Box::new(body_expr),
                        span: *span,
                    }
                } else {
                    CoreExpr::lambda_typed(params, param_types, result_ty, body_expr, *span)
                }
            }

            Expression::Call {
                function,
                arguments,
                span,
                ..
            } => {
                // Phase 4 Step 5: compile-time class method dispatch.
                // If the callee is a class method and the argument type is known,
                // resolve directly to the mangled instance function.
                if let Some(mangled) = self.try_resolve_class_call_expr(function, arguments) {
                    let method_name = match function.as_ref() {
                        Expression::Identifier { name, .. } => *name,
                        Expression::MemberAccess { member, .. } => *member,
                        _ => unreachable!("class call resolution only succeeds for direct callees"),
                    };
                    let mut args = self.resolve_direct_class_call_dict_args(method_name, arguments);
                    args.extend(arguments.iter().map(|a| self.lower_expr(a)));
                    return CoreExpr::App {
                        func: Box::new(CoreExpr::external_var(mangled, *span)),
                        args,
                        span: *span,
                    };
                }

                // Step 5b: Dictionary passing for constrained functions.
                // If the callee is a function with class constraints in its scheme,
                // resolve concrete dictionaries and prepend them as arguments.
                if let Expression::Identifier { name, id, .. } = function.as_ref() {
                    let dict_args = self.resolve_dict_args_for_call(*name, *id, arguments);
                    if !dict_args.is_empty() {
                        let func = self.lower_expr(function);
                        let mut all_args = dict_args;
                        all_args.extend(arguments.iter().map(|a| self.lower_expr(a)));
                        return CoreExpr::App {
                            func: Box::new(func),
                            args: all_args,
                            span: *span,
                        };
                    }
                }

                let func = self.lower_expr(function);
                let args: Vec<CoreExpr> = arguments.iter().map(|a| self.lower_expr(a)).collect();
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
                // Proposal 0152: dot access on a named-field ADT value
                // desugars into a Case expression that extracts the
                // declared positional field.
                if let Some(core) = self.try_lower_adt_field_access(object, *member, *span) {
                    return core;
                }
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
                id,
            } => {
                // Look up the scrutinee's HM-inferred type for typed pattern binders.
                let scrutinee_ty = self.hm_expr_types.get(&scrutinee.expr_id()).cloned();
                let scrut = self.lower_expr(scrutinee);
                let alts: Vec<CoreAlt> = arms
                    .iter()
                    .map(|arm| self.lower_match_arm_typed(arm, scrutinee_ty.as_ref()))
                    .collect();
                CoreExpr::Case {
                    scrutinee: Box::new(scrut),
                    alts,
                    join_ty: self.infer_core_type(*id),
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
                id,
            } => {
                let body = self.lower_expr(expr);
                let handle_result_ty = self.infer_core_type(*id);
                let handlers: Vec<CoreHandler> = arms
                    .iter()
                    .map(|arm| self.lower_handle_arm_typed(arm, *effect, handle_result_ty.clone()))
                    .collect();
                CoreExpr::Handle {
                    body: Box::new(body),
                    effect: *effect,
                    handlers,
                    span: *span,
                }
            }

            Expression::Sealing { expr, .. } => self.lower_expr(expr),

            Expression::NamedConstructor {
                name, fields, span, ..
            } => self.lower_named_constructor(*name, fields, *span),
            Expression::Spread {
                base,
                overrides,
                span,
                ..
            } => self.lower_spread_expression(base, overrides, *span),
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
        // result as Int while one operand is unresolved/None — emitting IAdd would skip
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
        // For comparisons the result is Bool, so only check operand types.
        let operands_int = matches!(left_ty, Some(InferType::Con(TypeConstructor::Int)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Int)));
        let operands_float = matches!(left_ty, Some(InferType::Con(TypeConstructor::Float)))
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
            // Comparisons — specialized when both operands are provably Int or Float.
            "==" if operands_int => CorePrimOp::ICmpEq,
            "==" if operands_float => CorePrimOp::FCmpEq,
            "==" => CorePrimOp::Eq,
            "!=" if operands_int => CorePrimOp::ICmpNe,
            "!=" if operands_float => CorePrimOp::FCmpNe,
            "!=" => CorePrimOp::NEq,
            "<" if operands_int => CorePrimOp::ICmpLt,
            "<" if operands_float => CorePrimOp::FCmpLt,
            "<" => CorePrimOp::Lt,
            "<=" if operands_int => CorePrimOp::ICmpLe,
            "<=" if operands_float => CorePrimOp::FCmpLe,
            "<=" => CorePrimOp::Le,
            ">" if operands_int => CorePrimOp::ICmpGt,
            ">" if operands_float => CorePrimOp::FCmpGt,
            ">" => CorePrimOp::Gt,
            ">=" if operands_int => CorePrimOp::ICmpGe,
            ">=" if operands_float => CorePrimOp::FCmpGe,
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

    // ── Proposal 0152: named-field desugaring ────────────────────────────────

    /// Lower `Foo { x: e1, y: e2 }` by reordering into declared positional
    /// order, resolving punning against in-scope bindings, and emitting a
    /// positional `Con` constructor.
    pub(super) fn lower_named_constructor(
        &mut self,
        name: Identifier,
        fields: &[NamedFieldInit],
        span: Span,
    ) -> CoreExpr {
        let declared = self
            .ctor_field_names
            .get(&name)
            .cloned()
            .unwrap_or_default();
        let mut positional: Vec<Option<CoreExpr>> = (0..declared.len()).map(|_| None).collect();
        for init in fields {
            let Some(index) = declared.iter().position(|n| *n == init.name) else {
                continue;
            };
            let value_expr = match &init.value {
                Some(v) => self.lower_expr(v),
                None => CoreExpr::external_var(init.name, init.span),
            };
            positional[index] = Some(value_expr);
        }
        let ordered: Vec<CoreExpr> = positional
            .into_iter()
            .map(|slot| slot.unwrap_or(CoreExpr::Lit(CoreLit::Int(0), span)))
            .collect();
        CoreExpr::Con {
            tag: CoreTag::Named(name),
            fields: ordered,
            span,
        }
    }

    /// Lower `{ ...base, field: value, ... }`. When the target variant is
    /// statically known (single-variant ADT or base is itself a
    /// `NamedConstructor`), reorder overrides and emit a positional `Con`
    /// whose missing fields are projected from `base` via a `Case`
    /// expression.
    pub(super) fn lower_spread_expression(
        &mut self,
        base: &Expression,
        overrides: &[NamedFieldInit],
        span: Span,
    ) -> CoreExpr {
        let variant = match self.resolve_spread_variant(base) {
            Some(v) => v,
            None => {
                // Type inference already emitted E464/E468; recover with a
                // literal unit-ish placeholder to keep lowering total.
                return CoreExpr::Lit(CoreLit::Int(0), span);
            }
        };
        let declared = self
            .ctor_field_names
            .get(&variant)
            .cloned()
            .unwrap_or_default();

        // Bind the base to a tmp so it's evaluated exactly once.
        let tmp_name = Identifier::new(0xFFFF_FFF0);
        let tmp_binder = CoreBinder::with_rep(self.alloc_binder_id(), tmp_name, FluxRep::BoxedRep);
        let base_expr = self.lower_expr(base);

        // For each declared field, use the override if present; otherwise
        // synthesize a Case over the tmp that extracts the positional field.
        let override_map: std::collections::HashMap<Identifier, &NamedFieldInit> =
            overrides.iter().map(|o| (o.name, o)).collect();

        let positional: Vec<CoreExpr> = declared
            .iter()
            .enumerate()
            .map(|(i, field_name)| {
                if let Some(init) = override_map.get(field_name) {
                    match &init.value {
                        Some(v) => self.lower_expr(v),
                        None => CoreExpr::external_var(*field_name, init.span),
                    }
                } else {
                    self.project_field_from_binder(variant, declared.len(), i, &tmp_binder, span)
                }
            })
            .collect();

        let body = CoreExpr::Con {
            tag: CoreTag::Named(variant),
            fields: positional,
            span,
        };

        CoreExpr::Let {
            var: tmp_binder,
            rhs: Box::new(base_expr),
            body: Box::new(body),
            span,
        }
    }

    /// Determine the concrete variant tag for a spread expression.
    /// Returns `Some(variant)` when the base is a `NamedConstructor` or the
    /// base's inferred ADT has exactly one named-field variant.
    fn resolve_spread_variant(&self, base: &Expression) -> Option<Identifier> {
        if let Expression::NamedConstructor { name, .. } = base {
            return Some(*name);
        }
        let base_ty = self.hm_expr_types.get(&base.expr_id())?;
        let adt_name = match base_ty {
            InferType::Con(TypeConstructor::Adt(n)) => *n,
            InferType::App(TypeConstructor::Adt(n), _) => *n,
            _ => return None,
        };
        let variants = self.adt_variants.get(&adt_name)?;
        let named_variants: Vec<Identifier> = variants
            .iter()
            .copied()
            .filter(|v| self.ctor_field_names.contains_key(v))
            .collect();
        if named_variants.len() == 1 {
            Some(named_variants[0])
        } else {
            None
        }
    }

    /// Synthesize `case tmp { Variant(x0, ..., xN) -> xI }` to extract the
    /// `target_index`-th positional field from a named-variant value.
    fn project_field_from_binder(
        &mut self,
        variant: Identifier,
        arity: usize,
        target_index: usize,
        tmp: &CoreBinder,
        span: Span,
    ) -> CoreExpr {
        let mut field_binders: Vec<CoreBinder> = Vec::with_capacity(arity);
        for _ in 0..arity {
            let id = self.alloc_binder_id();
            field_binders.push(CoreBinder::with_rep(
                id,
                Identifier::new(0),
                FluxRep::BoxedRep,
            ));
        }
        let target = &field_binders[target_index];
        let rhs = CoreExpr::bound_var(target, span);
        let pat = CorePat::Con {
            tag: CoreTag::Named(variant),
            fields: field_binders.iter().cloned().map(CorePat::Var).collect(),
        };
        let alt = CoreAlt {
            pat,
            guard: None,
            rhs,
            span,
        };
        CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::bound_var(tmp, span)),
            alts: vec![alt],
            join_ty: None,
            span,
        }
    }

    fn alloc_binder_id(&mut self) -> CoreBinderId {
        let id = CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        id
    }

    /// Proposal 0152: if `object`'s inferred type is a named-field ADT and
    /// `member` is declared in every variant's named-field list, emit a
    /// `Case` that pattern-matches each variant and yields the field.
    fn try_lower_adt_field_access(
        &mut self,
        object: &Expression,
        member: Identifier,
        span: Span,
    ) -> Option<CoreExpr> {
        let object_ty = self.hm_expr_types.get(&object.expr_id())?;
        let adt_name = match object_ty {
            InferType::Con(TypeConstructor::Adt(n)) => *n,
            InferType::App(TypeConstructor::Adt(n), _) => *n,
            _ => return None,
        };
        let variants = self.adt_variants.get(&adt_name)?.clone();
        let named_variants: Vec<Identifier> = variants
            .iter()
            .copied()
            .filter(|v| self.ctor_field_names.contains_key(v))
            .collect();
        if named_variants.is_empty() {
            return None;
        }
        // Every variant must declare `member` — partial-variant dot access
        // is not desugared here (type inference already flagged it or lifted
        // the result to Option<T>; full Case-based desugar is a future step).
        let mut per_variant: Vec<(Identifier, Vec<Identifier>, usize)> = Vec::new();
        for variant in &named_variants {
            let field_names = self.ctor_field_names.get(variant)?.clone();
            let index = field_names.iter().position(|n| *n == member)?;
            per_variant.push((*variant, field_names, index));
        }

        let scrutinee = self.lower_expr(object);
        let alts: Vec<CoreAlt> = per_variant
            .into_iter()
            .map(|(variant, field_names, target_index)| {
                let mut binders: Vec<CoreBinder> = Vec::with_capacity(field_names.len());
                for _ in 0..field_names.len() {
                    let id = self.alloc_binder_id();
                    binders.push(CoreBinder::with_rep(
                        id,
                        Identifier::new(0),
                        FluxRep::BoxedRep,
                    ));
                }
                let target = binders[target_index];
                let pat = CorePat::Con {
                    tag: CoreTag::Named(variant),
                    fields: binders.into_iter().map(CorePat::Var).collect(),
                };
                CoreAlt {
                    pat,
                    guard: None,
                    rhs: CoreExpr::bound_var(&target, span),
                    span,
                }
            })
            .collect();
        Some(CoreExpr::Case {
            scrutinee: Box::new(scrutinee),
            alts,
            join_ty: None,
            span,
        })
    }
}
