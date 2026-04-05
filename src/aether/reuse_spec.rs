use crate::core::{CoreBinderId, CoreExpr, CorePat, CoreTag, CoreVarRef};

use super::reuse_analysis::ReuseEnv;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseSpecCandidate {
    pub token_binder: CoreBinderId,
    pub tag: CoreTag,
    pub arity: usize,
    pub unchanged_fields: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReuseSpecDecision {
    PlainReuse,
    SelectiveWrite {
        field_mask: u64,
        saved_writes: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseSpecReason {
    NotEnoughSavings,
    UnknownArity,
    ProvenanceNotExact,
    TagMetadataCost,
    NoSelectiveBenefit,
}

pub fn specialize_reuse(expr: CoreExpr) -> CoreExpr {
    specialize_with_env(expr, &ReuseEnv::default())
}

fn specialize_with_env(expr: CoreExpr, env: &ReuseEnv) -> CoreExpr {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(specialize_with_env(*body, &ReuseEnv::default())),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(specialize_with_env(*func, env)),
            args: args
                .into_iter()
                .map(|arg| specialize_with_env(arg, env))
                .collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(specialize_with_env(*func, env)),
            args: args
                .into_iter()
                .map(|arg| specialize_with_env(arg, env))
                .collect(),
            arg_modes,
            span,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = specialize_with_env(*rhs, env);
            let body_env = env.with_alias(var.id, &rhs);
            CoreExpr::Let {
                var,
                rhs: Box::new(rhs),
                body: Box::new(specialize_with_env(*body, &body_env)),
                span,
            }
        }
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = specialize_with_env(*rhs, env);
            let body_env = env.with_alias(var.id, &rhs);
            CoreExpr::LetRec {
                var,
                rhs: Box::new(rhs),
                body: Box::new(specialize_with_env(*body, &body_env)),
                span,
            }
        }
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => {
            let bindings: Vec<_> = bindings
                .into_iter()
                .map(|(var, rhs)| {
                    let rhs = specialize_with_env(*rhs, env);
                    (var, Box::new(rhs))
                })
                .collect();
            CoreExpr::LetRecGroup {
                bindings,
                body: Box::new(specialize_with_env(*body, env)),
                span,
            }
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee = specialize_with_env(*scrutinee, env);
            let scrutinee_var = match &scrutinee {
                CoreExpr::Var { var, .. } => Some(*var),
                _ => None,
            };
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let alt_pat_tag = match &alt.pat {
                        CorePat::Con { tag, .. } => Some(tag),
                        _ => None,
                    };
                    let alt_env = scrutinee_var
                        .as_ref()
                        .map(|var| {
                            env.with_pattern_bindings(var, alt_pat_binders.as_deref(), alt_pat_tag)
                        })
                        .unwrap_or_else(|| env.clone());
                    alt.rhs = specialize_with_env(alt.rhs, &alt_env);
                    alt.guard = alt.guard.map(|guard| specialize_with_env(guard, env));
                    alt
                })
                .collect();
            CoreExpr::Case {
                scrutinee: Box::new(scrutinee),
                alts,
                span,
            }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|field| specialize_with_env(field, env))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|arg| specialize_with_env(arg, env))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(specialize_with_env(*value, env)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args
                .into_iter()
                .map(|arg| specialize_with_env(arg, env))
                .collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(specialize_with_env(*body, env)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut handler| {
                    handler.body = specialize_with_env(handler.body, &ReuseEnv::default());
                    handler
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(specialize_with_env(*body, env)),
            span,
        },
        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(specialize_with_env(*body, env)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask: _,
            span,
        } => {
            let fields: Vec<CoreExpr> = fields
                .into_iter()
                .map(|field| specialize_with_env(field, env))
                .collect();
            let decision = decide_specialization(&token, &tag, &fields, env);
            let field_mask = match decision {
                ReuseSpecDecision::PlainReuse => None,
                ReuseSpecDecision::SelectiveWrite { field_mask, .. } => Some(field_mask),
            };
            CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                span,
            }
        }
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(specialize_with_env(*object, env)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(specialize_with_env(*object, env)),
            index,
            span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(specialize_with_env(*unique_body, env)),
            shared_body: Box::new(specialize_with_env(*shared_body, env)),
            span,
        },
    }
}

fn decide_specialization(
    token: &CoreVarRef,
    tag: &CoreTag,
    fields: &[CoreExpr],
    env: &ReuseEnv,
) -> ReuseSpecDecision {
    let Ok(candidate) = build_candidate(token, tag, fields, env) else {
        return ReuseSpecDecision::PlainReuse;
    };
    match profitability_reason(&candidate) {
        None => {
            let field_mask = compute_field_mask(candidate.arity, &candidate.unchanged_fields)
                .expect("candidate arity already validated");
            ReuseSpecDecision::SelectiveWrite {
                field_mask,
                saved_writes: candidate.unchanged_fields.len(),
            }
        }
        Some(_) => ReuseSpecDecision::PlainReuse,
    }
}

fn build_candidate(
    token: &CoreVarRef,
    tag: &CoreTag,
    fields: &[CoreExpr],
    env: &ReuseEnv,
) -> Result<ReuseSpecCandidate, ReuseSpecReason> {
    let Some(token_binder) = token.binder else {
        return Err(ReuseSpecReason::ProvenanceNotExact);
    };
    let arity = fields.len();
    if arity >= u64::BITS as usize {
        return Err(ReuseSpecReason::UnknownArity);
    }
    let unchanged_fields = env.exact_unchanged_field_indices(token_binder, fields);
    if unchanged_fields.is_empty() {
        if env.has_field_provenance_for_token(token_binder) {
            return Err(ReuseSpecReason::ProvenanceNotExact);
        }
        return Err(ReuseSpecReason::NoSelectiveBenefit);
    }
    Ok(ReuseSpecCandidate {
        token_binder,
        tag: tag.clone(),
        arity,
        unchanged_fields,
    })
}

fn profitability_reason(candidate: &ReuseSpecCandidate) -> Option<ReuseSpecReason> {
    let saved_writes = candidate.unchanged_fields.len();
    match candidate.tag {
        CoreTag::Cons | CoreTag::Some | CoreTag::Left | CoreTag::Right => {
            if saved_writes >= 1 {
                None
            } else {
                Some(ReuseSpecReason::NotEnoughSavings)
            }
        }
        CoreTag::Named(_) => {
            if saved_writes >= 2 {
                None
            } else {
                Some(ReuseSpecReason::TagMetadataCost)
            }
        }
        CoreTag::Nil | CoreTag::None => Some(ReuseSpecReason::NoSelectiveBenefit),
    }
}

fn compute_field_mask(arity: usize, unchanged_fields: &[usize]) -> Result<u64, ReuseSpecReason> {
    if arity >= u64::BITS as usize {
        return Err(ReuseSpecReason::UnknownArity);
    }
    let mut field_mask = 0u64;
    for field_index in 0..arity {
        if !unchanged_fields.contains(&field_index) {
            field_mask |= 1u64 << field_index;
        }
    }
    Ok(field_mask)
}

fn pat_field_binder_ids(pat: &CorePat) -> Option<Vec<Option<CoreBinderId>>> {
    match pat {
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => Some(
            fields
                .iter()
                .map(|field| match field {
                    CorePat::Var(binder) => Some(binder.id),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{CoreAlt, CoreBinder, CoreExpr, CorePat, CoreTag, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::{ReuseSpecDecision, specialize_reuse};

    fn s() -> Span {
        Span::default()
    }

    fn binder(raw: u32, name: crate::syntax::Identifier) -> CoreBinder {
        CoreBinder::new(crate::core::CoreBinderId(raw), name)
    }

    fn v(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(binder, s())
    }

    fn expect_mask(expr: CoreExpr) -> Option<u64> {
        match expr {
            CoreExpr::Let { body, .. } => expect_mask(*body),
            CoreExpr::Reuse { field_mask, .. } => field_mask,
            other => panic!("expected reuse shape, got {other:?}"),
        }
    }

    #[test]
    fn plain_list_reuse_becomes_masked() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Reuse {
                    token: CoreVarRef::resolved(xs),
                    tag: CoreTag::Cons,
                    fields: vec![v(h), v(t)],
                    field_mask: None,
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, Some(0));
    }

    #[test]
    fn named_adt_one_unchanged_field_stays_plain() {
        let mut interner = Interner::new();
        let node = interner.intern("Node");
        let t = binder(1, interner.intern("t"));
        let color = binder(2, interner.intern("color"));
        let left = binder(3, interner.intern("left"));
        let key = binder(4, interner.intern("key"));
        let right = binder(5, interner.intern("right"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(t)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Named(node),
                    fields: vec![
                        CorePat::Var(color),
                        CorePat::Var(left),
                        CorePat::Var(key),
                        CorePat::Var(right),
                    ],
                },
                guard: None,
                rhs: CoreExpr::Reuse {
                    token: CoreVarRef::resolved(t),
                    tag: CoreTag::Named(node),
                    fields: vec![
                        CoreExpr::Lit(crate::core::CoreLit::Int(1), s()),
                        CoreExpr::Con {
                            tag: CoreTag::Nil,
                            fields: vec![],
                            span: s(),
                        },
                        CoreExpr::Lit(crate::core::CoreLit::Int(0), s()),
                        v(right),
                    ],
                    field_mask: None,
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, None);
    }

    #[test]
    fn named_adt_two_unchanged_fields_becomes_masked() {
        let mut interner = Interner::new();
        let node = interner.intern("Node");
        let t = binder(1, interner.intern("t"));
        let color = binder(2, interner.intern("color"));
        let left = binder(3, interner.intern("left"));
        let key = binder(4, interner.intern("key"));
        let right = binder(5, interner.intern("right"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(t)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Named(node),
                    fields: vec![
                        CorePat::Var(color),
                        CorePat::Var(left),
                        CorePat::Var(key),
                        CorePat::Var(right),
                    ],
                },
                guard: None,
                rhs: CoreExpr::Reuse {
                    token: CoreVarRef::resolved(t),
                    tag: CoreTag::Named(node),
                    fields: vec![
                        CoreExpr::Lit(crate::core::CoreLit::Int(1), s()),
                        v(left),
                        v(key),
                        CoreExpr::Con {
                            tag: CoreTag::Nil,
                            fields: vec![],
                            span: s(),
                        },
                    ],
                    field_mask: None,
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, Some(0b1001));
    }

    #[test]
    fn option_payload_reuse_becomes_masked() {
        let mut interner = Interner::new();
        let opt = binder(1, interner.intern("opt"));
        let x = binder(2, interner.intern("x"));
        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(opt)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Some,
                    fields: vec![CorePat::Var(x)],
                },
                guard: None,
                rhs: CoreExpr::Reuse {
                    token: CoreVarRef::resolved(opt),
                    tag: CoreTag::Some,
                    fields: vec![v(x)],
                    field_mask: None,
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, Some(0));
    }

    #[test]
    fn provenance_lost_reuse_stays_plain() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let tmp = binder(4, interner.intern("tmp"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: tmp,
                    rhs: Box::new(CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::unresolved(interner.intern("escape")),
                            span: s(),
                        }),
                        args: vec![v(t)],
                        span: s(),
                    }),
                    body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(xs),
                        tag: CoreTag::Cons,
                        fields: vec![CoreExpr::Lit(crate::core::CoreLit::Int(0), s()), v(tmp)],
                        field_mask: None,
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, None);
    }

    #[test]
    fn branch_local_alias_join_recovers_exact_mask() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let flag = binder(2, interner.intern("flag"));
        let h = binder(3, interner.intern("h"));
        let t = binder(4, interner.intern("t"));
        let tail = binder(5, interner.intern("tail"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: tail,
                    rhs: Box::new(CoreExpr::Case {
                        scrutinee: Box::new(v(flag)),
                        alts: vec![
                            CoreAlt {
                                pat: CorePat::Lit(crate::core::CoreLit::Bool(true)),
                                guard: None,
                                rhs: v(t),
                                span: s(),
                            },
                            CoreAlt {
                                pat: CorePat::Wildcard,
                                guard: None,
                                rhs: v(t),
                                span: s(),
                            },
                        ],
                        span: s(),
                    }),
                    body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(xs),
                        tag: CoreTag::Cons,
                        fields: vec![v(h), v(tail)],
                        field_mask: None,
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, Some(0));
    }

    #[test]
    fn precompute_let_reuse_still_specializes_exact_unchanged_fields() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let y = binder(4, interner.intern("y"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: y,
                    rhs: Box::new(CoreExpr::PrimOp {
                        op: crate::core::CorePrimOp::Add,
                        args: vec![v(h), CoreExpr::Lit(crate::core::CoreLit::Int(1), s())],
                        span: s(),
                    }),
                    body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(xs),
                        tag: CoreTag::Cons,
                        fields: vec![v(y), v(t)],
                        field_mask: None,
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, Some(0b1));
    }

    #[test]
    fn ambiguous_branch_alias_join_stays_plain() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let flag = binder(2, interner.intern("flag"));
        let h = binder(3, interner.intern("h"));
        let t = binder(4, interner.intern("t"));
        let tail = binder(5, interner.intern("tail"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: tail,
                    rhs: Box::new(CoreExpr::Case {
                        scrutinee: Box::new(v(flag)),
                        alts: vec![
                            CoreAlt {
                                pat: CorePat::Lit(crate::core::CoreLit::Bool(true)),
                                guard: None,
                                rhs: v(t),
                                span: s(),
                            },
                            CoreAlt {
                                pat: CorePat::Wildcard,
                                guard: None,
                                rhs: v(h),
                                span: s(),
                            },
                        ],
                        span: s(),
                    }),
                    body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(xs),
                        tag: CoreTag::Cons,
                        fields: vec![CoreExpr::Lit(crate::core::CoreLit::Int(0), s()), v(tail)],
                        field_mask: None,
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        let field_mask = match specialized {
            CoreExpr::Case { alts, .. } => expect_mask(alts[0].rhs.clone()),
            _ => panic!("expected case"),
        };
        assert_eq!(field_mask, None);
    }

    #[test]
    fn unique_drop_spec_reuse_can_still_be_specialized() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));

        let expr = CoreExpr::Case {
            scrutinee: Box::new(v(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::DropSpecialized {
                    scrutinee: CoreVarRef::resolved(xs),
                    unique_body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(xs),
                        tag: CoreTag::Cons,
                        fields: vec![v(h), v(t)],
                        field_mask: None,
                        span: s(),
                    }),
                    shared_body: Box::new(CoreExpr::Con {
                        tag: CoreTag::Cons,
                        fields: vec![v(h), v(t)],
                        span: s(),
                    }),
                    span: s(),
                },
                span: s(),
            }],
            span: s(),
        };

        let specialized = specialize_reuse(expr);
        match specialized {
            CoreExpr::Case { alts, .. } => match &alts[0].rhs {
                CoreExpr::DropSpecialized { unique_body, .. } => {
                    assert_eq!(expect_mask((**unique_body).clone()), Some(0));
                }
                other => panic!("expected drop specialized, got {other:?}"),
            },
            _ => panic!("expected case"),
        }
    }

    #[test]
    fn decision_plain_reuse_is_exposed() {
        assert_eq!(ReuseSpecDecision::PlainReuse, ReuseSpecDecision::PlainReuse);
    }
}
