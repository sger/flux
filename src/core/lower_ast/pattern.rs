use crate::{
    diagnostics::position::Span,
    syntax::expression::{Expression, HandleArm, MatchArm, Pattern},
    types::infer_type::InferType,
};

use crate::core::{CoreAlt, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat, CoreTag, FluxRep};

impl<'a> super::AstLowerer<'a> {
    // ── Pattern lowering ─────────────────────────────────────────────────────

    /// Lower a match arm with an optional scrutinee type for typed pattern binders.
    pub(super) fn lower_match_arm_typed(
        &mut self,
        arm: &MatchArm,
        scrutinee_ty: Option<&InferType>,
    ) -> CoreAlt {
        CoreAlt {
            pat: self.lower_pattern_typed(&arm.pattern, scrutinee_ty),
            guard: arm.guard.as_ref().map(|g| self.lower_expr(g)),
            rhs: self.lower_expr(&arm.body),
            span: arm.span,
        }
    }

    /// Lower an effect handler arm with typed binders from effect op signatures.
    pub(super) fn lower_handle_arm_typed(
        &mut self,
        arm: &HandleArm,
        effect: crate::syntax::Identifier,
        handle_result_ty: Option<crate::core::CoreType>,
        parameter_ty: Option<crate::core::CoreType>,
    ) -> CoreHandler {
        // Look up op signature: (effect, operation) → (param_types, return_type)
        let op_sig = self
            .effect_op_sigs
            .and_then(|sigs| sigs.get(&(effect, arm.operation_name)));

        let has_state = parameter_ty.is_some() && !arm.params.is_empty();
        let source_op_param_count = arm.params.len().saturating_sub(usize::from(has_state));
        let state_source = has_state
            .then(|| arm.params[source_op_param_count])
            .map(|p| self.bind_name(p));

        let (params, param_types, state, state_ty) = if let Some((param_tys, _ret_ty)) = op_sig {
            if param_tys.len() == source_op_param_count {
                let params = arm
                    .params
                    .iter()
                    .take(source_op_param_count)
                    .zip(param_tys.iter())
                    .map(|(&p, ty)| self.bind_name_with_type(p, ty))
                    .collect();
                let param_types = param_tys
                    .iter()
                    .map(crate::core::CoreType::try_from_infer)
                    .collect();
                (params, param_types, state_source, parameter_ty.clone())
            } else {
                (
                    arm.params
                        .iter()
                        .take(source_op_param_count)
                        .map(|&p| self.bind_name(p))
                        .collect(),
                    Vec::new(),
                    state_source,
                    parameter_ty.clone(),
                )
            }
        } else {
            (
                arm.params
                    .iter()
                    .take(source_op_param_count)
                    .map(|&p| self.bind_name(p))
                    .collect(),
                Vec::new(),
                state_source,
                parameter_ty.clone(),
            )
        };

        // Resume is always a closure (boxed).
        let resume = {
            let id = super::super::CoreBinderId(self.next_binder_id);
            self.next_binder_id += 1;
            super::super::CoreBinder::with_rep(id, arm.resume_param, FluxRep::BoxedRep)
        };
        let resume_ty = op_sig.and_then(|(_param_tys, ret_ty)| {
            crate::core::CoreType::try_from_infer(ret_ty).map(|op_ret_ty| {
                let mut params = vec![op_ret_ty];
                if let Some(parameter_ty) = parameter_ty.clone() {
                    params.push(parameter_ty);
                }
                crate::core::CoreType::Function(
                    params,
                    Box::new(
                        handle_result_ty
                            .clone()
                            .unwrap_or(crate::core::CoreType::Abstract(
                                crate::core::CoreAbstractType::Named(arm.resume_param, Vec::new()),
                            )),
                    ),
                )
            })
        });

        CoreHandler {
            operation: arm.operation_name,
            params,
            param_types,
            state,
            state_ty,
            resume,
            resume_ty,
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
    pub(super) fn expand_destructure_top_level(
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
                let tmp_binder = self.fresh_binder(tmp);
                out.push(CoreDef::new(tmp_binder, rhs, false, span));
                for (i, field_pat) in fields.into_iter().enumerate() {
                    if let CorePat::Var(binder) = field_pat {
                        out.push(CoreDef::new(
                            binder,
                            CoreExpr::TupleField {
                                object: Box::new(CoreExpr::bound_var(&tmp_binder, span)),
                                index: i,
                                span,
                            },
                            false,
                            span,
                        ));
                    }
                    // Nested non-variable patterns are skipped for now.
                }
            }
            CorePat::Var(binder) => {
                out.push(CoreDef::new(binder, rhs, false, span));
            }
            _ => {
                // General case: bind to a tmp.
                let tmp = crate::syntax::symbol::Symbol::new(5_000_000 + self.fresh);
                self.fresh += 1;
                out.push(CoreDef::new(self.fresh_binder(tmp), rhs, false, span));
            }
        }
    }

    // ── Pure pattern lowering (no side effects) ─────────────────────────────

    pub(super) fn lower_pattern(&mut self, pat: &Pattern) -> CorePat {
        self.lower_pattern_typed(pat, None)
    }

    /// Lower a pattern with an optional known type for the value being matched.
    ///
    /// When `ty` is `Some`, pattern variable binders get their `FluxRep` from
    /// the decomposed type (mirroring HM inference's `bind_pattern_variables`).
    /// When `ty` is `None`, falls back to `TaggedRep` (the default).
    pub(super) fn lower_pattern_typed(&mut self, pat: &Pattern, ty: Option<&InferType>) -> CorePat {
        use crate::types::type_constructor::TypeConstructor;

        match pat {
            Pattern::Wildcard { .. } => CorePat::Wildcard,
            Pattern::Identifier { name, .. } => {
                if let Some(t) = ty {
                    CorePat::Var(self.bind_name_with_type(*name, t))
                } else {
                    CorePat::Var(self.bind_name(*name))
                }
            }
            Pattern::Literal { expression, .. } => match expression {
                Expression::Integer { value, .. } => CorePat::Lit(CoreLit::Int(*value)),
                Expression::Float { value, .. } => CorePat::Lit(CoreLit::Float(*value)),
                Expression::String { value, .. } => CorePat::Lit(CoreLit::String(value.clone())),
                Expression::Boolean { value, .. } => CorePat::Lit(CoreLit::Bool(*value)),
                _ => CorePat::Wildcard,
            },
            Pattern::None { .. } => CorePat::Con {
                tag: CoreTag::None,
                fields: Vec::new(),
            },
            Pattern::Some { pattern, .. } => {
                // Extract inner type from Option<T>
                let inner_ty = ty.and_then(|t| match t {
                    InferType::App(TypeConstructor::Option, args) if args.len() == 1 => {
                        Some(&args[0])
                    }
                    _ => None,
                });
                CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![self.lower_pattern_typed(pattern, inner_ty)],
                }
            }
            Pattern::Left { pattern, .. } => {
                let left_ty = ty.and_then(|t| match t {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        Some(&args[0])
                    }
                    _ => None,
                });
                CorePat::Con {
                    tag: CoreTag::Left,
                    fields: vec![self.lower_pattern_typed(pattern, left_ty)],
                }
            }
            Pattern::Right { pattern, .. } => {
                let right_ty = ty.and_then(|t| match t {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        Some(&args[1])
                    }
                    _ => None,
                });
                CorePat::Con {
                    tag: CoreTag::Right,
                    fields: vec![self.lower_pattern_typed(pattern, right_ty)],
                }
            }
            Pattern::Cons { head, tail, .. } => {
                let elem_ty = ty.and_then(|t| match t {
                    InferType::App(TypeConstructor::List, args) if args.len() == 1 => {
                        Some(&args[0])
                    }
                    _ => None,
                });
                CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![
                        self.lower_pattern_typed(head, elem_ty),
                        self.lower_pattern_typed(tail, ty),
                    ],
                }
            }
            Pattern::EmptyList { .. } => CorePat::EmptyList,
            Pattern::Tuple { elements, .. } => {
                let elem_types: Option<&[InferType]> = ty.and_then(|t| match t {
                    InferType::Tuple(elems) if elems.len() == elements.len() => {
                        Some(elems.as_slice())
                    }
                    _ => None,
                });
                CorePat::Tuple(match elem_types {
                    Some(types) => elements
                        .iter()
                        .zip(types.iter())
                        .map(|(p, t)| self.lower_pattern_typed(p, Some(t)))
                        .collect(),
                    None => elements
                        .iter()
                        .map(|p| self.lower_pattern_typed(p, None))
                        .collect(),
                })
            }
            Pattern::Constructor { name, fields, .. } => {
                // Try to get field types from ADT constructor info via hm_expr_types.
                // ADT constructor field types require looking up the constructor's
                // type scheme, which we don't have direct access to here.
                // Fall back to untyped for ADT fields — this is a known limitation
                // (Phase 7e handles built-in patterns; ADT field typing needs
                // constructor type info threaded into the lowerer).
                CorePat::Con {
                    tag: CoreTag::Named(*name),
                    fields: fields
                        .iter()
                        .map(|p| self.lower_pattern_typed(p, None))
                        .collect(),
                }
            }
            Pattern::NamedConstructor { name, fields, .. } => {
                // Proposal 0152: reorder named fields into declaration order
                // and emit as a positional constructor pattern. Unknown or
                // duplicate fields have already been diagnosed in HM; we
                // recover with wildcards for any missing slot.
                let declared = self.ctor_field_names.get(name).cloned().unwrap_or_default();
                let mut positional: Vec<CorePat> =
                    (0..declared.len()).map(|_| CorePat::Wildcard).collect();
                for field in fields {
                    if let Some(index) = declared.iter().position(|n| *n == field.name) {
                        let sub = match &field.pattern {
                            Some(p) => self.lower_pattern_typed(p, None),
                            None => CorePat::Var(self.bind_name(field.name)),
                        };
                        positional[index] = sub;
                    }
                }
                CorePat::Con {
                    tag: CoreTag::Named(*name),
                    fields: positional,
                }
            }
        }
    }
}
