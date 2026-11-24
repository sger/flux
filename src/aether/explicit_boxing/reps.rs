//! What `explicit_boxing` and its checker agree on: which words are raw Ints
//! and which positions take them.

use std::collections::HashSet;

use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CorePat, CorePrimOp, CoreProgram};

/// How a value is held: a raw Int, or the uniform word every other position
/// takes. Bool, Float and Unit binders count as uniform here, since only Ints
/// change representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rep {
    Raw,
    Uniform,
}

/// Whether `op` takes raw Ints.
pub(crate) fn takes_raw(op: CorePrimOp) -> bool {
    use CorePrimOp::*;
    matches!(
        op,
        IAdd | ISub
            | IMul
            | IDiv
            | IMod
            | ICmpEq
            | ICmpNe
            | ICmpLt
            | ICmpLe
            | ICmpGt
            | ICmpGe
            | BitAnd
            | BitOr
            | BitXor
            | BitShl
            | BitShr
            | BoxInt
    )
}

/// Whether `op` gives a raw Int.
pub(crate) fn gives_raw(op: CorePrimOp) -> bool {
    use CorePrimOp::*;
    matches!(
        op,
        IAdd | ISub
            | IMul
            | IDiv
            | IMod
            | BitAnd
            | BitOr
            | BitXor
            | BitShl
            | BitShr
            | UnboxInt
            | UnboxIntMove
    )
}

/// Whether `op` unboxes: its argument is a uniform variable, never a literal.
pub(crate) fn unboxes(op: CorePrimOp) -> bool {
    matches!(op, CorePrimOp::UnboxInt | CorePrimOp::UnboxIntMove)
}

/// The Int operation a generic operator stands for when both of its
/// arguments are Ints.
pub(crate) fn int_op(op: CorePrimOp) -> Option<CorePrimOp> {
    use CorePrimOp::*;
    Some(match op {
        Add => IAdd,
        Sub => ISub,
        Mul => IMul,
        Div => IDiv,
        Mod => IMod,
        Eq => ICmpEq,
        NEq => ICmpNe,
        Lt => ICmpLt,
        Le => ICmpLe,
        Gt => ICmpGt,
        Ge => ICmpGe,
        _ => return None,
    })
}

/// Whether a `Case` reads its scrutinee as a heap object: a constructor,
/// tuple or list pattern needs the uniform word.
pub(crate) fn matches_structure(alts: &[CoreAlt]) -> bool {
    alts.iter()
        .any(|alt| matches!(alt.pat, CorePat::Con { .. } | CorePat::Tuple(_) | CorePat::EmptyList))
}

/// The local variables `expr` reads but does not bind. Binder ids are unique
/// in a program, so a variable is free when no binder inside `expr` has its id.
pub(crate) fn free_locals(expr: &CoreExpr) -> HashSet<CoreBinderId> {
    let mut read = HashSet::new();
    let mut bound = HashSet::new();
    walk(expr, &mut |node| match node {
        Node::Read(id) => {
            read.insert(id);
        }
        Node::Bind(binder) => {
            bound.insert(binder.id);
        }
    });
    read.retain(|id| !bound.contains(id));
    read
}

/// One past the largest binder id anywhere in `program`, including handler
/// state binders and binders inside a handler's parameter expression.
pub(crate) fn next_binder_id(program: &CoreProgram) -> u32 {
    let mut max = 0;
    for def in &program.defs {
        max = max.max(def.binder.id.0);
        walk(&def.expr, &mut |node| {
            if let Node::Bind(binder) = node {
                max = max.max(binder.id.0);
            }
        });
    }
    max + 1
}

pub(crate) enum Node {
    Read(CoreBinderId),
    Bind(CoreBinder),
}

/// Visits every variable read and every binder in `expr`.
pub(crate) fn walk(expr: &CoreExpr, visit: &mut impl FnMut(Node)) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(id) = var.binder {
                visit(Node::Read(id));
            }
        }
        CoreExpr::Lit(..) => {}
        CoreExpr::Lam { params, body, .. } => {
            for param in params {
                visit(Node::Bind(*param));
            }
            walk(body, visit);
        }
        CoreExpr::App { func, args, .. } => {
            walk(func, visit);
            for arg in args {
                walk(arg, visit);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } | CoreExpr::LetRec { var, rhs, body, .. } => {
            visit(Node::Bind(*var));
            walk(rhs, visit);
            walk(body, visit);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (binder, rhs) in bindings {
                visit(Node::Bind(*binder));
                walk(rhs, visit);
            }
            walk(body, visit);
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            walk(scrutinee, visit);
            for alt in alts {
                walk_pat(&alt.pat, visit);
                if let Some(guard) = &alt.guard {
                    walk(guard, visit);
                }
                walk(&alt.rhs, visit);
            }
        }
        CoreExpr::Con { fields: args, .. }
        | CoreExpr::PrimOp { args, .. }
        | CoreExpr::Perform { args, .. } => {
            for arg in args {
                walk(arg, visit);
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            walk(object, visit);
        }
        CoreExpr::Return { value, .. } => walk(value, visit),
        CoreExpr::Handle { body, parameter, handlers, .. } => {
            walk(body, visit);
            if let Some(parameter) = parameter {
                walk(parameter, visit);
            }
            for handler in handlers {
                for param in &handler.params {
                    visit(Node::Bind(*param));
                }
                if let Some(state) = &handler.state {
                    visit(Node::Bind(*state));
                }
                visit(Node::Bind(handler.resume));
                walk(&handler.body, visit);
            }
        }
    }
}

pub(crate) fn walk_pat(pat: &CorePat, visit: &mut impl FnMut(Node)) {
    match pat {
        CorePat::Var(binder) => visit(Node::Bind(*binder)),
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                walk_pat(field, visit);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}
