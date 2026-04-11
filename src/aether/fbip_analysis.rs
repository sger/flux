use std::collections::{BTreeSet, HashMap};

use crate::core::{CoreBinderId, CoreDef, CoreExpr, CoreProgram};
use crate::syntax::{Identifier, interner::Interner, statement::FipAnnotation};

use super::{AetherBuiltinEffect, builtin_effect_for_name, callee::AetherCalleeKind, is_heap_tag};
use super::{AetherDef, AetherExpr, AetherProgram};
use super::{borrow_infer::BorrowProvenance, callee::classify_direct_var_ref};

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
    DirectInferredGlobal,
    DirectImported,
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
    pub self_recursive: bool,
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
    summaries_by_name: &'a HashMap<Identifier, NamedFbipSummary>,
    interner: &'a Interner,
    current_def: Option<CoreBinderId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamedFbipSummary {
    summary: FbipSummary,
    provenance: BorrowProvenance,
}

pub fn analyze_program(
    program: &CoreProgram,
    interner: &Interner,
) -> HashMap<CoreBinderId, FbipSummary> {
    let mut summaries = HashMap::new();
    let mut summaries_by_name = HashMap::<Identifier, NamedFbipSummary>::new();

    for def in &program.defs {
        let summary = initial_summary(def);
        summaries.insert(def.binder.id, summary.clone());
        summaries_by_name.insert(
            def.name,
            NamedFbipSummary {
                summary,
                provenance: BorrowProvenance::Inferred,
            },
        );
    }

    register_explicit_imported_fallbacks(program, &mut summaries_by_name, interner);

    for def in &program.defs {
        if let Some(entry) = summaries_by_name.get_mut(&def.name) {
            entry.summary = summaries
                .get(&def.binder.id)
                .cloned()
                .expect("definition summary should exist");
            entry.provenance = BorrowProvenance::Inferred;
        }
    }

    for _ in 0..program.defs.len().max(1) * 8 {
        let summaries_snapshot = summaries.clone();
        let summaries_by_name_snapshot = summaries_by_name.clone();
        let mut changed = false;
        for def in &program.defs {
            let ctx = FbipContext {
                summaries: &summaries_snapshot,
                summaries_by_name: &summaries_by_name_snapshot,
                interner,
                current_def: Some(def.binder.id),
            };
            let next = summarize_definition(def, &ctx);
            let prev = summaries.get(&def.binder.id);
            if prev != Some(&next) {
                summaries.insert(def.binder.id, next.clone());
                summaries_by_name.insert(
                    def.name,
                    NamedFbipSummary {
                        summary: next,
                        provenance: BorrowProvenance::Inferred,
                    },
                );
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    summaries
}

pub fn analyze_program_aether(
    program: &AetherProgram,
    interner: &Interner,
) -> HashMap<CoreBinderId, FbipSummary> {
    let mut summaries = HashMap::new();
    let mut summaries_by_name = HashMap::<Identifier, NamedFbipSummary>::new();

    for def in &program.defs {
        let summary = initial_summary_aether(def);
        summaries.insert(def.binder.id, summary.clone());
        summaries_by_name.insert(
            def.name,
            NamedFbipSummary {
                summary,
                provenance: BorrowProvenance::Inferred,
            },
        );
    }

    register_explicit_imported_fallbacks(program.as_core(), &mut summaries_by_name, interner);

    for def in &program.defs {
        if let Some(entry) = summaries_by_name.get_mut(&def.name) {
            entry.summary = summaries
                .get(&def.binder.id)
                .cloned()
                .expect("definition summary should exist");
            entry.provenance = BorrowProvenance::Inferred;
        }
    }

    for _ in 0..program.defs.len().max(1) * 8 {
        let summaries_snapshot = summaries.clone();
        let summaries_by_name_snapshot = summaries_by_name.clone();
        let mut changed = false;
        for def in &program.defs {
            let ctx = FbipContext {
                summaries: &summaries_snapshot,
                summaries_by_name: &summaries_by_name_snapshot,
                interner,
                current_def: Some(def.binder.id),
            };
            let next = summarize_definition_aether(def, &ctx);
            let prev = summaries.get(&def.binder.id);
            if prev != Some(&next) {
                summaries.insert(def.binder.id, next.clone());
                summaries_by_name.insert(
                    def.name,
                    NamedFbipSummary {
                        summary: next,
                        provenance: BorrowProvenance::Inferred,
                    },
                );
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

fn initial_summary_aether(def: &AetherDef) -> FbipSummary {
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
    let mut fact = analyze_expr(&def.expr, ctx);
    suppress_redundant_self_recursive_noise(&mut fact);
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

fn summarize_definition_aether(def: &AetherDef, ctx: &FbipContext<'_>) -> FbipSummary {
    let mut fact = analyze_expr_aether(&def.expr, ctx);
    suppress_redundant_self_recursive_noise(&mut fact);
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

    let trusted = match def.fip {
        Some(FipAnnotation::Fip) => matches!(outcome, FbipOutcome::Fip),
        Some(FipAnnotation::Fbip) => matches!(outcome, FbipOutcome::Fip | FbipOutcome::Fbip { .. }),
        None => matches!(outcome, FbipOutcome::Fip | FbipOutcome::Fbip { .. }),
    };

    FbipSummary {
        annotation: def.fip,
        bound: match outcome {
            FbipOutcome::Fip => Some(0),
            FbipOutcome::Fbip { bound } => Some(bound),
            FbipOutcome::NotProvable => None,
        },
        outcome,
        has_constructors: fact.has_constructors,
        capabilities: fact.capabilities,
        reasons,
        call_details: fact.call_details,
        trusted,
    }
}

fn register_explicit_imported_fallbacks(
    program: &CoreProgram,
    summaries_by_name: &mut HashMap<Identifier, NamedFbipSummary>,
    interner: &Interner,
) {
    let mut unresolved = HashMap::<Identifier, usize>::new();
    for def in &program.defs {
        collect_unresolved_callees(&def.expr, &mut unresolved);
    }

    for (name, _) in unresolved {
        if summaries_by_name.contains_key(&name) {
            continue;
        }
        let resolved = interner.resolve(name);
        if builtin_effect_for_name(resolved).is_some() {
            continue;
        }
        summaries_by_name.insert(
            name,
            NamedFbipSummary {
                summary: FbipSummary {
                    annotation: None,
                    outcome: FbipOutcome::NotProvable,
                    bound: None,
                    has_constructors: false,
                    capabilities: BTreeSet::new(),
                    reasons: BTreeSet::new(),
                    call_details: Vec::new(),
                    trusted: false,
                },
                provenance: BorrowProvenance::Imported,
            },
        );
    }
}

fn suppress_redundant_self_recursive_noise(fact: &mut FbipFact) {
    let has_other_blocker = fact.reasons.iter().any(|reason| {
        matches!(
            reason,
            FbipFailureReason::FreshAllocation
                | FbipFailureReason::UnknownCall
                | FbipFailureReason::TokenUnavailable
                | FbipFailureReason::ControlFlowJoin
                | FbipFailureReason::EffectBoundary
                | FbipFailureReason::BuiltinBoundary
        )
    });

    if !has_other_blocker {
        return;
    }

    let removed_any = fact.call_details.iter().any(|detail| {
        detail.self_recursive && matches!(detail.outcome, FbipCallOutcome::KnownNotProvable)
    });

    if !removed_any {
        return;
    }

    fact.call_details.retain(|detail| {
        !(detail.self_recursive && matches!(detail.outcome, FbipCallOutcome::KnownNotProvable))
    });

    let has_non_self_known_not_provable = fact.call_details.iter().any(|detail| {
        !detail.self_recursive && matches!(detail.outcome, FbipCallOutcome::KnownNotProvable)
    });

    if !has_non_self_known_not_provable {
        fact.reasons.remove(&FbipFailureReason::NonFipCall);
        fact.capabilities.remove(&FbipCapability::CallsNonFip);
    }
}

fn analyze_expr(expr: &CoreExpr, ctx: &FbipContext<'_>) -> FbipFact {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => FbipFact::default(),
        CoreExpr::Lam { body, .. } => analyze_expr(body, ctx),
        CoreExpr::Return { value, .. } => analyze_expr(value, ctx),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            seq(analyze_expr(rhs, ctx), analyze_expr(body, ctx))
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            let mut fact = FbipFact::default();
            for (_, rhs) in bindings {
                fact = seq(fact, analyze_expr(rhs, ctx));
            }
            seq(fact, analyze_expr(body, ctx))
        }
        CoreExpr::PrimOp { args, .. } => fold_all(args.iter().map(|arg| analyze_expr(arg, ctx))),
        CoreExpr::App { func, args, .. } => {
            let mut fact = analyze_expr(func, ctx);
            fact = seq(
                fact,
                fold_all(args.iter().map(|arg| analyze_expr(arg, ctx))),
            );
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
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            analyze_expr(object, ctx)
        }
    }
}

fn analyze_expr_aether(expr: &AetherExpr, ctx: &FbipContext<'_>) -> FbipFact {
    match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => FbipFact::default(),
        AetherExpr::Lam { body, .. } => analyze_expr_aether(body, ctx),
        AetherExpr::Return { value, .. } => analyze_expr_aether(value, ctx),
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => seq(
            analyze_expr_aether(rhs, ctx),
            analyze_expr_aether(body, ctx),
        ),
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            let mut fact = FbipFact::default();
            for (_, rhs) in bindings {
                fact = seq(fact, analyze_expr_aether(rhs, ctx));
            }
            seq(fact, analyze_expr_aether(body, ctx))
        }
        AetherExpr::PrimOp { args, .. } => {
            fold_all(args.iter().map(|arg| analyze_expr_aether(arg, ctx)))
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            let mut fact = analyze_expr_aether(func, ctx);
            fact = seq(
                fact,
                fold_all(args.iter().map(|arg| analyze_expr_aether(arg, ctx))),
            );
            seq(fact, analyze_call_aether(func, ctx))
        }
        AetherExpr::Con { tag, fields, .. } => {
            let mut fact = fold_all(fields.iter().map(|field| analyze_expr_aether(field, ctx)));
            if is_heap_tag(tag) {
                fact.bound += 1;
                fact.has_constructors = true;
                fact.capabilities.insert(FbipCapability::AllocFresh);
                fact.reasons.insert(FbipFailureReason::FreshAllocation);
            }
            fact
        }
        AetherExpr::Reuse { fields, tag, .. } => {
            let mut fact = fold_all(fields.iter().map(|field| analyze_expr_aether(field, ctx)));
            if is_heap_tag(tag) {
                fact.has_constructors = true;
            }
            fact
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            let scrutinee_fact = analyze_expr_aether(scrutinee, ctx);
            let branch_facts = alts.iter().map(|alt| {
                let guard_fact = alt
                    .guard
                    .as_ref()
                    .map(|guard| analyze_expr_aether(guard, ctx))
                    .unwrap_or_default();
                seq(guard_fact, analyze_expr_aether(&alt.rhs, ctx))
            });
            seq(scrutinee_fact, join(branch_facts))
        }
        AetherExpr::Perform { args, .. } => {
            let mut fact = fold_all(args.iter().map(|arg| analyze_expr_aether(arg, ctx)));
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::EffectBoundary);
            fact.reasons.insert(FbipFailureReason::EffectBoundary);
            fact
        }
        AetherExpr::Handle { body, handlers, .. } => {
            let mut fact = analyze_expr_aether(body, ctx);
            for handler in handlers {
                fact = seq(fact, analyze_expr_aether(&handler.body, ctx));
            }
            fact.provable = false;
            fact.capabilities.insert(FbipCapability::EffectBoundary);
            fact.reasons.insert(FbipFailureReason::EffectBoundary);
            fact
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            analyze_expr_aether(object, ctx)
        }
        AetherExpr::Dup { body, .. } => analyze_expr_aether(body, ctx),
        AetherExpr::Drop { body, .. } => {
            let mut fact = analyze_expr_aether(body, ctx);
            if fact.has_constructors {
                fact.provable = false;
                fact.capabilities
                    .insert(FbipCapability::NeedsConstructorToken);
                fact.reasons.insert(FbipFailureReason::TokenUnavailable);
            }
            fact
        }
        AetherExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => join([
            analyze_expr_aether(unique_body, ctx),
            analyze_expr_aether(shared_body, ctx),
        ]),
    }
}

fn analyze_call(func: &CoreExpr, ctx: &FbipContext<'_>) -> FbipFact {
    let mut fact = FbipFact::default();
    let (summary, detail) = match func {
        CoreExpr::Var { var, .. } => {
            let callee_name = safe_symbol_name(ctx.interner, var.name);
            let classified = classify_direct_var_ref(
                var,
                |binder| ctx.summaries.contains_key(&binder),
                |name| {
                    if ctx
                        .interner
                        .try_resolve(name)
                        .is_some_and(|symbol| builtin_effect_for_name(symbol).is_some())
                    {
                        Some(BorrowProvenance::BaseRuntime)
                    } else {
                        ctx.summaries_by_name
                            .get(&name)
                            .map(|entry| entry.provenance)
                    }
                },
            );
            let self_recursive = classified.binder == ctx.current_def;
            match classified.kind {
                AetherCalleeKind::DirectLocal => (
                    classified
                        .binder
                        .and_then(|binder| ctx.summaries.get(&binder)),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectInternal,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive,
                    }),
                ),
                AetherCalleeKind::DirectInferredGlobal => (
                    ctx.summaries_by_name
                        .get(&var.name)
                        .map(|entry| &entry.summary),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectInferredGlobal,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::BaseRuntime => (
                    None,
                    builtin_effect_for_name(&callee_name).map(|effect| FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::Builtin(effect),
                        outcome: FbipCallOutcome::KnownBuiltin,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::Imported => (
                    ctx.summaries_by_name
                        .get(&var.name)
                        .map(|entry| &entry.summary),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectImported,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::Unknown => (
                    None,
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::Indirect,
                        outcome: FbipCallOutcome::UnknownIndirect,
                        self_recursive: false,
                    }),
                ),
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

fn analyze_call_aether(func: &AetherExpr, ctx: &FbipContext<'_>) -> FbipFact {
    let mut fact = FbipFact::default();
    let (summary, detail) = match func {
        AetherExpr::Var { var, .. } => {
            let callee_name = safe_symbol_name(ctx.interner, var.name);
            let classified = classify_direct_var_ref(
                var,
                |binder| ctx.summaries.contains_key(&binder),
                |name| {
                    if ctx
                        .interner
                        .try_resolve(name)
                        .is_some_and(|symbol| builtin_effect_for_name(symbol).is_some())
                    {
                        Some(BorrowProvenance::BaseRuntime)
                    } else {
                        ctx.summaries_by_name
                            .get(&name)
                            .map(|entry| entry.provenance)
                    }
                },
            );
            let self_recursive = classified.binder == ctx.current_def;
            match classified.kind {
                AetherCalleeKind::DirectLocal => (
                    classified
                        .binder
                        .and_then(|binder| ctx.summaries.get(&binder)),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectInternal,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive,
                    }),
                ),
                AetherCalleeKind::DirectInferredGlobal => (
                    ctx.summaries_by_name
                        .get(&var.name)
                        .map(|entry| &entry.summary),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectInferredGlobal,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::BaseRuntime => (
                    None,
                    builtin_effect_for_name(&callee_name).map(|effect| FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::Builtin(effect),
                        outcome: FbipCallOutcome::KnownBuiltin,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::Imported => (
                    ctx.summaries_by_name
                        .get(&var.name)
                        .map(|entry| &entry.summary),
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::DirectImported,
                        outcome: FbipCallOutcome::KnownNotProvable,
                        self_recursive: false,
                    }),
                ),
                AetherCalleeKind::Unknown => (
                    None,
                    Some(FbipCallDetail {
                        callee: callee_name,
                        kind: FbipCallKind::Indirect,
                        outcome: FbipCallOutcome::UnknownIndirect,
                        self_recursive: false,
                    }),
                ),
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

fn collect_unresolved_callees(expr: &CoreExpr, unresolved: &mut HashMap<Identifier, usize>) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => collect_unresolved_callees(body, unresolved),
        CoreExpr::App { func, args, .. } => {
            if let CoreExpr::Var { var, .. } = func.as_ref()
                && var.binder.is_none()
            {
                unresolved
                    .entry(var.name)
                    .and_modify(|arity| *arity = (*arity).max(args.len()))
                    .or_insert(args.len());
            }
            collect_unresolved_callees(func, unresolved);
            for arg in args {
                collect_unresolved_callees(arg, unresolved);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_unresolved_callees(rhs, unresolved);
            collect_unresolved_callees(body, unresolved);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                collect_unresolved_callees(rhs, unresolved);
            }
            collect_unresolved_callees(body, unresolved);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_unresolved_callees(scrutinee, unresolved);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_unresolved_callees(guard, unresolved);
                }
                collect_unresolved_callees(&alt.rhs, unresolved);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                collect_unresolved_callees(field, unresolved);
            }
        }
        CoreExpr::Return { value, .. } => collect_unresolved_callees(value, unresolved),
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_unresolved_callees(arg, unresolved);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_unresolved_callees(body, unresolved);
            for handler in handlers {
                collect_unresolved_callees(&handler.body, unresolved);
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_unresolved_callees(object, unresolved)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::{AetherDef, AetherExpr, AetherProgram};
    use crate::core::{
        CoreAlt, CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePat, CoreProgram, CoreTag,
        CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::{interner::Interner, statement::FipAnnotation};

    use super::{
        FbipCallKind, FbipCallOutcome, FbipFailureReason, FbipOutcome, analyze_program,
        analyze_program_aether,
    };

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

    fn adef(
        interner: &mut Interner,
        raw: u32,
        name: &str,
        expr: AetherExpr,
        fip: Option<FipAnnotation>,
    ) -> AetherDef {
        let binder = self::binder(interner, raw, name);
        AetherDef {
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

    fn aprogram(defs: Vec<AetherDef>) -> AetherProgram {
        AetherProgram::new(
            CoreProgram {
                defs: Vec::new(),
                top_level_items: Vec::new(),
            },
            defs,
            Vec::new(),
        )
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
        assert!(
            summary
                .reasons
                .contains(&FbipFailureReason::FreshAllocation)
        );
    }

    #[test]
    fn reuse_backed_rebuild_proves_fip() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 2, "xs");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");
        let program = aprogram(vec![adef(
            &mut interner,
            1,
            "f",
            AetherExpr::Reuse {
                token: CoreVarRef::resolved(xs),
                tag: CoreTag::Cons,
                fields: vec![
                    AetherExpr::bound_var(h, Span::default()),
                    AetherExpr::bound_var(t, Span::default()),
                ],
                field_mask: None,
                span: Span::default(),
            },
            Some(FipAnnotation::Fip),
        )]);
        let summaries = analyze_program_aether(&program, &interner);
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
        let callee = adef(
            &mut interner,
            1,
            "callee",
            AetherExpr::Reuse {
                token: CoreVarRef::resolved(token),
                tag: CoreTag::Some,
                fields: vec![AetherExpr::bound_var(field, Span::default())],
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
        let program = aprogram(vec![
            callee,
            AetherDef {
                name: caller.name,
                binder: caller.binder,
                expr: AetherExpr::from_core(caller.expr),
                borrow_signature: caller.borrow_signature,
                result_ty: caller.result_ty,
                is_anonymous: caller.is_anonymous,
                is_recursive: caller.is_recursive,
                fip: caller.fip,
                span: caller.span,
            },
        ]);
        let summaries = analyze_program_aether(&program, &interner);
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
        let callee = adef(
            &mut interner,
            1,
            "callee",
            AetherExpr::Reuse {
                token: CoreVarRef::resolved(token),
                tag: CoreTag::Some,
                fields: vec![AetherExpr::bound_var(field, Span::default())],
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
        let program = aprogram(vec![
            callee,
            AetherDef {
                name: caller.name,
                binder: caller.binder,
                expr: AetherExpr::from_core(caller.expr),
                borrow_signature: caller.borrow_signature,
                result_ty: caller.result_ty,
                is_anonymous: caller.is_anonymous,
                is_recursive: caller.is_recursive,
                fip: caller.fip,
                span: caller.span,
            },
        ]);
        let summaries = analyze_program_aether(&program, &interner);
        let summary = summaries.get(&caller_binder.id).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
        assert!(
            summary
                .call_details
                .iter()
                .any(|detail| matches!(detail.outcome, FbipCallOutcome::KnownFip))
        );
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
        assert!(
            summary
                .call_details
                .iter()
                .any(|detail| matches!(detail.kind, FbipCallKind::Builtin(_)))
        );
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
        assert!(
            summary
                .call_details
                .iter()
                .any(|detail| matches!(detail.kind, FbipCallKind::Builtin(_)))
        );
    }

    #[test]
    fn imported_name_only_fallback_is_not_indirect() {
        let mut interner = Interner::new();
        let foreign = interner.intern("foreign_fn");
        let x = binder(&mut interner, 2, "x");
        let program = CoreProgram {
            defs: vec![def(
                &mut interner,
                1,
                "caller",
                CoreExpr::App {
                    func: Box::new(CoreExpr::Var {
                        var: CoreVarRef::unresolved(foreign),
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
        assert_eq!(summary.outcome, FbipOutcome::NotProvable);
        assert!(summary.reasons.contains(&FbipFailureReason::NonFipCall));
        assert!(!summary.reasons.contains(&FbipFailureReason::UnknownCall));
        assert!(summary.call_details.iter().any(|detail| {
            matches!(detail.kind, FbipCallKind::DirectImported)
                && matches!(detail.outcome, FbipCallOutcome::KnownNotProvable)
        }));
    }

    #[test]
    fn unresolved_same_name_internal_uses_inferred_global_category() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let callee = adef(
            &mut interner,
            1,
            "callee",
            AetherExpr::Reuse {
                token: CoreVarRef::resolved(x),
                tag: CoreTag::Some,
                fields: vec![AetherExpr::bound_var(x, Span::default())],
                field_mask: None,
                span: Span::default(),
            },
            None,
        );
        let caller = adef(
            &mut interner,
            3,
            "caller",
            AetherExpr::App {
                func: Box::new(AetherExpr::Var {
                    var: CoreVarRef::unresolved(callee.name),
                    span: Span::default(),
                }),
                args: vec![AetherExpr::bound_var(x, Span::default())],
                span: Span::default(),
            },
            Some(FipAnnotation::Fip),
        );
        let program = aprogram(vec![callee, caller]);
        let summaries = analyze_program_aether(&program, &interner);
        let summary = summaries.get(&CoreBinderId(3)).unwrap();
        assert_eq!(summary.outcome, FbipOutcome::Fip);
        assert!(summary.call_details.iter().any(|detail| {
            matches!(detail.kind, FbipCallKind::DirectInferredGlobal)
                && matches!(detail.outcome, FbipCallOutcome::KnownFip)
        }));
    }

    #[test]
    fn drop_specialized_joins_conservatively() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let token = binder(&mut interner, 3, "token");
        let program = aprogram(vec![adef(
            &mut interner,
            1,
            "ds",
            AetherExpr::DropSpecialized {
                scrutinee: CoreVarRef::resolved(token),
                unique_body: Box::new(AetherExpr::Reuse {
                    token: CoreVarRef::resolved(token),
                    tag: CoreTag::Some,
                    fields: vec![AetherExpr::bound_var(x, Span::default())],
                    field_mask: None,
                    span: Span::default(),
                }),
                shared_body: Box::new(AetherExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![AetherExpr::bound_var(x, Span::default())],
                    span: Span::default(),
                }),
                span: Span::default(),
            },
            Some(FipAnnotation::Fbip),
        )]);
        let summaries = analyze_program_aether(&program, &interner);
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
        assert!(
            summary
                .reasons
                .contains(&FbipFailureReason::FreshAllocation)
        );
        assert!(
            !summary
                .reasons
                .contains(&FbipFailureReason::TokenUnavailable)
        );

        let token = binder(&mut interner, 3, "token");
        let dropped_program = aprogram(vec![adef(
            &mut interner,
            4,
            "dropped_rebuild",
            AetherExpr::Drop {
                var: CoreVarRef::resolved(token),
                body: Box::new(AetherExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![AetherExpr::bound_var(x, Span::default())],
                    span: Span::default(),
                }),
                span: Span::default(),
            },
            Some(FipAnnotation::Fbip),
        )]);
        let dropped_summaries = analyze_program_aether(&dropped_program, &interner);
        let dropped_summary = dropped_summaries.values().next().unwrap();
        assert!(
            dropped_summary
                .reasons
                .contains(&FbipFailureReason::TokenUnavailable)
        );
    }
}
