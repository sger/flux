use std::collections::HashMap;

use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CorePat, CoreProgram};
use crate::syntax::Identifier;

pub(super) type BinderScope = HashMap<Identifier, CoreBinderId>;

/// Whether a name is a dictionary's (`__dict_*`). Lowering builds it from its
/// interner; without one, no name is.
pub(super) type IsDictName<'a> = &'a dyn Fn(Identifier) -> bool;

/// Bind every `Var` to the innermost binder of its name.
///
/// A dictionary variable lowering already bound keeps its binder. Evidence
/// names the definition a dictionary comes from, and a nested function's own
/// `__dict_Eq` parameter would otherwise capture a reference to the enclosing
/// definition's.
pub(super) fn resolve_program_binders(program: &mut CoreProgram, is_dict: IsDictName<'_>) {
    let mut recursive_globals = BinderScope::new();
    for def in &program.defs {
        if !def.is_anonymous() && def.is_recursive {
            recursive_globals.insert(def.name, def.binder.id);
        }
    }
    let mut sequential_globals = BinderScope::new();
    for def in &mut program.defs {
        let mut scopes = vec![recursive_globals.clone(), sequential_globals.clone()];
        resolve_expr_binders(&mut def.expr, &mut scopes, is_dict);
        if !def.is_anonymous() && !def.is_recursive {
            sequential_globals.insert(def.name, def.binder.id);
        }
    }
}

/// Check that every bound `Var` names the innermost binder of its name, or,
/// for a dictionary variable lowering bound itself, a binder of its name that
/// is in scope; and that no external `Var` is shadowed by a binder.
pub(super) fn validate_program_binders(program: &CoreProgram, is_dict: IsDictName<'_>) -> bool {
    let mut recursive_globals = BinderScope::new();
    for def in &program.defs {
        if !def.is_anonymous() && def.is_recursive {
            recursive_globals.insert(def.name, def.binder.id);
        }
    }
    let mut sequential_globals = BinderScope::new();
    for def in &program.defs {
        let mut scopes = vec![recursive_globals.clone(), sequential_globals.clone()];
        if !validate_expr_binders(&def.expr, &mut scopes, is_dict) {
            return false;
        }
        if !def.is_anonymous() && !def.is_recursive {
            sequential_globals.insert(def.name, def.binder.id);
        }
    }
    true
}

fn resolve_expr_binders(
    expr: &mut CoreExpr,
    scopes: &mut Vec<BinderScope>,
    is_dict: IsDictName<'_>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            // A bound dictionary variable is left for validation to check.
            if !(var.binder.is_some() && is_dict(var.name)) {
                var.binder = lookup_binder(scopes, var.name);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            scopes.push(scope_for_binders(params));
            resolve_expr_binders(body, scopes, is_dict);
            scopes.pop();
        }
        CoreExpr::App { func, args, .. } => {
            resolve_expr_binders(func, scopes, is_dict);
            for arg in args {
                resolve_expr_binders(arg, scopes, is_dict);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            resolve_expr_binders(rhs, scopes, is_dict);
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            resolve_expr_binders(body, scopes, is_dict);
            scopes.pop();
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            resolve_expr_binders(rhs, scopes, is_dict);
            resolve_expr_binders(body, scopes, is_dict);
            scopes.pop();
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            // All binders are in scope for ALL RHS and body (mutual recursion)
            let binder_scope: BinderScope = bindings.iter().map(|(b, _)| (b.name, b.id)).collect();
            scopes.push(binder_scope);
            for (_, rhs) in bindings {
                resolve_expr_binders(rhs, scopes, is_dict);
            }
            resolve_expr_binders(body, scopes, is_dict);
            scopes.pop();
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            resolve_expr_binders(scrutinee, scopes, is_dict);
            for alt in alts {
                let mut pattern_scope = BinderScope::new();
                collect_pattern_binders(&alt.pat, &mut pattern_scope);
                scopes.push(pattern_scope);
                if let Some(guard) = &mut alt.guard {
                    resolve_expr_binders(guard, scopes, is_dict);
                }
                resolve_expr_binders(&mut alt.rhs, scopes, is_dict);
                scopes.pop();
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                resolve_expr_binders(field, scopes, is_dict);
            }
        }
        CoreExpr::Return { value, .. } => resolve_expr_binders(value, scopes, is_dict),
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                resolve_expr_binders(arg, scopes, is_dict);
            }
        }
        CoreExpr::Handle { body, parameter, handlers, .. } => {
            if let Some(parameter) = parameter {
                resolve_expr_binders(parameter, scopes, is_dict);
            }
            resolve_expr_binders(body, scopes, is_dict);
            for handler in handlers {
                let mut handler_scope = scope_for_binders(&handler.params);
                handler_scope.insert(handler.resume.name, handler.resume.id);
                if let Some(state) = handler.state {
                    handler_scope.insert(state.name, state.id);
                }
                scopes.push(handler_scope);
                resolve_expr_binders(&mut handler.body, scopes, is_dict);
                scopes.pop();
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            resolve_expr_binders(object, scopes, is_dict);
        }
    }
}

fn validate_expr_binders(
    expr: &CoreExpr,
    scopes: &mut Vec<BinderScope>,
    is_dict: IsDictName<'_>,
) -> bool {
    match expr {
        CoreExpr::Var { var, .. } => match (var.binder, lookup_binder(scopes, var.name)) {
            (Some(actual), Some(expected)) => {
                actual == expected || (is_dict(var.name) && in_scope(scopes, var.name, actual))
            }
            (None, None) => true,
            _ => false,
        },
        CoreExpr::Lit(_, _) => true,
        CoreExpr::Lam { params, body, .. } => {
            scopes.push(scope_for_binders(params));
            let ok = validate_expr_binders(body, scopes, is_dict);
            scopes.pop();
            ok
        }
        CoreExpr::App { func, args, .. } => {
            validate_expr_binders(func, scopes, is_dict)
                && args.iter().all(|arg| validate_expr_binders(arg, scopes, is_dict))
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            if !validate_expr_binders(rhs, scopes, is_dict) {
                return false;
            }
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            let ok = validate_expr_binders(body, scopes, is_dict);
            scopes.pop();
            ok
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            scopes.push(scope_for_binders(std::slice::from_ref(var)));
            let ok = validate_expr_binders(rhs, scopes, is_dict)
                && validate_expr_binders(body, scopes, is_dict);
            scopes.pop();
            ok
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            let binder_scope: BinderScope = bindings.iter().map(|(b, _)| (b.name, b.id)).collect();
            scopes.push(binder_scope);
            let ok = bindings.iter().all(|(_, rhs)| validate_expr_binders(rhs, scopes, is_dict))
                && validate_expr_binders(body, scopes, is_dict);
            scopes.pop();
            ok
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            if !validate_expr_binders(scrutinee, scopes, is_dict) {
                return false;
            }
            alts.iter().all(|alt| {
                let mut pattern_scope = BinderScope::new();
                collect_pattern_binders(&alt.pat, &mut pattern_scope);
                scopes.push(pattern_scope);
                let guard_ok = alt
                    .guard
                    .as_ref()
                    .is_none_or(|guard| validate_expr_binders(guard, scopes, is_dict));
                let rhs_ok = validate_expr_binders(&alt.rhs, scopes, is_dict);
                scopes.pop();
                guard_ok && rhs_ok
            })
        }
        CoreExpr::Con { fields, .. } => {
            fields.iter().all(|field| validate_expr_binders(field, scopes, is_dict))
        }
        CoreExpr::Return { value, .. } => validate_expr_binders(value, scopes, is_dict),
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            args.iter().all(|arg| validate_expr_binders(arg, scopes, is_dict))
        }
        CoreExpr::Handle { body, parameter, handlers, .. } => {
            if parameter
                .as_ref()
                .is_some_and(|parameter| !validate_expr_binders(parameter, scopes, is_dict))
            {
                return false;
            }
            if !validate_expr_binders(body, scopes, is_dict) {
                return false;
            }
            handlers.iter().all(|handler| {
                let mut handler_scope = scope_for_binders(&handler.params);
                handler_scope.insert(handler.resume.name, handler.resume.id);
                if let Some(state) = handler.state {
                    handler_scope.insert(state.name, state.id);
                }
                scopes.push(handler_scope);
                let ok = validate_expr_binders(&handler.body, scopes, is_dict);
                scopes.pop();
                ok
            })
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            validate_expr_binders(object, scopes, is_dict)
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

/// Whether some scope binds `name` to `id`, shadowed or not.
fn in_scope(scopes: &[BinderScope], name: Identifier, id: CoreBinderId) -> bool {
    scopes.iter().any(|scope| scope.get(&name) == Some(&id))
}

fn lookup_binder(scopes: &[BinderScope], name: Identifier) -> Option<CoreBinderId> {
    scopes.iter().rev().find_map(|scope| scope.get(&name).copied())
}

#[cfg(test)]
mod tests {
    use super::{resolve_program_binders, validate_program_binders};
    use crate::core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    /// `fn outer(__dict_Eq#1) { fn(__dict_Eq#2, x#3) { <var> } }`, with `<var>`
    /// bound to `#1`: the enclosing definition's dictionary, shadowed by the
    /// inner one's of the same name.
    fn program_referencing_the_outer_binder(
        interner: &mut Interner,
        name: &str,
    ) -> (CoreProgram, CoreBinderId) {
        let span = Span::default();
        let name = interner.intern(name);
        let outer = CoreBinder::new(CoreBinderId(1), name);
        let inner = CoreBinder::new(CoreBinderId(2), name);
        let x = CoreBinder::new(CoreBinderId(3), interner.intern("x"));
        let inner_lam = CoreExpr::lambda(vec![inner, x], CoreExpr::bound_var(&outer, span), span);
        let def = CoreDef::new(
            CoreBinder::new(CoreBinderId(0), interner.intern("outer")),
            CoreExpr::lambda(vec![outer], inner_lam, span),
            false,
            span,
        );
        (CoreProgram { defs: vec![def], top_level_items: Vec::new() }, outer.id)
    }

    fn referenced_binder(program: &CoreProgram) -> Option<CoreBinderId> {
        let CoreExpr::Lam { body, .. } = &program.defs[0].expr else {
            panic!("outer is a lambda");
        };
        let CoreExpr::Lam { body, .. } = body.as_ref() else {
            panic!("its body is a lambda");
        };
        let CoreExpr::Var { var, .. } = body.as_ref() else {
            panic!("whose body is a variable");
        };
        var.binder
    }

    /// A dictionary variable lowering bound to the enclosing definition's
    /// dictionary keeps that binder, though an inner one has its name.
    #[test]
    fn a_bound_dictionary_variable_keeps_a_shadowed_binder() {
        let mut interner = Interner::new();
        let (mut program, outer) = program_referencing_the_outer_binder(&mut interner, "__dict_Eq");
        let is_dict = |name| {
            interner.try_resolve(name).is_some_and(crate::types::class_env::is_dictionary_name)
        };
        resolve_program_binders(&mut program, &is_dict);
        assert_eq!(referenced_binder(&program), Some(outer));
        assert!(validate_program_binders(&program, &is_dict));
    }

    /// Any other variable still names the innermost binder of its name, and
    /// one bound to a shadowed binder is rejected.
    #[test]
    fn an_ordinary_variable_is_bound_to_the_innermost_binder() {
        let mut interner = Interner::new();
        let (mut program, outer) = program_referencing_the_outer_binder(&mut interner, "y");
        let is_dict = |name| {
            interner.try_resolve(name).is_some_and(crate::types::class_env::is_dictionary_name)
        };
        assert!(!validate_program_binders(&program, &is_dict));
        resolve_program_binders(&mut program, &is_dict);
        assert_ne!(referenced_binder(&program), Some(outer));
        assert!(validate_program_binders(&program, &is_dict));
    }
}
