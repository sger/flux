//! Which functions could have a consuming copy (proposal 0200).
//!
//! A function that only takes a parameter apart borrows it, so a caller holding the
//! last reference frees the value with a second walk. A clone in which the parameter
//! is owned frees each cell as it reads it. This module only decides where a clone
//! could help: a call site still has to hand over its last reference.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::core::{CoreBinderId, CoreDef, CoreExpr, CorePat, CoreProgram, CoreTopLevelItem};
use crate::syntax::{Identifier, interner::Interner};

use super::{analysis::use_counts, borrow_infer::BorrowMode};

/// E5. A clone copies the body, so a large one costs more code than the walk it
/// saves. To be set from measurements at C2.
const SIZE_LIMIT: usize = 200;

/// The candidate parameters of each function that has any.
pub type Candidates = HashMap<CoreBinderId, BTreeSet<usize>>;

/// A clone's name. `$` can't appear in a source name, and it survives into LLVM and
/// Mach-O symbols.
pub fn owned_clone_name(name: &str) -> String {
    format!("{name}$own")
}

/// The parameters that pass E1 to E5: the greatest fixpoint over `core`, so walkers
/// that call each other qualify together. `core` has had its borrow modes inferred.
pub fn candidates(core: &CoreProgram, interner: &Interner) -> Candidates {
    let effect_free = effect_free_functions(core);
    let mut found: Candidates = core
        .defs
        .iter()
        .filter(|def| effect_free.contains(&def.binder.id) && is_named_function(def, interner))
        .filter(|def| size(&def.expr) <= SIZE_LIMIT)
        .filter_map(|def| Some((def.binder.id, takes_apart_only(def)?)))
        .collect();

    // E3 refers to the set being built, so it is what shrinks it.
    loop {
        let mut removed = false;
        for def in &core.defs {
            let Some(positions) = found.get(&def.binder.id) else { continue };
            let CoreExpr::Lam { params, body, .. } = &def.expr else { continue };
            let kept: BTreeSet<usize> = positions
                .iter()
                .copied()
                .filter(|&index| hands_a_field_on(body, params[index].id, &found))
                .collect();
            if kept.len() != positions.len() {
                removed = true;
                if kept.is_empty() {
                    found.remove(&def.binder.id);
                } else {
                    found.insert(def.binder.id, kept);
                }
            }
        }
        if !removed {
            return found;
        }
    }
}

/// E1, for the function: top-level, named, and not `main` or a dictionary.
fn is_named_function(def: &CoreDef, interner: &Interner) -> bool {
    matches!(def.expr, CoreExpr::Lam { .. })
        && !def.is_anonymous
        && !def.is_dict_def
        && interner.resolve(def.name) != "main"
}

/// The parameters that pass the rest of E1, and E2: inferred Borrowed, reference
/// counted, and read only as the scrutinee of a `Case`.
fn takes_apart_only(def: &CoreDef) -> Option<BTreeSet<usize>> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else { return None };
    let signature = def.borrow_signature.as_ref()?;
    let positions: BTreeSet<usize> = params
        .iter()
        .enumerate()
        .filter(|(index, param)| {
            signature.params.get(*index) == Some(&BorrowMode::Borrowed)
                && param.rep.needs_rc()
                && !read_outside_a_scrutinee(body, param.id)
        })
        .map(|(index, _)| index)
        .collect();
    (!positions.is_empty()).then_some(positions)
}

fn read_outside_a_scrutinee(expr: &CoreExpr, param: CoreBinderId) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => var.binder == Some(param),
        CoreExpr::Case { scrutinee, alts, .. } if is_var(scrutinee, param) => {
            alts.iter().any(|alt| {
                alt.guard.as_ref().is_some_and(|guard| read_outside_a_scrutinee(guard, param))
                    || read_outside_a_scrutinee(&alt.rhs, param)
            })
        }
        _ => children(expr).into_iter().any(|child| read_outside_a_scrutinee(child, param)),
    }
}

/// E3: some arm on `param` with a flat pattern passes a field, used once in the arm,
/// to a candidate position.
fn hands_a_field_on(expr: &CoreExpr, param: CoreBinderId, found: &Candidates) -> bool {
    if let CoreExpr::Case { scrutinee, alts, .. } = expr
        && is_var(scrutinee, param)
        && alts.iter().any(|alt| {
            let used_once = fields_used_once(&alt.pat, alt.guard.as_ref(), &alt.rhs);
            !used_once.is_empty() && gives_to_a_candidate(&alt.rhs, &used_once, found)
        })
    {
        return true;
    }
    children(expr).into_iter().any(|child| hands_a_field_on(child, param, found))
}

/// The fields of a flat pattern, the only kind drop specialization splits, that the
/// arm uses once.
fn fields_used_once(
    pat: &CorePat,
    guard: Option<&CoreExpr>,
    rhs: &CoreExpr,
) -> HashSet<CoreBinderId> {
    let (CorePat::Con { fields, .. } | CorePat::Tuple(fields)) = pat else {
        return HashSet::new();
    };
    if !fields.iter().all(|field| matches!(field, CorePat::Var(_) | CorePat::Wildcard)) {
        return HashSet::new();
    }
    let in_rhs = use_counts(rhs);
    let in_guard = guard.map(use_counts).unwrap_or_default();
    fields
        .iter()
        .filter_map(|field| match field {
            CorePat::Var(binder) => Some(binder.id),
            _ => None,
        })
        .filter(|id| {
            in_rhs.get(id).copied().unwrap_or(0) + in_guard.get(id).copied().unwrap_or(0) == 1
        })
        .collect()
}

fn gives_to_a_candidate(
    expr: &CoreExpr,
    fields: &HashSet<CoreBinderId>,
    found: &Candidates,
) -> bool {
    if let CoreExpr::App { func, args, .. } = expr
        && let CoreExpr::Var { var, .. } = func.as_ref()
        && let Some(positions) = var.binder.and_then(|callee| found.get(&callee))
        && positions.iter().any(|&index| {
            matches!(
                args.get(index),
                Some(CoreExpr::Var { var, .. })
                    if var.binder.is_some_and(|id| fields.contains(&id))
            )
        })
    {
        return true;
    }
    children(expr).into_iter().any(|child| gives_to_a_candidate(child, fields, found))
}

/// E4: functions with no effect, lambda or local recursion, that call only primops
/// and each other. A clone then never meets a continuation. A call this can't name,
/// through a parameter, a dictionary or another module, could do anything.
fn effect_free_functions(core: &CoreProgram) -> HashSet<CoreBinderId> {
    let mut constructors = HashSet::new();
    collect_constructors(&core.top_level_items, &mut constructors);

    let mut callees: HashMap<CoreBinderId, HashSet<CoreBinderId>> = HashMap::new();
    for def in &core.defs {
        if let CoreExpr::Lam { body, .. } = &def.expr {
            let mut calls = HashSet::new();
            if collect_calls_if_effect_free(body, &constructors, &mut calls) {
                callees.insert(def.binder.id, calls);
            }
        }
    }
    loop {
        let free: HashSet<CoreBinderId> = callees.keys().copied().collect();
        callees.retain(|_, calls| calls.iter().all(|callee| free.contains(callee)));
        if callees.len() == free.len() {
            return free;
        }
    }
}

fn collect_constructors(items: &[CoreTopLevelItem], out: &mut HashSet<Identifier>) {
    for item in items {
        match item {
            CoreTopLevelItem::Data { variants, .. } => {
                out.extend(variants.iter().map(|variant| variant.name));
            }
            CoreTopLevelItem::Module { body, .. } => collect_constructors(body, out),
            _ => {}
        }
    }
}

fn collect_calls_if_effect_free(
    expr: &CoreExpr,
    constructors: &HashSet<Identifier>,
    calls: &mut HashSet<CoreBinderId>,
) -> bool {
    match expr {
        CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::Lam { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. } => return false,
        CoreExpr::App { func, .. } => match func.as_ref() {
            CoreExpr::Var { var, .. } => match var.binder {
                Some(callee) => {
                    calls.insert(callee);
                }
                None if constructors.contains(&var.name) => {}
                None => return false,
            },
            _ => return false,
        },
        _ => {}
    }
    children(expr).into_iter().all(|child| collect_calls_if_effect_free(child, constructors, calls))
}

fn is_var(expr: &CoreExpr, binder: CoreBinderId) -> bool {
    matches!(expr, CoreExpr::Var { var, .. } if var.binder == Some(binder))
}

fn size(expr: &CoreExpr) -> usize {
    1 + children(expr).into_iter().map(size).sum::<usize>()
}

fn children(expr: &CoreExpr) -> Vec<&CoreExpr> {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => Vec::new(),
        CoreExpr::Lam { body, .. } => vec![body],
        CoreExpr::App { func, args, .. } => std::iter::once(func.as_ref()).chain(args).collect(),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => vec![rhs, body],
        CoreExpr::LetRecGroup { bindings, body, .. } => bindings
            .iter()
            .map(|(_, rhs)| rhs.as_ref())
            .chain(std::iter::once(body.as_ref()))
            .collect(),
        CoreExpr::Case { scrutinee, alts, .. } => std::iter::once(scrutinee.as_ref())
            .chain(alts.iter().flat_map(|alt| alt.guard.iter().chain(std::iter::once(&alt.rhs))))
            .collect(),
        CoreExpr::Con { fields: args, .. }
        | CoreExpr::PrimOp { args, .. }
        | CoreExpr::Perform { args, .. } => args.iter().collect(),
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            vec![object]
        }
        CoreExpr::Return { value, .. } => vec![value],
        CoreExpr::Handle { body, parameter, handlers, .. } => std::iter::once(body.as_ref())
            .chain(parameter.as_deref())
            .chain(handlers.iter().map(|handler| &handler.body))
            .collect(),
    }
}
