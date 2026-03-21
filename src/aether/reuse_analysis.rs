use std::collections::HashMap;

use crate::core::{CoreAlt, CoreBinderId, CoreExpr, CoreTag, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::use_counts;
use super::{into_constructor_shape_for_tag, is_heap_tag};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReuseOrigin {
    Scrutinee(CoreBinderId, CoreTag),
    Field {
        token_binder: CoreBinderId,
        tag: CoreTag,
        field_index: usize,
        binder_id: CoreBinderId,
    },
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct ReuseEnv {
    origins: HashMap<CoreBinderId, ReuseOrigin>,
}

impl ReuseEnv {
    pub fn seed(
        token: &CoreVarRef,
        pat_binders: Option<&[Option<CoreBinderId>]>,
        pat_tag: Option<&CoreTag>,
    ) -> Self {
        let mut env = Self::default();
        let Some(token_binder) = token.binder else {
            return env;
        };
        if let Some(tag) = pat_tag.cloned() {
            env.origins.insert(
                token_binder,
                ReuseOrigin::Scrutinee(token_binder, tag.clone()),
            );
            if let Some(fields) = pat_binders {
                for (field_index, binder_id) in fields.iter().enumerate() {
                    if let Some(binder_id) = binder_id {
                        env.origins.insert(
                            *binder_id,
                            ReuseOrigin::Field {
                                token_binder,
                                tag: tag.clone(),
                                field_index,
                                binder_id: *binder_id,
                            },
                        );
                    }
                }
            }
        }
        env
    }

    pub fn with_alias(&self, binder: CoreBinderId, rhs: &CoreExpr) -> Self {
        let mut next = self.clone();
        let origin = self.origin_of_expr(rhs).unwrap_or(ReuseOrigin::Unknown);
        next.origins.insert(binder, origin);
        next
    }

    pub fn with_pattern_origin(
        &self,
        origin: &ReuseOrigin,
        pat_binders: Option<&[Option<CoreBinderId>]>,
        pat_tag: Option<&CoreTag>,
    ) -> Self {
        let mut next = self.clone();
        let (token_binder, tag) = match origin {
            ReuseOrigin::Scrutinee(token_binder, tag) => (*token_binder, tag.clone()),
            _ => return next,
        };
        let Some(pat_tag) = pat_tag.cloned() else {
            return next;
        };
        if let Some(fields) = pat_binders {
            for (field_index, binder_id) in fields.iter().enumerate() {
                if let Some(binder_id) = binder_id {
                    next.origins.insert(
                        *binder_id,
                        ReuseOrigin::Field {
                            token_binder,
                            tag: pat_tag.clone(),
                            field_index,
                            binder_id: *binder_id,
                        },
                    );
                }
            }
        }
        next.origins
            .insert(token_binder, ReuseOrigin::Scrutinee(token_binder, tag));
        next
    }

    pub fn with_pattern_bindings(
        &self,
        token: &CoreVarRef,
        pat_binders: Option<&[Option<CoreBinderId>]>,
        pat_tag: Option<&CoreTag>,
    ) -> Self {
        let mut next = self.clone();
        let seeded = Self::seed(token, pat_binders, pat_tag);
        next.origins.extend(seeded.origins);
        next
    }

    pub fn unchanged_field_index(
        &self,
        token_binder: CoreBinderId,
        field_index: usize,
        expr: &CoreExpr,
    ) -> bool {
        let Some(origin) = self.origin_of_expr(expr) else {
            return false;
        };
        matches!(
            origin,
            ReuseOrigin::Field {
                token_binder: origin_token,
                field_index: origin_index,
                ..
            } if origin_token == token_binder && origin_index == field_index
        )
    }

    pub fn exact_unchanged_field_indices(
        &self,
        token_binder: CoreBinderId,
        fields: &[CoreExpr],
    ) -> Vec<usize> {
        fields
            .iter()
            .enumerate()
            .filter_map(|(field_index, field)| {
                self.unchanged_field_index(token_binder, field_index, field)
                    .then_some(field_index)
            })
            .collect()
    }

    pub fn has_field_provenance_for_token(&self, token_binder: CoreBinderId) -> bool {
        self.origins.values().any(|origin| {
            matches!(
                origin,
                ReuseOrigin::Field {
                    token_binder: origin_token,
                    ..
                } if *origin_token == token_binder
            )
        })
    }

    pub fn origin_of_expr(&self, expr: &CoreExpr) -> Option<ReuseOrigin> {
        match expr {
            CoreExpr::Var { var, .. } => var
                .binder
                .and_then(|binder_id| self.origins.get(&binder_id).cloned()),
            CoreExpr::Let { var, rhs, body, .. } => {
                if !is_admin_rhs(rhs) {
                    return None;
                }
                let child = self.with_alias(var.id, rhs);
                child.origin_of_expr(body)
            }
            CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => self.origin_of_expr(body),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                let scrutinee_origin = self.origin_of_expr(scrutinee);
                let mut branch_origin: Option<ReuseOrigin> = None;
                for alt in alts {
                    if alt.guard.is_some() {
                        return None;
                    }
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let alt_pat_tag = match &alt.pat {
                        crate::core::CorePat::Con { tag, .. } => Some(tag),
                        _ => None,
                    };
                    let alt_env = scrutinee_origin
                        .as_ref()
                        .map(|origin| {
                            self.with_pattern_origin(
                                origin,
                                alt_pat_binders.as_deref(),
                                alt_pat_tag,
                            )
                        })
                        .unwrap_or_else(|| self.clone());
                    let alt_origin = alt_env.origin_of_expr(&alt.rhs)?;
                    match &branch_origin {
                        None => branch_origin = Some(alt_origin),
                        Some(existing) if origins_equivalent(existing, &alt_origin) => {}
                        Some(_) => return None,
                    }
                }
                branch_origin
            }
            _ => None,
        }
    }
}

fn origins_equivalent(lhs: &ReuseOrigin, rhs: &ReuseOrigin) -> bool {
    match (lhs, rhs) {
        (
            ReuseOrigin::Scrutinee(lhs_token, lhs_tag),
            ReuseOrigin::Scrutinee(rhs_token, rhs_tag),
        ) => lhs_token == rhs_token && lhs_tag == rhs_tag,
        (
            ReuseOrigin::Field {
                token_binder: lhs_token,
                tag: lhs_tag,
                field_index: lhs_field,
                ..
            },
            ReuseOrigin::Field {
                token_binder: rhs_token,
                tag: rhs_tag,
                field_index: rhs_field,
                ..
            },
        ) => lhs_token == rhs_token && lhs_tag == rhs_tag && lhs_field == rhs_field,
        (ReuseOrigin::Unknown, ReuseOrigin::Unknown) => true,
        _ => false,
    }
}

fn pat_field_binder_ids(pat: &crate::core::CorePat) -> Option<Vec<Option<CoreBinderId>>> {
    match pat {
        crate::core::CorePat::Con { fields, .. } | crate::core::CorePat::Tuple(fields) => Some(
            fields
                .iter()
                .map(|field| match field {
                    crate::core::CorePat::Var(binder) => Some(binder.id),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseFailureReason {
    ShapeMismatch,
    TokenEscapesIntoFields,
    ProvenanceLost,
    BranchAmbiguity,
    EffectfulBoundary,
    SharedBranchOnly,
}

impl ReuseFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReuseFailureReason::ShapeMismatch => "ShapeMismatch",
            ReuseFailureReason::TokenEscapesIntoFields => "TokenEscapesIntoFields",
            ReuseFailureReason::ProvenanceLost => "ProvenanceLost",
            ReuseFailureReason::BranchAmbiguity => "BranchAmbiguity",
            ReuseFailureReason::EffectfulBoundary => "EffectfulBoundary",
            ReuseFailureReason::SharedBranchOnly => "SharedBranchOnly",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReuseRewrite {
    pub expr: CoreExpr,
    pub reused: bool,
    pub reason: Option<ReuseFailureReason>,
}

pub fn rewrite_drop_body(
    token: &CoreVarRef,
    body: CoreExpr,
    drop_span: Span,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
    blocked_outer_token: Option<CoreBinderId>,
) -> ReuseRewrite {
    let env = ReuseEnv::seed(token, pat_binders, pat_tag);
    rewrite_drop_body_with_env(token, body, drop_span, pat_tag, blocked_outer_token, &env)
}

pub fn diagnose_drop_body(
    token: &CoreVarRef,
    body: &CoreExpr,
    pat_binders: Option<&[Option<CoreBinderId>]>,
    pat_tag: Option<&CoreTag>,
    blocked_outer_token: Option<CoreBinderId>,
) -> Option<ReuseFailureReason> {
    let rewritten = rewrite_drop_body(
        token,
        body.clone(),
        body.span(),
        pat_binders,
        pat_tag,
        blocked_outer_token,
    );
    if rewritten.reused {
        None
    } else {
        rewritten.reason
    }
}

fn rewrite_drop_body_with_env(
    token: &CoreVarRef,
    body: CoreExpr,
    drop_span: Span,
    pat_tag: Option<&CoreTag>,
    blocked_outer_token: Option<CoreBinderId>,
    env: &ReuseEnv,
) -> ReuseRewrite {
    let Some(token_binder) = token.binder else {
        return ReuseRewrite {
            expr: CoreExpr::Drop {
                var: *token,
                body: Box::new(body),
                span: drop_span,
            },
            reused: false,
            reason: Some(ReuseFailureReason::ProvenanceLost),
        };
    };

    match body {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            if use_counts(&rhs).contains_key(&token_binder) {
                return no_rewrite(
                    CoreExpr::Let {
                        var,
                        rhs,
                        body,
                        span,
                    },
                    ReuseFailureReason::TokenEscapesIntoFields,
                );
            }
            if !is_admin_rhs(&rhs) {
                return no_rewrite(
                    CoreExpr::Let {
                        var,
                        rhs,
                        body,
                        span,
                    },
                    ReuseFailureReason::EffectfulBoundary,
                );
            }
            let child_env = env.with_alias(var.id, &rhs);
            let inner = rewrite_drop_body_with_env(
                token,
                *body,
                drop_span,
                pat_tag,
                blocked_outer_token,
                &child_env,
            );
            if inner.reused {
                ReuseRewrite {
                    expr: CoreExpr::Let {
                        var,
                        rhs,
                        body: Box::new(inner.expr),
                        span,
                    },
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    CoreExpr::Let {
                        var,
                        rhs,
                        body: Box::new(inner.expr),
                        span,
                    },
                    inner.reason.unwrap_or(ReuseFailureReason::ProvenanceLost),
                )
            }
        }
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => no_rewrite(
            CoreExpr::LetRec {
                var,
                rhs,
                body,
                span,
            },
            ReuseFailureReason::EffectfulBoundary,
        ),
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee_origin = env.origin_of_expr(&scrutinee);
            let scrutinee_is_plain_token = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => match var.binder {
                    Some(binder_id) if binder_id == token_binder => true,
                    Some(binder_id) => matches!(
                        env.origins.get(&binder_id),
                        Some(ReuseOrigin::Scrutinee(origin_token, _)) if *origin_token == token_binder
                    ),
                    None => false,
                },
                _ => false,
            };
            if (!scrutinee_is_plain_token && use_counts(&scrutinee).contains_key(&token_binder))
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| use_counts(guard).contains_key(&token_binder))
                })
            {
                return no_rewrite(
                    CoreExpr::Case {
                        scrutinee,
                        alts,
                        span,
                    },
                    ReuseFailureReason::BranchAmbiguity,
                );
            }

            let mut any_reused = false;
            let mut reasons = Vec::new();
            let new_alts = alts
                .into_iter()
                .map(|alt| {
                    let alt_pat_binders = pat_field_binder_ids(&alt.pat);
                    let alt_pat_tag = match &alt.pat {
                        crate::core::CorePat::Con { tag, .. } => Some(tag),
                        _ => None,
                    };
                    let alt_env = scrutinee_origin
                        .as_ref()
                        .map(|origin| {
                            env.with_pattern_origin(origin, alt_pat_binders.as_deref(), alt_pat_tag)
                        })
                        .unwrap_or_else(|| env.clone());
                    if use_counts(&alt.rhs).contains_key(&token_binder) {
                        reasons.push(ReuseFailureReason::BranchAmbiguity);
                        return alt;
                    }

                    let inner = rewrite_drop_body_with_env(
                        token,
                        alt.rhs,
                        drop_span,
                        pat_tag,
                        blocked_outer_token,
                        &alt_env,
                    );
                    if inner.reused {
                        any_reused = true;
                    } else if let Some(reason) = inner.reason {
                        reasons.push(reason);
                    }
                    CoreAlt {
                        rhs: if inner.reused {
                            inner.expr
                        } else {
                            CoreExpr::Drop {
                                var: *token,
                                body: Box::new(inner.expr),
                                span: drop_span,
                            }
                        },
                        ..alt
                    }
                })
                .collect();

            if any_reused {
                ReuseRewrite {
                    expr: CoreExpr::Case {
                        scrutinee,
                        alts: new_alts,
                        span,
                    },
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    CoreExpr::Case {
                        scrutinee,
                        alts: new_alts,
                        span,
                    },
                    choose_reason(&reasons),
                )
            }
        }
        other => match build_reuse_expr(token, other.clone(), env, pat_tag, blocked_outer_token) {
            Ok(expr) => ReuseRewrite {
                expr,
                reused: true,
                reason: None,
            },
            Err(reason) => no_rewrite(other, reason),
        },
    }
}

fn build_reuse_expr(
    token: &CoreVarRef,
    body: CoreExpr,
    _env: &ReuseEnv,
    pat_tag: Option<&CoreTag>,
    blocked_outer_token: Option<CoreBinderId>,
) -> Result<CoreExpr, ReuseFailureReason> {
    let Some(token_binder) = token.binder else {
        return Err(ReuseFailureReason::ProvenanceLost);
    };
    let (tag, fields, span) =
        into_constructor_shape_for_tag(body, pat_tag).ok_or(ReuseFailureReason::ShapeMismatch)?;
    if !is_heap_tag(&tag) {
        return Err(ReuseFailureReason::ShapeMismatch);
    }
    if fields
        .iter()
        .any(|field| use_counts(field).contains_key(&token_binder))
    {
        return Err(ReuseFailureReason::TokenEscapesIntoFields);
    }
    if blocked_outer_token == Some(token_binder) {
        return Err(ReuseFailureReason::SharedBranchOnly);
    }

    Ok(CoreExpr::Reuse {
        token: *token,
        tag,
        fields,
        field_mask: None,
        span,
    })
}

fn no_rewrite(body: CoreExpr, reason: ReuseFailureReason) -> ReuseRewrite {
    ReuseRewrite {
        expr: body,
        reused: false,
        reason: Some(reason),
    }
}

fn choose_reason(reasons: &[ReuseFailureReason]) -> ReuseFailureReason {
    reasons
        .iter()
        .copied()
        .find(|reason| *reason != ReuseFailureReason::ShapeMismatch)
        .or_else(|| reasons.first().copied())
        .unwrap_or(ReuseFailureReason::ProvenanceLost)
}

fn is_admin_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => true,
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            fields.iter().all(is_admin_rhs)
        }
        CoreExpr::App { .. }
        | CoreExpr::AetherCall { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::Lam { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Dup { .. }
        | CoreExpr::Drop { .. }
        | CoreExpr::DropSpecialized { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{ReuseFailureReason, diagnose_drop_body, rewrite_drop_body};
    use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CoreTag, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    fn s() -> Span {
        Span::default()
    }

    fn binder(raw: u32, name: crate::syntax::Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), name)
    }

    fn v(binder: CoreBinder) -> CoreExpr {
        CoreExpr::bound_var(binder, s())
    }

    #[test]
    fn rewrites_list_alias_binding_to_reuse() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let tail = binder(4, interner.intern("tail"));

        let pat_binders = vec![Some(h.id), Some(t.id)];
        let body = CoreExpr::Let {
            var: tail,
            rhs: Box::new(v(t)),
            body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(h), v(tail)],
                span: s(),
            }),
            span: s(),
        };

        let rewritten = rewrite_drop_body(
            &CoreVarRef::resolved(xs),
            body,
            s(),
            Some(&pat_binders),
            Some(&CoreTag::Cons),
            None,
        );

        assert!(rewritten.reused);
        match rewritten.expr {
            CoreExpr::Let { body, .. } => match *body {
                CoreExpr::Reuse {
                    tag,
                    field_mask,
                    token,
                    ..
                } => {
                    assert_eq!(tag, CoreTag::Cons);
                    assert_eq!(token.binder, Some(xs.id));
                    assert_eq!(field_mask, None);
                }
                other => panic!("expected reuse under let spine, got {other:?}"),
            },
            other => panic!("expected let spine to be preserved, got {other:?}"),
        }
    }

    #[test]
    fn rewrites_named_adt_alias_binding_to_masked_reuse() {
        let mut interner = Interner::new();
        let node_name = interner.intern("Node");
        let t = binder(1, interner.intern("t"));
        let color = binder(2, interner.intern("color"));
        let left = binder(3, interner.intern("left"));
        let key = binder(4, interner.intern("key"));
        let right = binder(5, interner.intern("right"));
        let saved = binder(6, interner.intern("saved"));

        let pat_tag = CoreTag::Named(node_name);
        let pat_binders = vec![Some(color.id), Some(left.id), Some(key.id), Some(right.id)];
        let body = CoreExpr::Let {
            var: saved,
            rhs: Box::new(v(left)),
            body: Box::new(CoreExpr::Con {
                tag: pat_tag.clone(),
                fields: vec![v(color), v(saved), v(key), v(right)],
                span: s(),
            }),
            span: s(),
        };

        let rewritten = rewrite_drop_body(
            &CoreVarRef::resolved(t),
            body,
            s(),
            Some(&pat_binders),
            Some(&pat_tag),
            None,
        );

        assert!(rewritten.reused);
        match rewritten.expr {
            CoreExpr::Let { body, .. } => match *body {
                CoreExpr::Reuse {
                    tag,
                    field_mask,
                    token,
                    ..
                } => {
                    assert_eq!(tag, pat_tag);
                    assert_eq!(token.binder, Some(t.id));
                    assert_eq!(field_mask, None);
                }
                other => panic!("expected reuse under ADT let spine, got {other:?}"),
            },
            other => panic!("expected let spine to be preserved, got {other:?}"),
        }
    }

    #[test]
    fn rewrites_option_alias_binding_to_reuse() {
        let mut interner = Interner::new();
        let opt = binder(1, interner.intern("opt"));
        let x = binder(2, interner.intern("x"));
        let y = binder(3, interner.intern("y"));

        let pat_binders = vec![Some(x.id)];
        let body = CoreExpr::Let {
            var: y,
            rhs: Box::new(v(x)),
            body: Box::new(CoreExpr::Con {
                tag: CoreTag::Some,
                fields: vec![v(y)],
                span: s(),
            }),
            span: s(),
        };

        let rewritten = rewrite_drop_body(
            &CoreVarRef::resolved(opt),
            body,
            s(),
            Some(&pat_binders),
            Some(&CoreTag::Some),
            None,
        );

        assert!(rewritten.reused);
        match rewritten.expr {
            CoreExpr::Let { body, .. } => match *body {
                CoreExpr::Reuse { tag, token, .. } => {
                    assert_eq!(tag, CoreTag::Some);
                    assert_eq!(token.binder, Some(opt.id));
                }
                other => panic!("expected reuse under option let spine, got {other:?}"),
            },
            other => panic!("expected let spine to be preserved, got {other:?}"),
        }
    }

    #[test]
    fn effectful_intermediate_binding_blocks_reuse() {
        let mut interner = Interner::new();
        let node_name = interner.intern("Node");
        let escape_name = interner.intern("escape");
        let t = binder(1, interner.intern("t"));
        let color = binder(2, interner.intern("color"));
        let left = binder(3, interner.intern("left"));
        let key = binder(4, interner.intern("key"));
        let right = binder(5, interner.intern("right"));
        let saved = binder(6, interner.intern("saved"));

        let pat_tag = CoreTag::Named(node_name);
        let pat_binders = vec![Some(color.id), Some(left.id), Some(key.id), Some(right.id)];
        let body = CoreExpr::Let {
            var: saved,
            rhs: Box::new(CoreExpr::App {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::unresolved(escape_name),
                    span: s(),
                }),
                args: vec![v(key)],
                span: s(),
            }),
            body: Box::new(CoreExpr::Con {
                tag: pat_tag.clone(),
                fields: vec![v(color), v(left), v(saved), v(right)],
                span: s(),
            }),
            span: s(),
        };

        let reason = diagnose_drop_body(
            &CoreVarRef::resolved(t),
            &body,
            Some(&pat_binders),
            Some(&pat_tag),
            None,
        );

        assert_eq!(reason, Some(ReuseFailureReason::EffectfulBoundary));
    }
}
