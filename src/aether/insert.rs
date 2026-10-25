//! Aether Phase 5: environment-based Dup/Drop insertion.
//!
//! The planner walks expressions in reverse, carrying an ownership/liveness
//! environment. `Dup` and `Drop` are emitted as local consequences of that
//! environment instead of from whole-body use counts.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::core::{CoreBinder, CoreBinderId, CorePrimOp, CoreTag, CoreVarRef};
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
    /// Binders owned from a right-hand side not known to make a fresh value
    /// (a runtime function's result, a member read). Such a value may point
    /// into an argument without a reference of its own, so an argument it
    /// names is not released after its last use (`aliases_owned_unknown`).
    owned_unknown: HashSet<CoreBinderId>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TailPosition {
    Tail,
    NonTail,
}

/// Insert Dup/Drop annotations into a Core IR expression.
pub fn insert_dup_drop(expr: CoreExpr) -> CoreExpr {
    let mut scope = Scope::new();
    let mut field_parents = FieldParents::new();
    plan_expr(
        expr,
        AetherEnv::default(),
        ValueDemand::Owned,
        TailPosition::Tail,
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
        TailPosition::Tail,
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
        TailPosition::Tail,
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
    position: TailPosition,
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

            // The binding made the value, so it holds a reference of its own.
            let owner = !view && var.rep.needs_rc() && produces_fresh_value(&rhs, registry);
            let body_plan = plan_expr(
                *body,
                tail_env,
                demand,
                position,
                registry,
                scope,
                field_parents,
            );
            let mut binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore && !view {
                body_expr = wrap_drop(var, body_expr, span);
            }
            // A view only reads its field; each owned use copies it.
            if view && binder_demand == ValueDemand::Owned {
                binder_demand = ValueDemand::Borrowed;
            }
            if owner && binder_demand != ValueDemand::Ignore {
                let (expr, released) = release_owner(
                    var,
                    body_expr,
                    span,
                    demand != ValueDemand::Owned,
                    position,
                    scope,
                    registry,
                );
                body_expr = expr;
                if released {
                    binder_demand = ValueDemand::Owned;
                }
            }
            if !owner
                && !view
                && binder_demand == ValueDemand::Owned
                && !matches!(rhs.as_ref(), CoreExpr::Var { .. })
            {
                scope.owned_unknown.insert(var.id);
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(
                *rhs,
                rhs_tail,
                binder_demand,
                TailPosition::NonTail,
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
            let owner = var.rep.needs_rc() && matches!(rhs.as_ref(), CoreExpr::Lam { .. });
            let body_plan = plan_expr(
                *body,
                tail_env,
                demand,
                position,
                registry,
                scope,
                field_parents,
            );
            let mut binder_demand = binder_demand(&body_plan.env_before, var.id);
            let mut body_expr = body_plan.expr;
            if binder_demand == ValueDemand::Ignore {
                body_expr = wrap_drop(var, body_expr, span);
            } else if owner {
                let (expr, released) = release_owner(
                    var,
                    body_expr,
                    span,
                    demand != ValueDemand::Owned,
                    position,
                    scope,
                    registry,
                );
                body_expr = expr;
                if released {
                    binder_demand = ValueDemand::Owned;
                }
            }

            let mut rhs_tail = body_plan.env_before.clone();
            rhs_tail.remove(var.id);
            let rhs_plan = plan_expr(
                *rhs,
                rhs_tail,
                binder_demand,
                TailPosition::NonTail,
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
            let body_plan = plan_expr(
                *body,
                tail_env,
                demand,
                position,
                registry,
                scope,
                field_parents,
            );
            let mut body_expr = body_plan.expr;
            // An unused binding is released first; a used one, as for
            // `LetRec`, after its last use.
            let mut released = BTreeSet::new();
            for (var, rhs) in bindings.iter().rev() {
                let bd = binder_demand(&body_plan.env_before, var.id);
                if bd == ValueDemand::Ignore {
                    body_expr = wrap_drop(*var, body_expr, span);
                } else if var.rep.needs_rc() && matches!(rhs.as_ref(), CoreExpr::Lam { .. }) {
                    let (expr, was_released) = release_owner(
                        *var,
                        body_expr,
                        span,
                        demand != ValueDemand::Owned,
                        position,
                        scope,
                        registry,
                    );
                    body_expr = expr;
                    if was_released {
                        released.insert(var.id);
                    }
                }
            }

            let mut env = body_plan.env_before.clone();
            let mut new_bindings = Vec::with_capacity(bindings.len());
            for (var, rhs) in bindings.into_iter().rev() {
                let bd = if released.contains(&var.id) {
                    ValueDemand::Owned
                } else {
                    binder_demand(&env, var.id)
                };
                let mut rhs_tail = env.clone();
                rhs_tail.remove(var.id);
                let rhs_plan = plan_expr(
                    *rhs,
                    rhs_tail,
                    bd,
                    TailPosition::NonTail,
                    registry,
                    scope,
                    field_parents,
                );
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
                TailPosition::Tail,
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
                if scope.borrowed_params.contains(&param.id) {
                    continue;
                }
                if !body_plan.env_before.is_live(param.id) {
                    body_expr = wrap_drop(*param, body_expr, span);
                } else if param.rep.needs_rc() {
                    (body_expr, _) = release_owner(
                        *param,
                        body_expr,
                        span,
                        false,
                        TailPosition::Tail,
                        scope,
                        registry,
                    );
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
            // Making the closure retains each capture, so it only reads them:
            // the outer binding keeps its own reference.
            for capture in captures {
                if !param_ids.contains(&capture) {
                    env_before.mark_borrowed(capture);
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
                TailPosition::NonTail,
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
                    // The `held` pass fills this in on the finished plan.
                    owned_after: Vec::new(),
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
            owned_after: _,
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
                TailPosition::NonTail,
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
                    // The `held` pass fills this in on the finished plan.
                    owned_after: Vec::new(),
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

            // Pattern views of an owned value that some arm still uses. An arm
            // that does not use it releases it to balance that arm, so each
            // view it keeps is copied above the release first.
            let parent_views: Vec<(CoreBinder, CoreBinderId, Vec<bool>, Vec<bool>)> = {
                let mut views: Vec<_> = field_parents
                    .iter()
                    .filter(|(child, field)| {
                        field.pattern
                            && scope.views.contains(*child)
                            && scrutinee_var
                                .as_ref()
                                .is_none_or(|sv| sv.id != field.parent)
                            && scope.get(&field.parent).is_some()
                            && !scope.views.contains(&field.parent)
                            && !scope.lent(field.parent, field_parents)
                            && !tail_env.is_live(field.parent)
                    })
                    .filter_map(|(child, field)| {
                        let binder = scope.get(child).copied()?;
                        binder.rep.needs_rc().then(|| {
                            let uses =
                                |id| alts.iter().map(|alt| alt_uses_binder(alt, id)).collect();
                            (binder, field.parent, uses(*child), uses(field.parent))
                        })
                    })
                    .collect();
                views.sort_by_key(|(binder, ..)| binder.id);
                views
            };

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

                let owned_views: Vec<CoreBinder> = parent_views
                    .iter()
                    .filter(|(_, _, child_uses, parent_uses)| {
                        child_uses[index]
                            && !parent_uses[index]
                            && parent_uses
                                .iter()
                                .enumerate()
                                .any(|(other, uses)| other != index && *uses)
                    })
                    .map(|(binder, ..)| *binder)
                    .collect();
                for view in &owned_views {
                    scope.views.remove(&view.id);
                }

                let rhs_plan = plan_expr(
                    rhs,
                    tail_env.clone(),
                    demand,
                    position,
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
                        TailPosition::NonTail,
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
                for view in &owned_views {
                    scope.views.insert(view.id);
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

                if let Some(scrut_binder) = scrutinee_var.as_ref()
                    && head_drop
                {
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
                    owned_views,
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
                         owned_views,
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
                        // reference first. Unless the arm planned it as owned,
                        // its uses keep their own copies, so this one is never
                        // paired: it leaks (KI-121).
                        let children_kept: Vec<Vec<CoreBinder>> = compensation
                            .iter()
                            .map(|binder| {
                                let mut children: Vec<CoreBinder> = field_parents
                                    .keys()
                                    .filter(|child| {
                                        !owned_views.iter().any(|view| view.id == **child)
                                            && descends_from(**child, binder.id, field_parents)
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
                        // An owned view takes its copy whether or not the release
                        // happens: the arm planned it as holding one.
                        let copied: Vec<CoreBinder> = kept_fields
                            .into_iter()
                            .filter(|(_, given_away)| *given_away || released)
                            .map(|(field, _)| field)
                            .chain(owned_views)
                            .collect();
                        // That copy is the field's own reference: a path that only
                        // reads the field releases it after the last read.
                        let rhs = copied.iter().filter(|field| field.rep.needs_rc()).fold(
                            rhs,
                            |body, field| {
                                release_owner(
                                    *field,
                                    body,
                                    alt_span,
                                    demand != ValueDemand::Owned,
                                    position,
                                    scope,
                                    registry,
                                )
                                .0
                            },
                        );
                        let rhs = copied
                            .into_iter()
                            .rev()
                            .fold(rhs, |body, field| wrap_dups(field, body, alt_span, 1));

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
                TailPosition::NonTail,
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
                TailPosition::Tail,
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
            owned_after,
            span,
        } => {
            // The runtime takes the arguments. A value live across the effect
            // needs nothing more: if the effect suspends, the frame hands what
            // it holds to the continuation, and each resume copies it (`held`).
            let (args, env_before) =
                plan_expr_list(args, tail_env, registry, scope, field_parents, |_| {
                    ValueDemand::Owned
                });
            AetherPlan {
                expr: CoreExpr::Perform {
                    effect,
                    operation,
                    args,
                    owned_after,
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
            // The handle gives its state to the runtime, which releases it
            // with the handler.
            let parameter_plan = parameter.map(|p| {
                plan_expr(
                    *p,
                    tail_env.clone(),
                    ValueDemand::Owned,
                    TailPosition::NonTail,
                    registry,
                    scope,
                    field_parents,
                )
            });
            let body_plan = plan_expr(
                *body,
                tail_env.clone(),
                demand,
                position,
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
                    TailPosition::Tail,
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
            let body_plan = plan_expr(
                *body,
                tail_env,
                demand,
                position,
                registry,
                scope,
                field_parents,
            );
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
            let body_plan = plan_expr(
                *body,
                tail_env,
                demand,
                position,
                registry,
                scope,
                field_parents,
            );
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
                TailPosition::NonTail,
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
                TailPosition::NonTail,
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
                position,
                registry,
                scope,
                field_parents,
            );
            let shared_plan = plan_expr(
                *shared_body,
                tail_env.clone(),
                demand,
                position,
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
                TailPosition::NonTail,
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
            TailPosition::NonTail,
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
            // A view read out of the value and still used after this would
            // go with the value if the receiver frees it (audit L9). An
            // unboxed field is a copy of the value it read, not a view.
            let views_live_after = || {
                field_parents.iter().any(|(child, _)| {
                    tail_env.is_live(*child)
                        && scope.get(child).is_none_or(|binder| binder.rep.needs_rc())
                        && descends_from(*child, id, field_parents)
                })
            };
            let needs_dup = tail_env.is_live(id)
                || scope.views.contains(&id)
                || borrowed_parent
                || lent
                || views_live_after();
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
    /// Views of a value this arm releases to balance another arm, planned
    /// as owned: each holds the copy taken above that release.
    owned_views: Vec<CoreBinder>,
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

/// Releases `owner` after its last use on each path of `body` that does not
/// give it away. `lent_result` says the enclosing expression only reads
/// `body`'s result. Returns the body, and whether any path got the release:
/// a path whose last use is a Flux call in function-tail position keeps
/// the value instead, so the call stays a tail call.
fn release_owner(
    owner: CoreBinder,
    body: CoreExpr,
    span: Span,
    lent_result: bool,
    position: TailPosition,
    scope: &mut Scope,
    registry: Option<&BorrowRegistry>,
) -> (CoreExpr, bool) {
    let aliases = BTreeSet::from([owner.id]);
    if aliases_owned_unknown(&body, &aliases, scope, registry) {
        return (body, false);
    }
    let mut release = Release {
        owner,
        span,
        registry,
        released: false,
    };
    let path = ReleasePath {
        whole: aliases.clone(),
        aliases,
        lent_result,
        position,
        matched: None,
    };
    let body = release.after(body, &path, scope);
    (body, release.released)
}

/// Whether evaluating `expr` makes a value with a reference of its own: a
/// constructor, a closure, a collection literal, what a Flux function
/// returns (results are owned), or a runtime result the runtime counts.
/// Other runtime functions and projections can hand back a reference into
/// an argument.
fn produces_fresh_value(expr: &CoreExpr, registry: Option<&BorrowRegistry>) -> bool {
    fresh_result(expr, registry, &mut BTreeSet::new())
}

fn fresh_result(
    expr: &CoreExpr,
    registry: Option<&BorrowRegistry>,
    fresh: &mut BTreeSet<CoreBinderId>,
) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => var.binder.is_some_and(|id| fresh.contains(&id)),
        CoreExpr::Con { .. } | CoreExpr::Lam { .. } => true,
        CoreExpr::PrimOp { op, .. } => matches!(
            op,
            CorePrimOp::MakeList
                | CorePrimOp::MakeArray
                | CorePrimOp::MakeTuple
                | CorePrimOp::MakeHash
                | CorePrimOp::Interpolate
                // The runtime counts the value these hand out (0189 F3-1).
                | CorePrimOp::ArrayGet
                | CorePrimOp::Index
                | CorePrimOp::HamtGet
                | CorePrimOp::Unwrap
        ),
        CoreExpr::App { func, .. } | CoreExpr::AetherCall { func, .. } => {
            calls_flux(func, registry)
        }
        CoreExpr::Let { var, rhs, body, .. } | CoreExpr::LetRec { var, rhs, body, .. } => {
            // Holding an owned call result across cleanup does not turn it
            // into a view of the arguments that were lent to the call.
            let inserted = fresh_result(rhs, registry, fresh) && fresh.insert(var.id);
            let result = fresh_result(body, registry, fresh);
            if inserted {
                fresh.remove(&var.id);
            }
            result
        }
        CoreExpr::LetRecGroup { body, .. }
        | CoreExpr::Dup { body, .. }
        | CoreExpr::Drop { body, .. } => fresh_result(body, registry, fresh),
        CoreExpr::Case { alts, .. } => alts
            .iter()
            .all(|alt| fresh_result(&alt.rhs, registry, fresh)),
        _ => false,
    }
}

/// Whether a call to `func` runs a Flux function rather than a runtime one.
fn calls_flux(func: &CoreExpr, registry: Option<&BorrowRegistry>) -> bool {
    match func {
        CoreExpr::Var { var, .. } => match registry {
            Some(reg) => match reg.classify_var_ref(var).borrow_callee {
                BorrowCallee::BaseRuntime(_) => false,
                BorrowCallee::Unknown => var.binder.is_some(),
                _ => true,
            },
            None => var.binder.is_some(),
        },
        CoreExpr::MemberAccess { object, member, .. } => match (object.as_ref(), registry) {
            (CoreExpr::Var { var, .. }, Some(reg)) => !matches!(
                reg.resolve_member_access_callee(var.name, *member),
                BorrowCallee::BaseRuntime(_) | BorrowCallee::Unknown
            ),
            _ => false,
        },
        _ => true,
    }
}

fn mentions(expr: &CoreExpr, binders: &BTreeSet<CoreBinderId>) -> bool {
    super::free_vars::collect_free_vars_aether(expr)
        .iter()
        .any(|id| binders.contains(id))
}

/// The binder a `let` adds to `aliases`: one whose value is computed from
/// an alias without being made fresh may point into it. An unboxed value,
/// or a runtime result that is always an Int or a Bool, points into
/// nothing.
fn alias_of(
    var: &CoreBinder,
    rhs: &CoreExpr,
    aliases: &BTreeSet<CoreBinderId>,
    registry: Option<&BorrowRegistry>,
) -> Option<CoreBinderId> {
    (var.rep.needs_rc()
        && !returns_scalar(rhs)
        && mentions(rhs, aliases)
        && !produces_fresh_value(rhs, registry))
    .then_some(var.id)
}

/// Whether `expr` is a runtime operation whose result is an Int or a Bool,
/// whatever rep its binder was given: such a result points into nothing.
fn returns_scalar(expr: &CoreExpr) -> bool {
    matches!(
        expr,
        CoreExpr::PrimOp {
            op: CorePrimOp::Len
                | CorePrimOp::HamtSize
                | CorePrimOp::HamtContains
                | CorePrimOp::Not
                | CorePrimOp::Eq
                | CorePrimOp::NEq
                | CorePrimOp::Lt
                | CorePrimOp::Le
                | CorePrimOp::Gt
                | CorePrimOp::Ge
                | CorePrimOp::CmpEq
                | CorePrimOp::CmpNe
                | CorePrimOp::IsInt
                | CorePrimOp::IsFloat
                | CorePrimOp::IsString
                | CorePrimOp::IsBool
                | CorePrimOp::IsArray
                | CorePrimOp::IsNone
                | CorePrimOp::IsSome
                | CorePrimOp::IsList
                | CorePrimOp::IsMap,
            ..
        }
    )
}

/// Whether some alias of `aliases` in `expr` is owned from a value not known
/// to be fresh: it may have given away a reference that is really part of
/// the owner, so the owner is not released.
fn aliases_owned_unknown(
    expr: &CoreExpr,
    aliases: &BTreeSet<CoreBinderId>,
    scope: &Scope,
    registry: Option<&BorrowRegistry>,
) -> bool {
    match expr {
        CoreExpr::Let { var, rhs, body, .. } => {
            let mut inner = aliases.clone();
            if let Some(alias) = alias_of(var, rhs, aliases, registry) {
                if scope.owned_unknown.contains(&alias) {
                    return true;
                }
                inner.insert(alias);
            }
            aliases_owned_unknown(rhs, aliases, scope, registry)
                || aliases_owned_unknown(body, &inner, scope, registry)
        }
        CoreExpr::LetRec { body, .. }
        | CoreExpr::LetRecGroup { body, .. }
        | CoreExpr::Dup { body, .. }
        | CoreExpr::Drop { body, .. }
        | CoreExpr::Return { value: body, .. } => {
            aliases_owned_unknown(body, aliases, scope, registry)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            let matched = mentions(scrutinee, aliases);
            alts.iter().any(|alt| {
                let mut arm = aliases.clone();
                if matched {
                    arm.extend(pat_binders(&alt.pat));
                }
                aliases_owned_unknown(&alt.rhs, &arm, scope, registry)
            })
        }
        _ => false,
    }
}

/// Whether `dup var in body` is a copy of `var` and nothing more.
fn copies_itself(var: &CoreVarRef, body: &CoreExpr) -> bool {
    matches!(body, CoreExpr::Var { var: inner, .. } if inner.binder == var.binder)
}

/// The pattern variable that names the matched value itself, when that
/// value is a name in `whole`.
fn renames(
    scrutinee: &CoreExpr,
    pat: &crate::core::CorePat,
    whole: &BTreeSet<CoreBinderId>,
) -> Option<CoreBinderId> {
    match (scrutinee, pat) {
        (CoreExpr::Var { var, .. }, crate::core::CorePat::Var(name))
            if var.binder.is_some_and(|id| whole.contains(&id)) =>
        {
            Some(name.id)
        }
        _ => None,
    }
}

/// Whether some value `expr` can end in is a bare name in `whole`: the
/// value itself, not a copy of it.
fn tail_bare(expr: &CoreExpr, whole: &BTreeSet<CoreBinderId>) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => var.binder.is_some_and(|id| whole.contains(&id)),
        CoreExpr::Dup { var, body, .. } => !copies_itself(var, body) && tail_bare(body, whole),
        CoreExpr::Let { var, rhs, body, .. } => {
            let mut inner = whole.clone();
            if tail_bare(rhs, whole) {
                inner.insert(var.id);
            }
            tail_bare(body, &inner)
        }
        CoreExpr::LetRec { body, .. }
        | CoreExpr::LetRecGroup { body, .. }
        | CoreExpr::Drop { body, .. } => tail_bare(body, whole),
        CoreExpr::Case {
            scrutinee, alts, ..
        } => alts
            .iter()
            .any(|alt| match renames(scrutinee, &alt.pat, whole) {
                Some(name) => {
                    let mut arm = whole.clone();
                    arm.insert(name);
                    tail_bare(&alt.rhs, &arm)
                }
                None => tail_bare(&alt.rhs, whole),
            }),
        CoreExpr::Handle { body, handlers, .. } => {
            tail_bare(body, whole) || handlers.iter().any(|h| tail_bare(&h.body, whole))
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => tail_bare(unique_body, whole) || tail_bare(shared_body, whole),
        _ => false,
    }
}

/// Whether evaluating `expr` gives away a name in `whole` on some path: a
/// release of it, a reuse of its cell, or the bare name where a reference
/// is taken. `owned` says `expr`'s own value is taken. A copy, `dup w in w`,
/// gives nothing away.
fn gives_away(expr: &CoreExpr, whole: &BTreeSet<CoreBinderId>, owned: bool) -> bool {
    let named = |var: &CoreVarRef| var.binder.is_some_and(|id| whole.contains(&id));
    match expr {
        CoreExpr::Var { var, .. } => owned && named(var),
        CoreExpr::Lit(..) => false,
        CoreExpr::Dup { var, body, .. } => {
            !copies_itself(var, body) && gives_away(body, whole, owned)
        }
        CoreExpr::Drop { var, body, .. } => named(var) || gives_away(body, whole, owned),
        CoreExpr::Reuse { token, fields, .. } => {
            named(token) || fields.iter().any(|field| gives_away(field, whole, true))
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            named(scrutinee)
                || gives_away(unique_body, whole, owned)
                || gives_away(shared_body, whole, owned)
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            let mut inner = whole.clone();
            if tail_bare(rhs, whole) {
                inner.insert(var.id);
            }
            gives_away(rhs, whole, false) || gives_away(body, &inner, owned)
        }
        CoreExpr::LetRec { rhs, body, .. } => {
            gives_away(rhs, whole, false) || gives_away(body, whole, owned)
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            bindings
                .iter()
                .any(|(_, rhs)| gives_away(rhs, whole, false))
                || gives_away(body, whole, owned)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            gives_away(scrutinee, whole, false)
                || alts.iter().any(|alt| {
                    let mut arm = whole.clone();
                    arm.extend(renames(scrutinee, &alt.pat, whole));
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| gives_away(guard, &arm, false))
                        || gives_away(&alt.rhs, &arm, owned)
                })
        }
        CoreExpr::Con { fields, .. } => fields.iter().any(|field| gives_away(field, whole, true)),
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            ..
        } => {
            gives_away(func, whole, false)
                || args.iter().enumerate().any(|(index, arg)| {
                    let taken = arg_modes.get(index) == Some(&BorrowMode::Owned);
                    gives_away(arg, whole, taken)
                })
        }
        CoreExpr::App { func, args, .. } => {
            gives_away(func, whole, false) || args.iter().any(|arg| gives_away(arg, whole, true))
        }
        CoreExpr::PrimOp { op, args, .. } => args
            .iter()
            .enumerate()
            .any(|(index, arg)| gives_away(arg, whole, op.takes_arg(index))),
        CoreExpr::Perform { args, .. } => args.iter().any(|arg| gives_away(arg, whole, true)),
        CoreExpr::Return { value, .. } => gives_away(value, whole, true),
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            parameter
                .as_ref()
                .is_some_and(|parameter| gives_away(parameter, whole, true))
                || gives_away(body, whole, owned)
                || handlers.iter().any(|h| gives_away(&h.body, whole, owned))
        }
        // A closure's body is another function, and a capture is lent to it.
        CoreExpr::Lam { .. } => false,
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            gives_away(object, whole, false)
        }
    }
}

/// What the walk knows about the owner on the path it is on.
#[derive(Clone)]
struct ReleasePath {
    /// The owner and every binder that may point into it: a binding whose
    /// value is computed from one of them without being made fresh, or a
    /// pattern variable of a match on one of them.
    aliases: BTreeSet<CoreBinderId>,
    /// The names for the owner itself: giving one away gives the owner away.
    whole: BTreeSet<CoreBinderId>,
    /// The enclosing expression only reads the result.
    lent_result: bool,
    /// Cleanup after a call is deferred only when it would end a tail call.
    position: TailPosition,
    /// The constructor an enclosing arm matched the owner with.
    matched: Option<CoreTag>,
}

/// Puts a Drop of `owner` right after the last read of it on each path of
/// a body that neither gives it away nor runs after an effect. The Drop
/// also waits for the reads of `ReleasePath::aliases`.
///
/// A read in a binding's right-hand side releases at the head of that
/// binding's body, which keeps a tail call after it a tail call. When the
/// last read is the result itself, the result is held in a temporary across
/// the Drop, unless it is a Flux call in function-tail position: that path
/// keeps the value, so the call stays a tail call.
struct Release<'a> {
    owner: CoreBinder,
    span: Span,
    registry: Option<&'a BorrowRegistry>,
    /// Set when some path gets the Drop.
    released: bool,
}

impl Release<'_> {
    fn drop_before(&mut self, body: CoreExpr) -> CoreExpr {
        self.released = true;
        wrap_drop(self.owner, body, self.span)
    }

    /// Releases after the last use in `body`, or before it when it reads no
    /// alias. A cell the reuse pass could build in the owner's counts as a
    /// use: the owner's fields, dead views by then, would be overwritten
    /// with the references they hold.
    fn after(&mut self, body: CoreExpr, path: &ReleasePath, scope: &mut Scope) -> CoreExpr {
        let used = mentions(&body, &path.aliases)
            || path
                .matched
                .as_ref()
                .is_some_and(|tag| rhs_has_compatible_tag(&body, tag));
        if used {
            self.after_last_use(body, path, scope)
        } else {
            self.drop_before(body)
        }
    }

    fn after_last_use(
        &mut self,
        body: CoreExpr,
        path: &ReleasePath,
        scope: &mut Scope,
    ) -> CoreExpr {
        match body {
            CoreExpr::Let {
                var,
                rhs,
                body,
                span,
            } => {
                // Given away here on some path: those paths need no release,
                // and the walk keeps out of the others it cannot tell apart.
                if gives_away(&rhs, &path.whole, false) {
                    return CoreExpr::Let {
                        var,
                        rhs,
                        body,
                        span,
                    };
                }
                let mut inner = path.clone();
                if tail_bare(&rhs, &path.whole) {
                    inner.whole.insert(var.id);
                }
                inner
                    .aliases
                    .extend(alias_of(&var, &rhs, &path.aliases, self.registry));
                let body = Box::new(self.after(*body, &inner, scope));
                CoreExpr::Let {
                    var,
                    rhs,
                    body,
                    span,
                }
            }
            CoreExpr::LetRec {
                var,
                rhs,
                body,
                span,
            } => {
                if gives_away(&rhs, &path.whole, false) {
                    return CoreExpr::LetRec {
                        var,
                        rhs,
                        body,
                        span,
                    };
                }
                CoreExpr::LetRec {
                    var,
                    rhs,
                    body: Box::new(self.after(*body, path, scope)),
                    span,
                }
            }
            CoreExpr::LetRecGroup {
                bindings,
                body,
                span,
            } => {
                if bindings
                    .iter()
                    .any(|(_, rhs)| gives_away(rhs, &path.whole, false))
                {
                    return CoreExpr::LetRecGroup {
                        bindings,
                        body,
                        span,
                    };
                }
                CoreExpr::LetRecGroup {
                    bindings,
                    body: Box::new(self.after(*body, path, scope)),
                    span,
                }
            }
            CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
                var,
                body: Box::new(self.after(*body, path, scope)),
                span,
            },
            // The owner released here already.
            CoreExpr::Drop { var, body, span }
                if var.binder.is_some_and(|id| path.whole.contains(&id)) =>
            {
                CoreExpr::Drop { var, body, span }
            }
            CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
                var,
                body: Box::new(self.after(*body, path, scope)),
                span,
            },
            CoreExpr::Case {
                scrutinee,
                alts,
                join_ty,
                span,
            } => {
                let guards_give_away = alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| gives_away(guard, &path.whole, false))
                });
                if gives_away(&scrutinee, &path.whole, false) || guards_give_away {
                    return CoreExpr::Case {
                        scrutinee,
                        alts,
                        join_ty,
                        span,
                    };
                }
                let reads = mentions(&scrutinee, &path.aliases);
                let claimant = match scrutinee.as_ref() {
                    CoreExpr::Var { var, .. } => var.binder,
                    _ => None,
                };
                let whole_scrutinee = claimant.is_some_and(|id| path.whole.contains(&id));
                let arms = path.clone();
                let alts = alts
                    .into_iter()
                    .map(|alt| {
                        let mut arm = arms.clone();
                        if reads {
                            arm.aliases.extend(pat_binders(&alt.pat));
                        }
                        // Another value matched with a fitting constructor
                        // takes the constructor first, as for `head_drop`, so
                        // the owner's release can precede it. A view of the
                        // owner or a lent value has no cell to give.
                        let claims = claimant.is_some_and(|id| {
                            !arm.aliases.contains(&id) && !scope.borrowed_params.contains(&id)
                        });
                        match &alt.pat {
                            crate::core::CorePat::Var(name) if whole_scrutinee => {
                                arm.whole.insert(name.id);
                            }
                            crate::core::CorePat::Con { tag, .. } if whole_scrutinee => {
                                arm.matched = Some(tag.clone());
                            }
                            crate::core::CorePat::Con { tag, .. }
                                if claims
                                    && arm.matched.as_ref().is_some_and(|matched| {
                                        tags_shape_compatible(matched, tag)
                                    }) =>
                            {
                                arm.matched = None;
                            }
                            _ => {}
                        }
                        let rhs = self.after(alt.rhs, &arm, scope);
                        CoreAlt { rhs, ..alt }
                    })
                    .collect();
                CoreExpr::Case {
                    scrutinee,
                    alts,
                    join_ty,
                    span,
                }
            }
            // An early return leaves the function with an owned value:
            // release before it goes.
            CoreExpr::Return { value, span } => {
                let mut returned = path.clone();
                returned.lent_result = false;
                returned.position = TailPosition::Tail;
                CoreExpr::Return {
                    value: Box::new(self.after(*value, &returned, scope)),
                    span,
                }
            }
            // Given away: no release on this path. A release after an effect
            // runs in its continuation, which holds what the frame held
            // (`held`), so it is balanced however often the handler resumes.
            leaf if gives_away(&leaf, &path.whole, !path.lent_result) => leaf,
            CoreExpr::App { ref func, .. } | CoreExpr::AetherCall { ref func, .. }
                if path.position == TailPosition::Tail && calls_flux(func, self.registry) =>
            {
                body
            }
            // A cell built from copies and from names that point into nothing
            // the owner holds: the copies go first and the release before the
            // cell, which stays last, where the reuse pass can build it in a
            // released cell. For an owner a match took apart, keep the
            // general result-holding path so its field copies are evaluated
            // before its release.
            CoreExpr::Con { tag, fields, span }
                if path.matched.is_none()
                    && fields.iter().all(|field| match field {
                        CoreExpr::Lit(..) => true,
                        CoreExpr::Var { var, .. } => {
                            !var.binder.is_some_and(|id| path.aliases.contains(&id))
                        }
                        CoreExpr::Dup { var, body, .. } => copies_itself(var, body),
                        _ => false,
                    }) =>
            {
                let mut copies = Vec::new();
                let fields = fields
                    .into_iter()
                    .map(|field| match field {
                        CoreExpr::Dup { var, body, span } => {
                            copies.push((var, span));
                            *body
                        }
                        other => other,
                    })
                    .collect();
                let cell = self.drop_before(CoreExpr::Con { tag, fields, span });
                copies
                    .into_iter()
                    .rev()
                    .fold(cell, |body, (var, span)| CoreExpr::Dup {
                        var,
                        body: Box::new(body),
                        span,
                    })
            }
            // A lent result took no reference of its own, so the enclosing
            // expression would read it after the release.
            result @ CoreExpr::Var { .. } if path.lent_result => result,
            result => {
                // A fresh value holds its own reference, and so does an owned
                // `Var` (moved or copied). Anything else may point into the
                // owner without one (`flux_unwrap_or`), so it takes a copy
                // before the release.
                let counted = produces_fresh_value(&result, self.registry)
                    || (!path.lent_result && matches!(result, CoreExpr::Var { .. }));
                let held = scope.temp();
                let mut tail = self.drop_before(CoreExpr::Var {
                    var: CoreVarRef::resolved(&held),
                    span: self.span,
                });
                if !counted {
                    tail = wrap_dups(held, tail, self.span, 1);
                }
                CoreExpr::Let {
                    var: held,
                    rhs: Box::new(result),
                    body: Box::new(tail),
                    span: self.span,
                }
            }
        }
    }
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
    use crate::core::{
        CoreBinder, CoreBinderId, CoreLit, CorePat, CorePrimOp, CoreVarRef, FluxRep,
    };
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
            CoreExpr::Let { rhs, body, .. } => {
                borrowed_call_mask(rhs).or_else(|| borrowed_call_mask(body))
            }
            CoreExpr::Lam { body, .. }
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

    /// `lam(xs) -> case xs of Cons(h, t) -> Cons(h, t); _ -> Nil`. An owned
    /// `xs` is released once on each path: at the rebuilding arm's head for
    /// reuse, and in `_`.
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
        assert_eq!(drops(&owned, xs), 2, "{owned:?}");
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
    /// The outer function releases its own reference once the closure holds
    /// one.
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
        let CoreExpr::Let { rhs: closure, .. } = outer.as_ref() else {
            unreachable!("the closure held across the release: {outer:?}");
        };
        assert_eq!(drops(closure, xs), 0, "{planned:?}");
        assert_eq!(dups(closure, xs), 1, "{planned:?}");
        assert_eq!(drops(outer, xs), 1, "{planned:?}");
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

    /// `fn(x) { if true { len(x) } else { 0 } }`, `len` borrowing: the
    /// lambda's parameter, the branch that reads `x` and the one that ignores it.
    fn branch_read_lambda(interner: &mut Interner) -> (CoreBinder, CoreExpr, BorrowRegistry) {
        let x = binder(interner, 1, "x");
        let len_name = interner.intern("len");
        let registry = borrowed_len_registry(interner);

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

        (x, expr, registry)
    }

    fn drops_of(expr: &CoreExpr, binder: CoreBinderId) -> usize {
        count_binder_nodes(
            expr,
            binder,
            &|expr, binder| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(binder)),
        )
    }

    fn dups_of(expr: &CoreExpr, binder: CoreBinderId) -> usize {
        count_binder_nodes(
            expr,
            binder,
            &|expr, binder| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(binder)),
        )
    }

    fn assert_contract(expr: &CoreExpr) {
        let result = crate::aether::verify::verify_contract_aether(expr);
        assert!(result.is_ok(), "{result:?}\n{expr:?}");
    }

    /// KI-121: an owned parameter read in one branch and dead in the other is
    /// released on both paths, after the read in one and at the arm's head in
    /// the other.
    #[test]
    fn an_owned_parameter_read_in_one_branch_is_released_on_both() {
        let mut interner = Interner::new();
        let (x, expr, registry) = branch_read_lambda(&mut interner);
        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, x.id), 2, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// A Borrowed parameter is the caller's to release, on every path.
    #[test]
    fn a_borrowed_parameter_read_in_one_branch_is_never_released() {
        let mut interner = Interner::new();
        let (x, expr, registry) = branch_read_lambda(&mut interner);
        let rewritten =
            super::insert_dup_drop_for_def(expr, &registry, std::iter::once(x.id).collect());
        assert_eq!(drops_of(&rewritten, x.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `len` borrowing, and Flux functions `g()` (returns a fresh value),
    /// `own(v)` (takes its argument) and `walk(v)` (borrows it).
    fn release_registry(interner: &mut Interner) -> BorrowRegistry {
        use crate::aether::borrow_infer::{BorrowProvenance, BorrowSignature};
        let mut registry = borrowed_len_registry(interner);
        for (name, params) in [
            ("g", vec![]),
            ("own", vec![BorrowMode::Owned]),
            ("walk", vec![BorrowMode::Borrowed]),
        ] {
            registry.by_name.insert(
                interner.intern(name),
                BorrowSignature::new(params, BorrowProvenance::Inferred),
            );
        }
        registry
    }

    fn let_in(var: CoreBinder, rhs: CoreExpr, body: CoreExpr) -> CoreExpr {
        CoreExpr::Let {
            var,
            rhs: Box::new(rhs),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    /// Whether a `let` of `after` has `Drop binder` at the head of its body.
    fn released_after(expr: &CoreExpr, binder: CoreBinderId, after: CoreBinderId) -> bool {
        count_binder_nodes(expr, binder, &|expr, binder| {
            matches!(
                expr,
                CoreExpr::Let { var, body, .. }
                    if var.id == after
                        && matches!(body.as_ref(), CoreExpr::Drop { var, .. } if var.binder == Some(binder))
            )
        }) == 1
    }

    /// KI-121: `let x = g(); let t = len(x); t`. `x` is a fresh value only
    /// read, so it is released once, right after that read: at the head of
    /// `t`'s body.
    #[test]
    fn a_fresh_value_only_read_is_released_after_its_last_read() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        // `len` returns an Int, which points into nothing.
        let t = CoreBinder::with_rep(CoreBinderId(2), interner.intern("t"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let read = call(interner.intern("len"), vec![var(x)]);
        let expr = lam(vec![], let_in(x, g, let_in(t, read, var(t))));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, x.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, x.id, t.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// KI-121: `let x = g(); let a = own(x); let t = len(x); t`. The owned use
    /// copies `x`, because it is read again; the read is the last use, so
    /// the binding's own reference is released after it.
    #[test]
    fn an_owned_value_read_last_is_released_after_the_read() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let a = binder(&mut interner, 2, "a");
        // `len` returns an Int, which points into nothing.
        let t = CoreBinder::with_rep(CoreBinderId(3), interner.intern("t"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let give = call(interner.intern("own"), vec![var(x)]);
        let read = call(interner.intern("len"), vec![var(x)]);
        let expr = lam(
            vec![],
            let_in(x, g, let_in(a, give, let_in(t, read, var(t)))),
        );

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(dups_of(&rewritten, x.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, x.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, x.id, t.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let xs = g(); case xs of Cons(h, t) -> Cons(h, t), _ -> 0`. The
    /// rebuilding arm releases `xs` at its head for reuse, which gives the
    /// value away there; the `_` arm, which only drops it, releases it too.
    #[test]
    fn a_let_bound_list_reused_in_one_arm_is_released_in_the_other() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let matched = CoreExpr::Case {
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
                    rhs: CoreExpr::Lit(CoreLit::Int(0), Span::default()),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let expr = lam(vec![], let_in(xs, g, matched));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 2, "{rewritten:?}");
        assert!(
            matches!(
                first_arm_rhs(&rewritten, 1),
                CoreExpr::Drop { var, .. } if var.binder == Some(xs.id)
            ),
            "{rewritten:?}"
        );
        assert_contract(&rewritten);
    }

    /// The right-hand side of arm `index` of the first match in `expr`.
    fn first_arm_rhs(expr: &CoreExpr, index: usize) -> &CoreExpr {
        match expr {
            CoreExpr::Case { alts, .. } => &alts[index].rhs,
            CoreExpr::Lam { body, .. }
            | CoreExpr::Let { body, .. }
            | CoreExpr::Dup { body, .. }
            | CoreExpr::Drop { body, .. } => first_arm_rhs(body, index),
            other => panic!("no match in {other:?}"),
        }
    }

    /// PA L9's shape: `fn(xs) case xs of Cons(h, t) -> let a = own(xs) in let
    /// n = len(t) in n, _ -> 0`. `own` gets a copy, since `t` is read after;
    /// the arm then releases `xs` after that read, and `_` releases it too.
    #[test]
    fn a_value_copied_in_one_arm_is_released_after_its_last_read_there() {
        let mut interner = Interner::new();
        let [xs, h, t, a] = [(1, "xs"), (2, "h"), (3, "t"), (4, "a")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(5), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let read = call(interner.intern("len"), vec![var(t)]);
        let rhs = let_in(a, give, let_in(n, read, var(n)));
        let rewritten = planned_cons_arm(xs, h, t, rhs, &registry);

        assert_eq!(dups_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, xs.id), 2, "{rewritten:?}");
        assert!(released_after(&rewritten, xs.id, n.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let xs = g(); case true of true -> own(xs), _ -> len(xs)`: given away
    /// on one path, so released only on the other, after its read.
    #[test]
    fn a_value_given_away_on_one_path_is_released_on_the_other() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let read = call(interner.intern("len"), vec![var(xs)]);
        let expr = lam(vec![], let_in(xs, g, on_true(give, read)));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `case true of true -> <then>, _ -> <other>`.
    fn on_true(then: CoreExpr, other: CoreExpr) -> CoreExpr {
        CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: then,
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: other,
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        }
    }

    /// `let xs = g(); let y = xs in Cons(y, Nil)`: `y` is `xs`, so giving it
    /// away is the binding's release.
    #[test]
    fn a_let_alias_given_away_is_the_owners_release() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let y = binder(&mut interner, 2, "y");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let cell = CoreExpr::Con {
            tag: crate::core::CoreTag::Cons,
            fields: vec![var(y), nil()],
            span: Span::default(),
        };
        let expr = lam(vec![], let_in(xs, g, let_in(y, var(xs), cell)));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, y.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let xs = g(); case true of true -> (let k = perform io.ask(1) in
    /// len(xs)), _ -> len(xs)`. Each path releases `xs` after its read: on
    /// the first that is after the effect, in its continuation, which holds
    /// what the frame held.
    #[test]
    fn a_value_live_across_an_effect_is_released_after_its_read() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let k = CoreBinder::with_rep(CoreBinderId(2), interner.intern("k"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let effect = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let ask = CoreExpr::Perform {
            effect,
            operation: interner.intern("ask"),
            args: vec![CoreExpr::Lit(CoreLit::Int(1), Span::default())],
            owned_after: Vec::new(),
            span: Span::default(),
        };
        let mut read = || call(interner.intern("len"), vec![var(xs)]);
        let after_effect = let_in(k, ask, read());
        let expr = lam(vec![], let_in(xs, g, on_true(after_effect, read())));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 2, "{rewritten:?}");
        assert!(
            matches!(first_arm_rhs(&rewritten, 0), CoreExpr::Let { var, body, .. }
                if var.id == k.id && drops_of(body, xs.id) == 1),
            "{rewritten:?}"
        );
        assert_eq!(
            drops_of(first_arm_rhs(&rewritten, 1), xs.id),
            1,
            "{rewritten:?}"
        );
        assert_contract(&rewritten);
    }

    /// `fn(xs) case xs of Cons(h, t) -> let n = len(xs) in Cons(n, Nil), _ ->
    /// xs`. The fields are dead views by the time the arm builds its cell, so
    /// that cell must not be built in `xs`'s: the overwrite would lose the
    /// references the fields still hold.
    #[test]
    fn a_released_cell_is_not_reused_over_its_unread_fields() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let read = call(interner.intern("len"), vec![var(xs)]);
        let cell = CoreExpr::Con {
            tag: crate::core::CoreTag::Cons,
            fields: vec![var(n), nil()],
            span: Span::default(),
        };
        let matched = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Cons,
                        fields: vec![CorePat::Var(h), CorePat::Var(t)],
                    },
                    guard: None,
                    rhs: let_in(n, read, cell),
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
        };
        let planned =
            crate::aether::run_aether_def_with_registry(lam(vec![xs], matched), &registry, None);
        let reuses = count_binder_nodes(
            &planned,
            xs.id,
            &|expr, binder| matches!(expr, CoreExpr::Reuse { token, .. } if token.binder == Some(binder)),
        );
        assert_eq!(reuses, 0, "{planned:?}");
    }

    /// `case xs of Cons(h, t) -> (let n = len(xs) in case t of Cons(_, _) ->
    /// Cons(n, Nil), _ -> Nil), _ -> xs`. `t` is a view of `xs`, so the inner
    /// match claims no cell: a release before the inner `Cons` would let the
    /// reuse pass build it in `xs`'s cell over its unread fields.
    #[test]
    fn a_match_on_a_field_does_not_free_the_matched_cell_for_reuse() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let read = call(interner.intern("len"), vec![var(xs)]);
        let cell = CoreExpr::Con {
            tag: crate::core::CoreTag::Cons,
            fields: vec![var(n), nil()],
            span: Span::default(),
        };
        let on_field = CoreExpr::Case {
            scrutinee: Box::new(var(t)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Cons,
                        fields: vec![CorePat::Wildcard, CorePat::Wildcard],
                    },
                    guard: None,
                    rhs: cell,
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: nil(),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let matched = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Cons,
                        fields: vec![CorePat::Var(h), CorePat::Var(t)],
                    },
                    guard: None,
                    rhs: let_in(n, read, on_field),
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
        };
        let planned =
            crate::aether::run_aether_def_with_registry(lam(vec![xs], matched), &registry, None);
        let reuses = count_binder_nodes(
            &planned,
            xs.id,
            &|expr, binder| matches!(expr, CoreExpr::Reuse { token, .. } if token.binder == Some(binder)),
        );
        assert_eq!(reuses, 0, "{planned:?}");
    }

    /// `case xs of ys -> ys` ends in `xs` itself, so a binding of it is
    /// another name for the owner.
    #[test]
    fn a_var_pattern_of_the_owner_is_the_owner_itself() {
        let mut interner = Interner::new();
        let [xs, ys] = [(1, "xs"), (2, "ys")].map(|(id, name)| binder(&mut interner, id, name));
        let renamed = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Var(ys),
                guard: None,
                rhs: var(ys),
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };
        assert!(super::tail_bare(
            &renamed,
            &std::collections::BTreeSet::from([xs.id])
        ));
    }

    /// `let x = g(); walk(x)`, `walk` a Flux function borrowing `x`.
    /// Releasing `x` after the call would end the tail call, so it is left
    /// unreleased (PA R1, a counted leak).
    #[test]
    fn a_tail_call_that_reads_the_value_gets_no_temporary() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let tail = call(interner.intern("walk"), vec![var(x)]);
        let expr = lam(vec![], let_in(x, g, tail));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, x.id), 0, "{rewritten:?}");
        let CoreExpr::Lam { body, .. } = &rewritten else {
            panic!("expected a lambda");
        };
        let CoreExpr::Let { body, .. } = body.as_ref() else {
            panic!("expected the binding of x: {body:?}");
        };
        assert!(
            matches!(body.as_ref(), CoreExpr::AetherCall { .. }),
            "{body:?}"
        );
        assert_contract(&rewritten);
    }

    fn assert_nested_lent_temporary_is_released(nested_case: bool) {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let y = binder(&mut interner, 2, "y");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let read = call(interner.intern("walk"), vec![var(x)]);
        let rhs = let_in(x, g.clone(), read);
        let rhs = if nested_case {
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
                alts: vec![
                    CoreAlt {
                        pat: CorePat::Lit(CoreLit::Bool(true)),
                        guard: None,
                        rhs,
                        span: Span::default(),
                    },
                    CoreAlt {
                        pat: CorePat::Wildcard,
                        guard: None,
                        rhs: g,
                        span: Span::default(),
                    },
                ],
                join_ty: None,
                span: Span::default(),
            }
        } else {
            rhs
        };
        let expr = lam(vec![], let_in(y, rhs, var(y)));
        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, x.id), 1, "{rewritten:?}");
        let immediately_after_call = count_binder_nodes(&rewritten, x.id, &|expr, id| {
            matches!(expr, CoreExpr::Let { var: held, rhs, body, .. }
                if matches!(rhs.as_ref(), CoreExpr::AetherCall { args, .. }
                    if matches!(args.as_slice(), [CoreExpr::Var { var, .. }] if var.binder == Some(id)))
                && matches!(body.as_ref(), CoreExpr::Drop { var, body, .. }
                    if var.binder == Some(id)
                    && matches!(body.as_ref(), CoreExpr::Var { var, .. } if var.binder == Some(held.id))))
        });
        assert_eq!(immediately_after_call, 1, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    #[test]
    fn a_temporary_lent_inside_a_binding_rhs_is_released_after_the_call() {
        assert_nested_lent_temporary_is_released(false);
    }

    #[test]
    fn a_temporary_lent_inside_a_binding_rhs_case_is_released_after_the_call() {
        assert_nested_lent_temporary_is_released(true);
    }

    #[test]
    fn cleanup_of_a_lent_temporary_keeps_the_call_result_fresh() {
        use crate::aether::borrow_infer::{BorrowProvenance, BorrowSignature};
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let y = binder(&mut interner, 2, "y");
        let z = binder(&mut interner, 3, "z");
        let mut registry = release_registry(&mut interner);
        let pair = interner.intern("pair");
        registry.by_name.insert(
            pair,
            BorrowSignature::new(
                vec![BorrowMode::Borrowed, BorrowMode::Borrowed],
                BorrowProvenance::Inferred,
            ),
        );
        let fresh = call(interner.intern("g"), vec![]);
        let rhs = let_in(z, fresh.clone(), call(pair, vec![var(x), var(z)]));
        let tail = call(interner.intern("walk"), vec![var(y)]);
        let expr = lam(vec![], let_in(x, fresh, let_in(y, rhs, tail)));
        let planned = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&planned, x.id), 1, "{planned:?}");
        assert!(released_after(&planned, x.id, y.id), "{planned:?}");
        assert_contract(&planned);
    }

    /// `let xs = g(); case xs of ys -> (let a = own(ys) in 0)`. `ys` is `xs`
    /// itself, so giving `ys` away is the binding's release.
    #[test]
    fn a_var_pattern_given_away_moves_the_scrutinee() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let ys = binder(&mut interner, 2, "ys");
        let a = binder(&mut interner, 3, "a");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let give = call(interner.intern("own"), vec![var(ys)]);
        let matched = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Var(ys),
                guard: None,
                rhs: let_in(a, give, CoreExpr::Lit(CoreLit::Int(0), Span::default())),
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };
        let expr = lam(vec![], let_in(xs, g, matched));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, ys.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `letrec f = fn(n) { n } in let r = f(1) in r`: a local function only
    /// called is released after its call (R1; its per-call self closure is
    /// R5's).
    #[test]
    fn a_local_function_only_called_is_released() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 1, "f");
        let n = binder(&mut interner, 2, "n");
        let r = binder(&mut interner, 3, "r");
        let registry = release_registry(&mut interner);
        let invoke = CoreExpr::App {
            func: Box::new(var(f)),
            args: vec![CoreExpr::Lit(CoreLit::Int(1), Span::default())],
            span: Span::default(),
        };
        let expr = lam(
            vec![],
            CoreExpr::LetRec {
                var: f,
                rhs: Box::new(lam(vec![n], var(n))),
                body: Box::new(let_in(r, invoke, var(r))),
                span: Span::default(),
            },
        );

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, f.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, f.id, r.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let xs = g(); let f = fn() { own(xs) }; let r = f() in r`. Making
    /// the closure retains `xs`, so the binding still holds its own reference
    /// and releases it; the closure, only called, is released too.
    #[test]
    fn a_value_captured_by_a_closure_is_released_by_its_owner() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let f = binder(&mut interner, 2, "f");
        let r = binder(&mut interner, 3, "r");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let invoke = CoreExpr::App {
            func: Box::new(var(f)),
            args: vec![],
            span: Span::default(),
        };
        let expr = lam(
            vec![],
            let_in(
                xs,
                g,
                let_in(f, lam(vec![], give), let_in(r, invoke, var(r))),
            ),
        );

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, f.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, f.id, r.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let y = (case true of _ -> let x = g() in x) in let n = len(y) in n`.
    /// The block's result is only read, so it takes no reference of its own:
    /// releasing `x` there would free what `len(y)` reads. `x` is left
    /// unreleased (a counted leak, PA R1).
    #[test]
    fn a_value_the_enclosing_binding_only_reads_is_not_released() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 1, "x");
        let y = binder(&mut interner, 2, "y");
        let n = binder(&mut interner, 3, "n");
        let registry = release_registry(&mut interner);
        let g = call(interner.intern("g"), vec![]);
        let block = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
            alts: vec![CoreAlt {
                pat: CorePat::Wildcard,
                guard: None,
                rhs: let_in(x, g, var(x)),
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };
        let read = call(interner.intern("len"), vec![var(y)]);
        let expr = lam(vec![], let_in(y, block, let_in(n, read, var(n))));

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, x.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `fn(xs) { len(xs) }`, `len` a runtime function. A runtime result may
    /// point into its argument without a reference of its own
    /// (`flux_unwrap_or`), so the result is copied before `xs` is released.
    #[test]
    fn a_runtime_result_is_copied_before_its_argument_is_released() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let registry = release_registry(&mut interner);
        let read = call(interner.intern("len"), vec![var(xs)]);
        let expr = lam(vec![xs], read);

        let rewritten = insert_dup_drop_with_registry(expr, &registry);
        assert_eq!(drops_of(&rewritten, xs.id), 1, "{rewritten:?}");
        let CoreExpr::Lam { body, .. } = &rewritten else {
            panic!("expected a lambda");
        };
        let CoreExpr::Let {
            var: held, body, ..
        } = body.as_ref()
        else {
            panic!("expected the result held in a temporary: {body:?}");
        };
        assert!(
            matches!(
                body.as_ref(),
                CoreExpr::Dup { var, body, .. }
                    if var.binder == Some(held.id)
                        && matches!(body.as_ref(), CoreExpr::Drop { var, .. } if var.binder == Some(xs.id))
            ),
            "{body:?}"
        );
        assert_contract(&rewritten);
    }

    /// `fn(xs) { case xs of Cons(h, t) -> <rhs>, _ -> 0 }`, planned.
    fn planned_cons_arm(
        xs: CoreBinder,
        h: CoreBinder,
        t: CoreBinder,
        rhs: CoreExpr,
        registry: &BorrowRegistry,
    ) -> CoreExpr {
        let matched = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Cons,
                        fields: vec![CorePat::Var(h), CorePat::Var(t)],
                    },
                    guard: None,
                    rhs,
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
        };
        insert_dup_drop_with_registry(lam(vec![xs], matched), registry)
    }

    /// PA L9: `case xs of Cons(h, t) -> let a = own(xs) in walk(t)`. `t` is
    /// still read after `own` takes `xs`, which may free its cell and `t`
    /// with it, so `own` gets a copy of `xs` instead of `xs` itself.
    #[test]
    fn a_scrutinee_whose_field_is_read_later_is_copied_when_given_away() {
        let mut interner = Interner::new();
        let [xs, h, t, a] = [(1, "xs"), (2, "h"), (3, "t"), (4, "a")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let read = call(interner.intern("walk"), vec![var(t)]);
        let rewritten = planned_cons_arm(xs, h, t, let_in(a, give, read), &registry);

        assert_eq!(dups_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_eq!(dups_of(&rewritten, t.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `case xs of Cons(h, t) -> case t of Cons(a, b) -> let k = own(xs) in
    /// walk(b)`: `b` is read out of `t`, read out of `xs`, so it goes with
    /// `xs` too.
    #[test]
    fn a_scrutinee_whose_field_of_a_field_is_read_later_is_copied() {
        let mut interner = Interner::new();
        let [xs, h, t, a, b, k] = [(1, "xs"), (2, "h"), (3, "t"), (4, "a"), (5, "b"), (6, "k")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let read = call(interner.intern("walk"), vec![var(b)]);
        let inner = CoreExpr::Case {
            scrutinee: Box::new(var(t)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Cons,
                        fields: vec![CorePat::Var(a), CorePat::Var(b)],
                    },
                    guard: None,
                    rhs: let_in(k, give, read),
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
        };
        let rewritten = planned_cons_arm(xs, h, t, inner, &registry);

        assert_eq!(dups_of(&rewritten, xs.id), 1, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `case xs of Cons(h, t) -> let k = own(xs) in h + 1`, `h` an Int: a
    /// copy of the value, not a view of the cell, so `xs` moves.
    #[test]
    fn a_scrutinee_whose_unboxed_field_is_read_later_still_moves() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = CoreBinder::with_rep(CoreBinderId(2), interner.intern("h"), FluxRep::IntRep);
        let [t, k] = [(3, "t"), (4, "k")].map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let give = call(interner.intern("own"), vec![var(xs)]);
        let read = CoreExpr::PrimOp {
            op: CorePrimOp::IAdd,
            args: vec![var(h), CoreExpr::Lit(CoreLit::Int(1), Span::default())],
            span: Span::default(),
        };
        let rewritten = planned_cons_arm(xs, h, t, let_in(k, give, read), &registry);

        assert_eq!(dups_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `case xs of Cons(h, t) -> let n = len(xs) in walk(t)`: the arm only
    /// reads `xs`, so its fields stay views of it, with no copies.
    #[test]
    fn an_arm_that_only_reads_its_scrutinee_keeps_views() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let read_whole = call(interner.intern("len"), vec![var(xs)]);
        let read = call(interner.intern("walk"), vec![var(t)]);
        let rewritten = planned_cons_arm(xs, h, t, let_in(n, read_whole, read), &registry);

        assert_eq!(dups_of(&rewritten, xs.id), 0, "{rewritten:?}");
        assert_eq!(dups_of(&rewritten, t.id), 0, "{rewritten:?}");
        assert_contract(&rewritten);
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
        assert_eq!(drops(&planned, xs), 2, "{planned:?}");
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
        assert_eq!(drops(&planned, xs), 2, "{planned:?}");
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
        assert_eq!(drops(&planned, xs), 2, "{planned:?}");
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
        // Each copy is the field's own reference from then on, so the cell
        // takes it over rather than a second copy.
        assert_eq!(
            (dups(&planned, h), dups(&planned, t)),
            (1, 1),
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
        assert_eq!(drops(&planned, xs), 2, "{planned:?}");
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
        assert_eq!(drops(&planned, xs), 2, "{planned:?}");
    }

    /// The other arm only reads `xs`, so this arm keeps the cell and copies
    /// only the field it gives away. Each arm then releases `xs` after its
    /// last use.
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
            (1, 0, 2),
            "{planned:?}"
        );
    }

    /// `fn(xs) case xs of Cons(h, t) -> let n = len(h) in Cons(n, t), _ -> 0`.
    /// The arm releases `xs` at its head, so it copies `h` first; `h` is
    /// then only read, so that copy is released after the read.
    #[test]
    fn a_field_copied_before_its_cell_is_released_is_released_after_its_last_read() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let read = call(interner.intern("len"), vec![var(h)]);
        let rewritten = planned_cons_arm(
            xs,
            h,
            t,
            let_in(n, read, cons_of(vec![var(n), var(t)])),
            &registry,
        );

        assert_eq!(dups_of(&rewritten, h.id), 1, "{rewritten:?}");
        assert_eq!(drops_of(&rewritten, h.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, h.id, n.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// The same arm, but `h` is given away on one path and only read on the
    /// other: only the reading path releases it.
    #[test]
    fn a_field_given_away_on_one_path_is_released_on_the_other() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let give = call(interner.intern("own"), vec![var(h)]);
        let read = call(interner.intern("len"), vec![var(h)]);
        let rhs = on_true(
            cons_of(vec![give, var(t)]),
            let_in(n, read, cons_of(vec![var(n), var(t)])),
        );
        let rewritten = planned_cons_arm(xs, h, t, rhs, &registry);

        assert_eq!(drops_of(&rewritten, h.id), 1, "{rewritten:?}");
        assert!(released_after(&rewritten, h.id, n.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// The release of `h` sits between `xs`'s release and the cell the arm
    /// builds, and the reuse pass still builds that cell in `xs`'s.
    #[test]
    fn a_field_released_before_the_rebuilt_cell_keeps_its_reuse() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let n = CoreBinder::with_rep(CoreBinderId(4), interner.intern("n"), FluxRep::IntRep);
        let registry = release_registry(&mut interner);
        let read = call(interner.intern("len"), vec![var(h)]);
        let matched = case_of(
            xs,
            vec![
                (cons(h, t), let_in(n, read, cons_of(vec![var(n), var(t)]))),
                (CorePat::Wildcard, nil()),
            ],
        );
        let planned =
            crate::aether::run_aether_def_with_registry(lam(vec![xs], matched), &registry, None);
        let reuses = count_binder_nodes(
            &planned,
            xs.id,
            &|expr, binder| matches!(expr, CoreExpr::Reuse { token, .. } if token.binder == Some(binder)),
        );
        assert_eq!(reuses, 1, "{planned:?}");
    }

    /// `Cons(h, t) -> case h of Some(x) -> Cons(x, t), _ -> t`, `xs` matched
    /// as a parameter. `h` is taken apart by a match, so its release stays a
    /// plain drop on each arm: a split that nothing reuses would release it
    /// on neither path.
    #[test]
    fn a_field_taken_apart_by_a_match_keeps_a_plain_release() {
        let mut interner = Interner::new();
        let [xs, h, t, x] = [(1, "xs"), (2, "h"), (3, "t"), (4, "x")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let inner = CoreExpr::Case {
            scrutinee: Box::new(var(h)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Con {
                        tag: crate::core::CoreTag::Some,
                        fields: vec![CorePat::Var(x)],
                    },
                    guard: None,
                    rhs: cons_of(vec![var(x), var(t)]),
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: var(t),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let matched = case_of(xs, vec![(cons(h, t), inner), (CorePat::Wildcard, nil())]);
        let planned =
            crate::aether::run_aether_def_with_registry(lam(vec![xs], matched), &registry, None);
        let splits = count_binder_nodes(
            &planned,
            h.id,
            &|expr, binder| matches!(expr, CoreExpr::DropSpecialized { scrutinee, .. } if scrutinee.binder == Some(binder)),
        );
        assert_eq!(splits, 0, "{planned:?}");
        assert_eq!(drops_of(&planned, h.id), 2, "{planned:?}");
    }

    /// `let n = Len(h)`, `n` given a counted rep as ANF gives it: a length
    /// points into nothing, so `h` is released right after it.
    #[test]
    fn a_length_of_a_field_is_not_an_alias_of_it() {
        let mut interner = Interner::new();
        let [xs, h, t, n] = [(1, "xs"), (2, "h"), (3, "t"), (4, "n")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let read = CoreExpr::PrimOp {
            op: CorePrimOp::Len,
            args: vec![var(h)],
            span: Span::default(),
        };
        let rewritten = planned_cons_arm(
            xs,
            h,
            t,
            let_in(n, read, cons_of(vec![var(n), var(t)])),
            &registry,
        );
        assert!(released_after(&rewritten, h.id, n.id), "{rewritten:?}");
        assert_contract(&rewritten);
    }

    /// `let m = Max(h, t)` may be `h` itself, so `h` waits for `m`'s use.
    #[test]
    fn a_result_that_may_be_an_argument_is_an_alias_of_it() {
        let mut interner = Interner::new();
        let [xs, h, t, m] = [(1, "xs"), (2, "h"), (3, "t"), (4, "m")]
            .map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let pick = CoreExpr::PrimOp {
            op: CorePrimOp::Max,
            args: vec![var(h), var(t)],
            span: Span::default(),
        };
        let rewritten = planned_cons_arm(
            xs,
            h,
            t,
            let_in(m, pick, cons_of(vec![var(m), var(t)])),
            &registry,
        );
        assert!(!released_after(&rewritten, h.id, m.id), "{rewritten:?}");
    }

    /// `case xs of Cons(h, t) -> if c { own(t) } else { own(xs) }`: the
    /// first path releases `xs` to balance the second, copying `t` above
    /// that release. The copy is `t`'s own reference, so giving `t` away
    /// moves it rather than copying again.
    #[test]
    fn a_view_copied_above_a_balancing_release_is_moved_at_its_last_use() {
        let mut interner = Interner::new();
        let [xs, h, t] =
            [(1, "xs"), (2, "h"), (3, "t")].map(|(id, name)| binder(&mut interner, id, name));
        let registry = release_registry(&mut interner);
        let own = interner.intern("own");
        let arm = on_true(call(own, vec![var(t)]), call(own, vec![var(xs)]));
        let planned = insert_dup_drop_with_registry(
            lam(
                vec![xs],
                case_of(xs, vec![(cons(h, t), arm), (CorePat::Wildcard, nil())]),
            ),
            &registry,
        );
        assert_eq!(dups_of(&planned, t.id), 1, "{planned:?}");
        assert_contract(&planned);
    }
}
