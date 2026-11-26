//! Where a handle's clauses run, as both backends decide it from Aether:
//! natively the evidence entry's flag, on the VM the handler frame's. A
//! clause that resumes once, as its last act, runs where the perform was;
//! any other at its handle's prompt. Either runs in its handle's context,
//! unless it reads no evidence (KI-233, KI-243).

use crate::aether::{AetherExpr, AetherHandler};
use crate::core::CoreBinderId;
use crate::syntax::Identifier;

/// Where a handle's clauses run, as both backends read it from the handle's
/// evidence entry (natively) or handler frame (on the VM). Its values are
/// the native entry's flag.
///
/// @KI-185: the entry has one flag for all its clauses, so a handle has the
/// kind every one of its clauses allows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HandleKind {
    /// At the handle's prompt, under the evidence the handle was installed
    /// under, with a `resume` that delimits (KI-233).
    #[default]
    Captured = 0,
    /// At the prompt, under the perform's evidence, with a `resume` that only
    /// runs the frames between the perform and the prompt, so a loop of
    /// performs keeps a flat stack. Only for clauses that read no evidence:
    /// a clause that performs runs in its handle's context (KI-243).
    LastActPure = 1,
    /// Where the perform was, called as a function, under the evidence the
    /// handle was installed under, with no continuation captured (proposal
    /// 0205, part A).
    InPlace = 2,
    /// In place, with the perform's evidence left as it is: the clause reads
    /// none.
    InPlacePure = 3,
    /// At the prompt, under the evidence the handle was installed under,
    /// with a `resume` like `LastActPure`'s that also puts the perform's
    /// evidence back for the frames it runs. For a clause that resumes as its
    /// last act on some paths and performs: one that can't run in place, yet
    /// would nest a prompt per perform with a `resume` that delimits.
    LastAct = 4,
}

impl HandleKind {
    /// Whether the clauses run where the perform was.
    pub fn in_place(self) -> bool {
        matches!(self, Self::InPlace | Self::InPlacePure)
    }

    /// The kind a byte written as `kind as u8` names.
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Captured),
            1 => Some(Self::LastActPure),
            2 => Some(Self::InPlace),
            3 => Some(Self::InPlacePure),
            4 => Some(Self::LastAct),
            _ => None,
        }
    }
}

/// The kind of a handle with these clauses. `parameterized`: the handle has
/// a state, so a clause resumes with `resume(v, s)`.
///
/// @KI-243: a clause in place runs under its handle's evidence, so one that
/// performs, its own effect included (KI-233), can run in place. A clause
/// that only resumes as its last act, with a branch that doesn't, can't run
/// in place, and runs where the perform's evidence is only if it performs
/// nothing.
///
/// @KI-244: one that performs still gets a `resume` that doesn't delimit,
/// as it did before KI-243 was fixed: with one that does, each perform its
/// resumed code makes nests another prompt, and a loop of them overflows
/// the C stack.
pub fn handle_kind(handlers: &[AetherHandler], parameterized: bool) -> HandleKind {
    let arity = if parameterized { 2 } else { 1 };
    let pure = handlers.iter().all(|h| performs_nothing(&h.body, h.resume.id));
    if handlers.iter().all(|h| resumes_once_as_last_act(&h.body, h.resume.id, arity)) {
        if pure { HandleKind::InPlacePure } else { HandleKind::InPlace }
    } else if handlers.iter().all(|h| resumes_only_as_last_act(&h.body, h.resume.id)) {
        if pure { HandleKind::LastActPure } else { HandleKind::LastAct }
    } else {
        HandleKind::Captured
    }
}

/// Whether every path through a clause ends in exactly one call of `resume`
/// with `arity` arguments, and `resume` appears nowhere else: the clause can
/// run as a function at the perform, with `resume` giving back its argument.
pub fn resumes_once_as_last_act(body: &AetherExpr, resume: CoreBinderId, arity: usize) -> bool {
    let mentions = |expr: &AetherExpr| {
        crate::aether::free_vars::collect_free_vars_aether(expr).contains(&resume)
    };
    let last = |expr: &AetherExpr| resumes_once_as_last_act(expr, resume, arity);
    match body {
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } if matches!(func.as_ref(), AetherExpr::Var { var, .. } if var.binder == Some(resume)) => {
            args.len() == arity && !args.iter().any(mentions)
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            !mentions(rhs) && last(body)
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            !bindings.iter().any(|(_, rhs)| mentions(rhs)) && last(body)
        }
        AetherExpr::Case { scrutinee, alts, .. } => {
            !mentions(scrutinee)
                && alts
                    .iter()
                    .all(|alt| !alt.guard.as_ref().is_some_and(mentions) && last(&alt.rhs))
        }
        AetherExpr::Dup { var, body, .. }
        | AetherExpr::Drop { var, body, .. }
        | AetherExpr::FreeShell { var, body, .. } => var.binder != Some(resume) && last(body),
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            last(unique_body) && last(shared_body)
        }
        _ => false,
    }
}

/// Whether a clause reads no evidence: it performs nothing, installs no
/// handler and calls nothing but `resume`, any of which could perform.
/// Primops, constructors and lambdas it only builds don't.
pub fn performs_nothing(expr: &AetherExpr, resume: CoreBinderId) -> bool {
    let children: Vec<&AetherExpr> = match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(..) | AetherExpr::Lam { .. } => Vec::new(),
        AetherExpr::Perform { .. } | AetherExpr::Handle { .. } => return false,
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            if !matches!(func.as_ref(), AetherExpr::Var { var, .. } if var.binder == Some(resume)) {
                return false;
            }
            args.iter().collect()
        }
        AetherExpr::Dup { body, .. }
        | AetherExpr::Drop { body, .. }
        | AetherExpr::FreeShell { body, .. } => vec![body],
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            vec![rhs, body]
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => bindings
            .iter()
            .map(|(_, rhs)| rhs.as_ref())
            .chain(std::iter::once(body.as_ref()))
            .collect(),
        AetherExpr::Case { scrutinee, alts, .. } => std::iter::once(scrutinee.as_ref())
            .chain(alts.iter().flat_map(|alt| alt.guard.iter().chain(std::iter::once(&alt.rhs))))
            .collect(),
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::Reuse { fields: args, .. }
        | AetherExpr::PrimOp { args, .. } => args.iter().collect(),
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            vec![object]
        }
        AetherExpr::Return { value, .. } => vec![value],
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            vec![unique_body, shared_body]
        }
    };
    children.into_iter().all(|child| performs_nothing(child, resume))
}

/// Whether `expr` performs `effect` anywhere in it, lambdas included, other
/// than in the body of a `handle` for `effect` inside it, which reaches that
/// handle. Core's `performs_effect`, over Aether.
pub fn performs_effect(expr: &AetherExpr, effect: Identifier) -> bool {
    let children: Vec<&AetherExpr> = match expr {
        AetherExpr::Var { .. } | AetherExpr::Lit(..) => Vec::new(),
        AetherExpr::Perform { effect: performed, args, .. } => {
            if *performed == effect {
                return true;
            }
            args.iter().collect()
        }
        AetherExpr::Lam { body, .. }
        | AetherExpr::Dup { body, .. }
        | AetherExpr::Drop { body, .. }
        | AetherExpr::FreeShell { body, .. } => vec![body],
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            std::iter::once(func.as_ref()).chain(args).collect()
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            vec![rhs, body]
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => bindings
            .iter()
            .map(|(_, rhs)| rhs.as_ref())
            .chain(std::iter::once(body.as_ref()))
            .collect(),
        AetherExpr::Case { scrutinee, alts, .. } => std::iter::once(scrutinee.as_ref())
            .chain(alts.iter().flat_map(|alt| alt.guard.iter().chain(std::iter::once(&alt.rhs))))
            .collect(),
        AetherExpr::Con { fields: args, .. }
        | AetherExpr::Reuse { fields: args, .. }
        | AetherExpr::PrimOp { args, .. } => args.iter().collect(),
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            vec![object]
        }
        AetherExpr::Return { value, .. } => vec![value],
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            vec![unique_body, shared_body]
        }
        AetherExpr::Handle { body, effect: handled, parameter, handlers, .. } => {
            std::iter::once(body.as_ref())
                .filter(|_| *handled != effect)
                .chain(parameter.as_deref())
                .chain(handlers.iter().map(|h| &h.body))
                .collect()
        }
    };
    children.into_iter().any(|child| performs_effect(child, effect))
}

/// Whether a handler clause uses `resume` only as a call it ends with: after
/// such a call the clause has nothing left to do, so the resumed code needs no
/// handler of its own around it.
pub fn resumes_only_as_last_act(body: &AetherExpr, resume: CoreBinderId) -> bool {
    let mentions = |expr: &AetherExpr| {
        crate::aether::free_vars::collect_free_vars_aether(expr).contains(&resume)
    };
    let last = |expr: &AetherExpr| resumes_only_as_last_act(expr, resume);
    match body {
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } if matches!(func.as_ref(), AetherExpr::Var { var, .. } if var.binder == Some(resume)) => {
            !args.iter().any(mentions)
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            !mentions(rhs) && last(body)
        }
        AetherExpr::Case { scrutinee, alts, .. } => {
            !mentions(scrutinee)
                && alts
                    .iter()
                    .all(|alt| !alt.guard.as_ref().is_some_and(mentions) && last(&alt.rhs))
        }
        AetherExpr::Dup { var, body, .. }
        | AetherExpr::Drop { var, body, .. }
        | AetherExpr::FreeShell { var, body, .. } => var.binder != Some(resume) && last(body),
        AetherExpr::DropSpecialized { unique_body, shared_body, .. } => {
            last(unique_body) && last(shared_body)
        }
        other => !mentions(other),
    }
}
