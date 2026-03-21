use std::collections::{BTreeSet, HashMap};

use crate::core::{CoreBinderId, CoreDef, CoreExpr, CoreProgram};
use crate::syntax::{Identifier, statement::FipAnnotation};

use super::is_heap_tag;

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
}

impl Default for FbipFact {
    fn default() -> Self {
        Self {
            bound: 0,
            provable: true,
            has_constructors: false,
            capabilities: BTreeSet::new(),
            reasons: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FbipSummary {
    pub annotation: Option<FipAnnotation>,
    pub outcome: FbipOutcome,
    pub bound: Option<usize>,
    pub has_constructors: bool,
    pub capabilities: BTreeSet<FbipCapability>,
    pub reasons: BTreeSet<FbipFailureReason>,
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
}

pub fn analyze_program(program: &CoreProgram) -> HashMap<CoreBinderId, FbipSummary> {
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
    match def.fip {
        Some(FipAnnotation::Fip) | Some(FipAnnotation::Fbip) => FbipSummary {
            annotation: def.fip,
            outcome: FbipOutcome::Fip,
            bound: Some(0),
            has_constructors: false,
            capabilities: BTreeSet::new(),
            reasons: BTreeSet::new(),
        },
        None => FbipSummary {
            annotation: None,
            outcome: FbipOutcome::NotProvable,
            bound: None,
            has_constructors: false,
            capabilities: BTreeSet::new(),
            reasons: BTreeSet::from([FbipFailureReason::UnknownCall]),
        },
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
                fact.reasons.insert(FbipFailureReason::TokenUnavailable);
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
    let summary = match func {
        CoreExpr::Var { var, .. } => var
            .binder
            .and_then(|binder| ctx.summaries.get(&binder))
            .or_else(|| ctx.summaries_by_name.get(&var.name)),
        _ => None,
    };

    let Some(summary) = summary else {
        fact.provable = false;
        fact.capabilities.insert(FbipCapability::CallsUnknown);
        fact.reasons.insert(FbipFailureReason::UnknownCall);
        return fact;
    };

    if summary.annotation.is_none() {
        fact.provable = false;
        fact.capabilities.insert(FbipCapability::CallsUnknown);
        fact.reasons.insert(FbipFailureReason::UnknownCall);
        return fact;
    }

    match summary.outcome {
        FbipOutcome::Fip => {}
        FbipOutcome::Fbip { bound } => {
            fact.bound += bound;
        }
        FbipOutcome::NotProvable => {
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::CallsNonFip);
            fact.reasons.insert(FbipFailureReason::NonFipCall);
        }
    }
    fact
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
        if acc.provable && fact.provable {
            acc.bound = acc.bound.max(fact.bound);
        } else {
            acc.provable = false;
            acc.capabilities.insert(FbipCapability::BranchJoin);
            acc.reasons.insert(FbipFailureReason::ControlFlowJoin);
        }
    }

    acc
}

#[cfg(test)]
mod tests {
    use crate::core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePat, CoreProgram, CoreTag,
        CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::{interner::Interner, statement::FipAnnotation};

    use super::{FbipFailureReason, FbipOutcome, analyze_program};

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
        let summaries = analyze_program(&program);
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
        let summaries = analyze_program(&program);
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
        let summaries = analyze_program(&program);
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
        let summaries = analyze_program(&program);
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
        let summaries = analyze_program(&program);
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
        let summaries = analyze_program(&program);
        let summary = summaries.get(&caller_binder.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
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
        let summaries = analyze_program(&program);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::NotProvable);
        assert!(summary.reasons.contains(&FbipFailureReason::EffectBoundary));
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
        let summaries = analyze_program(&program);
        let summary = summaries.values().next().unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fbip { bound: 1 });
    }

    #[test]
    fn recursive_summary_stabilizes() {
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
        let summaries = analyze_program(&program);
        let summary = summaries.get(&f.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
    }
}
