use std::collections::{BTreeSet, HashMap};

use crate::core::{CoreBinderId, CoreDef, CoreExpr, CoreProgram};
use crate::syntax::{Identifier, interner::Interner, statement::FipAnnotation};

use super::{builtin_effect_for_name, is_heap_tag, AetherBuiltinEffect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FbipCapability {
    AllocFresh,
    Dealloc,
    CallsNonFip,
    CallsUnknown,
    EffectBoundary,
    NeedsConstructorToken,
    BranchJoin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FbipFailureReason {
    FreshAllocation,
    NonFipCall,
    UnknownCall,
    BuiltinBoundary,
    EffectBoundary,
    TokenUnavailable,
    ControlFlowJoin,
    NoConstructors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbipOutcome {
    Fip,
    Fbip { bound: usize },
    NotProvable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbipFact {
    pub bound: usize,
    pub provable: bool,
    pub has_constructors: bool,
    pub capabilities: BTreeSet<FbipCapability>,
    pub reasons: BTreeSet<FbipFailureReason>,
    pub call_details: Vec<FbipCallDetail>,
}

impl Default for FbipFact {
    fn default() -> Self {
        Self {
            bound: 0,
            provable: true,
            has_constructors: false,
            capabilities: BTreeSet::new(),
            reasons: BTreeSet::new(),
            call_details: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbipCallKind {
    DirectInternal,
    DirectNamed,
    Builtin(AetherBuiltinEffect),
    Indirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbipCallOutcome {
    KnownFip,
    KnownFbip(usize),
    KnownNotProvable,
    KnownBuiltin,
    UnknownIndirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbipCallDetail {
    pub callee: String,
    pub kind: FbipCallKind,
    pub outcome: FbipCallOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbipSummary {
    pub annotation: Option<FipAnnotation>,
    pub outcome: FbipOutcome,
    pub bound: Option<usize>,
    pub has_constructors: bool,
    pub capabilities: BTreeSet<FbipCapability>,
    pub reasons: BTreeSet<FbipFailureReason>,
    pub call_details: Vec<FbipCallDetail>,
    pub trusted: bool,
}

impl FbipSummary {
    pub fn proved_fip(&self) -> bool {
        matches!(self.outcome, FbipOutcome::Fip)
    }

    pub fn proved_fbip_bound(&self) -> Option<usize> {
        match self.outcome {
            FbipOutcome::Fip => Some(0),
            FbipOutcome::Fbip { bound } => Some(bound),
            FbipOutcome::NotProvable => None,
        }
    }
}

#[derive(Debug, Clone)]
struct FbipContext<'a> {
    summaries: &'a HashMap<CoreBinderId, FbipSummary>,
    summaries_by_name: &'a HashMap<Identifier, FbipSummary>,
    interner: &'a Interner,
}

pub fn analyze_program(program: &CoreProgram, interner: &Interner) -> HashMap<CoreBinderId, FbipSummary> {
    let mut summaries = HashMap::new();
    let mut summaries_by_name = HashMap::new();

    for def in &program.defs {
        let summary = initial_summary(def);
        summaries.insert(def.binder.id, summary.clone());
        summaries_by_name.insert(def.name, summary);
    }

    for _ in 0..program.defs.len().max(1) * 8 {
        let summaries_snapshot = summaries.clone();
        let summaries_by_name_snapshot = summaries_by_name.clone();
        let mut changed = false;
        let ctx = FbipContext {
            summaries: &summaries_snapshot,
            summaries_by_name: &summaries_by_name_snapshot,
            interner,
        };

        for def in &program.defs {
            let next = summarize_definition(def, &ctx);
            let prev = summaries.get(&def.binder.id);
            if prev != Some(&next) {
                summaries.insert(def.binder.id, next.clone());
                summaries_by_name.insert(def.name, next);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    summaries
}

fn initial_summary(def: &CoreDef) -> FbipSummary {
    FbipSummary {
        annotation: def.fip,
        outcome: FbipOutcome::NotProvable,
        bound: None,
        has_constructors: false,
        capabilities: BTreeSet::new(),
        reasons: BTreeSet::new(),
        call_details: Vec::new(),
        trusted: false,
    }
}

fn summarize_definition(def: &CoreDef, ctx: &FbipContext<'_>) -> FbipSummary {
    let fact = analyze_expr(&def.expr, ctx);
    let outcome = if fact.provable {
        if fact.bound == 0 {
            FbipOutcome::Fip
        } else {
            FbipOutcome::Fbip { bound: fact.bound }
        }
    } else {
        FbipOutcome::NotProvable
    };

    let mut reasons = fact.reasons.clone();
    if fact.has_constructors {
        reasons.remove(&FbipFailureReason::NoConstructors);
    } else {
        reasons.insert(FbipFailureReason::NoConstructors);
    }

    FbipSummary {
        annotation: def.fip,
        outcome,
        bound: fact.provable.then_some(fact.bound),
        has_constructors: fact.has_constructors,
        capabilities: fact.capabilities,
        reasons,
        call_details: fact.call_details,
        trusted: true,
    }
}

fn analyze_expr(expr: &CoreExpr, ctx: &FbipContext<'_>) -> FbipFact {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => FbipFact::default(),
        CoreExpr::Lam { body, .. } => analyze_expr(body, ctx),
        CoreExpr::Dup { body, .. } => analyze_expr(body, ctx),
        CoreExpr::Drop { body, .. } => {
            let mut fact = analyze_expr(body, ctx);
            fact.capabilities.insert(FbipCapability::Dealloc);
            if dropped_constructor_rebuild_needs_token(body) {
                fact.capabilities.insert(FbipCapability::NeedsConstructorToken);
                fact.reasons.insert(FbipFailureReason::TokenUnavailable);
            }
            fact
        }
        CoreExpr::Return { value, .. } => analyze_expr(value, ctx),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            seq(analyze_expr(rhs, ctx), analyze_expr(body, ctx))
        }
        CoreExpr::PrimOp { args, .. } => fold_all(args.iter().map(|arg| analyze_expr(arg, ctx))),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            let mut fact = analyze_expr(func, ctx);
            fact = seq(fact, fold_all(args.iter().map(|arg| analyze_expr(arg, ctx))));
            seq(fact, analyze_call(func, ctx))
        }
        CoreExpr::Con { tag, fields, .. } => {
            let mut fact = fold_all(fields.iter().map(|field| analyze_expr(field, ctx)));
            if is_heap_tag(tag) {
                fact.bound += 1;
                fact.has_constructors = true;
                fact.capabilities.insert(FbipCapability::AllocFresh);
                fact.reasons.insert(FbipFailureReason::FreshAllocation);
            }
            fact
        }
        CoreExpr::Reuse { fields, .. } => {
            let mut fact = fold_all(fields.iter().map(|field| analyze_expr(field, ctx)));
            fact.has_constructors = true;
            fact
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            let scrutinee_fact = analyze_expr(scrutinee, ctx);
            let branch_facts = alts.iter().map(|alt| {
                let guard_fact = alt
                    .guard
                    .as_ref()
                    .map(|guard| analyze_expr(guard, ctx))
                    .unwrap_or_default();
                seq(guard_fact, analyze_expr(&alt.rhs, ctx))
            });
            seq(scrutinee_fact, join(branch_facts))
        }
        CoreExpr::Perform { args, .. } => {
            let mut fact = fold_all(args.iter().map(|arg| analyze_expr(arg, ctx)));
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::EffectBoundary);
            fact.reasons.insert(FbipFailureReason::EffectBoundary);
            fact
        }
        CoreExpr::Handle { body, handlers, .. } => {
            let mut fact = analyze_expr(body, ctx);
            for handler in handlers {
                fact = seq(fact, analyze_expr(&handler.body, ctx));
            }
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::EffectBoundary);
            fact.reasons.insert(FbipFailureReason::EffectBoundary);
            fact
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => join([
            analyze_expr(unique_body, ctx),
            analyze_expr(shared_body, ctx),
        ]),
    }
}

fn analyze_call(func: &CoreExpr, ctx: &FbipContext<'_>) -> FbipFact {
    let mut fact = FbipFact::default();
    let (summary, detail) = match func {
        CoreExpr::Var { var, .. } => {
            let callee_name = safe_symbol_name(ctx.interner, var.name);
            if let Some(binder) = var.binder
                && let Some(summary) = ctx.summaries.get(&binder)
            {
                (
                    Some(summary),
                    Some(FbipCallDetail {
                        callee: callee_name.clone(),
                        kind: FbipCallKind::DirectInternal,
                        outcome: FbipCallOutcome::KnownNotProvable,
                    }),
                )
            } else if let Some(effect) =
                builtin_effect_for_name(&callee_name)
            {
                (
                    None,
                    Some(FbipCallDetail {
                        callee: callee_name.clone(),
                        kind: FbipCallKind::Builtin(effect),
                        outcome: FbipCallOutcome::KnownBuiltin,
                    }),
                )
            } else if let Some(summary) = ctx.summaries_by_name.get(&var.name) {
                (
                    Some(summary),
                    Some(FbipCallDetail {
                        callee: callee_name.clone(),
                        kind: FbipCallKind::DirectNamed,
                        outcome: FbipCallOutcome::KnownNotProvable,
                    }),
                )
            } else {
                (
                    None,
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::Indirect,
                        outcome: FbipCallOutcome::UnknownIndirect,
                    }),
                )
            }
        }
        _ => (None, None),
    };

    if let Some(detail) = &detail
        && matches!(detail.kind, FbipCallKind::Builtin(_))
    {
        fact.call_details.push(detail.clone());
        return fact;
    }

    let Some(summary) = summary else {
        fact.provable = false;
        fact.capabilities.insert(FbipCapability::CallsUnknown);
        fact.reasons.insert(FbipFailureReason::UnknownCall);
        if let Some(detail) = detail {
            fact.call_details.push(detail);
        }
        return fact;
    };

    let mut call_detail = detail.expect("summary-backed calls should have a detail");
    if !summary.trusted {
        fact.provable = false;
        fact.capabilities.insert(FbipCapability::CallsNonFip);
        fact.reasons.insert(FbipFailureReason::NonFipCall);
        call_detail.outcome = FbipCallOutcome::KnownNotProvable;
        fact.call_details.push(call_detail);
        return fact;
    }

    match summary.outcome {
        FbipOutcome::Fip => {
            call_detail.outcome = FbipCallOutcome::KnownFip;
        }
        FbipOutcome::Fbip { bound } => {
            fact.bound += bound;
            call_detail.outcome = FbipCallOutcome::KnownFbip(bound);
        }
        FbipOutcome::NotProvable => {
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::CallsNonFip);
            fact.reasons.insert(FbipFailureReason::NonFipCall);
            call_detail.outcome = FbipCallOutcome::KnownNotProvable;
        }
    }
    fact.call_details.push(call_detail);
    fact
}

fn safe_symbol_name(interner: &Interner, name: Identifier) -> String {
    interner
        .try_resolve(name)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{name:?}"))
}

fn fold_all<I>(facts: I) -> FbipFact
where
    I: IntoIterator<Item = FbipFact>,
{
    facts.into_iter().fold(FbipFact::default(), seq)
}

fn seq(mut left: FbipFact, right: FbipFact) -> FbipFact {
    left.bound += right.bound;
    left.provable &= right.provable;
    left.has_constructors |= right.has_constructors;
    left.capabilities.extend(right.capabilities);
    left.reasons.extend(right.reasons);
    left.call_details.extend(right.call_details);
    left
}

fn join<I>(facts: I) -> FbipFact
where
    I: IntoIterator<Item = FbipFact>,
{
    let mut iter = facts.into_iter();
    let Some(mut acc) = iter.next() else {
        return FbipFact::default();
    };

    for fact in iter {
        acc.has_constructors |= fact.has_constructors;
        acc.capabilities.extend(fact.capabilities);
        acc.reasons.extend(fact.reasons);
        acc.call_details.extend(fact.call_details);
        if acc.provable && fact.provable {
            acc.bound = acc.bound.max(fact.bound);
        } else {
            acc.provable = false;
            if acc.bound != fact.bound || acc.has_constructors != fact.has_constructors {
                acc.capabilities.insert(FbipCapability::BranchJoin);
                acc.reasons.insert(FbipFailureReason::ControlFlowJoin);
            }
        }
    }

    acc
}

fn dropped_constructor_rebuild_needs_token(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Con { tag, .. } => is_heap_tag(tag),
        CoreExpr::Let { body, .. } | CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            dropped_constructor_rebuild_needs_token(body)
        }
        CoreExpr::Case { alts, .. } => alts
            .iter()
            .any(|alt| dropped_constructor_rebuild_needs_token(&alt.rhs)),
        CoreExpr::Reuse { .. } | CoreExpr::DropSpecialized { .. } => false,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePat, CoreProgram, CoreTag,
        CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::{interner::Interner, statement::FipAnnotation};

    use super::{FbipCallKind, FbipCallOutcome, FbipFailureReason, FbipOutcome, analyze_program};

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span: Span::default(),
        }
    }

    fn def(
        interner: &mut Interner,
        raw: u32,
        name: &str,
        expr: CoreExpr,
        fip: Option<FipAnnotation>,
    ) -> CoreDef {
        let binder = self::binder(interner, raw, name);
        CoreDef {
            name: binder.name,
            binder,
            expr,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip,
            span: Span::default(),
        }
    }

    #[test]
    fn constructor_without_reuse_fails_fip_semantically() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 2, "xs");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "f",
                CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![var(xs), var(xs)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
        assert!(summary.reasons.contains(&FbipFailureReason::FreshAllocation));
    }

    #[test]
    fn reuse_backed_rebuild_proves_fip() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 2, "xs");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "f",
                CoreExpr::Reuse {
                    token: CoreVarRef::resolved(xs),
                    tag: CoreTag::Cons,
                    fields: vec![var(h), var(t)],
                    field_mask: None,
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
    }

    #[test]
    fn fbip_bound_joins_branch_max() {
        let mut interner = Interner::new();
        let cond = binder(&mut interner, 2, "cond");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "f",
                CoreExpr::Case {
                    scrutinee: Box::new(var(cond)),
                    alts: vec![
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: CoreExpr::Con {
                                tag: CoreTag::Some,
                                fields: vec![var(cond)],
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(cond), var(cond)],
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
    }

    #[test]
    fn unknown_call_breaks_proof() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 1, "f");
        let arg = binder(&mut interner, 2, "arg");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                3,
                "caller",
                CoreExpr::App {
                    func: Box::new(var(f)),
                    args: vec![var(arg)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::NotProvable);
        assert!(summary.reasons.contains(&FbipFailureReason::UnknownCall));
    }

    #[test]
    fn known_fip_callee_preserves_proof() {
        let mut interner = Interner::new();
        let token = binder(&mut interner, 2, "token");
        let field = binder(&mut interner, 3, "field");
        let callee = def(
            &mut interner,
            1,
            "callee",
            CoreExpr::Reuse {
                token: CoreVarRef::resolved(token),
                tag: CoreTag::Some,
                fields: vec![var(field)],
                field_mask: None,
                span: Span::default(),
            },
            Some(FipAnnotation::Fip),
        );
        let caller_binder = binder(&mut interner, 4, "caller");
        let caller = CoreDef {
            name: caller_binder.name,
            binder: caller_binder,
            expr: CoreExpr::App {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::resolved(callee.binder),
                    span: Span::default(),
                }),
                args: vec![var(field)],
                span: Span::default(),
            },
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: Some(FipAnnotation::Fip),
            span: Span::default(),
        };
        let program = CoreProgram {
            defs: vec![callee, caller],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.get(&caller_binder.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
    }

    #[test]
    fn known_fbip_callee_bound_composes() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let callee = def(
            &mut interner,
            1,
            "callee",
            CoreExpr::Con {
                tag: CoreTag::Some,
                fields: vec![var(x)],
                span: Span::default(),
            },
            Some(FipAnnotation::Fbip),
        );
        let caller_binder = binder(&mut interner, 3, "caller");
        let caller = CoreDef {
            name: caller_binder.name,
            binder: caller_binder,
            expr: CoreExpr::App {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::resolved(callee.binder),
                    span: Span::default(),
                }),
                args: vec![var(x)],
                span: Span::default(),
            },
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: Some(FipAnnotation::Fbip),
            span: Span::default(),
        };
        let program = CoreProgram {
            defs: vec![callee, caller],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.get(&caller_binder.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
    }

    #[test]
    fn known_unannotated_internal_callee_summary_composes() {
        let mut interner = Interner::new();
        let token = binder(&mut interner, 2, "token");
        let field = binder(&mut interner, 3, "field");
        let callee = def(
            &mut interner,
            1,
            "callee",
            CoreExpr::Reuse {
                token: CoreVarRef::resolved(token),
                tag: CoreTag::Some,
                fields: vec![var(field)],
                field_mask: None,
                span: Span::default(),
            },
            None,
        );
        let caller_binder = binder(&mut interner, 4, "caller");
        let caller = CoreDef {
            name: caller_binder.name,
            binder: caller_binder,
            expr: CoreExpr::App {
                func: Box::new(CoreExpr::Var {
                    var: CoreVarRef::resolved(callee.binder),
                    span: Span::default(),
                }),
                args: vec![var(field)],
                span: Span::default(),
            },
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: Some(FipAnnotation::Fip),
            span: Span::default(),
        };
        let program = CoreProgram {
            defs: vec![callee, caller],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.get(&caller_binder.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
        assert!(summary
            .call_details
            .iter()
            .any(|detail| matches!(detail.outcome, FbipCallOutcome::KnownFip)));
    }

    #[test]
    fn perform_boundary_breaks_proof() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let io = interner.intern("IO");
        let print = interner.intern("print");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "effectful",
                CoreExpr::Perform {
                    effect: io,
                    operation: print,
                    args: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::NotProvable);
        assert!(summary.reasons.contains(&FbipFailureReason::EffectBoundary));
    }

    #[test]
    fn builtin_io_call_is_not_unknown() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let print = interner.intern("print");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "log_only",
                CoreExpr::App {
                    func: Box::new(CoreExpr::Var {
                        var: CoreVarRef::unresolved(print),
                        span: Span::default(),
                    }),
                    args: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
        assert!(!summary.reasons.contains(&FbipFailureReason::UnknownCall));
        assert!(summary
            .call_details
            .iter()
            .any(|detail| matches!(detail.kind, FbipCallKind::Builtin(_))));
    }

    #[test]
    fn builtin_time_call_is_not_unknown() {
        let mut interner = Interner::new();
        let now_ms = interner.intern("now_ms");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "clock",
                CoreExpr::App {
                    func: Box::new(CoreExpr::Var {
                        var: CoreVarRef::unresolved(now_ms),
                        span: Span::default(),
                    }),
                    args: vec![],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
        assert!(!summary.reasons.contains(&FbipFailureReason::UnknownCall));
        assert!(summary
            .call_details
            .iter()
            .any(|detail| matches!(detail.kind, FbipCallKind::Builtin(_))));
    }

    #[test]
    fn drop_specialized_joins_conservatively() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let token = binder(&mut interner, 3, "token");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "ds",
                CoreExpr::DropSpecialized {
                    scrutinee: CoreVarRef::resolved(token),
                    unique_body: Box::new(CoreExpr::Reuse {
                        token: CoreVarRef::resolved(token),
                        tag: CoreTag::Some,
                        fields: vec![var(x)],
                        field_mask: None,
                        span: Span::default(),
                    }),
                    shared_body: Box::new(CoreExpr::Con {
                        tag: CoreTag::Some,
                        fields: vec![var(x)],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
    }

    #[test]
    fn recursive_self_call_does_not_get_free_fip_seed() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 1, "f");
        let x = binder(&mut interner, 2, "x");
        let program = CoreProgram {
            defs: vec![CoreDef {
                name: f.name,
                binder: f,
                expr: CoreExpr::App {
                    func: Box::new(var(f)),
                    args: vec![var(x)],
                    span: Span::default(),
                },
                borrow_signature: None,
                result_ty: None,
                is_anonymous: false,
                is_recursive: true,
                fip: Some(FipAnnotation::Fip),
                span: Span::default(),
            }],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&program, &interner);
        let summary = summaries.get(&f.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::NotProvable);
        assert!(summary.reasons.contains(&FbipFailureReason::NonFipCall));
    }

    #[test]
    fn token_unavailable_only_for_dropped_rebuilds() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let alloc_program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "plain_alloc",
                CoreExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let summaries = analyze_program(&alloc_program, &interner);
        let summary = summaries.values().next().unwrap();
        assert!(summary.reasons.contains(&FbipFailureReason::FreshAllocation));
        assert!(!summary.reasons.contains(&FbipFailureReason::TokenUnavailable));

        let token = binder(&mut interner, 3, "token");
        let dropped_program = CoreProgram {
            defs: vec![def(
                &mut interner,
                4,
                "dropped_rebuild",
                CoreExpr::Drop {
                    var: CoreVarRef::resolved(token),
                    body: Box::new(CoreExpr::Con {
                        tag: CoreTag::Some,
                        fields: vec![var(x)],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let dropped_summaries = analyze_program(&dropped_program, &interner);
        let dropped_summary = dropped_summaries.values().next().unwrap();
        assert!(dropped_summary
            .reasons
            .contains(&FbipFailureReason::TokenUnavailable));
    }
}
