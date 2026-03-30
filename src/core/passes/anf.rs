/// A-Normal Form (ANF) normalization pass.
///
/// Flattens nested subexpressions so every non-trivial intermediate value
/// is bound to a `Let`. After ANF, compound expressions (App, PrimOp, Con)
/// only contain *trivial* operands (Var or Lit).
///
/// ```text
/// Before:  PrimOp(Add, [App(f, [x]), Lit(1)])
/// After:   Let(t1, App(f, [x]),
///            Let(t2, PrimOp(Add, [Var(t1), Lit(1)]),
///              Var(t2)))
/// ```
///
/// Trivial expressions that are NOT let-bound:
/// - `Var`
/// - `Lit`
///
/// This simplifies the Core→CFG lowering (`to_ir.rs`) because each `Let`
/// maps directly to one IR instruction.
use crate::{
    core::{CoreBinder, CoreBinderId, CoreExpr, CorePrimOp, FluxRep},
    diagnostics::position::Span,
};

/// ANF-normalize a `CoreExpr`.
///
/// The `next_id` counter is used to allocate fresh `CoreBinderId`s for
/// synthetic let-bindings. It should start above the maximum binder ID
/// in the program.
pub fn anf_normalize(expr: CoreExpr, next_id: &mut u32) -> CoreExpr {
    anf_expr(expr, next_id)
}

/// Is this expression trivial (no need to let-bind)?
fn is_trivial(expr: &CoreExpr) -> bool {
    matches!(expr, CoreExpr::Var { .. } | CoreExpr::Lit(_, _))
}

/// Allocate a fresh binder for an ANF temporary with a known rep.
fn fresh_anf_binder(next_id: &mut u32, rep: FluxRep) -> CoreBinder {
    let id = *next_id;
    *next_id += 1;
    // Use the 4_000_000 range for ANF synthetic symbols.
    let sym = crate::syntax::symbol::Symbol::new(4_000_000 + id);
    CoreBinder::with_rep(CoreBinderId(id), sym, rep)
}

/// Ensure `expr` is trivial. If not, let-bind it and return the variable.
/// Accumulated bindings are pushed onto `bindings`.
fn anf_atom(
    expr: CoreExpr,
    next_id: &mut u32,
    bindings: &mut Vec<(CoreBinder, CoreExpr)>,
) -> CoreExpr {
    if is_trivial(&expr) {
        expr
    } else {
        let span = expr.span();
        let rep = rep_of_expr(&expr);
        let binder = fresh_anf_binder(next_id, rep);
        bindings.push((binder, expr));
        CoreExpr::bound_var(binder, span)
    }
}

/// Infer the `FluxRep` of a Core expression from its structure.
///
/// This is a best-effort analysis — typed primops give precise reps,
/// while generic operations and function calls fall back to `TaggedRep`.
fn rep_of_expr(expr: &CoreExpr) -> FluxRep {
    match expr {
        CoreExpr::Lit(lit, _) => match lit {
            crate::core::CoreLit::Int(_) => FluxRep::IntRep,
            crate::core::CoreLit::Float(_) => FluxRep::FloatRep,
            crate::core::CoreLit::Bool(_) => FluxRep::BoolRep,
            crate::core::CoreLit::String(_) => FluxRep::BoxedRep,
            crate::core::CoreLit::Unit => FluxRep::UnitRep,
        },
        CoreExpr::Var { var, .. } => {
            // If the variable has a resolved binder, we could look up its rep.
            // For now, fall back to TaggedRep.
            let _ = var;
            FluxRep::TaggedRep
        }
        CoreExpr::PrimOp { op, .. } => primop_result_rep(op),
        CoreExpr::Con { tag, .. } => {
            use crate::core::CoreTag;
            match tag {
                CoreTag::None | CoreTag::Nil => FluxRep::UnitRep,
                _ => FluxRep::BoxedRep,
            }
        }
        CoreExpr::Lam { .. } => FluxRep::BoxedRep,
        CoreExpr::Let { body, .. } | CoreExpr::LetRec { body, .. } => rep_of_expr(body),
        _ => FluxRep::TaggedRep,
    }
}

/// Determine the result representation of a primop.
pub fn primop_result_rep(op: &CorePrimOp) -> FluxRep {
    match op {
        // Typed integer arithmetic → IntRep
        CorePrimOp::IAdd
        | CorePrimOp::ISub
        | CorePrimOp::IMul
        | CorePrimOp::IDiv
        | CorePrimOp::IMod => FluxRep::IntRep,

        // Typed float arithmetic → FloatRep
        CorePrimOp::FAdd | CorePrimOp::FSub | CorePrimOp::FMul | CorePrimOp::FDiv => {
            FluxRep::FloatRep
        }

        // Comparisons → BoolRep
        CorePrimOp::Eq
        | CorePrimOp::NEq
        | CorePrimOp::Lt
        | CorePrimOp::Le
        | CorePrimOp::Gt
        | CorePrimOp::Ge
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
        | CorePrimOp::FCmpGe
        | CorePrimOp::And
        | CorePrimOp::Or
        | CorePrimOp::Not
        | CorePrimOp::StartsWith
        | CorePrimOp::EndsWith
        | CorePrimOp::StrContains => FluxRep::BoolRep,

        // String operations → BoxedRep
        CorePrimOp::Concat
        | CorePrimOp::Interpolate
        | CorePrimOp::StringConcat
        | CorePrimOp::StringSlice
        | CorePrimOp::ToString
        | CorePrimOp::Split
        | CorePrimOp::Join
        | CorePrimOp::Trim
        | CorePrimOp::Upper
        | CorePrimOp::Lower
        | CorePrimOp::Replace
        | CorePrimOp::Substring
        | CorePrimOp::Chars => FluxRep::BoxedRep,

        // Collection constructors → BoxedRep
        CorePrimOp::MakeList
        | CorePrimOp::MakeArray
        | CorePrimOp::MakeTuple
        | CorePrimOp::MakeHash => FluxRep::BoxedRep,

        // Array/HAMT operations that return collections → BoxedRep
        CorePrimOp::ArrayConcat
        | CorePrimOp::ArraySlice
        | CorePrimOp::ArrayPush
        | CorePrimOp::HamtSet
        | CorePrimOp::HamtDelete
        | CorePrimOp::HamtKeys
        | CorePrimOp::HamtValues => FluxRep::BoxedRep,

        // Length operations → IntRep
        CorePrimOp::StringLength | CorePrimOp::ArrayLen => FluxRep::IntRep,

        // I/O → UnitRep (print/println) or BoxedRep (read)
        CorePrimOp::Print | CorePrimOp::Println | CorePrimOp::WriteFile => FluxRep::UnitRep,
        CorePrimOp::ReadFile | CorePrimOp::ReadStdin | CorePrimOp::ReadLines => FluxRep::BoxedRep,

        // Polymorphic / unknown → TaggedRep
        _ => FluxRep::TaggedRep,
    }
}

/// Wrap a body expression with accumulated let-bindings (innermost last).
fn wrap_lets(bindings: Vec<(CoreBinder, CoreExpr)>, body: CoreExpr, span: Span) -> CoreExpr {
    bindings
        .into_iter()
        .rev()
        .fold(body, |acc, (binder, rhs)| CoreExpr::Let {
            var: binder,
            rhs: Box::new(rhs),
            body: Box::new(acc),
            span,
        })
}

/// Recursively ANF-normalize an expression.
fn anf_expr(expr: CoreExpr, next_id: &mut u32) -> CoreExpr {
    match expr {
        // Trivial — already in ANF.
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        // Lambda — normalize body only (params stay as-is).
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(anf_expr(*body, next_id)),
            span,
        },

        // Application — normalize func and each arg to atoms.
        CoreExpr::App { func, args, span } => {
            let mut bindings = Vec::new();
            let func = anf_expr(*func, next_id);
            let func = anf_atom(func, next_id, &mut bindings);
            let args: Vec<CoreExpr> = args
                .into_iter()
                .map(|a| {
                    let a = anf_expr(a, next_id);
                    anf_atom(a, next_id, &mut bindings)
                })
                .collect();
            let app = CoreExpr::App {
                func: Box::new(func),
                args,
                span,
            };
            wrap_lets(bindings, app, span)
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => {
            let mut bindings = Vec::new();
            let func = anf_expr(*func, next_id);
            let func = anf_atom(func, next_id, &mut bindings);
            let args: Vec<CoreExpr> = args
                .into_iter()
                .map(|a| {
                    let a = anf_expr(a, next_id);
                    anf_atom(a, next_id, &mut bindings)
                })
                .collect();
            let app = CoreExpr::AetherCall {
                func: Box::new(func),
                args,
                arg_modes,
                span,
            };
            wrap_lets(bindings, app, span)
        }

        // Let — normalize RHS and body.
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(anf_expr(*rhs, next_id)),
            body: Box::new(anf_expr(*body, next_id)),
            span,
        },

        // LetRec — normalize RHS and body.
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(anf_expr(*rhs, next_id)),
            body: Box::new(anf_expr(*body, next_id)),
            span,
        },

        // Case — normalize scrutinee to atom, normalize alt bodies.
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let mut bindings = Vec::new();
            let scrutinee = anf_expr(*scrutinee, next_id);
            let scrutinee = anf_atom(scrutinee, next_id, &mut bindings);
            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = anf_expr(alt.rhs, next_id);
                    alt.guard = alt.guard.map(|g| anf_expr(g, next_id));
                    alt
                })
                .collect();
            let case = CoreExpr::Case {
                scrutinee: Box::new(scrutinee),
                alts,
                span,
            };
            wrap_lets(bindings, case, span)
        }

        // Constructor — normalize fields to atoms.
        CoreExpr::Con { tag, fields, span } => {
            let mut bindings = Vec::new();
            let fields: Vec<CoreExpr> = fields
                .into_iter()
                .map(|f| {
                    let f = anf_expr(f, next_id);
                    anf_atom(f, next_id, &mut bindings)
                })
                .collect();
            let con = CoreExpr::Con { tag, fields, span };
            wrap_lets(bindings, con, span)
        }

        // PrimOp — normalize args to atoms.
        CoreExpr::PrimOp { op, args, span } => {
            let mut bindings = Vec::new();
            let args: Vec<CoreExpr> = args
                .into_iter()
                .map(|a| {
                    let a = anf_expr(a, next_id);
                    anf_atom(a, next_id, &mut bindings)
                })
                .collect();
            let primop = CoreExpr::PrimOp { op, args, span };
            wrap_lets(bindings, primop, span)
        }

        // Return — normalize value to atom.
        CoreExpr::Return { value, span } => {
            let mut bindings = Vec::new();
            let value = anf_expr(*value, next_id);
            let value = anf_atom(value, next_id, &mut bindings);
            let ret = CoreExpr::Return {
                value: Box::new(value),
                span,
            };
            wrap_lets(bindings, ret, span)
        }

        // Perform — normalize args to atoms.
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => {
            let mut bindings = Vec::new();
            let args: Vec<CoreExpr> = args
                .into_iter()
                .map(|a| {
                    let a = anf_expr(a, next_id);
                    anf_atom(a, next_id, &mut bindings)
                })
                .collect();
            let perform = CoreExpr::Perform {
                effect,
                operation,
                args,
                span,
            };
            wrap_lets(bindings, perform, span)
        }

        // Handle — normalize body and handler bodies.
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(anf_expr(*body, next_id)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = anf_expr(h.body, next_id);
                    h
                })
                .collect(),
            span,
        },

        // Dup/Drop — recurse into body.
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(anf_expr(*body, next_id)),
            span,
        },
        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(anf_expr(*body, next_id)),
            span,
        },

        // Reuse — normalize fields to atoms (same as Con), keep token as-is.
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => {
            let mut bindings = Vec::new();
            let fields: Vec<CoreExpr> = fields
                .into_iter()
                .map(|f| {
                    let f = anf_expr(f, next_id);
                    anf_atom(f, next_id, &mut bindings)
                })
                .collect();
            let reuse = CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                span,
            };
            wrap_lets(bindings, reuse, span)
        }

        // DropSpecialized — pass-through, recurse both branches.
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(anf_expr(*unique_body, next_id)),
            shared_body: Box::new(anf_expr(*shared_body, next_id)),
            span,
        },

        // MemberAccess — normalize object to atom.
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => {
            let mut bindings = Vec::new();
            let object = anf_expr(*object, next_id);
            let object = anf_atom(object, next_id, &mut bindings);
            let access = CoreExpr::MemberAccess {
                object: Box::new(object),
                member,
                span,
            };
            wrap_lets(bindings, access, span)
        }

        // TupleField — normalize object to atom.
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => {
            let mut bindings = Vec::new();
            let object = anf_expr(*object, next_id);
            let object = anf_atom(object, next_id, &mut bindings);
            let field = CoreExpr::TupleField {
                object: Box::new(object),
                index,
                span,
            };
            wrap_lets(bindings, field, span)
        }
    }
}
