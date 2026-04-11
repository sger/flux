/// Shared helper functions for Core IR passes.
///
/// These utilities (substitution, tree walking, free-variable analysis) are
/// used by multiple optimization passes.
use crate::core::{CoreBinderId, CoreExpr, CoreHandler, CorePat, CorePrimOp};

// ── Substitution ──────────────────────────────────────────────────────────────

/// Substitute `replacement` for free occurrences of `var` in `expr`.
///
/// This is capture-avoiding for `Lam` and `Let` binders.
pub(super) fn subst(expr: CoreExpr, var: CoreBinderId, replacement: &CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Var { var: ref_var, span } => {
            if ref_var.binder == Some(var) {
                replacement.clone()
            } else {
                CoreExpr::Var { var: ref_var, span }
            }
        }
        CoreExpr::Lam { params, body, span } => {
            if params.iter().any(|p| p.id == var) {
                // Shadowed — don't substitute inside.
                CoreExpr::Lam { params, body, span }
            } else {
                CoreExpr::Lam {
                    params,
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            span,
        } => {
            let rhs = subst(*rhs, var, replacement);
            if binding.id == var {
                CoreExpr::Let {
                    var: binding,
                    rhs: Box::new(rhs),
                    body,
                    span,
                }
            } else {
                CoreExpr::Let {
                    var: binding,
                    rhs: Box::new(rhs),
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(subst(*func, var, replacement)),
            args: args
                .into_iter()
                .map(|a| subst(a, var, replacement))
                .collect(),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee = subst(*scrutinee, var, replacement);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    if !pat_binds(&alt.pat, var) {
                        alt.guard = alt.guard.map(|g| subst(g, var, replacement));
                        alt.rhs = subst(alt.rhs, var, replacement);
                    }
                    alt
                })
                .collect();
            CoreExpr::Case {
                scrutinee: Box::new(scrutinee),
                alts,
                span,
            }
        }
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| subst(f, var, replacement))
                .collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| subst(a, var, replacement))
                .collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(subst(*value, var, replacement)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args
                .into_iter()
                .map(|a| subst(a, var, replacement))
                .collect(),
            span,
        },
        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            span,
        } => {
            if binding.id == var {
                CoreExpr::LetRec {
                    var: binding,
                    rhs,
                    body,
                    span,
                }
            } else {
                CoreExpr::LetRec {
                    var: binding,
                    rhs: Box::new(subst(*rhs, var, replacement)),
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => {
            if bindings.iter().any(|(b, _)| b.id == var) {
                CoreExpr::LetRecGroup {
                    bindings,
                    body,
                    span,
                }
            } else {
                CoreExpr::LetRecGroup {
                    bindings: bindings
                        .into_iter()
                        .map(|(b, rhs)| (b, Box::new(subst(*rhs, var, replacement))))
                        .collect(),
                    body: Box::new(subst(*body, var, replacement)),
                    span,
                }
            }
        }
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(subst(*body, var, replacement)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|handler| subst_handler(handler, var, replacement))
                .collect(),
            span,
        },
        CoreExpr::Dup {
            var: dup_var,
            body,
            span,
        } => CoreExpr::Dup {
            var: if dup_var.binder == Some(var) {
                // The var itself is being substituted — but Dup/Drop reference
                // variables, not arbitrary expressions. Keep the ref as-is.
                dup_var
            } else {
                dup_var
            },
            body: Box::new(subst(*body, var, replacement)),
            span,
        },
        CoreExpr::Drop {
            var: drop_var,
            body,
            span,
        } => CoreExpr::Drop {
            var: drop_var,
            body: Box::new(subst(*body, var, replacement)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields
                .into_iter()
                .map(|f| subst(f, var, replacement))
                .collect(),
            field_mask,
            span,
        },
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(subst(*object, var, replacement)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(subst(*object, var, replacement)),
            index,
            span,
        },
        other => other,
    }
}

pub(super) fn subst_handler(
    handler: CoreHandler,
    var: CoreBinderId,
    replacement: &CoreExpr,
) -> CoreHandler {
    if handler.resume.id == var || handler.params.iter().any(|param| param.id == var) {
        handler
    } else {
        CoreHandler {
            body: subst(handler.body, var, replacement),
            ..handler
        }
    }
}

// ── Tree walker ───────────────────────────────────────────────────────────────

pub(super) fn map_children(expr: CoreExpr, f: fn(CoreExpr) -> CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(f(*func)),
            args: args.into_iter().map(f).collect(),
            span,
        },
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(f(*rhs)),
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(f(*rhs)),
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(b, rhs)| (b, Box::new(f(*rhs))))
                .collect(),
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(f(*scrutinee)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = f(alt.rhs);
                    alt.guard = alt.guard.map(f);
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(f).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(f).collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(f(*value)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(f).collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(f(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut handler| {
                    handler.body = f(handler.body);
                    handler
                })
                .collect(),
            span,
        },
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(f(*body)),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields.into_iter().map(f).collect(),
            field_mask,
            span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(f(*unique_body)),
            shared_body: Box::new(f(*shared_body)),
            span,
        },
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(f(*object)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(f(*object)),
            index,
            span,
        },
        other => other,
    }
}

// ── Analysis helpers ──────────────────────────────────────────────────────────

/// Returns true when `expr` is trivially pure — only literals and variables.
/// Used by `inline_trivial_lets` which must not duplicate non-trivial computation.
pub(super) fn is_trivially_pure(expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::Lit(_, _) | CoreExpr::Var { .. })
}

/// Returns true when `expr` is guaranteed pure (no effects, no calls, cannot fail).
/// Uses primop-level classification: typed arithmetic on proven types is pure,
/// generic arithmetic that may fail on type mismatches is not.
/// Used by dead-binding elimination where the question is "can we drop this?".
pub(super) fn is_pure(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => true,
        CoreExpr::Lam { .. } => true,
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => fields.iter().all(is_pure),
        CoreExpr::PrimOp { op, args, .. } => is_primop_pure(op) && args.iter().all(is_pure),
        _ => false, // App, Let, LetRec, Case, Perform, Handle, Return
    }
}

/// Classify whether a primitive operation can fail at runtime.
fn is_primop_pure(op: &CorePrimOp) -> bool {
    match op {
        // Typed arithmetic on proven types — can't type-mismatch
        CorePrimOp::IAdd
        | CorePrimOp::ISub
        | CorePrimOp::IMul
        | CorePrimOp::FAdd
        | CorePrimOp::FSub
        | CorePrimOp::FMul
        | CorePrimOp::ICmpEq
        | CorePrimOp::ICmpNe
        | CorePrimOp::ICmpLt
        | CorePrimOp::ICmpLe
        | CorePrimOp::ICmpGt
        | CorePrimOp::ICmpGe
        | CorePrimOp::FCmpEq
        | CorePrimOp::FCmpNe
        | CorePrimOp::FCmpLt
        | CorePrimOp::FCmpLe
        | CorePrimOp::FCmpGt
        | CorePrimOp::FCmpGe => true,
        // Boolean/equality — can't fail
        CorePrimOp::And | CorePrimOp::Or | CorePrimOp::Not | CorePrimOp::Eq | CorePrimOp::NEq => {
            true
        }
        // Safe arithmetic (Proposal 0135) — total, always returns Option
        CorePrimOp::SafeDiv | CorePrimOp::SafeMod => true,
        // Constructors — always pure
        CorePrimOp::MakeList
        | CorePrimOp::MakeArray
        | CorePrimOp::MakeTuple
        | CorePrimOp::MakeHash
        | CorePrimOp::Concat
        | CorePrimOp::Interpolate => true,
        // Division — may fail (division by zero)
        CorePrimOp::Div
        | CorePrimOp::IDiv
        | CorePrimOp::FDiv
        | CorePrimOp::Mod
        | CorePrimOp::IMod => false,
        // Generic arithmetic — may fail (type mismatch under gradual typing)
        CorePrimOp::Add
        | CorePrimOp::Sub
        | CorePrimOp::Mul
        | CorePrimOp::Abs
        | CorePrimOp::Min
        | CorePrimOp::Max => false,
        // Comparisons — may fail on incomparable types
        CorePrimOp::Lt | CorePrimOp::Le | CorePrimOp::Gt | CorePrimOp::Ge => false,
        // Negation — may fail (wrong type)
        CorePrimOp::Neg => false,
        // Access ops — may fail (out of bounds, missing key)
        CorePrimOp::Index => false,
        // Promoted primops — most are impure (I/O, side effects) or may fail.
        // Pure type-inspection primops could be true, but conservatively false.
        CorePrimOp::Print
        | CorePrimOp::Println
        | CorePrimOp::ReadFile
        | CorePrimOp::WriteFile
        | CorePrimOp::ReadStdin
        | CorePrimOp::ReadLines
        | CorePrimOp::StringLength
        | CorePrimOp::StringConcat
        | CorePrimOp::StringSlice
        | CorePrimOp::ToString
        | CorePrimOp::Split
        | CorePrimOp::Join
        | CorePrimOp::Trim
        | CorePrimOp::Upper
        | CorePrimOp::Lower
        | CorePrimOp::StartsWith
        | CorePrimOp::EndsWith
        | CorePrimOp::Replace
        | CorePrimOp::Substring
        | CorePrimOp::Chars
        | CorePrimOp::StrContains
        | CorePrimOp::ArrayLen
        | CorePrimOp::ArrayGet
        | CorePrimOp::ArraySet
        | CorePrimOp::ArrayPush
        | CorePrimOp::ArrayConcat
        | CorePrimOp::ArraySlice
        | CorePrimOp::HamtGet
        | CorePrimOp::HamtSet
        | CorePrimOp::HamtDelete
        | CorePrimOp::HamtKeys
        | CorePrimOp::HamtValues
        | CorePrimOp::HamtMerge
        | CorePrimOp::HamtSize
        | CorePrimOp::HamtContains
        | CorePrimOp::TypeOf
        | CorePrimOp::IsInt
        | CorePrimOp::IsFloat
        | CorePrimOp::IsString
        | CorePrimOp::IsBool
        | CorePrimOp::IsArray
        | CorePrimOp::IsNone
        | CorePrimOp::IsSome
        | CorePrimOp::IsList
        | CorePrimOp::IsMap
        | CorePrimOp::Panic
        | CorePrimOp::Unwrap
        | CorePrimOp::ClockNow
        | CorePrimOp::Time
        | CorePrimOp::ParseInt
        | CorePrimOp::ParseInts
        | CorePrimOp::SplitInts
        | CorePrimOp::ToList
        | CorePrimOp::ToArray
        | CorePrimOp::Len
        | CorePrimOp::CmpEq
        | CorePrimOp::CmpNe
        | CorePrimOp::Try
        | CorePrimOp::AssertThrows => false,
        // Collection operations — pure (no I/O, no side effects)
        CorePrimOp::ArrayReverse
        | CorePrimOp::ArrayContains
        | CorePrimOp::Sort
        | CorePrimOp::SortBy
        | CorePrimOp::HoMap
        | CorePrimOp::HoFilter
        | CorePrimOp::HoFold
        | CorePrimOp::HoAny
        | CorePrimOp::HoAll
        | CorePrimOp::HoEach
        | CorePrimOp::HoFind
        | CorePrimOp::HoCount
        | CorePrimOp::Zip
        | CorePrimOp::Flatten
        | CorePrimOp::HoFlatMap => true,
        // Effect handler ops — not higher-order promoted
        CorePrimOp::EvvGet
        | CorePrimOp::EvvSet
        | CorePrimOp::FreshMarker
        | CorePrimOp::EvvInsert
        | CorePrimOp::YieldTo
        | CorePrimOp::YieldExtend
        | CorePrimOp::YieldPrompt
        | CorePrimOp::IsYielding
        | CorePrimOp::PerformDirect => false,
    }
}

/// Returns true when `var` appears free in `expr`.
pub(super) fn appears_free(var: CoreBinderId, expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { var: ref_var, .. } => ref_var.binder == Some(var),
        CoreExpr::Lit(_, _) => false,
        CoreExpr::Lam { params, body, .. } => {
            !params.iter().any(|p| p.id == var) && appears_free(var, body)
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            appears_free(var, func) || args.iter().any(|a| appears_free(var, a))
        }
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            ..
        } => appears_free(var, rhs) || (binding.id != var && appears_free(var, body)),
        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            ..
        } => binding.id != var && (appears_free(var, rhs) || appears_free(var, body)),
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            !bindings.iter().any(|(b, _)| b.id == var)
                && (bindings.iter().any(|(_, rhs)| appears_free(var, rhs))
                    || appears_free(var, body))
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            appears_free(var, scrutinee)
                || alts.iter().any(|alt| {
                    !pat_binds(&alt.pat, var)
                        && (alt.guard.as_ref().is_some_and(|g| appears_free(var, g))
                            || appears_free(var, &alt.rhs))
                })
        }
        CoreExpr::Con { fields, .. } => fields.iter().any(|f| appears_free(var, f)),
        CoreExpr::PrimOp { args, .. } => args.iter().any(|a| appears_free(var, a)),
        CoreExpr::Return { value, .. } => appears_free(var, value),
        CoreExpr::Perform { args, .. } => args.iter().any(|a| appears_free(var, a)),
        CoreExpr::Handle { body, handlers, .. } => {
            appears_free(var, body)
                || handlers.iter().any(|h| {
                    h.resume.id != var
                        && !h.params.iter().any(|p| p.id == var)
                        && appears_free(var, &h.body)
                })
        }
        CoreExpr::Dup {
            var: ref_var, body, ..
        }
        | CoreExpr::Drop {
            var: ref_var, body, ..
        } => ref_var.binder == Some(var) || appears_free(var, body),
        CoreExpr::Reuse { token, fields, .. } => {
            token.binder == Some(var) || fields.iter().any(|f| appears_free(var, f))
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            scrutinee.binder == Some(var)
                || appears_free(var, unique_body)
                || appears_free(var, shared_body)
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            appears_free(var, object)
        }
    }
}

/// Count the number of nodes in a `CoreExpr` (for size-based guards).
pub(super) fn expr_size(expr: &CoreExpr) -> usize {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => 1,
        CoreExpr::Lam { body, .. } => 1 + expr_size(body),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            1 + expr_size(func) + args.iter().map(expr_size).sum::<usize>()
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            1 + expr_size(rhs) + expr_size(body)
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            1 + bindings
                .iter()
                .map(|(_, rhs)| expr_size(rhs))
                .sum::<usize>()
                + expr_size(body)
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            1 + expr_size(scrutinee)
                + alts
                    .iter()
                    .map(|a| expr_size(&a.rhs) + a.guard.as_ref().map_or(0, expr_size))
                    .sum::<usize>()
        }
        CoreExpr::Con { fields, .. } => 1 + fields.iter().map(expr_size).sum::<usize>(),
        CoreExpr::PrimOp { args, .. } => 1 + args.iter().map(expr_size).sum::<usize>(),
        CoreExpr::Return { value, .. } => 1 + expr_size(value),
        CoreExpr::Perform { args, .. } => 1 + args.iter().map(expr_size).sum::<usize>(),
        CoreExpr::Handle { body, handlers, .. } => {
            1 + expr_size(body) + handlers.iter().map(|h| expr_size(&h.body)).sum::<usize>()
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => 1 + expr_size(body),
        CoreExpr::Reuse { fields, .. } => 1 + fields.iter().map(expr_size).sum::<usize>(),
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => 1 + expr_size(unique_body) + expr_size(shared_body),
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            1 + expr_size(object)
        }
    }
}

/// Returns true when pattern `pat` introduces a binding for `var`.
pub(super) fn pat_binds(pat: &CorePat, var: CoreBinderId) -> bool {
    match pat {
        CorePat::Var(binder) => binder.id == var,
        CorePat::Con { fields, .. } => fields.iter().any(|f| pat_binds(f, var)),
        CorePat::Tuple(fields) => fields.iter().any(|f| pat_binds(f, var)),
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => false,
    }
}
