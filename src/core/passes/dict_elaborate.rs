/// Dictionary elaboration pass for type classes (Proposal 0145, Step 5b).
///
/// Transforms type class dispatch from runtime `type_of()` checks to
/// GHC-style compile-time dictionary passing.
///
/// ## What this pass does
///
/// 1. **Dictionary construction**: For each concrete instance in `ClassEnv`,
///    emits a top-level `CoreDef` named `__dict_{Class}_{Type}` containing
///    a tuple of references to mangled instance functions.
///
/// 2. **Dictionary parameter insertion**: For polymorphic functions whose
///    `Scheme` has class constraints, prepends dictionary parameters to
///    the function's `Lam` and rewrites class method calls in the body
///    to extract methods from the dictionary via `TupleField`.
///
/// 3. **Dictionary passing at call sites**: When calling a constrained
///    function, inserts the appropriate dictionary as an argument.
///
/// Monomorphic call sites (already resolved to `__tc_*` mangled names by
/// `try_resolve_class_call` during AST-to-Core lowering) are left unchanged.
use std::collections::HashMap;

use crate::{
    core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePrimOp, CoreProgram, FluxRep},
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
    types::{class_env::ClassEnv, type_env::TypeEnv},
};

/// Entry point for dictionary elaboration.
///
/// 1. Emits `__dict_{Class}_{Type}` CoreDefs for each concrete instance.
/// 2. For constrained functions (scheme has class constraints), prepends
///    dictionary parameters and rewrites class method calls to extract
///    from the dictionary.
/// 3. At call sites to constrained functions, inserts dictionary arguments.
pub fn elaborate_dictionaries(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    type_env: &TypeEnv,
    interner: &Interner,
    next_id: &mut u32,
) {
    if class_env.classes.is_empty() {
        return;
    }

    // Check if any function actually has class constraints in its scheme.
    // If not, skip all elaboration (avoids injecting __dict_* defs into
    // programs that don't use polymorphic type class dispatch).
    let has_constrained_fns = program.defs.iter().any(|def| {
        type_env
            .lookup(def.name)
            .is_some_and(|s| !s.constraints.is_empty())
    });

    if !has_constrained_fns {
        return;
    }

    // Phase 2: Build dictionary CoreDefs for each concrete instance.
    let dict_defs = build_instance_dictionaries(class_env, interner, next_id);

    // Phase 3: Rewrite constrained function bodies.
    rewrite_constrained_functions(program, class_env, type_env, interner, next_id);

    // Phase 4: Insert dictionary arguments at call sites.
    insert_dict_args_at_call_sites(program, class_env, type_env, interner);

    // Prepend dictionary defs so they are available to all subsequent defs.
    let mut new_defs = dict_defs;
    new_defs.append(&mut program.defs);
    program.defs = new_defs;
}

/// Build a `CoreDef` for each concrete instance in the class environment.
///
/// Each dictionary is a tuple of references to the mangled instance functions,
/// ordered by the method declaration order in the class definition.
///
/// Example: for `instance Eq<Int> { fn eq(...) { ... }; fn neq(...) { ... } }`,
/// produces:
/// ```text
/// __dict_Eq_Int = MakeTuple(Var(__tc_Eq_Int_eq), Var(__tc_Eq_Int_neq))
/// ```
fn build_instance_dictionaries(
    class_env: &ClassEnv,
    interner: &Interner,
    next_id: &mut u32,
) -> Vec<CoreDef> {
    let mut defs = Vec::new();
    let span = Span::default();

    for instance in &class_env.instances {
        let class_def = match class_env.lookup_class(instance.class_name) {
            Some(c) => c,
            None => continue,
        };

        // Compute the type name string for this instance.
        // Multi-param classes join all type args: "Int_String".
        if instance.type_args.is_empty() {
            continue;
        }
        let type_name = instance
            .type_args
            .iter()
            .map(|a| a.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");

        let class_str = interner.resolve(instance.class_name).to_string();

        // Build the dictionary name: __dict_{Class}_{Type}
        // These names are pre-interned during dispatch generation (Phase 1b).
        let dict_name_str = format!("__dict_{class_str}_{type_name}");
        let dict_name = match interner.lookup(&dict_name_str) {
            Some(sym) => sym,
            None => continue, // Not pre-interned — skip this instance.
        };

        let dict_expr = if instance.context.is_empty() {
            let mut tuple_fields = Vec::new();
            for method_sig in &class_def.methods {
                let method_str = interner.resolve(method_sig.name).to_string();
                let mangled_str = format!("__tc_{class_str}_{type_name}_{method_str}");
                let mangled_sym = match interner.lookup(&mangled_str) {
                    Some(sym) => sym,
                    None => continue,
                };

                tuple_fields.push(CoreExpr::external_var(mangled_sym, span));
            }

            CoreExpr::PrimOp {
                op: CorePrimOp::MakeTuple,
                args: tuple_fields,
                span,
            }
        } else {
            build_contextual_dictionary_expr(instance, class_def, interner, next_id)
        };

        // Create the CoreDef for this dictionary.
        let binder_id = *next_id;
        *next_id += 1;
        let binder = CoreBinder::with_rep(CoreBinderId(binder_id), dict_name, FluxRep::BoxedRep);

        defs.push(CoreDef {
            name: dict_name,
            binder,
            expr: dict_expr,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip: None,
            span,
        });
    }

    defs
}

fn build_contextual_dictionary_expr(
    instance: &crate::types::class_env::InstanceDef,
    class_def: &crate::types::class_env::ClassDef,
    interner: &Interner,
    next_id: &mut u32,
) -> CoreExpr {
    let span = Span::default();
    let class_str = interner.resolve(instance.class_name).to_string();
    let type_name = instance
        .type_args
        .iter()
        .map(|a| a.display_with(interner))
        .collect::<Vec<_>>()
        .join("_");

    let context_binders: Vec<CoreBinder> = instance
        .context
        .iter()
        .enumerate()
        .map(|(idx, constraint)| {
            let class_name = interner.resolve(constraint.class_name);
            let label = if idx == 0 {
                format!("__dict_{class_name}")
            } else {
                format!("__dict_{class_name}_{idx}")
            };
            let binder_id = *next_id;
            *next_id += 1;
            let binder_name = interner.lookup(&label).unwrap_or(constraint.class_name);
            CoreBinder::with_rep(CoreBinderId(binder_id), binder_name, FluxRep::BoxedRep)
        })
        .collect();

    let tuple_fields = class_def
        .methods
        .iter()
        .filter_map(|method_sig| {
            let method_str = interner.resolve(method_sig.name).to_string();
            let mangled_str = format!("__tc_{class_str}_{type_name}_{method_str}");
            let mangled_sym = interner.lookup(&mangled_str)?;
            Some(build_contextual_dictionary_method_closure(
                mangled_sym,
                method_sig.arity,
                &context_binders,
                interner,
                next_id,
            ))
        })
        .collect();

    let tuple = CoreExpr::PrimOp {
        op: CorePrimOp::MakeTuple,
        args: tuple_fields,
        span,
    };

    prepend_lam_params(tuple, context_binders)
}

fn build_contextual_dictionary_method_closure(
    mangled_sym: Identifier,
    arity: usize,
    context_binders: &[CoreBinder],
    interner: &Interner,
    next_id: &mut u32,
) -> CoreExpr {
    let span = Span::default();
    let user_params: Vec<CoreBinder> = (0..arity)
        .map(|idx| {
            let binder_id = *next_id;
            *next_id += 1;
            CoreBinder::with_rep(
                CoreBinderId(binder_id),
                interner.lookup(&format!("__x{idx}")).unwrap_or(mangled_sym),
                FluxRep::TaggedRep,
            )
        })
        .collect();
    let mut args: Vec<CoreExpr> = context_binders
        .iter()
        .map(|binder| CoreExpr::bound_var(*binder, span))
        .collect();
    args.extend(
        user_params
            .iter()
            .map(|binder| CoreExpr::bound_var(*binder, span)),
    );
    CoreExpr::Lam {
        params: user_params,
        body: Box::new(CoreExpr::App {
            func: Box::new(CoreExpr::external_var(mangled_sym, span)),
            args,
            span,
        }),
        span,
    }
}

/// Rewrite constrained functions to accept dictionary parameters and
/// extract methods from them instead of calling polymorphic stubs.
fn rewrite_constrained_functions(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    type_env: &TypeEnv,
    interner: &Interner,
    next_id: &mut u32,
) {
    for def in &mut program.defs {
        let scheme = match type_env.lookup(def.name) {
            Some(s) => s,
            None => continue,
        };

        if scheme.constraints.is_empty() {
            continue;
        }

        let existing_dict_params = match &def.expr {
            CoreExpr::Lam { params, .. }
                if params.len() >= scheme.constraints.len()
                    && params[..scheme.constraints.len()]
                        .iter()
                        .all(|binder| interner.resolve(binder.name).starts_with("__dict_")) =>
            {
                params[..scheme.constraints.len()].to_vec()
            }
            _ => Vec::new(),
        };

        // Build dictionary parameters and method map for this function.
        let mut dict_params: Vec<CoreBinder> = Vec::new();
        let mut method_map: HashMap<Identifier, (CoreBinder, usize)> = HashMap::new();

        for (index, constraint) in scheme.constraints.iter().enumerate() {
            let class_def = match class_env.lookup_class(constraint.class_name) {
                Some(c) => c,
                None => continue,
            };

            let dict_binder = if let Some(existing) = existing_dict_params.get(index).copied() {
                existing
            } else {
                let class_str = interner.resolve(constraint.class_name);
                let param_name_str = format!("__dict_{class_str}");
                let param_name = interner
                    .lookup(&param_name_str)
                    .unwrap_or(constraint.class_name);
                let binder_id = *next_id;
                *next_id += 1;
                let binder =
                    CoreBinder::with_rep(CoreBinderId(binder_id), param_name, FluxRep::BoxedRep);
                dict_params.push(binder);
                binder
            };

            // Map each method of this class to its tuple index + dict binder.
            for (idx, method_sig) in class_def.methods.iter().enumerate() {
                method_map.insert(method_sig.name, (dict_binder, idx));
            }
        }

        if dict_params.is_empty() {
            continue;
        }

        // Rewrite the function body to extract methods from dictionaries.
        let old_expr = std::mem::replace(
            &mut def.expr,
            CoreExpr::Lit(crate::core::CoreLit::Unit, Span::default()),
        );
        let rewritten = rewrite_body_with_dicts(old_expr, &method_map);

        if dict_params.is_empty() {
            def.expr = rewritten;
        } else {
            // Prepend dictionary params to the function's Lam.
            def.expr = prepend_lam_params(rewritten, dict_params);
        }
    }
}

/// Insert dictionary arguments at call sites to constrained functions.
///
/// For each `App(Var(f), args)` where `f` has class constraints in its scheme,
/// prepend the appropriate dictionary arguments. Two cases:
///
/// 1. **Monomorphic site**: The constraint's type is concrete → pass
///    `Var(__dict_{Class}_{Type})`.
/// 2. **Polymorphic forwarding**: The caller also has a dictionary param
///    for that class → pass the caller's dictionary through.
fn insert_dict_args_at_call_sites(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    type_env: &TypeEnv,
    interner: &Interner,
) {
    // Build a set of function names that have constraints.
    let constrained_fns: HashMap<
        Identifier,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    > = program
        .defs
        .iter()
        .filter_map(|def| {
            let scheme = type_env.lookup(def.name)?;
            if scheme.constraints.is_empty() {
                None
            } else {
                Some((def.name, scheme.constraints.clone()))
            }
        })
        .collect();

    if constrained_fns.is_empty() {
        return;
    }

    // For each def, build its own dict_param map (for polymorphic forwarding),
    // then walk its body to insert dict args at call sites.
    for def in &mut program.defs {
        // Build the caller's own dict_param map (if it's a constrained function).
        let caller_dicts: HashMap<Identifier, CoreBinder> =
            if let Some(scheme) = type_env.lookup(def.name) {
                build_caller_dict_map(&def.expr, &scheme.constraints)
            } else {
                HashMap::new()
            };

        let old_expr = std::mem::replace(
            &mut def.expr,
            CoreExpr::Lit(crate::core::CoreLit::Unit, Span::default()),
        );
        def.expr = insert_dict_args_expr(
            old_expr,
            &constrained_fns,
            &caller_dicts,
            class_env,
            interner,
        );
    }
}

/// Extract dictionary parameter binders from a function's outermost Lam.
///
/// If the function has constraints, its Lam starts with `__dict_*` params
/// (prepended by Phase 3). This maps each class name to the corresponding
/// binder so we can forward them to callee functions.
fn build_caller_dict_map(
    expr: &CoreExpr,
    constraints: &[crate::ast::type_infer::constraint::SchemeConstraint],
) -> HashMap<Identifier, CoreBinder> {
    let mut map = HashMap::new();
    if constraints.is_empty() {
        return map;
    }
    if let CoreExpr::Lam { params, .. } = expr {
        // The first N params are dictionary params (one per constraint).
        for (i, constraint) in constraints.iter().enumerate() {
            if let Some(binder) = params.get(i) {
                map.insert(constraint.class_name, *binder);
            }
        }
    }
    map
}

fn insert_dict_args_expr(
    expr: CoreExpr,
    constrained_fns: &HashMap<
        Identifier,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    >,
    caller_dicts: &HashMap<Identifier, CoreBinder>,
    class_env: &ClassEnv,
    interner: &Interner,
) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            // Check if the callee is a constrained function.
            if let CoreExpr::Var { ref var, .. } = *func
                && let Some(callee_constraints) = constrained_fns.get(&var.name)
            {
                // Build dictionary arguments for the callee.
                let mut dict_args = Vec::new();
                for constraint in callee_constraints {
                    if let Some(dict_arg) =
                        resolve_dict_arg(constraint, caller_dicts, class_env, interner, span)
                    {
                        dict_args.push(dict_arg);
                    }
                }

                if !dict_args.is_empty() {
                    // Prepend dict args before the original args.
                    let mut all_args = dict_args;
                    all_args.extend(args.into_iter().map(|a| {
                        insert_dict_args_expr(a, constrained_fns, caller_dicts, class_env, interner)
                    }));
                    return CoreExpr::App {
                        func,
                        args: all_args,
                        span,
                    };
                }
            }
            // Not a constrained call — recurse normally.
            CoreExpr::App {
                func: Box::new(insert_dict_args_expr(
                    *func,
                    constrained_fns,
                    caller_dicts,
                    class_env,
                    interner,
                )),
                args: args
                    .into_iter()
                    .map(|a| {
                        insert_dict_args_expr(a, constrained_fns, caller_dicts, class_env, interner)
                    })
                    .collect(),
                span,
            }
        }

        // Recursive cases — same structure as rewrite_expr but threading
        // different context.
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(insert_dict_args_expr(
                *func,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            args: args
                .into_iter()
                .map(|a| {
                    insert_dict_args_expr(a, constrained_fns, caller_dicts, class_env, interner)
                })
                .collect(),
            arg_modes,
            span,
        },

        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(insert_dict_args_expr(
                *rhs,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(insert_dict_args_expr(
                *rhs,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(b, rhs)| {
                    (
                        b,
                        Box::new(insert_dict_args_expr(
                            *rhs,
                            constrained_fns,
                            caller_dicts,
                            class_env,
                            interner,
                        )),
                    )
                })
                .collect(),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(insert_dict_args_expr(
                *scrutinee,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = insert_dict_args_expr(
                        alt.rhs,
                        constrained_fns,
                        caller_dicts,
                        class_env,
                        interner,
                    );
                    alt.guard = alt.guard.map(|g| {
                        insert_dict_args_expr(g, constrained_fns, caller_dicts, class_env, interner)
                    });
                    alt
                })
                .collect(),
            span,
        },

        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| {
                    insert_dict_args_expr(f, constrained_fns, caller_dicts, class_env, interner)
                })
                .collect(),
            span,
        },

        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| {
                    insert_dict_args_expr(a, constrained_fns, caller_dicts, class_env, interner)
                })
                .collect(),
            span,
        },

        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(insert_dict_args_expr(
                *value,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
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
                .map(|a| {
                    insert_dict_args_expr(a, constrained_fns, caller_dicts, class_env, interner)
                })
                .collect(),
            span,
        },

        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = insert_dict_args_expr(
                        h.body,
                        constrained_fns,
                        caller_dicts,
                        class_env,
                        interner,
                    );
                    h
                })
                .collect(),
            span,
        },

        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
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
                .map(|f| {
                    insert_dict_args_expr(f, constrained_fns, caller_dicts, class_env, interner)
                })
                .collect(),
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
            unique_body: Box::new(insert_dict_args_expr(
                *unique_body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            shared_body: Box::new(insert_dict_args_expr(
                *shared_body,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(insert_dict_args_expr(
                *object,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            member,
            span,
        },

        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(insert_dict_args_expr(
                *object,
                constrained_fns,
                caller_dicts,
                class_env,
                interner,
            )),
            index,
            span,
        },
    }
}

/// Resolve a dictionary argument for a callee's constraint.
///
/// 1. If the caller has a dictionary for the same class, forward it.
/// 2. Otherwise, try to find a concrete `__dict_{Class}_{Type}` reference.
fn resolve_dict_arg(
    constraint: &crate::ast::type_infer::constraint::SchemeConstraint,
    caller_dicts: &HashMap<Identifier, CoreBinder>,
    _class_env: &ClassEnv,
    interner: &Interner,
    span: Span,
) -> Option<CoreExpr> {
    // Case 1: Polymorphic forwarding — caller has a dict for this class.
    if let Some(&binder) = caller_dicts.get(&constraint.class_name) {
        return Some(CoreExpr::bound_var(binder, span));
    }

    // Case 2: For now, we don't have enough type info at this stage
    // to determine which concrete dictionary to pass. This will be
    // resolved when we thread type info from AST-to-Core lowering.
    // For now, skip (the polymorphic stub still handles the call).
    //
    // TODO: When type info is available (e.g., from hm_expr_types),
    // resolve to Var(__dict_{Class}_{Type}).
    let _ = (interner, span);
    None
}

/// Prepend extra parameters to the outermost `Lam` of an expression.
/// If the expression is not a `Lam`, wrap it in one.
fn prepend_lam_params(expr: CoreExpr, extra_params: Vec<CoreBinder>) -> CoreExpr {
    match expr {
        CoreExpr::Lam {
            mut params,
            body,
            span,
        } => {
            let mut new_params = extra_params;
            new_params.append(&mut params);
            CoreExpr::Lam {
                params: new_params,
                body,
                span,
            }
        }
        other => {
            // Non-lambda constrained def (unlikely, but handle gracefully).
            CoreExpr::Lam {
                params: extra_params,
                body: Box::new(other),
                span: Span::default(),
            }
        }
    }
}

/// Look up the dictionary name for a concrete class+type combination.
///
/// Returns the interned symbol `__dict_{Class}_{Type}` if it exists.
pub fn dict_name_for(
    class_name: Identifier,
    type_name: &str,
    interner: &Interner,
) -> Option<Identifier> {
    let class_str = interner.resolve(class_name);
    let dict_str = format!("__dict_{class_str}_{type_name}");
    interner.lookup(&dict_str)
}

/// Rewrite a polymorphic function body to extract class methods from
/// dictionary parameters instead of calling the polymorphic stub.
///
/// `method_map` maps class method names to `(dict_param_binder, tuple_index)`.
pub fn rewrite_body_with_dicts(
    expr: CoreExpr,
    method_map: &HashMap<Identifier, (CoreBinder, usize)>,
) -> CoreExpr {
    rewrite_expr(expr, method_map)
}

fn rewrite_expr(expr: CoreExpr, method_map: &HashMap<Identifier, (CoreBinder, usize)>) -> CoreExpr {
    match expr {
        // Key case: App where the function is a class method reference.
        // Rewrite: App(Var(eq), args) → App(TupleField(Var(dict), idx), args)
        CoreExpr::App { func, args, span } => {
            if let CoreExpr::Var { ref var, .. } = *func
                && let Some(&(dict_binder, index)) = method_map.get(&var.name)
            {
                // Class method reference — extract from dictionary.
                let dict_ref = CoreExpr::bound_var(dict_binder, span);
                let method_extract = CoreExpr::TupleField {
                    object: Box::new(dict_ref),
                    index,
                    span,
                };
                let rewritten_args = args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map))
                    .collect();
                return CoreExpr::App {
                    func: Box::new(method_extract),
                    args: rewritten_args,
                    span,
                };
            }
            // Not a class method — recurse normally.
            CoreExpr::App {
                func: Box::new(rewrite_expr(*func, method_map)),
                args: args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map))
                    .collect(),
                span,
            }
        }

        // Recursive cases for all other expression forms.
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(rewrite_expr(*body, method_map)),
            span,
        },

        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => {
            // Same method extraction as the App case — AetherCall is produced
            // by the Aether RC pass from App nodes.
            if let CoreExpr::Var { ref var, .. } = *func
                && let Some(&(dict_binder, index)) = method_map.get(&var.name)
            {
                let dict_ref = CoreExpr::bound_var(dict_binder, span);
                let method_extract = CoreExpr::TupleField {
                    object: Box::new(dict_ref),
                    index,
                    span,
                };
                let rewritten_args = args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map))
                    .collect();
                return CoreExpr::AetherCall {
                    func: Box::new(method_extract),
                    args: rewritten_args,
                    arg_modes,
                    span,
                };
            }
            CoreExpr::AetherCall {
                func: Box::new(rewrite_expr(*func, method_map)),
                args: args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map))
                    .collect(),
                arg_modes,
                span,
            }
        }

        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(rewrite_expr(*rhs, method_map)),
            body: Box::new(rewrite_expr(*body, method_map)),
            span,
        },

        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_expr(*rhs, method_map)),
            body: Box::new(rewrite_expr(*body, method_map)),
            span,
        },

        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(b, rhs)| (b, Box::new(rewrite_expr(*rhs, method_map))))
                .collect(),
            body: Box::new(rewrite_expr(*body, method_map)),
            span,
        },

        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_expr(*scrutinee, method_map)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = rewrite_expr(alt.rhs, method_map);
                    alt.guard = alt.guard.map(|g| rewrite_expr(g, method_map));
                    alt
                })
                .collect(),
            span,
        },

        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_expr(f, method_map))
                .collect(),
            span,
        },

        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, method_map))
                .collect(),
            span,
        },

        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_expr(*value, method_map)),
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
                .map(|a| rewrite_expr(a, method_map))
                .collect(),
            span,
        },

        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_expr(*body, method_map)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_expr(h.body, method_map);
                    h
                })
                .collect(),
            span,
        },

        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(rewrite_expr(*body, method_map)),
            span,
        },

        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(rewrite_expr(*body, method_map)),
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
                .map(|f| rewrite_expr(f, method_map))
                .collect(),
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
            unique_body: Box::new(rewrite_expr(*unique_body, method_map)),
            shared_body: Box::new(rewrite_expr(*shared_body, method_map)),
            span,
        },

        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(rewrite_expr(*object, method_map)),
            member,
            span,
        },

        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(rewrite_expr(*object, method_map)),
            index,
            span,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::type_infer::constraint::SchemeConstraint,
        core::{CoreLit, CoreVarRef, FluxRep},
        syntax::type_expr::TypeExpr,
        types::{
            class_env::{ClassDef, InstanceDef, MethodSig},
            scheme::Scheme,
        },
    };

    fn s() -> Span {
        Span::default()
    }

    fn mk_binder(id: u32, name: Identifier) -> CoreBinder {
        CoreBinder::with_rep(CoreBinderId(id), name, FluxRep::BoxedRep)
    }

    /// Build a minimal ClassEnv with one class and one instance.
    fn build_eq_class_env(interner: &mut Interner) -> ClassEnv {
        let eq_sym = interner.intern("Eq");
        let a_sym = interner.intern("a");
        let eq_method = interner.intern("eq");
        let neq_method = interner.intern("neq");
        let int_sym = interner.intern("Int");

        // Pre-intern mangled names and dict name (normally done by Phase 1b).
        interner.intern("__tc_Eq_Int_eq");
        interner.intern("__tc_Eq_Int_neq");
        interner.intern("__dict_Eq_Int");

        let bool_type = TypeExpr::Named {
            name: interner.intern("Bool"),
            args: vec![],
            span: s(),
        };
        let a_type = TypeExpr::Named {
            name: a_sym,
            args: vec![],
            span: s(),
        };

        let class_def = ClassDef {
            name: eq_sym,
            // Test fixture: synthetic built-in-style class with no owning module.
            module: crate::types::class_id::ModulePath::EMPTY,
            type_params: vec![a_sym],
            superclasses: vec![],
            methods: vec![
                MethodSig {
                    name: eq_method,
                    type_params: vec![],
                    param_types: vec![a_type.clone(), a_type.clone()],
                    return_type: bool_type.clone(),
                    arity: 2,
                },
                MethodSig {
                    name: neq_method,
                    type_params: vec![],
                    param_types: vec![a_type.clone(), a_type],
                    return_type: bool_type,
                    arity: 2,
                },
            ],
            default_methods: vec![neq_method],
            span: s(),
        };

        let instance_def = InstanceDef {
            class_name: eq_sym,
            class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
            instance_module: crate::types::class_id::ModulePath::EMPTY,
            type_args: vec![TypeExpr::Named {
                name: int_sym,
                args: vec![],
                span: s(),
            }],
            context: vec![],
            method_names: vec![eq_method, neq_method],
            span: s(),
        };

        let mut env = ClassEnv::new();
        env.classes.insert(
            crate::types::class_id::ClassId::from_local_name(eq_sym),
            class_def,
        );
        env.instances.push(instance_def);
        env
    }

    // ── build_instance_dictionaries ──────────────────────────────────────

    #[test]
    fn build_dict_emits_one_def_per_instance() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let mut next_id = 100;

        let defs = build_instance_dictionaries(&class_env, &interner, &mut next_id);

        assert_eq!(defs.len(), 1);
        let dict_name = interner.resolve(defs[0].name);
        assert_eq!(dict_name, "__dict_Eq_Int");
    }

    #[test]
    fn build_dict_uses_make_tuple_primop() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let mut next_id = 100;

        let defs = build_instance_dictionaries(&class_env, &interner, &mut next_id);

        match &defs[0].expr {
            CoreExpr::PrimOp { op, args, .. } => {
                assert!(matches!(op, CorePrimOp::MakeTuple));
                assert_eq!(args.len(), 2, "Eq has 2 methods → 2 tuple fields");
            }
            other => panic!("expected PrimOp(MakeTuple), got {other:?}"),
        }
    }

    #[test]
    fn build_dict_references_mangled_instance_functions() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let mut next_id = 100;

        let defs = build_instance_dictionaries(&class_env, &interner, &mut next_id);

        if let CoreExpr::PrimOp { args, .. } = &defs[0].expr {
            let names: Vec<String> = args
                .iter()
                .map(|a| match a {
                    CoreExpr::Var { var, .. } => interner.resolve(var.name).to_string(),
                    other => panic!("expected Var, got {other:?}"),
                })
                .collect();
            assert_eq!(names, vec!["__tc_Eq_Int_eq", "__tc_Eq_Int_neq"]);
        }
    }

    #[test]
    fn build_dict_allocates_fresh_binder_id() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let mut next_id = 42;

        let defs = build_instance_dictionaries(&class_env, &interner, &mut next_id);

        assert_eq!(defs[0].binder.id.0, 42);
        assert_eq!(next_id, 43);
    }

    #[test]
    fn build_dict_skips_instance_when_dict_name_not_interned() {
        let mut interner = Interner::new();
        let eq_sym = interner.intern("Eq");
        let a_sym = interner.intern("a");
        let eq_method = interner.intern("eq");
        let float_sym = interner.intern("Float");

        // Do NOT intern __dict_Eq_Float — simulating missing Phase 1b.
        interner.intern("__tc_Eq_Float_eq");

        let class_def = ClassDef {
            name: eq_sym,
            module: crate::types::class_id::ModulePath::EMPTY,
            type_params: vec![a_sym],
            superclasses: vec![],
            methods: vec![MethodSig {
                name: eq_method,
                type_params: vec![],
                param_types: vec![],
                return_type: TypeExpr::Named {
                    name: a_sym,
                    args: vec![],
                    span: s(),
                },
                arity: 1,
            }],
            default_methods: vec![],
            span: s(),
        };

        let instance_def = InstanceDef {
            class_name: eq_sym,
            class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
            instance_module: crate::types::class_id::ModulePath::EMPTY,
            type_args: vec![TypeExpr::Named {
                name: float_sym,
                args: vec![],
                span: s(),
            }],
            context: vec![],
            method_names: vec![eq_method],
            span: s(),
        };

        let mut env = ClassEnv::new();
        env.classes.insert(
            crate::types::class_id::ClassId::from_local_name(eq_sym),
            class_def,
        );
        env.instances.push(instance_def);

        let mut next_id = 0;
        let defs = build_instance_dictionaries(&env, &interner, &mut next_id);
        assert!(
            defs.is_empty(),
            "should skip when __dict_ name not pre-interned"
        );
    }

    // ── method_index ─────────────────────────────────────────────────────

    #[test]
    fn method_index_returns_declaration_order() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let eq_sym = interner.lookup("Eq").unwrap();
        let eq_method = interner.lookup("eq").unwrap();
        let neq_method = interner.lookup("neq").unwrap();

        assert_eq!(class_env.method_index(eq_sym, eq_method), Some(0));
        assert_eq!(class_env.method_index(eq_sym, neq_method), Some(1));
    }

    #[test]
    fn method_index_returns_none_for_unknown() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let eq_sym = interner.lookup("Eq").unwrap();
        let bogus = interner.intern("nonexistent");

        assert_eq!(class_env.method_index(eq_sym, bogus), None);
    }

    // ── rewrite_body_with_dicts ──────────────────────────────────────────

    #[test]
    fn rewrite_replaces_class_method_call_with_tuple_field() {
        let mut interner = Interner::new();
        let eq_method = interner.intern("eq");
        let x_name = interner.intern("x");
        let y_name = interner.intern("y");

        let dict_binder = mk_binder(50, interner.intern("__dict_Eq"));
        let x_binder = mk_binder(1, x_name);
        let y_binder = mk_binder(2, y_name);

        let mut method_map = HashMap::new();
        method_map.insert(eq_method, (dict_binder, 0_usize));

        // Build: App(Var(eq), [Var(x), Var(y)])
        let expr = CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef::unresolved(eq_method),
                span: s(),
            }),
            args: vec![
                CoreExpr::bound_var(x_binder, s()),
                CoreExpr::bound_var(y_binder, s()),
            ],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map);

        // Expected: App(TupleField(Var(dict), 0), [Var(x), Var(y)])
        match rewritten {
            CoreExpr::App { func, args, .. } => {
                match *func {
                    CoreExpr::TupleField { object, index, .. } => {
                        assert_eq!(index, 0);
                        match *object {
                            CoreExpr::Var { var, .. } => {
                                assert_eq!(var.binder, Some(CoreBinderId(50)));
                            }
                            other => panic!("expected Var(dict), got {other:?}"),
                        }
                    }
                    other => panic!("expected TupleField, got {other:?}"),
                }
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected App, got {other:?}"),
        }
    }

    #[test]
    fn rewrite_leaves_non_class_calls_unchanged() {
        let mut interner = Interner::new();
        let println_name = interner.intern("println");
        let x_binder = mk_binder(1, interner.intern("x"));

        let method_map = HashMap::new(); // No class methods.

        let expr = CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef::unresolved(println_name),
                span: s(),
            }),
            args: vec![CoreExpr::bound_var(x_binder, s())],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map);

        // Should remain App(Var(println), [Var(x)]) — unchanged.
        match rewritten {
            CoreExpr::App { func, .. } => match *func {
                CoreExpr::Var { var, .. } => {
                    assert_eq!(interner.resolve(var.name), "println");
                }
                other => panic!("expected Var(println), got {other:?}"),
            },
            other => panic!("expected App, got {other:?}"),
        }
    }

    #[test]
    fn rewrite_replaces_bound_vars_matching_method_name() {
        // Class method references in constrained function bodies are always
        // rewritten to dict extraction, regardless of binder status.
        // resolve_program_binders may have set the binder to the polymorphic
        // stub, but dict elaboration overrides it with TupleField extraction.
        let mut interner = Interner::new();
        let eq_method = interner.intern("eq");

        let dict_binder = mk_binder(50, interner.intern("__dict_Eq"));
        let local_eq = mk_binder(99, eq_method);

        let mut method_map = HashMap::new();
        method_map.insert(eq_method, (dict_binder, 0));

        // App(Var(eq, binder=99), [Lit(1)]) — bound var with class method name.
        let expr = CoreExpr::App {
            func: Box::new(CoreExpr::bound_var(local_eq, s())),
            args: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map);

        // SHOULD be rewritten — dict elaboration rewrites by name match.
        match rewritten {
            CoreExpr::App { func, .. } => match *func {
                CoreExpr::TupleField { index, .. } => {
                    assert_eq!(index, 0);
                }
                other => panic!("expected TupleField, got {other:?}"),
            },
            other => panic!("expected App, got {other:?}"),
        }
    }

    // ── prepend_lam_params ───────────────────────────────────────────────

    #[test]
    fn prepend_lam_params_adds_to_existing_lam() {
        let mut interner = Interner::new();
        let dict = mk_binder(10, interner.intern("__dict"));
        let x = mk_binder(1, interner.intern("x"));

        let lam = CoreExpr::Lam {
            params: vec![x],
            body: Box::new(CoreExpr::Lit(CoreLit::Unit, s())),
            span: s(),
        };

        let result = prepend_lam_params(lam, vec![dict]);

        match result {
            CoreExpr::Lam { params, .. } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].id.0, 10, "dict should be first");
                assert_eq!(params[1].id.0, 1, "x should be second");
            }
            other => panic!("expected Lam, got {other:?}"),
        }
    }

    #[test]
    fn prepend_lam_params_wraps_non_lam() {
        let mut interner = Interner::new();
        let dict = mk_binder(10, interner.intern("__dict"));

        let lit = CoreExpr::Lit(CoreLit::Int(42), s());
        let result = prepend_lam_params(lit, vec![dict]);

        match result {
            CoreExpr::Lam { params, body, .. } => {
                assert_eq!(params.len(), 1);
                assert!(matches!(*body, CoreExpr::Lit(CoreLit::Int(42), _)));
            }
            other => panic!("expected Lam, got {other:?}"),
        }
    }

    // ── elaborate_dictionaries (integration) ─────────────────────────────

    #[test]
    fn elaborate_skips_when_no_constrained_functions() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let type_env = TypeEnv::new();

        let main_name = interner.intern("main");
        let main_binder = mk_binder(0, main_name);
        let mut program = CoreProgram {
            defs: vec![CoreDef {
                name: main_name,
                binder: main_binder,
                expr: CoreExpr::Lit(CoreLit::Int(0), s()),
                borrow_signature: None,
                result_ty: None,
                is_anonymous: false,
                is_recursive: false,
                fip: None,
                span: s(),
            }],
            top_level_items: vec![],
        };

        let original_len = program.defs.len();
        let mut next_id = 10;
        elaborate_dictionaries(&mut program, &class_env, &type_env, &interner, &mut next_id);

        // No constrained functions → no dict defs added.
        assert_eq!(program.defs.len(), original_len);
    }

    #[test]
    fn elaborate_adds_dict_defs_when_constrained_function_exists() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let eq_sym = interner.lookup("Eq").unwrap();

        // Set up a TypeEnv with a constrained scheme for `contains`.
        let mut type_env = TypeEnv::new();
        let contains_name = interner.intern("contains");
        let contains_scheme = Scheme {
            forall: vec![0],
            constraints: vec![SchemeConstraint {
                class_name: eq_sym,
                type_vars: vec![0],
            }],
            infer_type: crate::types::infer_type::InferType::Var(0),
        };
        type_env.bind(contains_name, contains_scheme);

        // Build a minimal program with `contains` calling `eq`.
        let contains_binder = mk_binder(0, contains_name);
        let eq_method = interner.lookup("eq").unwrap();
        let x_binder = mk_binder(1, interner.intern("x"));
        let y_binder = mk_binder(2, interner.intern("y"));

        let mut program = CoreProgram {
            defs: vec![CoreDef {
                name: contains_name,
                binder: contains_binder,
                expr: CoreExpr::Lam {
                    params: vec![x_binder, y_binder],
                    body: Box::new(CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::unresolved(eq_method),
                            span: s(),
                        }),
                        args: vec![
                            CoreExpr::bound_var(x_binder, s()),
                            CoreExpr::bound_var(y_binder, s()),
                        ],
                        span: s(),
                    }),
                    span: s(),
                },
                borrow_signature: None,
                result_ty: None,
                is_anonymous: false,
                is_recursive: false,
                fip: None,
                span: s(),
            }],
            top_level_items: vec![],
        };

        let mut next_id = 100;
        elaborate_dictionaries(&mut program, &class_env, &type_env, &interner, &mut next_id);

        // Should have: 1 dict def (__dict_Eq_Int) + 1 original def (contains).
        assert_eq!(program.defs.len(), 2);

        // First def is the dictionary.
        let dict_def = &program.defs[0];
        assert_eq!(interner.resolve(dict_def.name), "__dict_Eq_Int");

        // Second def is `contains` — should now have 3 params (dict + x + y).
        let contains_def = &program.defs[1];
        assert_eq!(interner.resolve(contains_def.name), "contains");
        match &contains_def.expr {
            CoreExpr::Lam { params, body, .. } => {
                assert_eq!(params.len(), 3, "should have dict + x + y params");
                // Body should use TupleField for the eq call.
                match body.as_ref() {
                    CoreExpr::App { func, .. } => {
                        assert!(
                            matches!(func.as_ref(), CoreExpr::TupleField { index: 0, .. }),
                            "eq call should be rewritten to TupleField(dict, 0)"
                        );
                    }
                    other => panic!("expected App in body, got {other:?}"),
                }
            }
            other => panic!("expected Lam for contains, got {other:?}"),
        }
    }

    // ── dict_name_for ────────────────────────────────────────────────────

    #[test]
    fn dict_name_for_finds_interned_name() {
        let mut interner = Interner::new();
        let eq_sym = interner.intern("Eq");
        interner.intern("__dict_Eq_Int");

        assert!(dict_name_for(eq_sym, "Int", &interner).is_some());
    }

    #[test]
    fn dict_name_for_returns_none_when_missing() {
        let mut interner = Interner::new();
        let eq_sym = interner.intern("Eq");

        assert!(dict_name_for(eq_sym, "Float", &interner).is_none());
    }

    // ── SchemeConstraint in Scheme ────────────────────────────────────────

    #[test]
    fn scheme_instantiate_remaps_constraint_type_vars() {
        let mut interner = Interner::new();
        let eq_sym = interner.intern("Eq");

        let scheme = Scheme {
            forall: vec![0],
            constraints: vec![SchemeConstraint {
                class_name: eq_sym,
                type_vars: vec![0],
            }],
            infer_type: crate::types::infer_type::InferType::Var(0),
        };

        let mut counter = 100;
        let (_ty, mapping, constraints) = scheme.instantiate(&mut counter);

        assert_eq!(constraints.len(), 1);
        let new_var = mapping.get(&0).copied().unwrap();
        assert_eq!(constraints[0].type_vars, vec![new_var]);
        assert_eq!(constraints[0].class_name, eq_sym);
    }

    #[test]
    fn scheme_mono_has_empty_constraints() {
        let scheme = Scheme::mono(crate::types::infer_type::InferType::Con(
            crate::types::type_constructor::TypeConstructor::Int,
        ));
        assert!(scheme.constraints.is_empty());
    }
}
