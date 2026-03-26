//! Aether Phase 5: environment-based Dup/Drop insertion.
//!
//! The planner walks expressions in reverse, carrying an ownership/liveness
//! environment. `Dup` and `Drop` are emitted as local consequences of that
//! environment instead of from whole-body use counts.

use std::collections::HashMap;

use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreTag, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::{
    AetherEnv, AetherPlan, ValueDemand, join_branch_envs, pat_binders, use_counts,
};
use super::borrow_infer::{BorrowMode, BorrowRegistry};
use super::constructor_shape_for_tag;

type Scope = HashMap<CoreBinderId, CoreBinder>;

/// Insert Dup/Drop annotations into a Core IR expression.
pub fn insert_dup_drop(expr: CoreExpr) -> CoreExpr {
    let mut scope = Scope::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        None,
        &mut scope,
    )
    .expr
}

/// Insert Dup/Drop annotations, consulting the borrow registry to skip
/// Rc::clone for arguments passed to borrowed parameters.
pub fn insert_dup_drop_with_registry(expr: CoreExpr, registry: &BorrowRegistry) -> CoreExpr {
    let mut scope = Scope::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        Some(registry),
        &mut scope,
    )
    .expr
}

fn plan_expr(
    expr: CoreExpr,
    tail_env: AetherEnv,
    demand: ValueDemand,
    registry: Option<&BorrowRegistry>,
    scope: &mut Scope,
) -> AetherPlan {
    match expr {
        CoreExpr::Var { var, span } => plan_var(var, span, tail_env, demand, scope),
        CoreExpr::Lit(_, _) => AetherPlan {
            expr,
            env_before: tail_env,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            scope.insert(var.id, var);
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope);
            let binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore {
                body_expr = wrap_drop(var, body_expr, span);
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(*rhs, rhs_tail, binder_demand, registry, scope);
            scope.remove(&var.id);

            let mut env_before = rhs_plan.env_before;
            env_before.remove(var.id);

            AetherPlan {
                expr: CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs_plan.expr),
                    body: Box::new(body_expr),
                    span,
                },
                env_before,
            }
        }
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => {
            scope.insert(var.id, var);
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope);
            let binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore {
                body_expr = wrap_drop(var, body_expr, span);
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(*rhs, rhs_tail, binder_demand, registry, scope);
            scope.remove(&var.id);

            let mut env_before = rhs_plan.env_before;
            env_before.remove(var.id);

            AetherPlan {
                expr: CoreExpr::LetRec {
                    var,
                    rhs: Box::new(rhs_plan.expr),
                    body: Box::new(body_expr),
                    span,
                },
                env_before,
            }
        }
        CoreExpr::Lam { params, body, span } => {
            let mut param_ids = Vec::with_capacity(params.len());
            for param in &params {
                scope.insert(param.id, *param);
                param_ids.push(param.id);
            }

            let body_plan = plan_expr(
                *body,
                AetherEnv::default(),
                ValueDemand::Owned,
                registry,
                scope,
            );
            for param in &params {
                scope.remove(&param.id);
            }

            let mut body_expr = body_plan.expr;
            for param in params.iter().rev() {
                if !body_plan.env_before.is_live(param.id) {
                    body_expr = wrap_drop(*param, body_expr, span);
                }
            }

            let mut env_before = AetherEnv::default();
            env_before.union_from(&tail_env);
            for capture in body_plan.env_before.live.iter().copied() {
                if !param_ids.contains(&capture) {
                    if body_plan.env_before.is_owned(capture) {
                        env_before.mark_owned(capture);
                    } else if body_plan.env_before.is_borrowed(capture) {
                        env_before.mark_borrowed(capture);
                    }
                }
            }

            AetherPlan {
                expr: CoreExpr::Lam {
                    params,
                    body: Box::new(body_expr),
                    span,
                },
                env_before,
            }
        }
        CoreExpr::App { func, args, span } => {
            let resolved_callee = registry.and_then(|reg| match func.as_ref() {
                CoreExpr::Var { var, .. } => Some(reg.classify_var_ref(var).borrow_callee),
                _ => None,
            });
            let arg_modes: Vec<_> = (0..args.len())
                .map(|index| {
                    let borrowed = resolved_callee
                        .zip(registry)
                        .is_some_and(|(callee, reg)| reg.is_borrowed(callee, index));
                    if borrowed {
                        BorrowMode::Borrowed
                    } else {
                        BorrowMode::Owned
                    }
                })
                .collect();
            let (args, env_after_args) = plan_expr_list(args, tail_env, registry, scope, |index| {
                if arg_modes[index] == BorrowMode::Borrowed {
                    ValueDemand::Borrowed
                } else {
                    ValueDemand::Owned
                }
            });
            let func_plan = plan_expr(
                *func,
                env_after_args,
                ValueDemand::Borrowed,
                registry,
                scope,
            );
            AetherPlan {
                expr: CoreExpr::AetherCall {
                    func: Box::new(func_plan.expr),
                    args,
                    arg_modes,
                    span,
                },
                env_before: func_plan.env_before,
            }
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => {
            let (args, env_after_args) = plan_expr_list(args, tail_env, registry, scope, |index| {
                match arg_modes[index] {
                    BorrowMode::Borrowed => ValueDemand::Borrowed,
                    BorrowMode::Owned => ValueDemand::Owned,
                }
            });
            let func_plan = plan_expr(
                *func,
                env_after_args,
                ValueDemand::Borrowed,
                registry,
                scope,
            );
            AetherPlan {
                expr: CoreExpr::AetherCall {
                    func: Box::new(func_plan.expr),
                    args,
                    arg_modes,
                    span,
                },
                env_before: func_plan.env_before,
            }
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee_var = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => var.binder.map(|id| CoreBinder::new(id, var.name)),
                _ => None,
            };

            let mut branch_plans = Vec::with_capacity(alts.len());
            for alt in alts {
                let CoreAlt {
                    pat,
                    guard,
                    rhs,
                    span: alt_span,
                } = alt;
                let pat_ids = pat_binders(&pat);
                for binder_id in &pat_ids {
                    if let Some(binder) = find_binder_in_pat(&pat, *binder_id) {
                        scope.insert(*binder_id, binder);
                    }
                }

                let rhs_plan = plan_expr(rhs, tail_env.clone(), demand, registry, scope);
                let (guard, branch_env) = if let Some(guard) = guard {
                    let guard_plan = plan_expr(
                        guard,
                        rhs_plan.env_before.clone(),
                        ValueDemand::Borrowed,
                        registry,
                        scope,
                    );
                    (Some(guard_plan.expr), guard_plan.env_before)
                } else {
                    (None, rhs_plan.env_before.clone())
                };

                for binder_id in &pat_ids {
                    scope.remove(binder_id);
                }

                let mut rhs = rhs_plan.expr;
                for binder_id in pat_ids.iter().rev().copied() {
                    if !rhs_plan.env_before.is_live(binder_id)
                        && !expr_uses_binder(&rhs, binder_id)
                        && let Some(binder) = find_binder_in_pat(&pat, binder_id)
                    {
                        rhs = wrap_drop(binder, rhs, alt_span);
                    }
                }

                let mut env_without_pats = branch_env.clone();
                env_without_pats.remove_all(pat_ids.iter().copied());

                if let Some(scrut_binder) = scrutinee_var
                    && is_constructor_pat(&pat)
                    && !env_without_pats.is_live(scrut_binder.id)
                    && !expr_uses_binder(&rhs, scrut_binder.id)
                    && has_compatible_con(&pat, &rhs)
                {
                    rhs = wrap_drop(scrut_binder, rhs, alt_span);
                }

                branch_plans.push((pat, guard, rhs, alt_span, branch_env, env_without_pats));
            }

            let joined = join_branch_envs(
                &branch_plans
                    .iter()
                    .map(|(_, _, _, _, _, env_without_pats)| env_without_pats.clone())
                    .collect::<Vec<_>>(),
            );

            let alts = branch_plans
                .into_iter()
                .map(
                    |(pat, guard, rhs, alt_span, _branch_env, env_without_pats)| {
                        let compensation: Vec<_> = joined
                            .owned
                            .iter()
                            .copied()
                            .filter(|binder_id| {
                                !env_without_pats.is_live(*binder_id)
                                    && !tail_env.is_live(*binder_id)
                            })
                            .filter(|binder_id| !expr_uses_binder(&rhs, *binder_id))
                            .filter(|binder_id| !expr_drops_binder(&rhs, *binder_id))
                            .filter_map(|binder_id| scope.get(&binder_id).copied())
                            .collect();
                        let rhs = compensation
                            .into_iter()
                            .rev()
                            .fold(rhs, |body, binder| wrap_drop(binder, body, alt_span));

                        CoreAlt {
                            pat,
                            guard,
                            rhs,
                            span: alt_span,
                        }
                    },
                )
                .collect();

            let scrutinee_plan =
                plan_expr(*scrutinee, joined, ValueDemand::Borrowed, registry, scope);

            AetherPlan {
                expr: CoreExpr::Case {
                    scrutinee: Box::new(scrutinee_plan.expr),
                    alts,
                    span,
                },
                env_before: scrutinee_plan.env_before,
            }
        }
        CoreExpr::Con { tag, fields, span } => {
            let (fields, env_before) =
                plan_expr_list(fields, tail_env, registry, scope, |_| ValueDemand::Owned);
            AetherPlan {
                expr: CoreExpr::Con { tag, fields, span },
                env_before,
            }
        }
        CoreExpr::PrimOp { op, args, span } => {
            let (args, env_before) =
                plan_expr_list(args, tail_env, registry, scope, |_| ValueDemand::Borrowed);
            AetherPlan {
                expr: CoreExpr::PrimOp { op, args, span },
                env_before,
            }
        }
        CoreExpr::Return { value, span } => {
            let value_plan = plan_expr(*value, tail_env, ValueDemand::Owned, registry, scope);
            AetherPlan {
                expr: CoreExpr::Return {
                    value: Box::new(value_plan.expr),
                    span,
                },
                env_before: value_plan.env_before,
            }
        }
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => {
            let (args, mut env_before) =
                plan_expr_list(args, tail_env, registry, scope, |_| ValueDemand::Owned);
            for binder in env_before.live.clone() {
                env_before.mark_owned(binder);
            }
            AetherPlan {
                expr: CoreExpr::Perform {
                    effect,
                    operation,
                    args,
                    span,
                },
                env_before,
            }
        }
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => {
            let body_plan = plan_expr(*body, tail_env.clone(), demand, registry, scope);
            let mut incoming_envs = vec![body_plan.env_before.clone()];
            let mut planned_handlers = Vec::with_capacity(handlers.len());

            for handler in handlers {
                scope.insert(handler.resume.id, handler.resume);
                let mut shadow_ids = vec![handler.resume.id];
                for param in &handler.params {
                    scope.insert(param.id, *param);
                    shadow_ids.push(param.id);
                }

                let handler_plan =
                    plan_expr(handler.body, tail_env.clone(), demand, registry, scope);
                for shadow in &shadow_ids {
                    scope.remove(shadow);
                }

                let mut env_before = handler_plan.env_before.clone();
                env_before.remove_all(shadow_ids);
                incoming_envs.push(env_before);

                planned_handlers.push(crate::core::CoreHandler {
                    operation: handler.operation,
                    params: handler.params,
                    resume: handler.resume,
                    body: handler_plan.expr,
                    span: handler.span,
                });
            }

            let joined = join_branch_envs(&incoming_envs);

            AetherPlan {
                expr: CoreExpr::Handle {
                    body: Box::new(body_plan.expr),
                    effect,
                    handlers: planned_handlers,
                    span,
                },
                env_before: joined,
            }
        }
        CoreExpr::Dup { var, body, span } => {
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope);
            AetherPlan {
                expr: CoreExpr::Dup {
                    var,
                    body: Box::new(body_plan.expr),
                    span,
                },
                env_before: body_plan.env_before,
            }
        }
        CoreExpr::Drop { var, body, span } => {
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope);
            AetherPlan {
                expr: CoreExpr::Drop {
                    var,
                    body: Box::new(body_plan.expr),
                    span,
                },
                env_before: body_plan.env_before,
            }
        }
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => {
            let (fields, mut env_before) =
                plan_expr_list(fields, tail_env, registry, scope, |_| ValueDemand::Owned);
            if let Some(token_id) = token.binder {
                env_before.mark_owned(token_id);
            }
            AetherPlan {
                expr: CoreExpr::Reuse {
                    token,
                    tag,
                    fields,
                    field_mask,
                    span,
                },
                env_before,
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => {
            let unique_plan = plan_expr(*unique_body, tail_env.clone(), demand, registry, scope);
            let shared_plan = plan_expr(*shared_body, tail_env.clone(), demand, registry, scope);
            let joined = join_branch_envs(&[
                unique_plan.env_before.clone(),
                shared_plan.env_before.clone(),
            ]);
            let scrutinee_plan = plan_expr(
                CoreExpr::Var {
                    var: scrutinee,
                    span,
                },
                joined,
                ValueDemand::Borrowed,
                registry,
                scope,
            );

            let CoreExpr::Var { var: scrutinee, .. } = scrutinee_plan.expr else {
                unreachable!("scrutinee plan must stay a variable");
            };

            AetherPlan {
                expr: CoreExpr::DropSpecialized {
                    scrutinee,
                    unique_body: Box::new(unique_plan.expr),
                    shared_body: Box::new(shared_plan.expr),
                    span,
                },
                env_before: scrutinee_plan.env_before,
            }
        }
    }
}

fn plan_expr_list<F>(
    exprs: Vec<CoreExpr>,
    tail_env: AetherEnv,
    registry: Option<&BorrowRegistry>,
    scope: &mut Scope,
    demand_for_index: F,
) -> (Vec<CoreExpr>, AetherEnv)
where
    F: Fn(usize) -> ValueDemand,
{
    let mut env = tail_env;
    let mut planned = Vec::with_capacity(exprs.len());
    for (index, expr) in exprs.into_iter().enumerate().rev() {
        let plan = plan_expr(expr, env, demand_for_index(index), registry, scope);
        env = plan.env_before;
        planned.push(plan.expr);
    }
    planned.reverse();
    (planned, env)
}

fn plan_var(
    var: CoreVarRef,
    span: Span,
    mut tail_env: AetherEnv,
    demand: ValueDemand,
    scope: &Scope,
) -> AetherPlan {
    let Some(id) = var.binder else {
        return AetherPlan {
            expr: CoreExpr::Var { var, span },
            env_before: tail_env,
        };
    };

    match demand {
        ValueDemand::Ignore => AetherPlan {
            expr: CoreExpr::Var { var, span },
            env_before: tail_env,
        },
        ValueDemand::Borrowed => {
            tail_env.mark_borrowed(id);
            AetherPlan {
                expr: CoreExpr::Var { var, span },
                env_before: tail_env,
            }
        }
        ValueDemand::Owned => {
            let needs_dup = tail_env.is_live(id);
            tail_env.mark_owned(id);
            let expr = if needs_dup {
                if let Some(binder) = scope.get(&id).copied() {
                    wrap_dups(binder, CoreExpr::Var { var, span }, span, 1)
                } else {
                    CoreExpr::Var { var, span }
                }
            } else {
                CoreExpr::Var { var, span }
            };
            AetherPlan {
                expr,
                env_before: tail_env,
            }
        }
    }
}

fn binder_demand(env: &AetherEnv, binder: CoreBinderId) -> ValueDemand {
    if env.is_owned(binder) {
        ValueDemand::Owned
    } else if env.is_borrowed(binder) {
        ValueDemand::Borrowed
    } else {
        ValueDemand::Ignore
    }
}

fn wrap_drop(binder: CoreBinder, body: CoreExpr, span: Span) -> CoreExpr {
    CoreExpr::Drop {
        var: CoreVarRef::resolved(binder),
        body: Box::new(body),
        span,
    }
}

fn wrap_dups(binder: CoreBinder, body: CoreExpr, span: Span, n: usize) -> CoreExpr {
    let mut result = body;
    for _ in 0..n {
        result = CoreExpr::Dup {
            var: CoreVarRef::resolved(binder),
            body: Box::new(result),
            span,
        };
    }
    result
}

fn find_binder_in_pat(pat: &crate::core::CorePat, target: CoreBinderId) -> Option<CoreBinder> {
    match pat {
        crate::core::CorePat::Var(binder) => {
            if binder.id == target {
                Some(*binder)
            } else {
                None
            }
        }
        crate::core::CorePat::Con { fields, .. } | crate::core::CorePat::Tuple(fields) => {
            for f in fields {
                if let Some(b) = find_binder_in_pat(f, target) {
                    return Some(b);
                }
            }
            None
        }
        crate::core::CorePat::Lit(_)
        | crate::core::CorePat::Wildcard
        | crate::core::CorePat::EmptyList => None,
    }
}

fn is_constructor_pat(pat: &crate::core::CorePat) -> bool {
    matches!(
        pat,
        crate::core::CorePat::Con { .. } | crate::core::CorePat::Tuple(_)
    )
}

fn has_compatible_con(pat: &crate::core::CorePat, rhs: &CoreExpr) -> bool {
    let pat_tag = match pat {
        crate::core::CorePat::Con { tag, .. } => Some(tag),
        _ => None,
    };
    let Some(pat_tag) = pat_tag else {
        return false;
    };
    find_con_tag_in_spine(rhs, Some(pat_tag))
        .is_some_and(|ref con_tag| tags_shape_compatible(pat_tag, con_tag))
}

fn find_con_tag_in_spine(
    expr: &CoreExpr,
    expected_tag: Option<&crate::core::CoreTag>,
) -> Option<crate::core::CoreTag> {
    match expr {
        CoreExpr::Reuse { tag, .. } => Some(tag.clone()),
        _ if constructor_shape_for_tag(expr, expected_tag).is_some() => {
            constructor_shape_for_tag(expr, expected_tag).map(|(tag, _, _)| tag)
        }
        CoreExpr::Case { alts, .. } => alts
            .iter()
            .find_map(|alt| find_con_tag_in_spine(&alt.rhs, expected_tag)),
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            find_con_tag_in_spine(body, expected_tag)
        }
        _ => None,
    }
}

fn tags_shape_compatible(a: &CoreTag, b: &CoreTag) -> bool {
    match (a, b) {
        (CoreTag::Cons, CoreTag::Cons) => true,
        (CoreTag::Some, CoreTag::Some) => true,
        (CoreTag::Left, CoreTag::Left) => true,
        (CoreTag::Right, CoreTag::Right) => true,
        (CoreTag::Named(a), CoreTag::Named(b)) => a == b,
        _ => false,
    }
}

fn expr_uses_binder(expr: &CoreExpr, binder: CoreBinderId) -> bool {
    use_counts(expr).get(&binder).copied().unwrap_or(0) > 0
}

fn expr_drops_binder(expr: &CoreExpr, binder: CoreBinderId) -> bool {
    match expr {
        CoreExpr::Drop { var, body, .. } => {
            var.binder == Some(binder) || expr_drops_binder(body, binder)
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            expr_drops_binder(rhs, binder) || expr_drops_binder(body, binder)
        }
        CoreExpr::Lam { body, .. }
        | CoreExpr::Dup { body, .. }
        | CoreExpr::Return { value: body, .. } => expr_drops_binder(body, binder),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            expr_drops_binder(func, binder) || args.iter().any(|arg| expr_drops_binder(arg, binder))
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            expr_drops_binder(scrutinee, binder)
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| expr_drops_binder(guard, binder))
                        || expr_drops_binder(&alt.rhs, binder)
                })
        }
        CoreExpr::Con { fields, .. }
        | CoreExpr::PrimOp { args: fields, .. }
        | CoreExpr::Reuse { fields, .. }
        | CoreExpr::Perform { args: fields, .. } => {
            fields.iter().any(|field| expr_drops_binder(field, binder))
        }
        CoreExpr::Handle { body, handlers, .. } => {
            expr_drops_binder(body, binder)
                || handlers
                    .iter()
                    .any(|handler| expr_drops_binder(&handler.body, binder))
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => expr_drops_binder(unique_body, binder) || expr_drops_binder(shared_body, binder),
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::borrow_infer::{BorrowMode, BorrowRegistry};
    use crate::core::{
        CoreBinder, CoreBinderId, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::insert_dup_drop_with_registry;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span: Span::default(),
        }
    }

    fn count_binder_nodes<F>(expr: &CoreExpr, binder: CoreBinderId, predicate: &F) -> usize
    where
        F: Fn(&CoreExpr, CoreBinderId) -> bool,
    {
        let self_count = usize::from(predicate(expr, binder));
        let children = match expr {
            CoreExpr::Lam { body, .. } => count_binder_nodes(body, binder, predicate),
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                count_binder_nodes(func, binder, predicate)
                    + args
                        .iter()
                        .map(|arg| count_binder_nodes(arg, binder, predicate))
                        .sum::<usize>()
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                count_binder_nodes(rhs, binder, predicate)
                    + count_binder_nodes(body, binder, predicate)
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                count_binder_nodes(scrutinee, binder, predicate)
                    + alts
                        .iter()
                        .map(|alt| {
                            alt.guard
                                .as_ref()
                                .map_or(0, |guard| count_binder_nodes(guard, binder, predicate))
                                + count_binder_nodes(&alt.rhs, binder, predicate)
                        })
                        .sum::<usize>()
            }
            CoreExpr::Con { fields, .. }
            | CoreExpr::PrimOp { args: fields, .. }
            | CoreExpr::Reuse { fields, .. }
            | CoreExpr::Perform { args: fields, .. } => fields
                .iter()
                .map(|field| count_binder_nodes(field, binder, predicate))
                .sum(),
            CoreExpr::Return { value, .. }
            | CoreExpr::Dup { body: value, .. }
            | CoreExpr::Drop { body: value, .. } => count_binder_nodes(value, binder, predicate),
            CoreExpr::Handle { body, handlers, .. } => {
                count_binder_nodes(body, binder, predicate)
                    + handlers
                        .iter()
                        .map(|handler| count_binder_nodes(&handler.body, binder, predicate))
                        .sum::<usize>()
            }
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                count_binder_nodes(unique_body, binder, predicate)
                    + count_binder_nodes(shared_body, binder, predicate)
            }
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => 0,
        };

        self_count + children
    }

    #[test]
    fn indirect_calls_default_to_owned_aether_call_modes() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 1, "f");
        let x = binder(&mut interner, 2, "x");

        let expr = CoreExpr::Lam {
            params: vec![f, x],
            body: Box::new(CoreExpr::App {
                func: Box::new(var(f)),
                args: vec![var(x)],
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &BorrowRegistry::default());
        let CoreExpr::Lam { body, .. } = rewritten else {
            panic!("expected lambda body");
        };
        match body.as_ref() {
            CoreExpr::AetherCall { arg_modes, .. } => {
                assert_eq!(arg_modes, &[BorrowMode::Owned]);
            }
            other => panic!("expected AetherCall, got {other:?}"),
        }
    }

    #[test]
    fn read_only_closure_capture_does_not_dup_capture() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let f = binder(&mut interner, 2, "f");
        let len_name = interner.intern("len");
        let mut registry = BorrowRegistry::default();
        registry.by_name.insert(
            len_name,
            crate::aether::borrow_infer::BorrowSignature::new(
                vec![BorrowMode::Borrowed],
                crate::aether::borrow_infer::BorrowProvenance::BaseRuntime,
            ),
        );

        let expr = CoreExpr::Lam {
            params: vec![xs],
            body: Box::new(CoreExpr::Let {
                var: f,
                rhs: Box::new(CoreExpr::Lam {
                    params: vec![],
                    body: Box::new(CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::unresolved(len_name),
                            span: Span::default(),
                        }),
                        args: vec![var(xs)],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }),
                body: Box::new(CoreExpr::PrimOp {
                    op: CorePrimOp::Add,
                    args: vec![
                        CoreExpr::App {
                            func: Box::new(var(f)),
                            args: vec![],
                            span: Span::default(),
                        },
                        CoreExpr::App {
                            func: Box::new(var(f)),
                            args: vec![],
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        let dups_for_xs = count_binder_nodes(
            &rewritten,
            xs.id,
            &|expr, binder| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(binder)),
        );
        assert_eq!(
            dups_for_xs, 0,
            "read-only closure capture should not duplicate the captured binder"
        );
    }

    #[test]
    fn handler_shadowing_does_not_keep_outer_binder_live() {
        let mut interner = Interner::new();
        let outer = binder(&mut interner, 1, "x");
        let resume = binder(&mut interner, 2, "resume");
        let shadow = binder(&mut interner, 3, "x");
        let effect = interner.intern("IO");
        let op = interner.intern("read");

        let expr = CoreExpr::Lam {
            params: vec![outer],
            body: Box::new(CoreExpr::Handle {
                body: Box::new(CoreExpr::Lit(CoreLit::Unit, Span::default())),
                effect,
                handlers: vec![CoreHandler {
                    operation: op,
                    params: vec![shadow],
                    resume,
                    body: var(shadow),
                    span: Span::default(),
                }],
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &BorrowRegistry::default());
        let drops_for_outer = count_binder_nodes(
            &rewritten,
            outer.id,
            &|expr, binder| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(binder)),
        );
        assert!(
            drops_for_outer >= 1,
            "unused outer binder should still be dropped when handler params shadow it"
        );
    }

    #[test]
    fn branch_borrow_only_path_does_not_force_dead_path_drop() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let len_name = interner.intern("len");
        let mut registry = BorrowRegistry::default();
        registry.by_name.insert(
            len_name,
            crate::aether::borrow_infer::BorrowSignature::new(
                vec![BorrowMode::Borrowed],
                crate::aether::borrow_infer::BorrowProvenance::BaseRuntime,
            ),
        );

        let expr = CoreExpr::Lam {
            params: vec![x],
            body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
                alts: vec![
                    crate::core::CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(CoreExpr::Var {
                                var: CoreVarRef::unresolved(len_name),
                                span: Span::default(),
                            }),
                            args: vec![var(x)],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                    crate::core::CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Lit(CoreLit::Int(0), Span::default()),
                        span: Span::default(),
                    },
                ],
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        let drops_for_x = count_binder_nodes(
            &rewritten,
            x.id,
            &|expr, binder| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(binder)),
        );
        assert_eq!(
            drops_for_x, 0,
            "borrow-only branch joins should not synthesize dead-path drops for the binder"
        );
    }
}
