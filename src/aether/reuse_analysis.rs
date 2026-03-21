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
    Forwarded {
        tag: CoreTag,
        fields: Vec<ForwardedFieldOrigin>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardedFieldOrigin {
    Exact(Box<ReuseOrigin>),
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct ReuseEnv {
    origins: HashMap<CoreBinderId, ReuseOrigin>,
    aliases: HashMap<CoreBinderId, CoreExpr>,
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
        next.aliases.insert(binder, rhs.clone());
        next
    }

    pub fn with_pattern_origin(
        &self,
        origin: &ReuseOrigin,
        pat_binders: Option<&[Option<CoreBinderId>]>,
        pat_tag: Option<&CoreTag>,
    ) -> Self {
        let mut next = self.clone();
        match origin {
            ReuseOrigin::Scrutinee(token_binder, tag) => {
                let Some(pat_tag) = pat_tag.cloned() else {
                    return next;
                };
                if let Some(fields) = pat_binders {
                    for (field_index, binder_id) in fields.iter().enumerate() {
                        if let Some(binder_id) = binder_id {
                            next.origins.insert(
                                *binder_id,
                                ReuseOrigin::Field {
                                    token_binder: *token_binder,
                                    tag: pat_tag.clone(),
                                    field_index,
                                    binder_id: *binder_id,
                                },
                            );
                        }
                    }
                }
                next.origins.insert(
                    *token_binder,
                    ReuseOrigin::Scrutinee(*token_binder, tag.clone()),
                );
            }
            ReuseOrigin::Field {
                binder_id: field_token_binder,
                ..
            } => {
                let Some(pat_tag) = pat_tag.cloned() else {
                    return next;
                };
                next.origins.insert(
                    *field_token_binder,
                    ReuseOrigin::Scrutinee(*field_token_binder, pat_tag.clone()),
                );
                if let Some(fields) = pat_binders {
                    for (field_index, binder_id) in fields.iter().enumerate() {
                        if let Some(binder_id) = binder_id {
                            next.origins.insert(
                                *binder_id,
                                ReuseOrigin::Field {
                                    token_binder: *field_token_binder,
                                    tag: pat_tag.clone(),
                                    field_index,
                                    binder_id: *binder_id,
                                },
                            );
                        }
                    }
                }
            }
            ReuseOrigin::Forwarded { tag, fields } => {
                let Some(pat_tag) = pat_tag else {
                    return next;
                };
                if tag != pat_tag {
                    return next;
                }
                if let Some(pat_binders) = pat_binders {
                    for (binder_id, field_origin) in pat_binders.iter().zip(fields.iter()) {
                        if let (Some(binder_id), Some(origin)) = (binder_id, field_origin.exact()) {
                            next.origins.insert(*binder_id, origin.clone());
                        }
                    }
                }
            }
            ReuseOrigin::Unknown => {}
        }
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
            CoreExpr::Con { tag, fields, .. } => {
                let field_origins = fields
                    .iter()
                    .map(|field| {
                        self.origin_of_expr(field)
                            .map(ForwardedFieldOrigin::from_origin)
                            .unwrap_or(ForwardedFieldOrigin::Unknown)
                    })
                    .collect::<Vec<_>>();
                if !field_origins.iter().any(|field| field.exact().is_some()) {
                    return None;
                }
                if has_duplicate_exact_origins(&field_origins) {
                    return None;
                }
                Some(ReuseOrigin::Forwarded {
                    tag: tag.clone(),
                    fields: field_origins,
                })
            }
            CoreExpr::Reuse { tag, fields, .. } => {
                let field_origins = fields
                    .iter()
                    .map(|field| {
                        self.origin_of_expr(field)
                            .map(ForwardedFieldOrigin::from_origin)
                            .unwrap_or(ForwardedFieldOrigin::Unknown)
                    })
                    .collect::<Vec<_>>();
                if !field_origins.iter().any(|field| field.exact().is_some()) {
                    return None;
                }
                if has_duplicate_exact_origins(&field_origins) {
                    return None;
                }
                Some(ReuseOrigin::Forwarded {
                    tag: tag.clone(),
                    fields: field_origins,
                })
            }
            CoreExpr::Let { var, rhs, body, .. } => {
                if !is_alias_preserving_rhs(rhs) {
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
        (
            ReuseOrigin::Forwarded {
                tag: lhs_tag,
                fields: lhs_fields,
            },
            ReuseOrigin::Forwarded {
                tag: rhs_tag,
                fields: rhs_fields,
            },
        ) => {
            lhs_tag == rhs_tag
                && lhs_fields.len() == rhs_fields.len()
                && lhs_fields
                    .iter()
                    .zip(rhs_fields.iter())
                    .all(|(lhs, rhs)| forwarded_field_origins_equivalent(lhs, rhs))
        }
        (ReuseOrigin::Unknown, ReuseOrigin::Unknown) => true,
        _ => false,
    }
}

fn forwarded_field_origins_equivalent(
    lhs: &ForwardedFieldOrigin,
    rhs: &ForwardedFieldOrigin,
) -> bool {
    match (lhs, rhs) {
        (ForwardedFieldOrigin::Unknown, ForwardedFieldOrigin::Unknown) => true,
        (ForwardedFieldOrigin::Exact(lhs), ForwardedFieldOrigin::Exact(rhs)) => {
            origins_equivalent(lhs, rhs)
        }
        _ => false,
    }
}

fn has_duplicate_exact_origins(fields: &[ForwardedFieldOrigin]) -> bool {
    fields.iter().enumerate().any(|(idx, origin)| {
        let Some(origin) = origin.exact() else {
            return false;
        };
        fields[idx + 1..]
            .iter()
            .filter_map(ForwardedFieldOrigin::exact)
            .any(|other| origins_equivalent(origin, other))
    })
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
            if token_appears_in_expr(token_binder, &rhs) {
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
            if !is_safe_precompute_rhs(token_binder, &rhs) {
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
            if (!scrutinee_is_plain_token && token_appears_in_expr(token_binder, &scrutinee))
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| token_appears_in_expr(token_binder, guard))
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
                    if token_appears_in_expr(token_binder, &alt.rhs) {
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

#[allow(dead_code)]
fn rewrite_nested_drop_sites(
    expr: CoreExpr,
    env: &ReuseEnv,
    blocked_outer_token: Option<CoreBinderId>,
) -> ReuseRewrite {
    match expr {
        CoreExpr::Drop { var, body, span } => {
            let pat_tag = var
                .binder
                .and_then(|binder_id| env.origins.get(&binder_id))
                .and_then(origin_tag);
            rewrite_drop_body_with_env(
                &var,
                *body,
                span,
                pat_tag.as_ref(),
                blocked_outer_token,
                env,
            )
        }
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs_inner = rewrite_nested_drop_sites(*rhs, env, blocked_outer_token);
            let rhs = rhs_inner.expr;
            let child_env = env.with_alias(var.id, &rhs);
            let body_inner = rewrite_nested_drop_sites(*body, &child_env, blocked_outer_token);
            let reused = rhs_inner.reused || body_inner.reused;
            let expr = CoreExpr::Let {
                var,
                rhs: Box::new(rhs),
                body: Box::new(body_inner.expr),
                span,
            };
            if reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    rhs_inner
                        .reason
                        .or(body_inner.reason)
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee_origin = env.origin_of_expr(&scrutinee);
            let mut reused = false;
            let mut reasons = Vec::new();
            let alts = alts
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
                    let rhs_inner =
                        rewrite_nested_drop_sites(alt.rhs, &alt_env, blocked_outer_token);
                    reused |= rhs_inner.reused;
                    if let Some(reason) = rhs_inner.reason {
                        reasons.push(reason);
                    }
                    CoreAlt {
                        rhs: rhs_inner.expr,
                        guard: alt.guard.map(|guard| {
                            rewrite_nested_drop_sites(guard, env, blocked_outer_token).expr
                        }),
                        ..alt
                    }
                })
                .collect();
            if reused {
                ReuseRewrite {
                    expr: CoreExpr::Case {
                        scrutinee,
                        alts,
                        span,
                    },
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    CoreExpr::Case {
                        scrutinee,
                        alts,
                        span,
                    },
                    choose_reason(&reasons),
                )
            }
        }
        CoreExpr::Con { tag, fields, span } => {
            rewrite_nested_children(fields, env, blocked_outer_token, |fields| CoreExpr::Con {
                tag,
                fields,
                span,
            })
        }
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => rewrite_nested_children(fields, env, blocked_outer_token, |fields| CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        }),
        CoreExpr::App { func, args, span } => {
            let func_inner = rewrite_nested_drop_sites(*func, env, blocked_outer_token);
            let args_inner = args
                .into_iter()
                .map(|arg| rewrite_nested_drop_sites(arg, env, blocked_outer_token))
                .collect::<Vec<_>>();
            let reused = func_inner.reused || args_inner.iter().any(|arg| arg.reused);
            let args = args_inner.into_iter().map(|arg| arg.expr).collect();
            let expr = CoreExpr::App {
                func: Box::new(func_inner.expr),
                args,
                span,
            };
            if reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    func_inner
                        .reason
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => {
            let func_inner = rewrite_nested_drop_sites(*func, env, blocked_outer_token);
            let args_inner = args
                .into_iter()
                .map(|arg| rewrite_nested_drop_sites(arg, env, blocked_outer_token))
                .collect::<Vec<_>>();
            let reused = func_inner.reused || args_inner.iter().any(|arg| arg.reused);
            let args = args_inner.into_iter().map(|arg| arg.expr).collect();
            let expr = CoreExpr::AetherCall {
                func: Box::new(func_inner.expr),
                args,
                arg_modes,
                span,
            };
            if reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    func_inner
                        .reason
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::PrimOp { op, args, span } => {
            rewrite_nested_children(args, env, blocked_outer_token, |args| CoreExpr::PrimOp {
                op,
                args,
                span,
            })
        }
        CoreExpr::Return { value, span } => {
            let inner = rewrite_nested_drop_sites(*value, env, blocked_outer_token);
            let expr = CoreExpr::Return {
                value: Box::new(inner.expr),
                span,
            };
            if inner.reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    inner.reason.unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => rewrite_nested_children(args, env, blocked_outer_token, |args| CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        }),
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => {
            let body_inner = rewrite_nested_drop_sites(*body, env, blocked_outer_token);
            let handlers = handlers
                .into_iter()
                .map(|mut handler| {
                    handler.body = rewrite_nested_drop_sites(
                        handler.body,
                        &ReuseEnv::default(),
                        blocked_outer_token,
                    )
                    .expr;
                    handler
                })
                .collect();
            let expr = CoreExpr::Handle {
                body: Box::new(body_inner.expr),
                effect,
                handlers,
                span,
            };
            if body_inner.reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    body_inner
                        .reason
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => {
            let unique = rewrite_nested_drop_sites(*unique_body, env, blocked_outer_token);
            let shared = rewrite_nested_drop_sites(*shared_body, env, blocked_outer_token);
            let expr = CoreExpr::DropSpecialized {
                scrutinee,
                unique_body: Box::new(unique.expr),
                shared_body: Box::new(shared.expr),
                span,
            };
            if unique.reused || shared.reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    unique
                        .reason
                        .or(shared.reason)
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        CoreExpr::Lam { params, body, span } => {
            let inner = rewrite_nested_drop_sites(*body, &ReuseEnv::default(), blocked_outer_token);
            let expr = CoreExpr::Lam {
                params,
                body: Box::new(inner.expr),
                span,
            };
            if inner.reused {
                ReuseRewrite {
                    expr,
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    expr,
                    inner.reason.unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        other => no_rewrite(other, ReuseFailureReason::ShapeMismatch),
    }
}

#[allow(dead_code)]
fn rewrite_nested_children<F>(
    children: Vec<CoreExpr>,
    env: &ReuseEnv,
    blocked_outer_token: Option<CoreBinderId>,
    rebuild: F,
) -> ReuseRewrite
where
    F: FnOnce(Vec<CoreExpr>) -> CoreExpr,
{
    let rewritten = children
        .into_iter()
        .map(|child| rewrite_nested_drop_sites(child, env, blocked_outer_token))
        .collect::<Vec<_>>();
    let reused = rewritten.iter().any(|child| child.reused);
    let reason = rewritten
        .iter()
        .find_map(|child| child.reason)
        .unwrap_or(ReuseFailureReason::ShapeMismatch);
    let expr = rebuild(rewritten.into_iter().map(|child| child.expr).collect());
    if reused {
        ReuseRewrite {
            expr,
            reused: true,
            reason: None,
        }
    } else {
        no_rewrite(expr, reason)
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
        .any(|field| token_appears_in_expr(token_binder, field))
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

#[allow(dead_code)]
fn rewrite_forwarded_wrapper_body(
    outer_token: &CoreVarRef,
    expr: CoreExpr,
    pat_tag: Option<&CoreTag>,
    env: &ReuseEnv,
    blocked_outer_token: Option<CoreBinderId>,
) -> ReuseRewrite {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            if token_appears_in_expr(outer_token.binder.unwrap_or(CoreBinderId(0)), &rhs) {
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
            if !is_safe_precompute_rhs(outer_token.binder.unwrap_or(CoreBinderId(0)), &rhs) {
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
            let rhs = *rhs;
            let child_env = env.with_alias(var.id, &rhs);
            let body_inner = rewrite_forwarded_wrapper_body(
                outer_token,
                *body,
                pat_tag,
                &child_env,
                blocked_outer_token,
            );
            if body_inner.reused {
                ReuseRewrite {
                    expr: CoreExpr::Let {
                        var,
                        rhs: Box::new(rhs),
                        body: Box::new(body_inner.expr),
                        span,
                    },
                    reused: true,
                    reason: None,
                }
            } else {
                no_rewrite(
                    CoreExpr::Let {
                        var,
                        rhs: Box::new(rhs),
                        body: Box::new(body_inner.expr),
                        span,
                    },
                    body_inner
                        .reason
                        .unwrap_or(ReuseFailureReason::ShapeMismatch),
                )
            }
        }
        other => try_rewrite_forwarded_wrapper_constructor(
            outer_token,
            other,
            pat_tag,
            env,
            blocked_outer_token,
        ),
    }
}

#[allow(dead_code)]
fn try_rewrite_forwarded_wrapper_constructor(
    outer_token: &CoreVarRef,
    body: CoreExpr,
    pat_tag: Option<&CoreTag>,
    env: &ReuseEnv,
    blocked_outer_token: Option<CoreBinderId>,
) -> ReuseRewrite {
    let Some(outer_token_binder) = outer_token.binder else {
        return no_rewrite(body, ReuseFailureReason::ProvenanceLost);
    };
    let Some((tag, fields, span)) = constructor_shape_for_expr(&body, pat_tag) else {
        return no_rewrite(body, ReuseFailureReason::ShapeMismatch);
    };

    let mut rewritten_fields = Vec::with_capacity(fields.len());
    let mut reused = false;
    for field in fields {
        if field_has_token_provenance(env, outer_token_binder, &field)
            && let Some(field_expr) = try_rewrite_forwarded_child_field(
                outer_token_binder,
                field.clone(),
                env,
                blocked_outer_token,
            )
        {
            rewritten_fields.push(field_expr);
            reused = true;
            continue;
        }
        rewritten_fields.push(field);
    }

    if reused {
        ReuseRewrite {
            expr: rebuild_constructor_shape(body, tag, rewritten_fields, span),
            reused: true,
            reason: None,
        }
    } else {
        no_rewrite(
            rebuild_constructor_shape(body, tag, rewritten_fields, span),
            ReuseFailureReason::ShapeMismatch,
        )
    }
}

#[allow(dead_code)]
fn try_rewrite_forwarded_child_field(
    child_token_binder: CoreBinderId,
    expr: CoreExpr,
    env: &ReuseEnv,
    blocked_outer_token: Option<CoreBinderId>,
) -> Option<CoreExpr> {
    let resolved = expand_alias_expr(&expr, env);
    let rewritten = build_child_reuse_expr(
        &CoreVarRef {
            name: crate::syntax::Identifier::new(0),
            binder: Some(child_token_binder),
        },
        resolved,
        blocked_outer_token,
    )?;
    if token_appears_in_expr(child_token_binder, &rewritten) {
        return None;
    }
    Some(rewritten)
}

#[allow(dead_code)]
fn build_child_reuse_expr(
    token: &CoreVarRef,
    expr: CoreExpr,
    blocked_outer_token: Option<CoreBinderId>,
) -> Option<CoreExpr> {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let token_binder = token.binder?;
            if token_appears_in_expr(token_binder, &rhs)
                || !is_safe_precompute_rhs(token_binder, &rhs)
            {
                return None;
            }
            Some(CoreExpr::Let {
                var,
                rhs,
                body: Box::new(build_child_reuse_expr(token, *body, blocked_outer_token)?),
                span,
            })
        }
        other => {
            let token_binder = token.binder?;
            let (tag, fields, span) = into_constructor_shape_for_tag(other, None)?;
            if !is_heap_tag(&tag)
                || blocked_outer_token == Some(token_binder)
                || fields
                    .iter()
                    .any(|field| token_appears_in_expr(token_binder, field))
            {
                return None;
            }
            Some(CoreExpr::Reuse {
                token: *token,
                tag,
                fields,
                field_mask: None,
                span,
            })
        }
    }
}

#[allow(dead_code)]
fn expand_alias_expr(expr: &CoreExpr, env: &ReuseEnv) -> CoreExpr {
    match expr {
        CoreExpr::Var { var, .. } => var
            .binder
            .and_then(|binder_id| env.aliases.get(&binder_id).cloned())
            .unwrap_or_else(|| expr.clone()),
        _ => expr.clone(),
    }
}

#[allow(dead_code)]
fn constructor_shape_for_expr(
    expr: &CoreExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<CoreExpr>, Span)> {
    into_constructor_shape_for_tag(expr.clone(), expected_tag).or_else(|| match expr {
        CoreExpr::App { func, args, span }
        | CoreExpr::AetherCall {
            func, args, span, ..
        } => {
            let CoreExpr::Var { var, .. } = func.as_ref() else {
                return None;
            };
            (var.binder.is_none()).then(|| (CoreTag::Named(var.name), args.clone(), *span))
        }
        _ => None,
    })
}

#[allow(dead_code)]
fn rebuild_constructor_shape(
    original: CoreExpr,
    tag: CoreTag,
    fields: Vec<CoreExpr>,
    span: Span,
) -> CoreExpr {
    match original {
        CoreExpr::Con { .. } => CoreExpr::Con { tag, fields, span },
        CoreExpr::App { func, .. } => CoreExpr::App {
            func,
            args: fields,
            span,
        },
        CoreExpr::AetherCall {
            func, arg_modes, ..
        } => CoreExpr::AetherCall {
            func,
            args: fields,
            arg_modes,
            span,
        },
        other => other,
    }
}

#[allow(dead_code)]
fn field_has_token_provenance(env: &ReuseEnv, token_binder: CoreBinderId, expr: &CoreExpr) -> bool {
    env.origin_of_expr(expr)
        .as_ref()
        .is_some_and(|origin| origin_mentions_token(origin, token_binder))
}

#[allow(dead_code)]
fn origin_mentions_token(origin: &ReuseOrigin, token_binder: CoreBinderId) -> bool {
    match origin {
        ReuseOrigin::Scrutinee(origin_token, _) => *origin_token == token_binder,
        ReuseOrigin::Field {
            token_binder: origin_token,
            ..
        } => *origin_token == token_binder,
        ReuseOrigin::Forwarded { fields, .. } => fields.iter().any(|field| {
            field
                .exact()
                .is_some_and(|origin| origin_mentions_token(origin, token_binder))
        }),
        ReuseOrigin::Unknown => false,
    }
}

#[allow(dead_code)]
fn origin_tag(origin: &ReuseOrigin) -> Option<CoreTag> {
    match origin {
        ReuseOrigin::Scrutinee(_, tag)
        | ReuseOrigin::Field { tag, .. }
        | ReuseOrigin::Forwarded { tag, .. } => Some(tag.clone()),
        ReuseOrigin::Unknown => None,
    }
}

impl ForwardedFieldOrigin {
    fn from_origin(origin: ReuseOrigin) -> Self {
        match origin {
            ReuseOrigin::Unknown => ForwardedFieldOrigin::Unknown,
            other => ForwardedFieldOrigin::Exact(Box::new(other)),
        }
    }

    fn exact(&self) -> Option<&ReuseOrigin> {
        match self {
            ForwardedFieldOrigin::Exact(origin) => Some(origin.as_ref()),
            ForwardedFieldOrigin::Unknown => None,
        }
    }
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

fn token_appears_in_expr(token_binder: CoreBinderId, expr: &CoreExpr) -> bool {
    use_counts(expr).contains_key(&token_binder)
}

fn is_alias_preserving_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => true,
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            fields.iter().all(is_alias_preserving_rhs)
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

fn is_safe_precompute_rhs(token_binder: CoreBinderId, expr: &CoreExpr) -> bool {
    if token_appears_in_expr(token_binder, expr) {
        return false;
    }

    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => true,
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => fields
            .iter()
            .all(|field| is_safe_precompute_rhs(token_binder, field)),
        CoreExpr::PrimOp { args, .. } => args
            .iter()
            .all(|arg| is_safe_precompute_rhs(token_binder, arg)),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            is_safe_precompute_rhs(token_binder, func)
                && args
                    .iter()
                    .all(|arg| is_safe_precompute_rhs(token_binder, arg))
        }
        CoreExpr::Dup { body, .. }
        | CoreExpr::Drop { body, .. }
        | CoreExpr::Return { value: body, .. } => is_safe_precompute_rhs(token_binder, body),
        CoreExpr::Let { rhs, body, .. } => {
            is_safe_precompute_rhs(token_binder, rhs) && is_safe_precompute_rhs(token_binder, body)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            is_safe_precompute_rhs(token_binder, scrutinee)
                && alts.iter().all(|alt| {
                    alt.guard
                        .as_ref()
                        .is_none_or(|guard| is_safe_precompute_rhs(token_binder, guard))
                        && is_safe_precompute_rhs(token_binder, &alt.rhs)
                })
        }
        CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::Lam { .. }
        | CoreExpr::LetRec { .. }
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

    #[allow(dead_code)]
    fn collect_reuses(expr: &CoreExpr, out: &mut Vec<CoreTag>) {
        match expr {
            CoreExpr::Reuse { tag, fields, .. } => {
                out.push(tag.clone());
                for field in fields {
                    collect_reuses(field, out);
                }
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_reuses(rhs, out);
                collect_reuses(body, out);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                collect_reuses(scrutinee, out);
                for alt in alts {
                    collect_reuses(&alt.rhs, out);
                    if let Some(guard) = &alt.guard {
                        collect_reuses(guard, out);
                    }
                }
            }
            CoreExpr::Con { fields, .. } => {
                for field in fields {
                    collect_reuses(field, out);
                }
            }
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                collect_reuses(func, out);
                for arg in args {
                    collect_reuses(arg, out);
                }
            }
            CoreExpr::PrimOp { args, .. } => {
                for arg in args {
                    collect_reuses(arg, out);
                }
            }
            CoreExpr::Dup { body, .. }
            | CoreExpr::Drop { body, .. }
            | CoreExpr::Return { value: body, .. }
            | CoreExpr::Lam { body, .. } => collect_reuses(body, out),
            CoreExpr::Perform { args, .. } => {
                for arg in args {
                    collect_reuses(arg, out);
                }
            }
            CoreExpr::Handle { body, handlers, .. } => {
                collect_reuses(body, out);
                for handler in handlers {
                    collect_reuses(&handler.body, out);
                }
            }
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                collect_reuses(unique_body, out);
                collect_reuses(shared_body, out);
            }
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        }
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
        let io = interner.intern("IO");
        let print = interner.intern("print");
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
            rhs: Box::new(CoreExpr::Perform {
                effect: io,
                operation: print,
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

    #[test]
    fn aether_call_precompute_let_can_still_reuse() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let f = binder(4, interner.intern("f"));
        let y = binder(5, interner.intern("y"));
        let ys = binder(6, interner.intern("ys"));
        let map = binder(7, interner.intern("my_map"));

        let pat_binders = vec![Some(h.id), Some(t.id)];
        let body = CoreExpr::Let {
            var: y,
            rhs: Box::new(CoreExpr::AetherCall {
                func: Box::new(v(f)),
                args: vec![v(h)],
                arg_modes: vec![crate::aether::borrow_infer::BorrowMode::Owned],
                span: s(),
            }),
            body: Box::new(CoreExpr::Let {
                var: ys,
                rhs: Box::new(CoreExpr::AetherCall {
                    func: Box::new(v(map)),
                    args: vec![v(t), v(f)],
                    arg_modes: vec![
                        crate::aether::borrow_infer::BorrowMode::Borrowed,
                        crate::aether::borrow_infer::BorrowMode::Borrowed,
                    ],
                    span: s(),
                }),
                body: Box::new(CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![v(y), v(ys)],
                    span: s(),
                }),
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
                CoreExpr::Let { body, .. } => match *body {
                    CoreExpr::Reuse { token, tag, .. } => {
                        assert_eq!(token.binder, Some(xs.id));
                        assert_eq!(tag, CoreTag::Cons);
                    }
                    other => panic!("expected reuse after safe precompute lets, got {other:?}"),
                },
                other => panic!("expected nested let spine, got {other:?}"),
            },
            other => panic!("expected let spine, got {other:?}"),
        }
    }

    #[test]
    fn token_use_in_precompute_let_blocks_reuse() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let y = binder(4, interner.intern("y"));

        let pat_binders = vec![Some(h.id), Some(t.id)];
        let body = CoreExpr::Let {
            var: y,
            rhs: Box::new(CoreExpr::AetherCall {
                func: Box::new(v(h)),
                args: vec![v(xs)],
                arg_modes: vec![crate::aether::borrow_infer::BorrowMode::Owned],
                span: s(),
            }),
            body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(y), v(t)],
                span: s(),
            }),
            span: s(),
        };

        let reason = diagnose_drop_body(
            &CoreVarRef::resolved(xs),
            &body,
            Some(&pat_binders),
            Some(&CoreTag::Cons),
            None,
        );

        assert_eq!(reason, Some(ReuseFailureReason::TokenEscapesIntoFields));
    }

    #[test]
    fn primop_precompute_let_can_still_reuse() {
        let mut interner = Interner::new();
        let xs = binder(1, interner.intern("xs"));
        let h = binder(2, interner.intern("h"));
        let t = binder(3, interner.intern("t"));
        let inc = binder(4, interner.intern("inc"));

        let pat_binders = vec![Some(h.id), Some(t.id)];
        let body = CoreExpr::Let {
            var: inc,
            rhs: Box::new(CoreExpr::PrimOp {
                op: crate::core::CorePrimOp::Add,
                args: vec![v(h), CoreExpr::Lit(crate::core::CoreLit::Int(1), s())],
                span: s(),
            }),
            body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(inc), v(t)],
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
    }

    #[test]
    fn wrapper_alias_origin_keeps_exact_forwarded_field_provenance() {
        let mut interner = Interner::new();
        let pair2 = interner.intern("Pair2");
        let pair = binder(1, interner.intern("pair"));
        let xs = binder(2, interner.intern("xs"));
        let acc = binder(3, interner.intern("acc"));
        let tmp = binder(4, interner.intern("tmp"));

        let pair_pat = vec![Some(xs.id), Some(acc.id)];
        let env = super::ReuseEnv::seed(
            &CoreVarRef::resolved(pair),
            Some(&pair_pat),
            Some(&CoreTag::Named(pair2)),
        );
        let env = env.with_alias(
            tmp.id,
            &CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![v(xs), v(acc)],
                span: s(),
            },
        );

        let origin = env
            .origin_of_expr(&v(tmp))
            .expect("wrapper child alias should keep forwarded provenance");
        match origin {
            super::ReuseOrigin::Forwarded { tag, fields } => {
                assert_eq!(tag, CoreTag::Cons);
                assert!(fields[0].exact().is_some());
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected forwarded origin, got {other:?}"),
        }
    }

    #[test]
    fn matching_on_field_origin_seeds_child_token_provenance() {
        let mut interner = Interner::new();
        let pair2 = interner.intern("Pair2");
        let pair = binder(1, interner.intern("pair"));
        let xs = binder(2, interner.intern("xs"));
        let acc = binder(3, interner.intern("acc"));
        let y = binder(4, interner.intern("y"));
        let ys = binder(5, interner.intern("ys"));

        let pair_pat = vec![Some(xs.id), Some(acc.id)];
        let env = super::ReuseEnv::seed(
            &CoreVarRef::resolved(pair),
            Some(&pair_pat),
            Some(&CoreTag::Named(pair2)),
        );
        let child_env = env.with_pattern_origin(
            env.origin_of_expr(&v(xs))
                .as_ref()
                .expect("xs field origin"),
            Some(&[Some(y.id), Some(ys.id)]),
            Some(&CoreTag::Cons),
        );

        assert!(
            child_env.unchanged_field_index(xs.id, 0, &v(y)),
            "matching on a wrapper field should seed the child token as the new scrutinee"
        );
        assert!(
            child_env.unchanged_field_index(xs.id, 1, &v(ys)),
            "forwarded child tail provenance should remain exact after the nested match"
        );
    }
}
