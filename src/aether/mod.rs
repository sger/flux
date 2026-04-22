//! Aether — Flux's compile-time reference counting optimization.
//!
//! Implements Perceus-style dup/drop insertion for Core IR, enabling:
//! - Phase 5: explicit `Dup` (Rc::clone) and `Drop` (early release) in Core IR
//! - Phase 6: borrowing analysis to elide dup/drop for read-only params
//! - Phase 7: reuse tokens for zero-allocation functional updates
//!
//! Aether contract:
//! - `Dup` / `Drop` operate only on resolved Core binders.
//! - `Reuse` may target only heap-allocating constructor tags and the reuse
//!   token must not appear free in any field expression.
//! - `DropSpecialized` must use a resolved scrutinee and both branches must
//!   remain valid Aether expressions that do not use the scrutinee value after
//!   specialization.
//!
//! The pass runs as the final Core IR transformation (after ANF normalization).
//! Existing passes (1-7) never see Dup/Drop nodes.

pub mod analysis;
pub mod borrow_infer;
pub mod callee;
pub mod check_fbip;
pub mod display;
pub mod drop_spec;
pub mod fbip_analysis;
pub mod free_vars;
pub mod fusion;
pub mod insert;
pub mod reuse;
pub mod reuse_analysis;
pub mod reuse_spec;
pub mod verify;

use crate::core::{
    CoreAlt, CoreBinder, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp, CoreProgram,
    CoreTag, CoreTopLevelItem, CoreType, CoreVarRef,
};
use crate::diagnostics::position::Span;
use crate::syntax::{Identifier, interner::Interner, statement::FipAnnotation};

// Aether's builtin-effect classifier was retired as part of Proposal 0161.
// The registry at `crate::syntax::builtin_effects::primop_coarse_effect_label`
// is now the single source of truth. Callers that previously matched on
// `AetherBuiltinEffect::{Io, Time}` now hold the interned label string
// (`"IO"` / `"Time"` / fine-grained) directly.

/// Backend-only Aether lowering product.
///
/// This is not a second semantic IR: it is clean Core plus RC/ownership
/// planning materialized for RC backends and debugging surfaces.
#[derive(Debug, Clone)]
pub struct AetherAlt {
    pub pat: CorePat,
    pub guard: Option<AetherExpr>,
    pub rhs: AetherExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AetherHandler {
    pub operation: Identifier,
    pub params: Vec<CoreBinder>,
    pub param_types: Vec<Option<CoreType>>,
    pub resume: CoreBinder,
    pub resume_ty: Option<CoreType>,
    pub body: AetherExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AetherExpr {
    Var {
        var: CoreVarRef,
        span: Span,
    },
    Lit(CoreLit, Span),
    Lam {
        params: Vec<CoreBinder>,
        param_types: Vec<Option<CoreType>>,
        result_ty: Option<CoreType>,
        body: Box<AetherExpr>,
        span: Span,
    },
    App {
        func: Box<AetherExpr>,
        args: Vec<AetherExpr>,
        span: Span,
    },
    AetherCall {
        func: Box<AetherExpr>,
        args: Vec<AetherExpr>,
        arg_modes: Vec<crate::aether::borrow_infer::BorrowMode>,
        span: Span,
    },
    Let {
        var: CoreBinder,
        rhs: Box<AetherExpr>,
        body: Box<AetherExpr>,
        span: Span,
    },
    LetRec {
        var: CoreBinder,
        rhs: Box<AetherExpr>,
        body: Box<AetherExpr>,
        span: Span,
    },
    LetRecGroup {
        bindings: Vec<(CoreBinder, Box<AetherExpr>)>,
        body: Box<AetherExpr>,
        span: Span,
    },
    Case {
        scrutinee: Box<AetherExpr>,
        alts: Vec<AetherAlt>,
        join_ty: Option<CoreType>,
        span: Span,
    },
    Con {
        tag: CoreTag,
        fields: Vec<AetherExpr>,
        span: Span,
    },
    PrimOp {
        op: CorePrimOp,
        args: Vec<AetherExpr>,
        span: Span,
    },
    MemberAccess {
        object: Box<AetherExpr>,
        member: Identifier,
        span: Span,
    },
    TupleField {
        object: Box<AetherExpr>,
        index: usize,
        span: Span,
    },
    Return {
        value: Box<AetherExpr>,
        span: Span,
    },
    Perform {
        effect: Identifier,
        operation: Identifier,
        args: Vec<AetherExpr>,
        span: Span,
    },
    Handle {
        body: Box<AetherExpr>,
        effect: Identifier,
        handlers: Vec<AetherHandler>,
        span: Span,
    },
    Dup {
        var: CoreVarRef,
        body: Box<AetherExpr>,
        span: Span,
    },
    Drop {
        var: CoreVarRef,
        body: Box<AetherExpr>,
        span: Span,
    },
    Reuse {
        token: CoreVarRef,
        tag: CoreTag,
        fields: Vec<AetherExpr>,
        field_mask: Option<u64>,
        span: Span,
    },
    DropSpecialized {
        scrutinee: CoreVarRef,
        unique_body: Box<AetherExpr>,
        shared_body: Box<AetherExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct AetherDef {
    pub name: Identifier,
    pub binder: CoreBinder,
    pub expr: AetherExpr,
    pub borrow_signature: Option<crate::aether::borrow_infer::BorrowSignature>,
    pub result_ty: Option<CoreType>,
    pub is_anonymous: bool,
    pub is_recursive: bool,
    pub fip: Option<FipAnnotation>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AetherProgram {
    pub core: CoreProgram,
    pub defs: Vec<AetherDef>,
    pub top_level_items: Vec<CoreTopLevelItem>,
}

impl AetherProgram {
    pub fn from_core(core: CoreProgram) -> Self {
        let defs = core
            .defs
            .iter()
            .cloned()
            .map(AetherDef::from_core)
            .collect();
        let top_level_items = core.top_level_items.clone();
        Self {
            core,
            defs,
            top_level_items,
        }
    }

    pub fn as_core(&self) -> &CoreProgram {
        &self.core
    }

    pub fn defs(&self) -> &[AetherDef] {
        &self.defs
    }

    pub fn top_level_items(&self) -> &[CoreTopLevelItem] {
        &self.top_level_items
    }

    pub fn into_core(self) -> CoreProgram {
        self.core
    }

    pub fn new(
        core: CoreProgram,
        defs: Vec<AetherDef>,
        top_level_items: Vec<CoreTopLevelItem>,
    ) -> Self {
        Self {
            core,
            defs,
            top_level_items,
        }
    }
}

impl AetherDef {
    pub fn from_core(def: CoreDef) -> Self {
        Self {
            name: def.name,
            binder: def.binder,
            expr: AetherExpr::from_core(def.expr),
            borrow_signature: def.borrow_signature,
            result_ty: def.result_ty,
            is_anonymous: def.is_anonymous,
            is_recursive: def.is_recursive,
            fip: def.fip,
            span: def.span,
        }
    }
}

impl AetherExpr {
    pub fn bound_var(binder: CoreBinder, span: Span) -> Self {
        Self::Var {
            var: CoreVarRef::resolved(&binder),
            span,
        }
    }

    pub fn unresolved_var(name: Identifier, span: Span) -> Self {
        Self::Var {
            var: CoreVarRef::unresolved(name),
            span,
        }
    }

    pub fn from_core(expr: CoreExpr) -> Self {
        match expr {
            CoreExpr::Var { var, span } => Self::Var { var, span },
            CoreExpr::Lit(lit, span) => Self::Lit(lit, span),
            CoreExpr::Lam {
                params,
                param_types,
                result_ty,
                body,
                span,
            } => Self::Lam {
                params,
                param_types,
                result_ty,
                body: Box::new(Self::from_core(*body)),
                span,
            },
            CoreExpr::App { func, args, span } => Self::App {
                func: Box::new(Self::from_core(*func)),
                args: args.into_iter().map(Self::from_core).collect(),
                span,
            },
            CoreExpr::Let {
                var,
                rhs,
                body,
                span,
            } => Self::Let {
                var,
                rhs: Box::new(Self::from_core(*rhs)),
                body: Box::new(Self::from_core(*body)),
                span,
            },
            CoreExpr::LetRec {
                var,
                rhs,
                body,
                span,
            } => Self::LetRec {
                var,
                rhs: Box::new(Self::from_core(*rhs)),
                body: Box::new(Self::from_core(*body)),
                span,
            },
            CoreExpr::LetRecGroup {
                bindings,
                body,
                span,
            } => Self::LetRecGroup {
                bindings: bindings
                    .into_iter()
                    .map(|(binder, rhs)| (binder, Box::new(Self::from_core(*rhs))))
                    .collect(),
                body: Box::new(Self::from_core(*body)),
                span,
            },
            CoreExpr::Case {
                scrutinee,
                alts,
                join_ty,
                span,
            } => Self::Case {
                scrutinee: Box::new(Self::from_core(*scrutinee)),
                alts: alts.into_iter().map(AetherAlt::from_core).collect(),
                join_ty,
                span,
            },
            CoreExpr::Con { tag, fields, span } => Self::Con {
                tag,
                fields: fields.into_iter().map(Self::from_core).collect(),
                span,
            },
            CoreExpr::PrimOp { op, args, span } => Self::PrimOp {
                op,
                args: args.into_iter().map(Self::from_core).collect(),
                span,
            },
            CoreExpr::MemberAccess {
                object,
                member,
                span,
            } => Self::MemberAccess {
                object: Box::new(Self::from_core(*object)),
                member,
                span,
            },
            CoreExpr::TupleField {
                object,
                index,
                span,
            } => Self::TupleField {
                object: Box::new(Self::from_core(*object)),
                index,
                span,
            },
            CoreExpr::Return { value, span } => Self::Return {
                value: Box::new(Self::from_core(*value)),
                span,
            },
            CoreExpr::Perform {
                effect,
                operation,
                args,
                span,
            } => Self::Perform {
                effect,
                operation,
                args: args.into_iter().map(Self::from_core).collect(),
                span,
            },
            CoreExpr::Handle {
                body,
                effect,
                handlers,
                span,
            } => Self::Handle {
                body: Box::new(Self::from_core(*body)),
                effect,
                handlers: handlers.into_iter().map(AetherHandler::from_core).collect(),
                span,
            },
        }
    }

    pub fn into_core(self) -> CoreExpr {
        match self {
            Self::Var { var, span } => CoreExpr::Var { var, span },
            Self::Lit(lit, span) => CoreExpr::Lit(lit, span),
            Self::Lam {
                params,
                param_types,
                result_ty,
                body,
                span,
            } => CoreExpr::Lam {
                params,
                param_types,
                result_ty,
                body: Box::new(body.into_core()),
                span,
            },
            Self::App { func, args, span } => CoreExpr::App {
                func: Box::new(func.into_core()),
                args: args.into_iter().map(Self::into_core).collect(),
                span,
            },
            Self::Let {
                var,
                rhs,
                body,
                span,
            } => CoreExpr::Let {
                var,
                rhs: Box::new(rhs.into_core()),
                body: Box::new(body.into_core()),
                span,
            },
            Self::LetRec {
                var,
                rhs,
                body,
                span,
            } => CoreExpr::LetRec {
                var,
                rhs: Box::new(rhs.into_core()),
                body: Box::new(body.into_core()),
                span,
            },
            Self::LetRecGroup {
                bindings,
                body,
                span,
            } => CoreExpr::LetRecGroup {
                bindings: bindings
                    .into_iter()
                    .map(|(binder, rhs)| (binder, Box::new(rhs.into_core())))
                    .collect(),
                body: Box::new(body.into_core()),
                span,
            },
            Self::Case {
                scrutinee,
                alts,
                join_ty,
                span,
            } => CoreExpr::Case {
                scrutinee: Box::new(scrutinee.into_core()),
                alts: alts.into_iter().map(AetherAlt::into_core).collect(),
                join_ty,
                span,
            },
            Self::Con { tag, fields, span } => CoreExpr::Con {
                tag,
                fields: fields.into_iter().map(Self::into_core).collect(),
                span,
            },
            Self::PrimOp { op, args, span } => CoreExpr::PrimOp {
                op,
                args: args.into_iter().map(Self::into_core).collect(),
                span,
            },
            Self::MemberAccess {
                object,
                member,
                span,
            } => CoreExpr::MemberAccess {
                object: Box::new(object.into_core()),
                member,
                span,
            },
            Self::TupleField {
                object,
                index,
                span,
            } => CoreExpr::TupleField {
                object: Box::new(object.into_core()),
                index,
                span,
            },
            Self::Return { value, span } => CoreExpr::Return {
                value: Box::new(value.into_core()),
                span,
            },
            Self::Perform {
                effect,
                operation,
                args,
                span,
            } => CoreExpr::Perform {
                effect,
                operation,
                args: args.into_iter().map(Self::into_core).collect(),
                span,
            },
            Self::Handle {
                body,
                effect,
                handlers,
                span,
            } => CoreExpr::Handle {
                body: Box::new(body.into_core()),
                effect,
                handlers: handlers.into_iter().map(AetherHandler::into_core).collect(),
                span,
            },
            Self::AetherCall { .. }
            | Self::Dup { .. }
            | Self::Drop { .. }
            | Self::Reuse { .. }
            | Self::DropSpecialized { .. } => {
                panic!("Aether-only expressions cannot be projected back into semantic Core")
            }
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Var { span, .. }
            | Self::Lit(_, span)
            | Self::Lam { span, .. }
            | Self::App { span, .. }
            | Self::AetherCall { span, .. }
            | Self::Let { span, .. }
            | Self::LetRec { span, .. }
            | Self::LetRecGroup { span, .. }
            | Self::Case { span, .. }
            | Self::Con { span, .. }
            | Self::PrimOp { span, .. }
            | Self::MemberAccess { span, .. }
            | Self::TupleField { span, .. }
            | Self::Return { span, .. }
            | Self::Perform { span, .. }
            | Self::Handle { span, .. }
            | Self::Dup { span, .. }
            | Self::Drop { span, .. }
            | Self::Reuse { span, .. }
            | Self::DropSpecialized { span, .. } => *span,
        }
    }
}

impl AetherAlt {
    fn from_core(alt: CoreAlt) -> Self {
        Self {
            pat: alt.pat,
            guard: alt.guard.map(AetherExpr::from_core),
            rhs: AetherExpr::from_core(alt.rhs),
            span: alt.span,
        }
    }

    fn into_core(self) -> CoreAlt {
        CoreAlt {
            pat: self.pat,
            guard: self.guard.map(AetherExpr::into_core),
            rhs: self.rhs.into_core(),
            span: self.span,
        }
    }
}

impl AetherHandler {
    fn from_core(handler: CoreHandler) -> Self {
        Self {
            operation: handler.operation,
            params: handler.params,
            param_types: handler.param_types,
            resume: handler.resume,
            resume_ty: handler.resume_ty,
            body: AetherExpr::from_core(handler.body),
            span: handler.span,
        }
    }

    fn into_core(self) -> CoreHandler {
        CoreHandler {
            operation: self.operation,
            params: self.params,
            param_types: self.param_types,
            resume: self.resume,
            resume_ty: self.resume_ty,
            body: self.body.into_core(),
            span: self.span,
        }
    }
}

/// Look up the coarse effect label (`"IO"`, `"Time"`, `"Panic"`) a builtin
/// function carries, if any. The result routes through the
/// `primop_coarse_effect_label` registry — this function just bridges from
/// function name to the primop enum the registry expects.
///
/// Returns `None` when the name does not refer to a known builtin primop
/// or the primop has no effect (arithmetic, collection access, etc.).
pub fn builtin_effect_for_name(name: &str) -> Option<&'static str> {
    // Try zero-, one-, and two-arg forms; the registry is arity-agnostic so
    // the first successful resolution wins.
    for arity in 0..=3 {
        if let Some(op) = CorePrimOp::from_name(name, arity)
            && let Some(label) = crate::syntax::builtin_effects::primop_coarse_effect_label(op)
        {
            return Some(label);
        }
    }
    None
}

/// Statistics collected from an Aether-transformed Core IR expression.
#[derive(Debug, Clone, Default)]
pub struct AetherStats {
    pub dups: usize,
    pub drops: usize,
    pub reuses: usize,
    pub drop_specs: usize,
    /// Number of heap constructor allocations (Con nodes with heap tags).
    pub allocs: usize,
}

/// FBIP status auto-detected from Aether stats (Perceus Section 2.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbipStatus {
    /// Zero unreused allocations — functional but fully in-place on the unique path.
    Fip,
    /// N unreused allocations — functional but partially in-place.
    Fbip(usize),
    /// No constructors in the function — FBIP classification not applicable.
    NotApplicable,
}

impl AetherStats {
    /// Total constructor sites (both fresh allocations and reused ones).
    pub fn total_constructors(&self) -> usize {
        self.allocs + self.reuses
    }

    /// Auto-detect FBIP status from allocation and reuse counts.
    /// - `fip`: all constructor sites are reused (zero fresh allocations)
    /// - `fbip(N)`: N fresh allocations (not reused)
    /// - `NotApplicable`: no constructor sites at all
    pub fn fbip_status(&self) -> FbipStatus {
        if self.total_constructors() == 0 {
            FbipStatus::NotApplicable
        } else if self.allocs == 0 {
            FbipStatus::Fip
        } else {
            FbipStatus::Fbip(self.allocs)
        }
    }
}

impl std::fmt::Display for AetherStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dups: {}  Drops: {}  Reuses: {}  DropSpecs: {}",
            self.dups, self.drops, self.reuses, self.drop_specs
        )?;
        match self.fbip_status() {
            FbipStatus::Fip => write!(f, "  FBIP: fip"),
            FbipStatus::Fbip(n) => write!(f, "  FBIP: fbip({})", n),
            FbipStatus::NotApplicable => Ok(()),
        }
    }
}

/// Walk an Aether expression and count Dup/Drop/Reuse nodes.
pub fn collect_stats(expr: &AetherExpr) -> AetherStats {
    let mut stats = AetherStats::default();
    count_nodes(expr, &mut stats);
    stats
}

fn count_nodes(expr: &AetherExpr, stats: &mut AetherStats) {
    match expr {
        AetherExpr::Dup { body, .. } => {
            stats.dups += 1;
            count_nodes(body, stats);
        }
        AetherExpr::Drop { body, .. } => {
            stats.drops += 1;
            count_nodes(body, stats);
        }
        AetherExpr::Reuse { fields, .. } => {
            stats.reuses += 1;
            for f in fields {
                count_nodes(f, stats);
            }
        }
        AetherExpr::Var { .. } | AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { body, .. } => count_nodes(body, stats),
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            count_nodes(func, stats);
            for a in args {
                count_nodes(a, stats);
            }
        }
        AetherExpr::Let { rhs, body, .. } | AetherExpr::LetRec { rhs, body, .. } => {
            count_nodes(rhs, stats);
            count_nodes(body, stats);
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                count_nodes(rhs, stats);
            }
            count_nodes(body, stats);
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            count_nodes(scrutinee, stats);
            for alt in alts {
                count_nodes(&alt.rhs, stats);
                if let Some(g) = &alt.guard {
                    count_nodes(g, stats);
                }
            }
        }
        AetherExpr::Con { tag, fields, .. } => {
            // Count heap-allocating constructors (not Nil/None which are value types)
            if is_heap_tag(tag) {
                stats.allocs += 1;
            }
            for f in fields {
                count_nodes(f, stats);
            }
        }
        AetherExpr::PrimOp { args, .. } => {
            for a in args {
                count_nodes(a, stats);
            }
        }
        AetherExpr::Return { value, .. } => count_nodes(value, stats),
        AetherExpr::Perform { args, .. } => {
            for a in args {
                count_nodes(a, stats);
            }
        }
        AetherExpr::Handle { body, handlers, .. } => {
            count_nodes(body, stats);
            for h in handlers {
                count_nodes(&h.body, stats);
            }
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            count_nodes(object, stats);
        }
        AetherExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            stats.drop_specs += 1;
            count_nodes(unique_body, stats);
            count_nodes(shared_body, stats);
        }
    }
}

/// Returns true for constructor tags that allocate on the heap.
/// Nil and None are value types (no heap allocation).
pub(crate) fn is_heap_tag(tag: &CoreTag) -> bool {
    match tag {
        CoreTag::Cons | CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Named(_) => true,
        CoreTag::Nil | CoreTag::None => false,
    }
}

/// View an expression as a constructor allocation shape for Aether passes.
///
/// Named ADT constructors can still appear as external constructor applications
/// in Core; those require an expected tag from the enclosing pattern to
/// disambiguate them from ordinary external calls.
pub fn constructor_shape_for_tag<'a>(
    expr: &'a CoreExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [CoreExpr], Span)> {
    match expr {
        CoreExpr::Con { tag, fields, span } => Some((tag.clone(), fields.as_slice(), *span)),
        CoreExpr::App { func, args, span } => {
            constructor_app_shape_for_tag(func.as_ref(), args, *span, expected_tag)
        }
        _ => None,
    }
}

pub fn constructor_shape_for_tag_aether<'a>(
    expr: &'a AetherExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [AetherExpr], Span)> {
    match expr {
        AetherExpr::Con { tag, fields, span } => Some((tag.clone(), fields.as_slice(), *span)),
        AetherExpr::App { func, args, span } => {
            constructor_app_shape_for_tag_aether(func.as_ref(), args, *span, expected_tag)
        }
        AetherExpr::AetherCall {
            func, args, span, ..
        } => constructor_app_shape_for_tag_aether(func.as_ref(), args, *span, expected_tag),
        _ => None,
    }
}

/// Consume an expression if it is constructor-shaped.
pub fn into_constructor_shape_for_tag(
    expr: CoreExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<CoreExpr>, Span)> {
    match expr {
        CoreExpr::Con { tag, fields, span } => Some((tag, fields, span)),
        CoreExpr::App { func, args, span } => {
            into_constructor_app_shape_for_tag(*func, args, span, expected_tag)
        }
        _ => None,
    }
}

pub fn into_constructor_shape_for_tag_aether(
    expr: AetherExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<AetherExpr>, Span)> {
    match expr {
        AetherExpr::Con { tag, fields, span } => Some((tag, fields, span)),
        AetherExpr::App { func, args, span } => {
            into_constructor_app_shape_for_tag_aether(*func, args, span, expected_tag)
        }
        AetherExpr::AetherCall {
            func, args, span, ..
        } => into_constructor_app_shape_for_tag_aether(*func, args, span, expected_tag),
        _ => None,
    }
}

fn constructor_app_shape_for_tag<'a>(
    func: &'a CoreExpr,
    args: &'a [CoreExpr],
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [CoreExpr], Span)> {
    let CoreExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(var, expected_tag)?;
    Some((tag, args, span))
}

fn constructor_app_shape_for_tag_aether<'a>(
    func: &'a AetherExpr,
    args: &'a [AetherExpr],
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [AetherExpr], Span)> {
    let AetherExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(var, expected_tag)?;
    Some((tag, args, span))
}

fn into_constructor_app_shape_for_tag(
    func: CoreExpr,
    args: Vec<CoreExpr>,
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<CoreExpr>, Span)> {
    let CoreExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(&var, expected_tag)?;
    Some((tag, args, span))
}

fn into_constructor_app_shape_for_tag_aether(
    func: AetherExpr,
    args: Vec<AetherExpr>,
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<AetherExpr>, Span)> {
    let AetherExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(&var, expected_tag)?;
    Some((tag, args, span))
}

fn core_tag_from_constructor_var(
    var: &CoreVarRef,
    expected_tag: Option<&CoreTag>,
) -> Option<CoreTag> {
    if var.binder.is_some() {
        return None;
    }
    match expected_tag {
        Some(CoreTag::Named(name)) if var.name == *name => Some(CoreTag::Named(*name)),
        _ => None,
    }
}

/// Run the full Aether optimization pipeline on a Core IR expression.
///
/// Pipeline order:
/// 1. Dup/drop insertion (Phase 5) — insert explicit Rc operations
/// 2. Drop specialization (Phase 8) — split into unique/shared paths
/// 3. Dup/drop fusion (Phase 9) — cancel adjacent dup/drop pairs
/// 4. Baseline reuse insertion (Phase 7) — emit legal plain `Reuse` sites
/// 5. Reuse specialization (Phase 7b) — add profitable selective-write masks
///
/// This is the public entry point called from `run_core_passes`.
pub fn run_aether_pass(expr: CoreExpr) -> AetherExpr {
    run_aether_expr(AetherExpr::from_core(expr))
}

/// Run the full Aether pipeline on a backend-only Aether expression.
pub fn run_aether_expr(expr: AetherExpr) -> AetherExpr {
    let expr = insert::insert_dup_drop_aether(expr);
    let expr = drop_spec::specialize_drops_aether(expr);
    let expr = fusion::fuse_dup_drop_aether(expr);
    let expr = reuse::insert_reuse_aether(expr);
    reuse_spec::specialize_reuse_aether(expr)
}

/// Run the Aether pipeline with a borrow registry for cross-function optimization.
/// Arguments to borrowed parameters will skip Rc::clone.
pub fn run_aether_pass_with_registry(
    expr: CoreExpr,
    registry: &borrow_infer::BorrowRegistry,
) -> AetherExpr {
    run_aether_expr_with_registry(AetherExpr::from_core(expr), registry)
}

/// Run the Aether pipeline with a borrow registry on a backend-only Aether expression.
pub fn run_aether_expr_with_registry(
    expr: AetherExpr,
    registry: &borrow_infer::BorrowRegistry,
) -> AetherExpr {
    let expr = insert::insert_dup_drop_with_registry_aether(expr, registry);
    let expr = drop_spec::specialize_drops_aether(expr);
    let expr = fusion::fuse_dup_drop_aether(expr);
    let expr = reuse::insert_reuse_aether(expr);
    reuse_spec::specialize_reuse_aether(expr)
}

#[allow(clippy::result_large_err)]
pub fn lower_core_to_aether_program(
    core: &CoreProgram,
    interner: Option<&Interner>,
    preloaded_registry: borrow_infer::BorrowRegistry,
) -> Result<(AetherProgram, Vec<crate::diagnostics::Diagnostic>), crate::diagnostics::Diagnostic> {
    let mut warnings = Vec::new();
    let mut semantic_core = core.clone();
    let borrow_registry = borrow_infer::infer_borrow_modes_with_preloaded(
        &mut semantic_core,
        interner,
        preloaded_registry,
    );

    let defs = semantic_core
        .defs
        .iter()
        .cloned()
        .map(|def| AetherDef {
            name: def.name,
            binder: def.binder,
            expr: run_aether_expr_with_registry(AetherExpr::from_core(def.expr), &borrow_registry),
            borrow_signature: def.borrow_signature,
            result_ty: def.result_ty,
            is_anonymous: def.is_anonymous,
            is_recursive: def.is_recursive,
            fip: def.fip,
            span: def.span,
        })
        .collect();

    if let Some(interner) = interner {
        let fbip_result = check_fbip::check_fbip(&semantic_core, interner);
        warnings.extend(fbip_result.warnings);
        if let Some(error) = fbip_result.error {
            return Err(error);
        }
    }

    let top_level_items = semantic_core.top_level_items.clone();
    Ok((
        AetherProgram::new(semantic_core, defs, top_level_items),
        warnings,
    ))
}
