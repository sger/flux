//! Aether Phase 5: environment-based Dup/Drop insertion.
//!
//! The planner walks expressions in reverse, carrying an ownership/liveness
//! environment. `Dup` and `Drop` are emitted as local consequences of that
//! environment instead of from whole-body use counts.

use std::collections::{HashMap, HashSet};

use crate::core::{CoreBinder, CoreBinderId, CoreTag, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::{
    AetherEnv, AetherPlan, ValueDemand, join_branch_envs, pat_binders, use_counts_aether,
};
use super::borrow_infer::{BorrowCallee, BorrowMode, BorrowRegistry};
use super::{AetherAlt as CoreAlt, AetherExpr, AetherHandler, constructor_shape_for_tag_aether};

type CoreExpr = AetherExpr;

/// Temporaries this pass makes take ids from here up, far above any binder
/// lowering makes: one is only read in the function it is bound in, so it
/// can never shadow a real binder there.
const FIRST_TEMP_ID: u32 = 0xF000_0000;
/// Their names, past the evidence pass's (`6_000_000 + id`).
const FIRST_TEMP_SYMBOL: u32 = 7_000_000;

/// The binders in scope, by id, and the function's Borrowed parameters.
#[derive(Default)]
struct Scope {
    binders: HashMap<CoreBinderId, CoreBinder>,
    /// Parameters the caller lends: the function never releases them, and
    /// an owned use of one, or of a field taken from one, takes a Dup.
    borrowed_params: HashSet<CoreBinderId>,
    /// Values read out of a cell that still holds them (the fields of an arm
    /// that does not release its cell, `let x = t.i`), with no reference of
    /// their own: an owned use takes a Dup, and they are never released (0189
    /// rule 3).
    views: HashSet<CoreBinderId>,
    /// How many temporaries `temp` has handed out in this definition.
    temps: u32,
}

impl Scope {
    fn new() -> Self {
        Self::default()
    }

    /// A fresh binder for a value the plan has to name. Its name is never
    /// interned, so the debugger never lists it.
    fn temp(&mut self) -> CoreBinder {
        let n = self.temps;
        self.temps += 1;
        CoreBinder::new(
            CoreBinderId(FIRST_TEMP_ID + n),
            crate::syntax::symbol::Symbol::new(FIRST_TEMP_SYMBOL + n),
        )
    }

    /// Lends `ids` for a body planned next, returning the ones that were not
    /// lent already, for `unlend` to take back after it.
    fn lend(&mut self, ids: impl IntoIterator<Item = CoreBinderId>) -> Vec<CoreBinderId> {
        ids.into_iter()
            .filter(|id| self.borrowed_params.insert(*id))
            .collect()
    }

    fn unlend(&mut self, ids: Vec<CoreBinderId>) {
        for id in ids {
            self.borrowed_params.remove(&id);
        }
    }

    /// Whether `binder` is a Borrowed parameter, or a field taken from one
    /// (through `field_parents`).
    fn lent(&self, binder: CoreBinderId, field_parents: &FieldParents) -> bool {
        let mut current = binder;
        // A field's parent chain is acyclic; the bound is only a guard.
        for _ in 0..64 {
            if self.borrowed_params.contains(&current) {
                return true;
            }
            match field_parents.get(&current) {
                Some(field) => current = field.parent,
                None => return false,
            }
        }
        false
    }
}

impl std::ops::Deref for Scope {
    type Target = HashMap<CoreBinderId, CoreBinder>;
    fn deref(&self) -> &Self::Target {
        &self.binders
    }
}

impl std::ops::DerefMut for Scope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.binders
    }
}

/// Maps an extracted field binder to its scrutinee and provenance.
/// Tracks which variables were extracted from which scrutinee via
/// constructor pattern matching. Used to prevent dropping a scrutinee
/// while its destructured fields are still live, and to dup fields
/// from borrowed scrutinees when used in constructors (owned demand).
#[derive(Clone)]
struct FieldParent {
    parent: CoreBinderId,
    tag: Option<CoreTag>,
    pattern: bool,
}

type FieldParents = HashMap<CoreBinderId, FieldParent>;

/// Insert Dup/Drop annotations into a Core IR expression.
pub fn insert_dup_drop(expr: CoreExpr) -> CoreExpr {
    let mut scope = Scope::new();
    let mut field_parents = FieldParents::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        None,
        &mut scope,
        &mut field_parents,
    )
    .expr
}

/// Insert Dup/Drop annotations into a backend-only Aether expression.
pub fn insert_dup_drop_aether(expr: CoreExpr) -> CoreExpr {
    insert_dup_drop(expr)
}

/// Insert Dup/Drop annotations, consulting the borrow registry to skip
/// Rc::clone for arguments passed to borrowed parameters.
pub fn insert_dup_drop_with_registry(expr: CoreExpr, registry: &BorrowRegistry) -> CoreExpr {
    let mut scope = Scope::new();
    let mut field_parents = FieldParents::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        Some(registry),
        &mut scope,
        &mut field_parents,
    )
    .expr
}

/// Insert Dup/Drop annotations into a backend-only Aether expression,
/// consulting the borrow registry to skip unnecessary clones.
pub fn insert_dup_drop_with_registry_aether(expr: CoreExpr, registry: &BorrowRegistry) -> CoreExpr {
    insert_dup_drop_with_registry(expr, registry)
}

/// Insert Dup/Drop annotations into a definition whose parameters
/// `borrowed_params` are Borrowed: its callers lend them, so it never
/// releases them (`docs/internals/aether_formal_semantics.md`: a borrowed
/// argument stays available after the call).
pub fn insert_dup_drop_for_def(
    expr: CoreExpr,
    registry: &BorrowRegistry,
    borrowed_params: HashSet<CoreBinderId>,
) -> CoreExpr {
    let mut scope = Scope {
        borrowed_params,
        ..Scope::default()
    };
    let mut field_parents = FieldParents::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        Some(registry),
        &mut scope,
        &mut field_parents,
    )
    .expr
}

fn plan_expr(
    expr: CoreExpr,
    tail_env: AetherEnv,
    demand: ValueDemand,
    registry: Option<&BorrowRegistry>,
    scope: &mut Scope,
    field_parents: &mut FieldParents,
) -> AetherPlan {
    match expr {
        CoreExpr::Var { var, span } => plan_var(var, span, tail_env, demand, scope, field_parents),
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

            // Track field→parent for TupleField access (e.g. `let total = st.2`).
            // This prevents the parent from being dropped while the field
            // binder is still live (use-after-free on borrowed field).
            let view = if let CoreExpr::TupleField { object, .. } = rhs.as_ref()
                && let CoreExpr::Var { var: obj_var, .. } = object.as_ref()
                && let Some(parent_id) = obj_var.binder
            {
                scope.views.insert(var.id);
                field_parents.insert(
                    var.id,
                    FieldParent {
                        parent: parent_id,
                        tag: None,
                        pattern: false,
                    },
                );
                true
            } else {
                false
            };

            let body_plan = plan_expr(*body, tail_env, demand, registry, scope, field_parents);
            let mut binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore && !view {
                body_expr = wrap_drop(var, body_expr, span);
            }
            // A view only reads its field; each owned use copies it.
            if view && binder_demand == ValueDemand::Owned {
                binder_demand = ValueDemand::Borrowed;
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(
                *rhs,
                rhs_tail,
                binder_demand,
                registry,
                scope,
                field_parents,
            );
            scope.remove(&var.id);
            scope.views.remove(&var.id);
            field_parents.remove(&var.id);

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
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope, field_parents);
            let binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore {
                body_expr = wrap_drop(var, body_expr, span);
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(
                *rhs,
                rhs_tail,
                binder_demand,
                registry,
                scope,
                field_parents,
            );
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
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => {
            for (var, _) in &bindings {
                scope.insert(var.id, *var);
            }
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope, field_parents);
            let mut body_expr = body_plan.expr;
            // Insert drops for unused bindings
            for (var, _) in bindings.iter().rev() {
                let bd = binder_demand(&body_plan.env_before, var.id);
                if bd == ValueDemand::Ignore {
                    body_expr = wrap_drop(*var, body_expr, span);
                }
            }

            let mut env = body_plan.env_before.clone();
            let mut new_bindings = Vec::with_capacity(bindings.len());
            for (var, rhs) in bindings.into_iter().rev() {
                let bd = binder_demand(&env, var.id);
                let mut rhs_tail = env.clone();
                rhs_tail.remove(var.id);
                let rhs_plan = plan_expr(*rhs, rhs_tail, bd, registry, scope, field_parents);
                env = rhs_plan.env_before;
                env.remove(var.id);
                new_bindings.push((var, Box::new(rhs_plan.expr)));
            }
            new_bindings.reverse();

            for (var, _) in new_bindings.iter() {
                scope.remove(&var.id);
            }

            AetherPlan {
                expr: CoreExpr::LetRecGroup {
                    bindings: new_bindings,
                    body: Box::new(body_expr),
                    span,
                },
                env_before: env,
            }
        }
        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => {
            let mut param_ids = Vec::with_capacity(params.len());
            for param in &params {
                scope.insert(param.id, *param);
                param_ids.push(param.id);
            }
            // Making the closure retains each capture (`flux_make_closure`),
            // so inside the body a capture is lent, like a Borrowed
            // parameter: giving it away takes a Dup, and only releasing the
            // closure releases it (0189 V2).
            let lent = scope.lend(
                super::free_vars::collect_free_vars_aether(&body)
                    .into_iter()
                    .filter(|id| !param_ids.contains(id)),
            );

            let body_plan = plan_expr(
                *body,
                AetherEnv::default(),
                ValueDemand::Owned,
                registry,
                scope,
                field_parents,
            );
            for param in &params {
                scope.remove(&param.id);
            }
            scope.unlend(lent);

            let mut body_expr = body_plan.expr;
            for param in params.iter().rev() {
                if !body_plan.env_before.is_live(param.id)
                    && !scope.borrowed_params.contains(&param.id)
                {
                    body_expr = wrap_drop(*param, body_expr, span);
                }
            }

            let mut env_before = AetherEnv::default();
            env_before.union_from(&tail_env);
            // Sort for deterministic iteration order (proposal 0174 D1):
            // `live` is a `HashSet`, so iterating it directly leaks
            // process-randomized hash order into downstream reuse-spec
            // decisions. The downstream pipeline isn't fully order-
            // insensitive even though set membership is.
            let mut captures: Vec<CoreBinderId> =
                body_plan.env_before.live.iter().cloned().collect();
            captures.sort();
            for capture in captures {
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
                    param_types,
                    result_ty,
                    body: Box::new(body_expr),
                    span,
                },
                env_before,
            }
        }
        CoreExpr::App { func, args, span } => {
            let resolved_callee = planned_callee(func.as_ref(), registry, scope);
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
            let guarded_borrowed_args =
                call_guard_mask(&args, &arg_modes, &tail_env, field_parents);
            let (args, env_after_args) =
                plan_expr_list(args, tail_env, registry, scope, field_parents, |index| {
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
                field_parents,
            );
            AetherPlan {
                expr: CoreExpr::AetherCall {
                    func: Box::new(func_plan.expr),
                    args,
                    arg_modes,
                    guarded_borrowed_args,
                    span,
                },
                env_before: func_plan.env_before,
            }
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes: existing_arg_modes,
            guarded_borrowed_args: _,
            span,
        } => {
            let resolved_callee = planned_callee(func.as_ref(), registry, scope);
            let arg_modes: Vec<_> = if let Some(reg) = registry {
                (0..args.len())
                    .map(|index| {
                        let borrowed =
                            resolved_callee.is_some_and(|callee| reg.is_borrowed(callee, index));
                        if borrowed {
                            BorrowMode::Borrowed
                        } else {
                            BorrowMode::Owned
                        }
                    })
                    .collect()
            } else {
                existing_arg_modes.clone()
            };
            let guarded_borrowed_args =
                call_guard_mask(&args, &arg_modes, &tail_env, field_parents);
            let (args, env_after_args) =
                plan_expr_list(args, tail_env, registry, scope, field_parents, |index| {
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
                field_parents,
            );
            AetherPlan {
                expr: CoreExpr::AetherCall {
                    func: Box::new(func_plan.expr),
                    args,
                    arg_modes,
                    guarded_borrowed_args,
                    span,
                },
                env_before: func_plan.env_before,
            }
        }
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let scrutinee_var = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => var.binder.map(|id| CoreBinder::new(id, var.name)),
                _ => None,
            };

            let arm_uses: Vec<bool> = alts
                .iter()
                .map(|alt| {
                    scrutinee_var
                        .as_ref()
                        .is_some_and(|sv| alt_uses_binder(alt, sv.id))
                })
                .collect();

            let mut branch_plans = Vec::with_capacity(alts.len());
            for (index, alt) in alts.into_iter().enumerate() {
                let CoreAlt {
                    pat,
                    guard,
                    rhs,
                    span: alt_span,
                } = alt;
                let pat_ids = pat_binders(&pat);
                // The arm may release the matched cell: it owns the cell,
                // matched a constructor, and nothing reads the cell again.
                let may_release = scrutinee_var.as_ref().is_some_and(|sv| {
                    !scope.lent(sv.id, field_parents)
                        && !scope.views.contains(&sv.id)
                        && is_constructor_pat(&pat)
                        && !tail_env.is_live(sv.id)
                        && !arm_uses[index]
                });
                // Released at the head when the arm builds a cell of the same
                // shape for the reuse pass. Decided before the arm is planned,
                // because the plan of its fields depends on it.
                // This arm's own fields claim nothing: the arm takes the
                // constructor first.
                let head_drop = may_release
                    && has_compatible_con(&pat, &rhs, &|inner, inner_alt| {
                        !pat_ids.contains(&inner)
                            && !scope.lent(inner, field_parents)
                            && !scope.views.contains(&inner)
                            && !tail_env.is_live(inner)
                            && !alt_uses_binder(inner_alt, inner)
                    });
                // Where the cell may be released, at the head or to balance an
                // arm that gives it away, the release can free the fields with
                // it. So the arm takes its own reference to each field it
                // keeps first, `dup fields; drop xs` (audit L2), and the
                // fields are owned rather than views.
                let own_fields = may_release
                    && (head_drop
                        || arm_uses
                            .iter()
                            .enumerate()
                            .any(|(other, uses)| other != index && *uses));
                // A name for the whole matched value is that value, and a view
                // only when the value is one.
                let views = !own_fields
                    && match &pat {
                        crate::core::CorePat::Var(_) => scrutinee_var
                            .as_ref()
                            .is_some_and(|sv| scope.views.contains(&sv.id)),
                        _ => true,
                    };
                for binder_id in &pat_ids {
                    if let Some(binder) = find_binder_in_pat(&pat, *binder_id) {
                        scope.insert(*binder_id, binder);
                    }
                    if views {
                        scope.views.insert(*binder_id);
                    }
                }

                // Track field→scrutinee relationships for nested cases.
                // Pattern binders are fields extracted from the scrutinee; an
                // owned field no longer depends on it.
                if !own_fields && let Some(sv) = scrutinee_var.as_ref() {
                    let parent_tag = pat_constructor_tag(&pat);
                    for &bid in &pat_ids {
                        field_parents.insert(
                            bid,
                            FieldParent {
                                parent: sv.id,
                                tag: parent_tag.clone(),
                                pattern: true,
                            },
                        );
                    }
                }

                let rhs_plan = plan_expr(
                    rhs,
                    tail_env.clone(),
                    demand,
                    registry,
                    scope,
                    field_parents,
                );
                let (guard, branch_env) = if let Some(guard) = guard {
                    // A failed guard falls through to the later arms, which may
                    // still read or release what it uses, but `env_before` holds
                    // only this arm's liveness. Lent, each value the guard gives
                    // away is copied first (audit M2).
                    let lent = scope.lend(super::free_vars::collect_free_vars_aether(&guard));
                    let guard_plan = plan_expr(
                        guard,
                        rhs_plan.env_before.clone(),
                        ValueDemand::Borrowed,
                        registry,
                        scope,
                        field_parents,
                    );
                    scope.unlend(lent);
                    (Some(guard_plan.expr), guard_plan.env_before)
                } else {
                    (None, rhs_plan.env_before.clone())
                };

                for binder_id in &pat_ids {
                    scope.remove(binder_id);
                    scope.views.remove(binder_id);
                    field_parents.remove(binder_id);
                }

                // The owned fields the arm keeps, and whether it gives each
                // away.
                let kept_fields: Vec<(CoreBinder, bool)> = if own_fields {
                    pat_ids
                        .iter()
                        .copied()
                        .filter(|&id| rhs_plan.env_before.is_live(id))
                        .filter_map(|id| {
                            find_binder_in_pat(&pat, id)
                                .map(|binder| (binder, rhs_plan.env_before.is_owned(id)))
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                let mut rhs = rhs_plan.expr;

                let mut env_without_pats = branch_env.clone();
                env_without_pats.remove_all(pat_ids.iter().cloned());

                if head_drop && let Some(scrut_binder) = scrutinee_var.as_ref() {
                    rhs = wrap_drop(*scrut_binder, rhs, alt_span);
                }

                branch_plans.push(PlannedArm {
                    pat,
                    guard,
                    rhs,
                    span: alt_span,
                    env_without_pats,
                    pat_ids,
                    kept_fields,
                    head_drop,
                });
            }

            let joined = join_branch_envs(
                &branch_plans
                    .iter()
                    .map(|arm| arm.env_without_pats.clone())
                    .collect::<Vec<_>>(),
            );

            let alts = branch_plans
                .into_iter()
                .map(
                    |PlannedArm {
                         pat,
                         guard,
                         rhs,
                         span: alt_span,
                         env_without_pats,
                         pat_ids,
                         kept_fields,
                         head_drop,
                     }| {
                        let compensation: Vec<_> = joined
                            .owned
                            .iter()
                            .cloned()
                            // This arm's fields are planned inside it, and
                            // never balance another arm.
                            .filter(|binder_id| !pat_ids.contains(binder_id))
                            // A view has no reference of its own to give back.
                            .filter(|binder_id| !scope.views.contains(binder_id))
                            // Lent by the caller: never released here.
                            .filter(|binder_id| !scope.lent(*binder_id, field_parents))
                            .filter(|binder_id| {
                                !env_without_pats.is_live(*binder_id)
                                    && !tail_env.is_live(*binder_id)
                            })
                            .filter(|binder_id| {
                                !field_parents
                                    .get(binder_id)
                                    .is_some_and(|field| field.pattern)
                            })
                            .filter(|binder_id| {
                                let Some(field) = field_parents.get(binder_id).cloned() else {
                                    return true;
                                };
                                !env_without_pats.is_live(field.parent)
                                    && !expr_uses_binder(&rhs, field.parent)
                            })
                            .filter(|binder_id| {
                                // A view read out of the value, at any depth,
                                // that the arm still uses.
                                let mut used: Vec<(CoreBinderId, Option<CoreTag>)> = field_parents
                                    .iter()
                                    .filter(|(child_id, _)| {
                                        descends_from(**child_id, *binder_id, field_parents)
                                            && (env_without_pats.is_live(**child_id)
                                                || expr_uses_binder(&rhs, **child_id))
                                    })
                                    .map(|(child_id, field)| (*child_id, field.tag.clone()))
                                    .collect();
                                used.sort_by_key(|(child_id, _)| *child_id);
                                used.is_empty()
                                    || used
                                        .iter()
                                        .find_map(|(_, tag)| tag.clone())
                                        .is_some_and(|tag| rhs_has_compatible_tag(&rhs, &tag))
                            })
                            .filter(|binder_id| !expr_uses_binder(&rhs, *binder_id))
                            .filter(|binder_id| !expr_drops_binder(&rhs, *binder_id))
                            .filter_map(|binder_id| scope.get(&binder_id).cloned())
                            .collect();
                        let released = head_drop
                            || scrutinee_var
                                .as_ref()
                                .is_some_and(|sv| compensation.iter().any(|b| b.id == sv.id));
                        // A view read out of a released value, at any depth, is
                        // still used below the release, so it takes its own
                        // reference first. Its uses keep their own copies, so
                        // this one is never paired: it leaks (KI-121).
                        let children_kept: Vec<Vec<CoreBinder>> = compensation
                            .iter()
                            .map(|binder| {
                                let mut children: Vec<CoreBinder> = field_parents
                                    .keys()
                                    .filter(|child| {
                                        descends_from(**child, binder.id, field_parents)
                                            && (env_without_pats.is_live(**child)
                                                || expr_uses_binder(&rhs, **child))
                                    })
                                    .filter_map(|child| scope.get(child).cloned())
                                    .collect();
                                children.sort_by_key(|child| child.id);
                                children
                            })
                            .collect();
                        let rhs = compensation.into_iter().zip(children_kept).rev().fold(
                            rhs,
                            |body, (binder, children)| {
                                let body = wrap_drop(binder, body, alt_span);
                                children
                                    .into_iter()
                                    .rev()
                                    .fold(body, |body, child| wrap_dups(child, body, alt_span, 1))
                            },
                        );
                        // `dup fields; drop xs`: each kept field is copied before
                        // the release, or before it is given away.
                        let rhs = kept_fields
                            .into_iter()
                            .rev()
                            .filter(|(_, given_away)| *given_away || released)
                            .fold(rhs, |body, (field, _)| wrap_dups(field, body, alt_span, 1));

                        CoreAlt {
                            pat,
                            guard,
                            rhs,
                            span: alt_span,
                        }
                    },
                )
                .collect();

            let scrutinee_plan = plan_expr(
                *scrutinee,
                joined,
                ValueDemand::Borrowed,
                registry,
                scope,
                field_parents,
            );

            AetherPlan {
                expr: CoreExpr::Case {
                    scrutinee: Box::new(scrutinee_plan.expr),
                    alts,
                    join_ty,
                    span,
                },
                env_before: scrutinee_plan.env_before,
            }
        }
        CoreExpr::Con { tag, fields, span } => {
            let (fields, env_before) =
                plan_expr_list(fields, tail_env, registry, scope, field_parents, |_| {
                    ValueDemand::Owned
                });
            AetherPlan {
                expr: CoreExpr::Con { tag, fields, span },
                env_before,
            }
        }
        CoreExpr::PrimOp { op, args, span } => {
            let (args, env_before) =
                plan_expr_list(args, tail_env, registry, scope, field_parents, |index| {
                    if op.takes_arg(index) {
                        ValueDemand::Owned
                    } else {
                        ValueDemand::Borrowed
                    }
                });
            AetherPlan {
                expr: CoreExpr::PrimOp { op, args, span },
                env_before,
            }
        }
        CoreExpr::Return { value, span } => {
            let value_plan = plan_expr(
                *value,
                tail_env,
                ValueDemand::Owned,
                registry,
                scope,
                field_parents,
            );
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
                plan_expr_list(args, tail_env, registry, scope, field_parents, |_| {
                    ValueDemand::Owned
                });
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
            parameter,
            handlers,
            span,
        } => {
            let parameter_plan = parameter
                .map(|p| plan_expr(*p, tail_env.clone(), demand, registry, scope, field_parents));
            let body_plan = plan_expr(
                *body,
                tail_env.clone(),
                demand,
                registry,
                scope,
                field_parents,
            );
            let mut incoming_envs = vec![body_plan.env_before.clone()];
            let mut planned_handlers = Vec::with_capacity(handlers.len());

            for handler in handlers {
                scope.insert(handler.resume.id, handler.resume);
                let mut shadow_ids = vec![handler.resume.id];
                for param in &handler.params {
                    scope.insert(param.id, *param);
                    shadow_ids.push(param.id);
                }
                if let Some(state) = handler.state {
                    scope.insert(state.id, state);
                    shadow_ids.push(state.id);
                }
                // The runtime lends a clause its `resume`, arguments and state,
                // and releases the operation's argument once the clause
                // returns (`runtime/c/effects.c`): one given away is copied
                // (0189 V3).
                let lent = scope.lend(shadow_ids.iter().copied());

                let handler_plan = plan_expr(
                    handler.body,
                    tail_env.clone(),
                    demand,
                    registry,
                    scope,
                    field_parents,
                );
                for shadow in &shadow_ids {
                    scope.remove(shadow);
                }
                scope.unlend(lent);

                let mut env_before = handler_plan.env_before.clone();
                env_before.remove_all(shadow_ids);
                incoming_envs.push(env_before);

                planned_handlers.push(AetherHandler {
                    operation: handler.operation,
                    params: handler.params,
                    param_types: handler.param_types,
                    state: handler.state,
                    state_ty: handler.state_ty,
                    resume: handler.resume,
                    resume_ty: handler.resume_ty,
                    body: handler_plan.expr,
                    span: handler.span,
                });
            }

            let joined = join_branch_envs(&incoming_envs);

            AetherPlan {
                expr: CoreExpr::Handle {
                    body: Box::new(body_plan.expr),
                    effect,
                    parameter: parameter_plan.map(|p| Box::new(p.expr)),
                    handlers: planned_handlers,
                    span,
                },
                env_before: joined,
            }
        }
        CoreExpr::Dup { var, body, span } => {
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope, field_parents);
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
            let body_plan = plan_expr(*body, tail_env, demand, registry, scope, field_parents);
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
                plan_expr_list(fields, tail_env, registry, scope, field_parents, |_| {
                    ValueDemand::Owned
                });
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
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => {
            let object_plan = plan_expr(
                *object,
                tail_env,
                ValueDemand::Borrowed,
                registry,
                scope,
                field_parents,
            );
            AetherPlan {
                expr: CoreExpr::MemberAccess {
                    object: Box::new(object_plan.expr),
                    member,
                    span,
                },
                env_before: object_plan.env_before,
            }
        }
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => {
            let given_away =
                demand == ValueDemand::Owned && matches!(object.as_ref(), CoreExpr::Var { .. });
            let object_plan = plan_expr(
                *object,
                tail_env,
                ValueDemand::Borrowed,
                registry,
                scope,
                field_parents,
            );
            let mut expr = CoreExpr::TupleField {
                object: Box::new(object_plan.expr),
                index,
                span,
            };
            // The tuple keeps its field: one given away is copied first.
            if given_away {
                let held = scope.temp();
                expr = CoreExpr::Let {
                    var: held,
                    rhs: Box::new(expr),
                    body: Box::new(wrap_dups(
                        held,
                        CoreExpr::Var {
                            var: CoreVarRef::resolved(&held),
                            span,
                        },
                        span,
                        1,
                    )),
                    span,
                };
            }
            AetherPlan {
                expr,
                env_before: object_plan.env_before,
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => {
            let unique_plan = plan_expr(
                *unique_body,
                tail_env.clone(),
                demand,
                registry,
                scope,
                field_parents,
            );
            let shared_plan = plan_expr(
                *shared_body,
                tail_env.clone(),
                demand,
                registry,
                scope,
                field_parents,
            );
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
                field_parents,
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

/// The callee whose signature a call's arguments are planned by. A closure
/// bound in this definition has none: a global of the same name may borrow
/// its arguments, but the closure's body plans its parameters as owned, so
/// a lent argument would be released by the callee. Other callees keep the
/// registry's lookup by name, because a reference to a global can carry a
/// binder id other than its definition's.
fn planned_callee(
    func: &CoreExpr,
    registry: Option<&BorrowRegistry>,
    scope: &Scope,
) -> Option<BorrowCallee> {
    let reg = registry?;
    match func {
        CoreExpr::Var { var, .. } if var.binder.is_some_and(|id| scope.contains_key(&id)) => {
            Some(BorrowCallee::Unknown)
        }
        CoreExpr::Var { var, .. } => Some(reg.classify_var_ref(var).borrow_callee),
        CoreExpr::MemberAccess { object, member, .. } => match object.as_ref() {
            CoreExpr::Var { var, .. } => Some(reg.resolve_member_access_callee(var.name, *member)),
            _ => None,
        },
        _ => None,
    }
}

/// Decide which borrowed call arguments need an owning reference while the
/// callee runs. A borrowed parameter is normally safe to pass without a
/// clone, but the callee's reuse analysis only sees the reference count. If
/// the caller still owns the argument (or a live scrutinee owns an extracted
/// field), that count can under-report reachability and let the callee mutate
/// the caller's value in place.
///
/// A binder also passed Owned in the same call can need a guard too. The
/// arguments are planned right to left, so the rightmost use of a binder dead
/// after the call takes no Dup: if that use is Owned, the callee gets the
/// caller's only reference and may free it while a borrowed position still
/// lends it. An Owned use further left already has its Dup, and a guard there
/// would only add a Drop that blocks a tail call.
fn call_guard_mask(
    args: &[CoreExpr],
    arg_modes: &[BorrowMode],
    tail_env: &AetherEnv,
    field_parents: &FieldParents,
) -> Vec<bool> {
    let mut seen = HashSet::new();
    let mut given = HashSet::new();
    for (arg, mode) in args.iter().zip(arg_modes).rev() {
        if let CoreExpr::Var { var, .. } = arg
            && let Some(binder) = var.binder
            && seen.insert(binder)
            && *mode == BorrowMode::Owned
        {
            given.insert(binder);
        }
    }
    let held = |binder: CoreBinderId| tail_env.is_live(binder) || given.contains(&binder);

    args.iter()
        .zip(arg_modes.iter())
        .map(|(arg, mode)| {
            if *mode != BorrowMode::Borrowed {
                return false;
            }

            let CoreExpr::Var { var, .. } = arg else {
                // A non-variable borrowed expression has no stable ownership
                // provenance available at this point, so guard it
                // conservatively.
                return true;
            };
            let Some(mut current) = var.binder else {
                return true;
            };

            if held(current) {
                return true;
            }

            let mut visited = HashSet::new();
            while let Some(field) = field_parents.get(&current) {
                if held(field.parent) {
                    return true;
                }
                if !visited.insert(current) || field.parent == current {
                    break;
                }
                current = field.parent;
            }

            false
        })
        .collect()
}

fn plan_expr_list<F>(
    exprs: Vec<CoreExpr>,
    tail_env: AetherEnv,
    registry: Option<&BorrowRegistry>,
    scope: &mut Scope,
    field_parents: &mut FieldParents,
    demand_for_index: F,
) -> (Vec<CoreExpr>, AetherEnv)
where
    F: Fn(usize) -> ValueDemand,
{
    let mut env = tail_env;
    let mut planned = Vec::with_capacity(exprs.len());
    for (index, expr) in exprs.into_iter().enumerate().rev() {
        let plan = plan_expr(
            expr,
            env,
            demand_for_index(index),
            registry,
            scope,
            field_parents,
        );
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
    field_parents: &FieldParents,
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
            let borrowed_parent = field_parents
                .get(&id)
                .is_some_and(|field| tail_env.is_borrowed(field.parent));
            // An owned use of something the caller lent takes its own
            // reference: the caller keeps the one it lent.
            let lent = scope.lent(id, field_parents);
            let needs_dup =
                tail_env.is_live(id) || scope.views.contains(&id) || borrowed_parent || lent;
            tail_env.mark_owned(id);
            let expr = if needs_dup {
                if let Some(binder) = scope.get(&id).cloned() {
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

/// A case arm planned, waiting for the releases that balance the other arms.
struct PlannedArm {
    pat: crate::core::CorePat,
    guard: Option<CoreExpr>,
    rhs: CoreExpr,
    span: Span,
    env_without_pats: AetherEnv,
    pat_ids: Vec<CoreBinderId>,
    /// The fields the arm owns and keeps, and whether it gives each away.
    kept_fields: Vec<(CoreBinder, bool)>,
    /// Whether the arm releases the matched cell at its head.
    head_drop: bool,
}

/// Whether `child` was read out of `ancestor`, directly or through other
/// fields: releasing `ancestor` can free it.
fn descends_from(
    child: CoreBinderId,
    ancestor: CoreBinderId,
    field_parents: &FieldParents,
) -> bool {
    let mut current = child;
    // A field's parent chain is acyclic; the bound is only a guard.
    for _ in 0..64 {
        match field_parents.get(&current) {
            Some(field) if field.parent == ancestor => return true,
            Some(field) => current = field.parent,
            None => return false,
        }
    }
    false
}

fn wrap_drop(binder: CoreBinder, body: CoreExpr, span: Span) -> CoreExpr {
    // Unboxed primitives (Int, Float, Bool) don't need reference counting.
    if !binder.rep.needs_rc() {
        return body;
    }
    CoreExpr::Drop {
        var: CoreVarRef::resolved(&binder),
        body: Box::new(body),
        span,
    }
}

fn wrap_dups(binder: CoreBinder, body: CoreExpr, span: Span, n: usize) -> CoreExpr {
    // Unboxed primitives (Int, Float, Bool) don't need reference counting.
    if !binder.rep.needs_rc() {
        return body;
    }
    let mut result = body;
    for _ in 0..n {
        result = CoreExpr::Dup {
            var: CoreVarRef::resolved(&binder),
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

fn pat_constructor_tag(pat: &crate::core::CorePat) -> Option<CoreTag> {
    match pat {
        crate::core::CorePat::Con { tag, .. } => Some(tag.clone()),
        _ => None,
    }
}

/// Whether `rhs`, not yet planned, builds a cell `pat`'s value could be
/// reused for. `claims` says whether an inner arm on its own scrutinee will
/// release that scrutinee at its head, taking the cell for itself.
fn has_compatible_con(
    pat: &crate::core::CorePat,
    rhs: &CoreExpr,
    claims: &dyn Fn(CoreBinderId, &CoreAlt) -> bool,
) -> bool {
    let pat_tag = match pat {
        crate::core::CorePat::Con { tag, .. } => Some(tag),
        _ => None,
    };
    let Some(pat_tag) = pat_tag else {
        return false;
    };
    unclaimed_con_tag_in_spine(rhs, pat_tag, claims, &mut Vec::new())
        .is_some_and(|ref con_tag| tags_shape_compatible(pat_tag, con_tag))
}

/// The tag of a constructor along `expr`'s spine that a scrutinee being
/// released could reuse: as [`find_con_tag_in_spine`], but leaving out an
/// inner arm that `claims` for its own scrutinee. The reuse pass gives such
/// a constructor to the inner scrutinee, so releasing the outer one for it
/// would only release it early. A scrutinee a pattern on the way down
/// bound (`bound`) is a view, and never claims one.
fn unclaimed_con_tag_in_spine(
    expr: &CoreExpr,
    expected_tag: &CoreTag,
    claims: &dyn Fn(CoreBinderId, &CoreAlt) -> bool,
    bound: &mut Vec<CoreBinderId>,
) -> Option<CoreTag> {
    match expr {
        CoreExpr::Reuse { .. } => None,
        _ if constructor_shape_for_tag_aether(expr, Some(expected_tag)).is_some() => {
            constructor_shape_for_tag_aether(expr, Some(expected_tag)).map(|(tag, _, _)| tag)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            let scrutinee = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => var.binder,
                _ => None,
            };
            alts.iter().find_map(|alt| {
                let claimed = scrutinee.is_some_and(|inner| {
                    !bound.contains(&inner)
                        && pat_constructor_tag(&alt.pat)
                            .is_some_and(|tag| tags_shape_compatible(expected_tag, &tag))
                        && claims(inner, alt)
                });
                if claimed {
                    return None;
                }
                let depth = bound.len();
                bound.extend(pat_binders(&alt.pat));
                let found = unclaimed_con_tag_in_spine(&alt.rhs, expected_tag, claims, bound);
                bound.truncate(depth);
                found
            })
        }
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            unclaimed_con_tag_in_spine(body, expected_tag, claims, bound)
        }
        _ => None,
    }
}

fn alt_uses_binder(alt: &CoreAlt, binder: CoreBinderId) -> bool {
    expr_uses_binder(&alt.rhs, binder)
        || alt
            .guard
            .as_ref()
            .is_some_and(|guard| expr_uses_binder(guard, binder))
}

fn rhs_has_compatible_tag(rhs: &CoreExpr, tag: &CoreTag) -> bool {
    find_con_tag_in_spine(rhs, Some(tag))
        .is_some_and(|ref con_tag| tags_shape_compatible(tag, con_tag))
}

fn find_con_tag_in_spine(
    expr: &CoreExpr,
    expected_tag: Option<&crate::core::CoreTag>,
) -> Option<crate::core::CoreTag> {
    match expr {
        CoreExpr::Reuse { tag, .. } => Some(tag.clone()),
        _ if constructor_shape_for_tag_aether(expr, expected_tag).is_some() => {
            constructor_shape_for_tag_aether(expr, expected_tag).map(|(tag, _, _)| tag)
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
    use_counts_aether(expr).get(&binder).cloned().unwrap_or(0) > 0
}

pub(super) fn expr_drops_binder(expr: &CoreExpr, binder: CoreBinderId) -> bool {
    match expr {
        CoreExpr::Drop { var, body, .. } => {
            var.binder == Some(binder) || expr_drops_binder(body, binder)
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            expr_drops_binder(rhs, binder) || expr_drops_binder(body, binder)
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            bindings
                .iter()
                .any(|(_, rhs)| expr_drops_binder(rhs, binder))
                || expr_drops_binder(body, binder)
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
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            parameter
                .as_ref()
                .is_some_and(|parameter| expr_drops_binder(parameter, binder))
                || expr_drops_binder(body, binder)
                || handlers
                    .iter()
                    .any(|handler| expr_drops_binder(&handler.body, binder))
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => expr_drops_binder(unique_body, binder) || expr_drops_binder(shared_body, binder),
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            expr_drops_binder(object, binder)
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::borrow_infer::{BorrowMode, BorrowRegistry};
    use crate::aether::{
        AetherAlt as CoreAlt, AetherExpr as CoreExpr, AetherHandler as CoreHandler,
    };
    use crate::core::{CoreBinder, CoreBinderId, CoreLit, CorePat, CorePrimOp, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::insert_dup_drop_with_registry;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(&binder),
            span: Span::default(),
        }
    }

    fn borrowed_call_mask(expr: &CoreExpr) -> Option<Vec<bool>> {
        match expr {
            CoreExpr::AetherCall {
                guarded_borrowed_args,
                ..
            } => Some(guarded_borrowed_args.clone()),
            CoreExpr::Lam { body, .. }
            | CoreExpr::Let { body, .. }
            | CoreExpr::LetRec { body, .. }
            | CoreExpr::Dup { body, .. }
            | CoreExpr::Drop { body, .. }
            | CoreExpr::Return { value: body, .. } => borrowed_call_mask(body),
            CoreExpr::Con { fields, .. }
            | CoreExpr::PrimOp { args: fields, .. }
            | CoreExpr::Perform { args: fields, .. } => fields.iter().find_map(borrowed_call_mask),
            CoreExpr::Case { alts, .. } => alts.iter().find_map(|alt| borrowed_call_mask(&alt.rhs)),
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                borrowed_call_mask(object)
            }
            _ => None,
        }
    }

    fn borrowed_len_registry(interner: &mut Interner) -> BorrowRegistry {
        let mut registry = BorrowRegistry::default();
        registry.by_name.insert(
            interner.intern("len"),
            crate::aether::borrow_infer::BorrowSignature::new(
                vec![BorrowMode::Borrowed],
                crate::aether::borrow_infer::BorrowProvenance::BaseRuntime,
            ),
        );
        registry
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
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                bindings
                    .iter()
                    .map(|(_, rhs)| count_binder_nodes(rhs, binder, predicate))
                    .sum::<usize>()
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
            CoreExpr::Handle {
                body,
                parameter,
                handlers,
                ..
            } => {
                parameter.as_ref().map_or(0, |parameter| {
                    count_binder_nodes(parameter, binder, predicate)
                }) + count_binder_nodes(body, binder, predicate)
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
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                count_binder_nodes(object, binder, predicate)
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
            param_types: vec![],
            result_ty: None,
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

    /// A move primop takes over the value it sends, so a value read after the
    /// send gets its own reference first (PA L3).
    #[test]
    fn a_move_primop_takes_its_value() {
        let mut interner = Interner::new();
        let ch = binder(&mut interner, 1, "ch");
        let xs = binder(&mut interner, 2, "xs");
        let ok = binder(&mut interner, 3, "ok");
        let expr = CoreExpr::Lam {
            params: vec![ch, xs],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Let {
                var: ok,
                rhs: Box::new(CoreExpr::PrimOp {
                    op: CorePrimOp::ChanTrySendMove,
                    args: vec![var(ch), var(xs)],
                    span: Span::default(),
                }),
                body: Box::new(var(xs)),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &BorrowRegistry::default());
        let dups = |binder| {
            count_binder_nodes(
                &rewritten,
                binder,
                &|expr, binder| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(binder)),
            )
        };
        assert_eq!(dups(xs.id), 1, "{rewritten:?}");
        assert_eq!(dups(ch.id), 0, "{rewritten:?}");
    }

    /// A closure bound locally takes its arguments owned, whatever a global
    /// of the same name borrows: its body plans its parameters as its own.
    #[test]
    fn a_local_closure_never_takes_a_global_signature_by_name() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 1, "f");
        let x = binder(&mut interner, 2, "x");
        let mut registry = BorrowRegistry::default();
        registry.by_name.insert(
            f.name,
            crate::aether::borrow_infer::BorrowSignature::new(
                vec![BorrowMode::Borrowed],
                crate::aether::borrow_infer::BorrowProvenance::Inferred,
            ),
        );

        let expr = CoreExpr::Lam {
            params: vec![f, x],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::App {
                func: Box::new(var(f)),
                args: vec![var(x)],
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
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
    fn borrowed_call_guards_follow_liveness_and_field_aliases() {
        let mut interner = Interner::new();
        let len = interner.intern("len");
        let registry = borrowed_len_registry(&mut interner);
        let pair = crate::core::CoreTag::Named(interner.intern("Pair"));

        let linear = CoreExpr::Lam {
            params: vec![binder(&mut interner, 1, "x")],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::App {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::unresolved(len),
                    span: Span::default(),
                }),
                args: vec![var(binder(&mut interner, 1, "x"))],
                span: Span::default(),
            }),
            span: Span::default(),
        };
        let linear = insert_dup_drop_with_registry(linear, &registry);
        assert_eq!(borrowed_call_mask(&linear), Some(vec![false]));

        let x = binder(&mut interner, 2, "x");
        let live = CoreExpr::Lam {
            params: vec![x],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Con {
                tag: pair.clone(),
                fields: vec![
                    CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::unresolved(len),
                            span: Span::default(),
                        }),
                        args: vec![var(x)],
                        span: Span::default(),
                    },
                    var(x),
                ],
                span: Span::default(),
            }),
            span: Span::default(),
        };
        let live = insert_dup_drop_with_registry(live, &registry);
        assert_eq!(borrowed_call_mask(&live), Some(vec![true]));

        let xs = binder(&mut interner, 3, "xs");
        let field = binder(&mut interner, 4, "field");
        let nested = binder(&mut interner, 5, "nested");
        let aliased = CoreExpr::Lam {
            params: vec![xs],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Let {
                var: field,
                rhs: Box::new(CoreExpr::TupleField {
                    object: Box::new(var(xs)),
                    index: 0,
                    span: Span::default(),
                }),
                body: Box::new(CoreExpr::Let {
                    var: nested,
                    rhs: Box::new(CoreExpr::TupleField {
                        object: Box::new(var(field)),
                        index: 0,
                        span: Span::default(),
                    }),
                    body: Box::new(CoreExpr::Con {
                        tag: pair,
                        fields: vec![
                            CoreExpr::App {
                                func: Box::new(CoreExpr::Var {
                                    var: CoreVarRef::unresolved(len),
                                    span: Span::default(),
                                }),
                                args: vec![var(nested)],
                                span: Span::default(),
                            },
                            var(xs),
                        ],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };
        let aliased = insert_dup_drop_with_registry(aliased, &registry);
        assert_eq!(borrowed_call_mask(&aliased), Some(vec![true]));
    }

    /// `lam(xs) -> case xs of Cons(h, t) -> Cons(h, t); _ -> Nil`.
    fn rebuild(xs: CoreBinder, h: CoreBinder, t: CoreBinder) -> CoreExpr {
        CoreExpr::Lam {
            params: vec![xs],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(var(xs)),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Con {
                            tag: crate::core::CoreTag::Cons,
                            fields: vec![CorePat::Var(h), CorePat::Var(t)],
                        },
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: crate::core::CoreTag::Cons,
                            fields: vec![var(h), var(t)],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: crate::core::CoreTag::Nil,
                            fields: vec![],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                ],
                join_ty: None,
                span: Span::default(),
            }),
            span: Span::default(),
        }
    }

    fn dups(expr: &CoreExpr, b: CoreBinder) -> usize {
        count_binder_nodes(
            expr,
            b.id,
            &|expr, binder| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(binder)),
        )
    }

    fn drops(expr: &CoreExpr, b: CoreBinder) -> usize {
        count_binder_nodes(
            expr,
            b.id,
            &|expr, binder| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(binder)),
        )
    }

    /// The views the F3-2.0 verifier finds given away without a copy.
    fn views_consumed(expr: &CoreExpr) -> usize {
        crate::aether::verify::verify_def_views_aether(expr, None).len()
    }

    #[test]
    fn a_borrowed_parameter_is_never_released() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        // Owned: the cell is released for reuse, so the arm copies its fields
        // first; on the unique path drop_spec takes the copies out again and
        // the cell hands its own references over.
        let owned = super::insert_dup_drop_for_def(
            rebuild(xs, h, t),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&owned, xs), 1, "{owned:?}");
        assert_eq!((dups(&owned, h), dups(&owned, t)), (1, 1), "{owned:?}");

        // Borrowed: the caller keeps the cell, so it is never dropped, and
        // the new cell takes its own reference to each field.
        let lent = super::insert_dup_drop_for_def(
            rebuild(xs, h, t),
            &BorrowRegistry::default(),
            [xs.id].into_iter().collect(),
        );
        assert_eq!(drops(&lent, xs), 0, "{lent:?}");
        assert_eq!((dups(&lent, h), dups(&lent, t)), (1, 1), "{lent:?}");
    }

    fn call(name: crate::syntax::Identifier, args: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef::unresolved(name),
                span: Span::default(),
            }),
            args,
            span: Span::default(),
        }
    }

    fn lam(params: Vec<CoreBinder>, body: CoreExpr) -> CoreExpr {
        CoreExpr::Lam {
            params,
            param_types: vec![],
            result_ty: None,
            body: Box::new(body),
            span: Span::default(),
        }
    }

    /// `lam(xs) -> case xs of pat [if guard] -> rhs; _ -> Nil`.
    fn match_one(xs: CoreBinder, pat: CorePat, guard: Option<CoreExpr>, rhs: CoreExpr) -> CoreExpr {
        lam(
            vec![xs],
            CoreExpr::Case {
                scrutinee: Box::new(var(xs)),
                alts: vec![
                    CoreAlt {
                        pat,
                        guard,
                        rhs,
                        span: Span::default(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Con {
                            tag: crate::core::CoreTag::Nil,
                            fields: vec![],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                ],
                join_ty: None,
                span: Span::default(),
            },
        )
    }

    fn cons(h: CoreBinder, t: CoreBinder) -> CorePat {
        CorePat::Con {
            tag: crate::core::CoreTag::Cons,
            fields: vec![CorePat::Var(h), CorePat::Var(t)],
        }
    }

    /// 0189 V1: the cell holds the field, so giving the field away on its
    /// last use takes a copy, or the cell's release frees it under the
    /// callee.
    #[test]
    fn a_pattern_field_given_away_is_copied() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let f = interner.intern("f");
        let ok = interner.intern("ok");

        let plain = insert_dup_drop_with_registry(
            match_one(xs, cons(h, t), None, call(f, vec![var(t)])),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&plain, t), 1, "{plain:?}");
        assert_eq!(views_consumed(&plain), 0, "{plain:?}");

        let guarded = insert_dup_drop_with_registry(
            match_one(
                xs,
                cons(h, t),
                Some(call(ok, vec![var(h)])),
                call(f, vec![var(t)]),
            ),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&guarded, t), 1, "{guarded:?}");
        assert_eq!(views_consumed(&guarded), 0, "{guarded:?}");
    }

    /// A name for the whole matched value is the value itself: it holds the
    /// scrutinee's reference, so giving it away needs no copy.
    #[test]
    fn a_name_for_the_matched_value_is_not_copied() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let ys = binder(&mut interner, 2, "ys");
        let f = interner.intern("f");

        let planned = insert_dup_drop_with_registry(
            match_one(xs, CorePat::Var(ys), None, call(f, vec![var(ys)])),
            &BorrowRegistry::default(),
        );
        assert_eq!(
            (dups(&planned, ys), dups(&planned, xs)),
            (0, 0),
            "{planned:?}"
        );
        assert_eq!(views_consumed(&planned), 0, "{planned:?}");
    }

    /// `lam(params) -> case scrutinee of Cons(h, t) if guard -> Nil; _ -> fallback`.
    fn guarded_match(
        params: Vec<CoreBinder>,
        scrutinee: CoreBinder,
        [h, t]: [CoreBinder; 2],
        guard: CoreExpr,
        fallback: CoreExpr,
    ) -> CoreExpr {
        lam(
            params,
            CoreExpr::Case {
                scrutinee: Box::new(var(scrutinee)),
                alts: vec![
                    CoreAlt {
                        pat: cons(h, t),
                        guard: Some(guard),
                        rhs: CoreExpr::Con {
                            tag: crate::core::CoreTag::Nil,
                            fields: vec![],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: fallback,
                        span: Span::default(),
                    },
                ],
                join_ty: None,
                span: Span::default(),
            },
        )
    }

    /// A failed guard falls through to `_ -> x`, which still owns `x`, so the
    /// guard gives away a copy (audit M2).
    #[test]
    fn a_guard_copies_a_value_a_later_arm_uses() {
        let mut interner = Interner::new();
        let [x, junk, h, t] = [(1, "x"), (2, "junk"), (3, "h"), (4, "t")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let discard = interner.intern("discard");

        let planned = insert_dup_drop_with_registry(
            guarded_match(
                vec![x, junk],
                junk,
                [h, t],
                call(discard, vec![var(x)]),
                var(x),
            ),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&planned, x), 1, "{planned:?}");
    }

    /// The next arm reads the matched list again, so the guard gives away a
    /// copy.
    #[test]
    fn a_guard_copies_the_matched_value_it_gives_away() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let discard = interner.intern("discard");

        let planned = insert_dup_drop_with_registry(
            guarded_match(vec![xs], xs, [h, t], call(discard, vec![var(xs)]), var(xs)),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&planned, xs), 1, "{planned:?}");
    }

    #[test]
    fn a_guard_that_only_reads_needs_no_copy() {
        let mut interner = Interner::new();
        let [x, junk, h, t] = [(1, "x"), (2, "junk"), (3, "h"), (4, "t")]
            .map(|(id, name)| binder(&mut interner, id, name));

        let planned = insert_dup_drop_with_registry(
            guarded_match(vec![x, junk], junk, [h, t], var(x), var(x)),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&planned, x), 0, "{planned:?}");
    }

    fn field_of(object: CoreBinder, index: usize) -> CoreExpr {
        CoreExpr::TupleField {
            object: Box::new(var(object)),
            index,
            span: Span::default(),
        }
    }

    /// `let x = t.0` reads the field without a reference of its own: giving
    /// `x` away copies it, and an unused `x` is never released.
    #[test]
    fn a_tuple_field_bound_by_let_is_a_view() {
        let mut interner = Interner::new();
        let t = binder(&mut interner, 1, "t");
        let x = binder(&mut interner, 2, "x");
        let f = interner.intern("f");
        let let_x = |body| CoreExpr::Let {
            var: x,
            rhs: Box::new(field_of(t, 0)),
            body: Box::new(body),
            span: Span::default(),
        };

        let given = insert_dup_drop_with_registry(
            lam(vec![t], let_x(call(f, vec![var(x)]))),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&given, x), 1, "{given:?}");
        assert_eq!(views_consumed(&given), 0, "{given:?}");

        let unused = insert_dup_drop_with_registry(
            lam(
                vec![t],
                let_x(CoreExpr::Lit(CoreLit::Int(0), Span::default())),
            ),
            &BorrowRegistry::default(),
        );
        assert_eq!(drops(&unused, x), 0, "{unused:?}");
    }

    /// `f(t.0)` gives away a field it never named: it is held in a
    /// temporary and copied, `let tmp = t.0 in dup tmp in tmp`.
    #[test]
    fn a_tuple_field_given_away_is_copied() {
        let mut interner = Interner::new();
        let t = binder(&mut interner, 1, "t");
        let f = interner.intern("f");

        let planned = insert_dup_drop_with_registry(
            lam(vec![t], call(f, vec![field_of(t, 0)])),
            &BorrowRegistry::default(),
        );
        let tmp = CoreBinder::new(CoreBinderId(super::FIRST_TEMP_ID), interner.intern("tmp"));
        assert_eq!(dups(&planned, tmp), 1, "{planned:?}");
        assert_eq!(views_consumed(&planned), 0, "{planned:?}");
    }

    /// Each owned use of a view takes its own copy.
    #[test]
    fn a_view_given_away_twice_is_copied_twice() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let cell = |fields| CoreExpr::Con {
            tag: crate::core::CoreTag::Cons,
            fields,
            span: Span::default(),
        };

        let planned = insert_dup_drop_with_registry(
            match_one(
                xs,
                cons(h, t),
                None,
                cell(vec![var(h), cell(vec![var(h), var(t)])]),
            ),
            &BorrowRegistry::default(),
        );
        assert_eq!(
            (dups(&planned, h), dups(&planned, t)),
            (2, 1),
            "{planned:?}"
        );
        assert_eq!(views_consumed(&planned), 0, "{planned:?}");
    }

    /// A name for a matched view is a view too.
    #[test]
    fn a_name_for_a_matched_view_is_copied() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let ys = binder(&mut interner, 4, "ys");
        let f = interner.intern("f");
        let inner = CoreExpr::Case {
            scrutinee: Box::new(var(t)),
            alts: vec![CoreAlt {
                pat: CorePat::Var(ys),
                guard: None,
                rhs: call(f, vec![var(ys)]),
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let planned = insert_dup_drop_with_registry(
            match_one(xs, cons(h, t), None, inner),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&planned, ys), 1, "{planned:?}");
        assert_eq!(views_consumed(&planned), 0, "{planned:?}");
    }

    /// `let x = t.0 in case c of 0 -> f(x); 1 -> h(t); _ -> 0`: the arms
    /// that do not give `x` away must not release it to even up with the
    /// arm that does, since `t` still holds it.
    #[test]
    fn a_view_is_never_released_to_balance_another_arm() {
        let mut interner = Interner::new();
        let t = binder(&mut interner, 1, "t");
        let c = binder(&mut interner, 2, "c");
        let x = binder(&mut interner, 3, "x");
        let f = interner.intern("f");
        let h = interner.intern("h");
        let arm = |pat, rhs| CoreAlt {
            pat,
            guard: None,
            rhs,
            span: Span::default(),
        };
        let body = CoreExpr::Case {
            scrutinee: Box::new(var(c)),
            alts: vec![
                arm(CorePat::Lit(CoreLit::Int(0)), call(f, vec![var(x)])),
                arm(CorePat::Lit(CoreLit::Int(1)), call(h, vec![var(t)])),
                arm(
                    CorePat::Wildcard,
                    CoreExpr::Lit(CoreLit::Int(0), Span::default()),
                ),
            ],
            join_ty: None,
            span: Span::default(),
        };
        let expr = lam(
            vec![t, c],
            CoreExpr::Let {
                var: x,
                rhs: Box::new(field_of(t, 0)),
                body: Box::new(body),
                span: Span::default(),
            },
        );

        let planned = insert_dup_drop_with_registry(expr, &BorrowRegistry::default());
        assert_eq!(drops(&planned, x), 0, "{planned:?}");
    }

    /// `let x = t.0 in case x of Cons(a, b) -> Cons(a, b)`: `x` holds no
    /// reference, so the match neither releases it nor reuses its cell.
    #[test]
    fn a_matched_view_is_never_released() {
        let mut interner = Interner::new();
        let t = binder(&mut interner, 1, "t");
        let x = binder(&mut interner, 2, "x");
        let a = binder(&mut interner, 3, "a");
        let b = binder(&mut interner, 4, "b");
        let CoreExpr::Lam { body: rebuilt, .. } = rebuild(x, a, b) else {
            unreachable!("rebuild makes a lambda");
        };
        let expr = lam(
            vec![t],
            CoreExpr::Let {
                var: x,
                rhs: Box::new(field_of(t, 0)),
                body: rebuilt,
                span: Span::default(),
            },
        );

        let planned = insert_dup_drop_with_registry(expr, &BorrowRegistry::default());
        assert_eq!(drops(&planned, x), 0, "{planned:?}");
    }

    /// 0189 V2: making a closure retains its captures, so the closure holds
    /// its own reference and each call that gives a capture away copies it.
    #[test]
    fn a_capture_given_away_is_copied() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let f = interner.intern("f");

        let returned = insert_dup_drop_with_registry(
            lam(vec![xs], lam(vec![], var(xs))),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&returned, xs), 1, "{returned:?}");
        assert_eq!(views_consumed(&returned), 0, "{returned:?}");

        let passed = insert_dup_drop_with_registry(
            lam(vec![xs], lam(vec![], call(f, vec![var(xs)]))),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&passed, xs), 1, "{passed:?}");
        assert_eq!(views_consumed(&passed), 0, "{passed:?}");
    }

    /// `\() -> case c of 0 -> f(xs); _ -> 0`: the arm that does not give
    /// `xs` away must not release it to even up, since the closure keeps it.
    #[test]
    fn a_lambda_never_releases_its_capture() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let c = binder(&mut interner, 2, "c");
        let f = interner.intern("f");
        let arm = |pat, rhs| CoreAlt {
            pat,
            guard: None,
            rhs,
            span: Span::default(),
        };
        let body = CoreExpr::Case {
            scrutinee: Box::new(var(c)),
            alts: vec![
                arm(CorePat::Lit(CoreLit::Int(0)), call(f, vec![var(xs)])),
                arm(
                    CorePat::Wildcard,
                    CoreExpr::Lit(CoreLit::Int(0), Span::default()),
                ),
            ],
            join_ty: None,
            span: Span::default(),
        };

        let planned = insert_dup_drop_with_registry(
            lam(vec![xs, c], lam(vec![], body)),
            &BorrowRegistry::default(),
        );
        let CoreExpr::Lam { body: outer, .. } = &planned else {
            unreachable!("planned a lambda");
        };
        assert_eq!(drops(outer, xs), 0, "{planned:?}");
        assert_eq!(dups(outer, xs), 1, "{planned:?}");
    }

    /// 0189 V3: the runtime lends a clause its argument and releases it once
    /// the clause returns (`runtime/c/effects.c`), so an arm that gives the
    /// argument, or its `resume`, away copies it.
    #[test]
    fn a_handler_value_given_away_is_copied() {
        let mut interner = Interner::new();
        let resume = binder(&mut interner, 1, "resume");
        let v = binder(&mut interner, 2, "v");
        let f = interner.intern("f");
        let effect = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let op = interner.intern("ask");
        let handle = |body| CoreExpr::Handle {
            body: Box::new(CoreExpr::Lit(CoreLit::Unit, Span::default())),
            effect,
            parameter: None,
            handlers: vec![CoreHandler {
                operation: op,
                params: vec![v],
                param_types: vec![],
                resume,
                resume_ty: None,
                state: None,
                state_ty: None,
                body,
                span: Span::default(),
            }],
            span: Span::default(),
        };

        let returned =
            insert_dup_drop_with_registry(lam(vec![], handle(var(v))), &BorrowRegistry::default());
        assert_eq!(dups(&returned, v), 1, "{returned:?}");
        assert_eq!(views_consumed(&returned), 0, "{returned:?}");

        let stored = insert_dup_drop_with_registry(
            lam(vec![], handle(call(f, vec![var(resume)]))),
            &BorrowRegistry::default(),
        );
        assert_eq!(dups(&stored, resume), 1, "{stored:?}");
        assert_eq!(views_consumed(&stored), 0, "{stored:?}");
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
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Let {
                var: f,
                rhs: Box::new(CoreExpr::Lam {
                    params: vec![],
                    param_types: vec![],
                    result_ty: None,
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
        let effect = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let op = interner.intern("read");

        let expr = CoreExpr::Lam {
            params: vec![outer],
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Handle {
                body: Box::new(CoreExpr::Lit(CoreLit::Unit, Span::default())),
                effect,
                parameter: None,
                handlers: vec![CoreHandler {
                    operation: op,
                    params: vec![shadow],
                    param_types: vec![],
                    resume,
                    resume_ty: None,
                    state: None,
                    state_ty: None,
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
            param_types: vec![],
            result_ty: None,
            body: Box::new(CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
                alts: vec![
                    CoreAlt {
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
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::Lit(CoreLit::Int(0), Span::default()),
                        span: Span::default(),
                    },
                ],
                join_ty: None,
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

    /// The same binder lent and given in one call: guard the lent position
    /// only when the given one is the rightmost use, which the planner gives
    /// the caller's reference without a Dup. Further left, the given position
    /// already has its Dup, and a guard's Drop would block a tail call.
    #[test]
    fn a_binder_lent_and_given_in_one_call_is_guarded_only_when_given_last() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let args = [var(xs), var(xs)];
        let env = super::AetherEnv::default();
        let parents = super::FieldParents::new();
        let mask = |modes: &[BorrowMode]| super::call_guard_mask(&args, modes, &env, &parents);

        assert_eq!(
            mask(&[BorrowMode::Borrowed, BorrowMode::Owned]),
            [true, false]
        );
        assert_eq!(
            mask(&[BorrowMode::Owned, BorrowMode::Borrowed]),
            [false, false]
        );
    }

    /// The expressions directly inside `expr`.
    fn children(expr: &CoreExpr) -> Vec<&CoreExpr> {
        match expr {
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => vec![],
            CoreExpr::Lam { body, .. }
            | CoreExpr::Return { value: body, .. }
            | CoreExpr::Dup { body, .. }
            | CoreExpr::Drop { body, .. }
            | CoreExpr::MemberAccess { object: body, .. }
            | CoreExpr::TupleField { object: body, .. } => vec![body],
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                std::iter::once(func.as_ref()).chain(args).collect()
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                vec![rhs, body]
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => bindings
                .iter()
                .map(|(_, rhs)| rhs.as_ref())
                .chain(std::iter::once(body.as_ref()))
                .collect(),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => std::iter::once(scrutinee.as_ref())
                .chain(
                    alts.iter()
                        .flat_map(|alt| alt.guard.iter().chain([&alt.rhs])),
                )
                .collect(),
            CoreExpr::Con { fields, .. }
            | CoreExpr::PrimOp { args: fields, .. }
            | CoreExpr::Reuse { fields, .. }
            | CoreExpr::Perform { args: fields, .. } => fields.iter().collect(),
            CoreExpr::Handle {
                body,
                parameter,
                handlers,
                ..
            } => parameter
                .iter()
                .map(|parameter| parameter.as_ref())
                .chain(std::iter::once(body.as_ref()))
                .chain(handlers.iter().map(|handler| &handler.body))
                .collect(),
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => vec![unique_body, shared_body],
        }
    }

    fn uses(expr: &CoreExpr, b: CoreBinder) -> usize {
        count_binder_nodes(
            expr,
            b.id,
            &|expr, binder| matches!(expr, CoreExpr::Var { var, .. } if var.binder == Some(binder)),
        )
    }

    /// Whether each release of `xs` is preceded, in the run of dups and drops
    /// just above it, by a copy of every one of `fields` used after it: the
    /// release may free the cell and the fields with it (Perceus' `dup
    /// fields; drop xs`).
    fn copied_before_release(expr: &CoreExpr, xs: CoreBinder, fields: &[CoreBinder]) -> bool {
        fn walk(
            expr: &CoreExpr,
            run: &mut Vec<CoreBinderId>,
            xs: CoreBinder,
            fields: &[CoreBinder],
        ) -> bool {
            match expr {
                CoreExpr::Dup { var, body, .. } => {
                    run.extend(var.binder);
                    walk(body, run, xs, fields)
                }
                CoreExpr::Drop { var, body, .. } if var.binder == Some(xs.id) => {
                    fields
                        .iter()
                        .all(|field| uses(body, *field) == 0 || run.contains(&field.id))
                        && walk(body, &mut Vec::new(), xs, fields)
                }
                CoreExpr::Drop { body, .. } => walk(body, run, xs, fields),
                other => children(other)
                    .into_iter()
                    .all(|child| walk(child, &mut Vec::new(), xs, fields)),
            }
        }
        walk(expr, &mut Vec::new(), xs, fields)
    }

    fn cons_of(fields: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::Con {
            tag: crate::core::CoreTag::Cons,
            fields,
            span: Span::default(),
        }
    }

    /// Audit L2: the drop of the matched cell may free it and its fields, so
    /// the fields the arm keeps are copied first.
    #[test]
    fn fields_are_copied_before_the_matched_cell_is_released() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));

        let planned = super::insert_dup_drop_for_def(
            rebuild(xs, h, t),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(copied_before_release(&planned, xs, &[h, t]), "{planned:?}");
        assert_eq!(
            (dups(&planned, h), dups(&planned, t)),
            (1, 1),
            "{planned:?}"
        );
    }

    /// Audit L1's shape: a field stored twice needs two references, one from
    /// the head of the arm and one at its first use.
    #[test]
    fn a_field_used_twice_is_copied_at_the_head_and_at_its_first_use() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));

        let planned = super::insert_dup_drop_for_def(
            match_one(
                xs,
                cons(h, t),
                None,
                cons_of(vec![var(h), cons_of(vec![var(h), var(t)])]),
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(copied_before_release(&planned, xs, &[h, t]), "{planned:?}");
        assert_eq!(
            (dups(&planned, h), dups(&planned, t)),
            (2, 1),
            "{planned:?}"
        );
    }

    /// A nested field lives in an inner cell, which the outer cell's release
    /// frees with it.
    #[test]
    fn a_nested_field_is_copied_before_the_outer_cell_is_released() {
        let mut interner = Interner::new();
        let [xs, h, h2, t2] = [(1, "xs"), (2, "h"), (3, "h2"), (4, "t2")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let pat = CorePat::Con {
            tag: crate::core::CoreTag::Cons,
            fields: vec![CorePat::Var(h), cons(h2, t2)],
        };

        let planned = super::insert_dup_drop_for_def(
            match_one(xs, pat, None, cons_of(vec![var(h2), var(t2)])),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(
            copied_before_release(&planned, xs, &[h2, t2]),
            "{planned:?}"
        );
    }

    /// `case xs of Cons(h, t) -> f(h); _ -> xs`: the first arm owes the cell
    /// its release, and must keep `h` past it.
    #[test]
    fn a_cell_released_for_another_arm_copies_the_fields_first() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let f = interner.intern("f");

        let planned = super::insert_dup_drop_for_def(
            lam(
                vec![xs],
                CoreExpr::Case {
                    scrutinee: Box::new(var(xs)),
                    alts: vec![
                        CoreAlt {
                            pat: cons(h, t),
                            guard: None,
                            rhs: call(f, vec![var(h)]),
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: var(xs),
                            span: Span::default(),
                        },
                    ],
                    join_ty: None,
                    span: Span::default(),
                },
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(copied_before_release(&planned, xs, &[h, t]), "{planned:?}");
    }

    /// An inner match's arm that does not use the outer cell releases it, and
    /// the outer arm's fields are still used there.
    #[test]
    fn an_inner_release_of_the_outer_cell_copies_its_fields() {
        let mut interner = Interner::new();
        let [xs, c, h, t] = [(1, "xs"), (2, "c"), (3, "h"), (4, "t")]
            .map(|(id, name)| binder(&mut interner, id, name));

        let inner = CoreExpr::Case {
            scrutinee: Box::new(var(c)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: var(xs),
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: cons_of(vec![var(h), var(t)]),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let planned = super::insert_dup_drop_for_def(
            lam(
                vec![xs, c],
                CoreExpr::Case {
                    scrutinee: Box::new(var(xs)),
                    alts: vec![
                        CoreAlt {
                            pat: cons(h, t),
                            guard: None,
                            rhs: inner,
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: var(xs),
                            span: Span::default(),
                        },
                    ],
                    join_ty: None,
                    span: Span::default(),
                },
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(copied_before_release(&planned, xs, &[h, t]), "{planned:?}");
        // The fields stay views, so each use keeps its own copy as well: one
        // reference each leaks until views are released (KI-121).
        assert_eq!(
            (dups(&planned, h), dups(&planned, t)),
            (2, 2),
            "{planned:?}"
        );
    }

    /// The guard runs while the cell is still held, so its copies are its
    /// own; the arm copies the fields again before releasing the cell.
    #[test]
    fn a_guarded_arm_copies_its_fields_after_the_guard() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let ok = interner.intern("ok");

        let planned = super::insert_dup_drop_for_def(
            match_one(
                xs,
                cons(h, t),
                Some(call(ok, vec![var(h)])),
                cons_of(vec![var(h), var(t)]),
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
        assert!(copied_before_release(&planned, xs, &[h, t]), "{planned:?}");
    }

    fn case_of(scrutinee: CoreBinder, alts: Vec<(CorePat, CoreExpr)>) -> CoreExpr {
        CoreExpr::Case {
            scrutinee: Box::new(var(scrutinee)),
            alts: alts
                .into_iter()
                .map(|(pat, rhs)| CoreAlt {
                    pat,
                    guard: None,
                    rhs,
                    span: Span::default(),
                })
                .collect(),
            join_ty: None,
            span: Span::default(),
        }
    }

    fn nil() -> CoreExpr {
        CoreExpr::Con {
            tag: crate::core::CoreTag::Nil,
            fields: vec![],
            span: Span::default(),
        }
    }

    /// `h2` is read out of `t`, which is read out of `xs`: releasing `xs`
    /// can free all three, so `h2` is copied first, or `xs` is kept.
    #[test]
    fn a_field_of_a_field_is_copied_before_the_outer_cell_is_released() {
        let mut interner = Interner::new();
        let [xs, c, h, t, h2, t2] = [
            (1, "xs"),
            (2, "c"),
            (3, "h"),
            (4, "t"),
            (5, "h2"),
            (6, "t2"),
        ]
        .map(|(id, name)| binder(&mut interner, id, name));

        let innermost = case_of(
            c,
            vec![
                (CorePat::Lit(CoreLit::Bool(true)), var(xs)),
                (CorePat::Wildcard, var(h2)),
            ],
        );
        let planned = super::insert_dup_drop_for_def(
            lam(
                vec![xs, c],
                case_of(
                    xs,
                    vec![
                        (
                            cons(h, t),
                            case_of(
                                t,
                                vec![(cons(h2, t2), innermost), (CorePat::Wildcard, nil())],
                            ),
                        ),
                        (CorePat::Wildcard, nil()),
                    ],
                ),
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert!(copied_before_release(&planned, xs, &[h2]), "{planned:?}");
    }

    /// `zs` is bound by a pattern inside the arm, so it is a view and never
    /// releases its cell; the constructor is `xs`'s to reuse, and `xs` is
    /// released at the head.
    #[test]
    fn a_binder_matched_deeper_in_the_arm_claims_no_constructor() {
        let mut interner = Interner::new();
        let [xs, h, t, a, zs, b, c] = [
            (1, "xs"),
            (2, "h"),
            (3, "t"),
            (4, "a"),
            (5, "zs"),
            (6, "b"),
            (7, "c"),
        ]
        .map(|(id, name)| binder(&mut interner, id, name));

        let deepest = case_of(
            zs,
            vec![
                (cons(b, c), cons_of(vec![var(b), var(c)])),
                (CorePat::Wildcard, nil()),
            ],
        );
        let planned = super::insert_dup_drop_for_def(
            lam(
                vec![xs],
                case_of(
                    xs,
                    vec![
                        (
                            cons(h, t),
                            case_of(h, vec![(cons(a, zs), deepest), (CorePat::Wildcard, nil())]),
                        ),
                        (CorePat::Wildcard, nil()),
                    ],
                ),
            ),
            &BorrowRegistry::default(),
            Default::default(),
        );
        assert_eq!(drops(&planned, xs), 1, "{planned:?}");
    }

    /// The other arm only reads `xs`, so this arm keeps the cell and copies
    /// only the field it gives away.
    #[test]
    fn a_kept_cell_copies_only_the_fields_given_away() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let f = interner.intern("f");
        let len = interner.intern("len");
        let registry = borrowed_len_registry(&mut interner);

        let planned = super::insert_dup_drop_for_def(
            lam(
                vec![xs],
                case_of(
                    xs,
                    vec![
                        (cons(h, t), call(f, vec![var(h)])),
                        (CorePat::Wildcard, call(len, vec![var(xs)])),
                    ],
                ),
            ),
            &registry,
            Default::default(),
        );
        assert_eq!(
            (dups(&planned, h), dups(&planned, t), drops(&planned, xs)),
            (1, 0, 0),
            "{planned:?}"
        );
    }
}
