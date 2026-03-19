use std::collections::HashMap;

use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CorePat, CoreProgram};
use crate::syntax::Identifier;

pub(super) type BinderScope = HashMap<Identifier, CoreBinderId>;

pub(super) fn resolve_program_binders(program: &mut CoreProgram) {
    let mut recursive_globals = BinderScope::new();
    for def in &program.defs {
        if !def.is_anonymous() && def.is_recursive {
            recursive_globals.insert(def.name, def.binder.id);
        }
    }
    let mut sequential_globals = BinderScope::new();
    for def in &mut program.defs {
        let mut scopes = vec![recursive_globals.clone(), sequential_globals.clone()];
        resolve_expr_binders(&mut def.expr, &mut scopes);
        if !def.is_anonymous() && !def.is_recursive {
            sequential_globals.insert(def.name, def.binder.id);
        }
    }
}

pub(super) fn validate_program_binders(program: &CoreProgram) -> bool {
    let mut recursive_globals = BinderScope::new();
    for def in &program.defs {
        if !def.is_anonymous() && def.is_recursive {
            recursive_globals.insert(def.name, def.binder.id);
        }
    }
    let mut sequential_globals = BinderScope::new();
    for def in &program.defs {
        let mut scopes = vec![recursive_globals.clone(), sequential_globals.clone()];
        if !validate_expr_binders(&def.expr, &mut scopes) {
            return false;
        }
        if !def.is_anonymous() && !def.is_recursive {
            sequential_globals.insert(def.name, def.binder.id);
        }
    }
    true
}

fn resolve_expr_binders(expr: &mut CoreExpr, scopes: &mut Vec<BinderScope>) {
    match expr {
        CoreExpr::Var { var, .. } => {
            var.binder = lookup_binder(scopes, var.name);
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            scopes.push(scope_for_binders(params));
            resolve_expr_binders(body, scopes);
            scopes.pop();
        }
        CoreExpr::App { func, args, .. } => {
            resolve_expr_binders(func, scopes);
            for arg in args {
                resolve_expr_binders(arg, scopes);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            resolve_expr_binders(rhs, scopes);
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            resolve_expr_binders(body, scopes);
            scopes.pop();
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            resolve_expr_binders(rhs, scopes);
            resolve_expr_binders(body, scopes);
            scopes.pop();
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            resolve_expr_binders(scrutinee, scopes);
            for alt in alts {
                let mut pattern_scope = BinderScope::new();
                collect_pattern_binders(&alt.pat, &mut pattern_scope);
                scopes.push(pattern_scope);
                if let Some(guard) = &mut alt.guard {
                    resolve_expr_binders(guard, scopes);
                }
                resolve_expr_binders(&mut alt.rhs, scopes);
                scopes.pop();
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                resolve_expr_binders(field, scopes);
            }
        }
        CoreExpr::Return { value, .. } => resolve_expr_binders(value, scopes),
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                resolve_expr_binders(arg, scopes);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            resolve_expr_binders(body, scopes);
            for handler in handlers {
                let mut handler_scope = scope_for_binders(&handler.params);
                handler_scope.insert(handler.resume.name, handler.resume.id);
                scopes.push(handler_scope);
                resolve_expr_binders(&mut handler.body, scopes);
                scopes.pop();
            }
        }
        CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
            var.binder = lookup_binder(scopes, var.name);
            resolve_expr_binders(body, scopes);
        }
        CoreExpr::Reuse {
            token, fields, ..
        } => {
            token.binder = lookup_binder(scopes, token.name);
            for field in fields {
                resolve_expr_binders(field, scopes);
            }
        }
    }
}

fn validate_expr_binders(expr: &CoreExpr, scopes: &mut Vec<BinderScope>) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => match (var.binder, lookup_binder(scopes, var.name)) {
            (Some(actual), Some(expected)) => actual == expected,
            (None, None) => true,
            _ => false,
        },
        CoreExpr::Lit(_, _) => true,
        CoreExpr::Lam { params, body, .. } => {
            scopes.push(scope_for_binders(params));
            let ok = validate_expr_binders(body, scopes);
            scopes.pop();
            ok
        }
        CoreExpr::App { func, args, .. } => {
            validate_expr_binders(func, scopes)
                && args.iter().all(|arg| validate_expr_binders(arg, scopes))
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            if !validate_expr_binders(rhs, scopes) {
                return false;
            }
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            let ok = validate_expr_binders(body, scopes);
            scopes.pop();
            ok
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            let ok = validate_expr_binders(rhs, scopes) && validate_expr_binders(body, scopes);
            scopes.pop();
            ok
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            if !validate_expr_binders(scrutinee, scopes) {
                return false;
            }
            alts.iter().all(|alt| {
                let mut pattern_scope = BinderScope::new();
                collect_pattern_binders(&alt.pat, &mut pattern_scope);
                scopes.push(pattern_scope);
                let guard_ok = alt
                    .guard
                    .as_ref()
                    .is_none_or(|guard| validate_expr_binders(guard, scopes));
                let rhs_ok = validate_expr_binders(&alt.rhs, scopes);
                scopes.pop();
                guard_ok && rhs_ok
            })
        }
        CoreExpr::Con { fields, .. } => fields
            .iter()
            .all(|field| validate_expr_binders(field, scopes)),
        CoreExpr::Return { value, .. } => validate_expr_binders(value, scopes),
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            args.iter().all(|arg| validate_expr_binders(arg, scopes))
        }
        CoreExpr::Handle { body, handlers, .. } => {
            if !validate_expr_binders(body, scopes) {
                return false;
            }
            handlers.iter().all(|handler| {
                let mut handler_scope = scope_for_binders(&handler.params);
                handler_scope.insert(handler.resume.name, handler.resume.id);
                scopes.push(handler_scope);
                let ok = validate_expr_binders(&handler.body, scopes);
                scopes.pop();
                ok
            })
        }
        CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
            let var_ok = match (var.binder, lookup_binder(scopes, var.name)) {
                (Some(actual), Some(expected)) => actual == expected,
                (None, None) => true,
                _ => false,
            };
            var_ok && validate_expr_binders(body, scopes)
        }
        CoreExpr::Reuse {
            token, fields, ..
        } => {
            let token_ok = match (token.binder, lookup_binder(scopes, token.name)) {
                (Some(actual), Some(expected)) => actual == expected,
                (None, None) => true,
                _ => false,
            };
            token_ok && fields.iter().all(|field| validate_expr_binders(field, scopes))
        }
    }
}

fn scope_for_binders(binders: &[CoreBinder]) -> BinderScope {
    let mut scope = BinderScope::new();
    for binder in binders {
        scope.insert(binder.name, binder.id);
    }
    scope
}

fn collect_pattern_binders(pat: &CorePat, scope: &mut BinderScope) {
    match pat {
        CorePat::Var(binder) => {
            scope.insert(binder.name, binder.id);
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                collect_pattern_binders(field, scope);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}

fn lookup_binder(scopes: &[BinderScope], name: Identifier) -> Option<CoreBinderId> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(&name).copied())
}
