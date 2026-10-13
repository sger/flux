/// Dictionary definitions for type classes (Proposal 0145, Step 5b).
///
/// For each class instance a program refers to, emits a top-level `CoreDef`
/// named `__dict_{Class}_{Type}`: a tuple of references to the mangled
/// instance functions, built from its context's dictionaries for a contextual
/// instance.
///
/// Dictionary parameters, the dictionaries each call passes, and method calls
/// on a dictionary are all placed by lowering, from the solver's evidence
/// (0.0.8 plan, step 1e); this pass only defines the dictionaries they name.
use std::collections::{HashMap, HashSet};

use crate::{
    core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePrimOp, CoreProgram, FluxRep},
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
    types::class_env::{ClassEnv, DictSlot},
};

/// Define the dictionaries a program lowered from evidence refers to.
///
/// Lowering from evidence has already given each constrained definition its
/// parameters and each call its dictionaries: what is left is defining the
/// `__dict_*` globals the program names.
pub fn build_referenced_dictionaries(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    interner: &Interner,
    next_id: &mut u32,
) {
    normalize_all_existing_dict_param_types(program, interner);
    if class_env.classes.is_empty() {
        return;
    }
    define_referenced_dictionaries(program, class_env, interner, next_id);
}

/// Prepend a definition for every dictionary the program refers to, and for
/// every dictionary those refer to, each before its first user.
fn define_referenced_dictionaries(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    interner: &Interner,
    next_id: &mut u32,
) {
    let referenced_dicts = collect_referenced_dictionary_names(program, interner);
    if referenced_dicts.is_empty() {
        return;
    }

    // Phase 2: Build only the concrete dictionary CoreDefs this module
    // actually references. Constrained definitions that only thread incoming
    // dictionary parameters do not need local copies of every instance dict.
    let dict_defs = referenced_dictionary_defs(
        build_instance_dictionaries(class_env, interner, next_id),
        referenced_dicts,
        interner,
    );
    if dict_defs.is_empty() {
        return;
    }

    // Prepend dictionary defs so they are available to all subsequent defs.
    let mut new_defs = dict_defs;
    new_defs.append(&mut program.defs);
    program.defs = new_defs;
}

/// The dictionaries out of `all` that `referenced` names, and every
/// dictionary those name in turn, each after the ones it names.
///
/// A dictionary's superclass slots name the superclass dictionaries for the
/// same head (`__dict_Measurable_Int` holds `__dict_Sizeable_Int`), so a
/// program that only names the subclass dictionary still reads the
/// superclass one through it. And a dictionary is a tuple built where it is
/// defined, so the ones it names must already be.
fn referenced_dictionary_defs(
    all: Vec<CoreDef>,
    mut referenced: HashSet<Identifier>,
    interner: &Interner,
) -> Vec<CoreDef> {
    let names_in = |def: &CoreDef| {
        let mut names = HashSet::new();
        collect_referenced_dictionary_names_expr(&def.expr, interner, &mut names);
        names
    };
    loop {
        let reached: HashSet<_> = all
            .iter()
            .filter(|def| referenced.contains(&def.name))
            .flat_map(names_in)
            .filter(|name| !referenced.contains(name))
            .collect();
        if reached.is_empty() {
            break;
        }
        referenced.extend(reached);
    }

    let mut pending: Vec<CoreDef> = all
        .into_iter()
        .filter(|def| referenced.contains(&def.name))
        .collect();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let unplaced: HashSet<_> = pending.iter().map(|def| def.name).collect();
        let (ready, waiting): (Vec<_>, Vec<_>) = pending.into_iter().partition(|def| {
            names_in(def)
                .iter()
                .all(|name| *name == def.name || !unplaced.contains(name))
        });
        if ready.is_empty() {
            // A cycle between dictionaries: keep the build order for the rest.
            ordered.extend(waiting);
            break;
        }
        ordered.extend(ready);
        pending = waiting;
    }
    ordered
}

fn collect_referenced_dictionary_names(
    program: &CoreProgram,
    interner: &Interner,
) -> HashSet<Identifier> {
    let mut refs = HashSet::new();
    for def in &program.defs {
        collect_referenced_dictionary_names_expr(&def.expr, interner, &mut refs);
    }
    refs
}

fn collect_referenced_dictionary_names_expr(
    expr: &CoreExpr,
    interner: &Interner,
    refs: &mut HashSet<Identifier>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            // This scan runs for every program, including ones whose Core
            // still carries placeholder identifiers from a failed HM pass,
            // so an unknown symbol must be skipped rather than resolved.
            if interner
                .try_resolve(var.name)
                .is_some_and(|name| name.starts_with("__dict_"))
            {
                refs.insert(var.name);
            }
        }
        CoreExpr::Lit(..) => {}
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            collect_referenced_dictionary_names_expr(body, interner, refs);
        }
        CoreExpr::App { func, args, .. } => {
            collect_referenced_dictionary_names_expr(func, interner, refs);
            for arg in args {
                collect_referenced_dictionary_names_expr(arg, interner, refs);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_referenced_dictionary_names_expr(rhs, interner, refs);
            collect_referenced_dictionary_names_expr(body, interner, refs);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                collect_referenced_dictionary_names_expr(rhs, interner, refs);
            }
            collect_referenced_dictionary_names_expr(body, interner, refs);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_referenced_dictionary_names_expr(scrutinee, interner, refs);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_referenced_dictionary_names_expr(guard, interner, refs);
                }
                collect_referenced_dictionary_names_expr(&alt.rhs, interner, refs);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                collect_referenced_dictionary_names_expr(field, interner, refs);
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_referenced_dictionary_names_expr(object, interner, refs);
        }
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_referenced_dictionary_names_expr(arg, interner, refs);
            }
        }
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            collect_referenced_dictionary_names_expr(body, interner, refs);
            if let Some(parameter) = parameter {
                collect_referenced_dictionary_names_expr(parameter, interner, refs);
            }
            for handler in handlers {
                collect_referenced_dictionary_names_expr(&handler.body, interner, refs);
            }
        }
    }
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
        if class_env.lookup_class_by_id(instance.class_id).is_none() {
            continue;
        }

        // Compute the type name string for this instance.
        // Multi-param classes join all type args: "Int_String".
        if instance.type_args.is_empty() {
            continue;
        }
        let type_name = instance.type_key.clone();

        // Build the dictionary name: __dict_{Class}_{Type}
        // These names are pre-interned during dispatch generation (Phase 1b).
        let dict_name_str =
            crate::types::class_env::dictionary_name(instance.class_id, &type_name, interner);
        let dict_name = match interner.lookup(&dict_name_str) {
            Some(sym) => sym,
            None => continue, // Not pre-interned — skip this instance.
        };

        let dict_expr = if instance.context.is_empty() {
            let Some(slot_names) = class_env.dictionary_slot_names(instance, interner) else {
                continue;
            };
            // All slots or none. Dropping the ones that happen to be
            // un-interned would emit a short tuple, and every slot after the
            // gap would then be read at the wrong index — a silent
            // miscompile rather than a missing method.
            let Some(tuple_fields) = slot_names
                .iter()
                .map(|name| Some(CoreExpr::external_var(interner.lookup(name)?, span)))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };

            CoreExpr::PrimOp {
                op: CorePrimOp::MakeTuple,
                args: tuple_fields,
                span,
            }
        } else {
            match build_contextual_dictionary_expr(class_env, instance, interner, next_id) {
                Some(expr) => expr,
                None => continue,
            }
        };

        // Create the CoreDef for this dictionary.
        let binder_id = *next_id;
        *next_id += 1;
        let binder = CoreBinder::with_rep(CoreBinderId(binder_id), dict_name, FluxRep::BoxedRep);

        defs.push(CoreDef {
            binder_types: Default::default(),
            name: dict_name,
            binder,
            expr: dict_expr,
            is_dict_def: true,
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

/// Build the dictionary *constructor* for an instance with a context.
///
/// The result is `\ctx_dicts. (superclass evidence..., method closures...)`,
/// with the slot order [`ClassEnv::dictionary_layout`] defines.
///
/// Returns `None` when a slot cannot be built — an un-interned method symbol,
/// or superclass evidence this pass cannot construct. Emitting the dictionary
/// without that slot is not an option: every later slot would then be read at
/// the wrong index.
///
/// [`ClassEnv::dictionary_layout`]: crate::types::class_env::ClassEnv::dictionary_layout
fn build_contextual_dictionary_expr(
    class_env: &ClassEnv,
    instance: &crate::types::class_env::InstanceDef,
    interner: &Interner,
    next_id: &mut u32,
) -> Option<CoreExpr> {
    let span = Span::default();
    let type_name = instance.type_key.clone();

    let mut context_occurrences: HashMap<crate::types::class_id::ClassId, usize> = HashMap::new();
    let context_binders: Vec<CoreBinder> = instance
        .context
        .iter()
        .enumerate()
        .map(|(idx, constraint)| {
            let class_id = instance
                .context_class_ids
                .get(idx)
                .copied()
                .unwrap_or_else(|| {
                    crate::types::class_id::ClassId::from_local_name(constraint.class_name)
                });
            let occurrence = context_occurrences.entry(class_id).or_insert(0);
            let label = if *occurrence == 0 {
                crate::types::class_env::dictionary_prefix(class_id, interner)
            } else {
                format!(
                    "{}_{occurrence}",
                    crate::types::class_env::dictionary_prefix(class_id, interner)
                )
            };
            *occurrence += 1;
            let binder_id = *next_id;
            *next_id += 1;
            let binder_name = interner.lookup(&label).unwrap_or(constraint.class_name);
            CoreBinder::with_rep(CoreBinderId(binder_id), binder_name, FluxRep::BoxedRep)
        })
        .collect();

    let class_def = class_env.lookup_class_by_id(instance.class_id)?;
    let tuple_fields = class_env
        .dictionary_layout(instance.class_id)?
        .into_iter()
        .map(|slot| match slot {
            DictSlot::Superclass(superclass) => superclass_evidence_expr(
                class_env,
                instance,
                superclass,
                &context_binders,
                interner,
            ),
            DictSlot::Method(method) => {
                let method_sig = class_def.methods.iter().find(|m| m.name == method)?;
                let mangled_sym =
                    interner.lookup(&crate::types::class_env::mangled_method_name(
                        instance.class_id,
                        &type_name,
                        interner.resolve(method),
                        interner,
                    ))?;
                Some(build_contextual_dictionary_method_closure(
                    mangled_sym,
                    method_sig.arity,
                    &context_binders,
                    interner,
                    next_id,
                ))
            }
        })
        .collect::<Option<Vec<_>>>()?;

    let tuple = CoreExpr::PrimOp {
        op: CorePrimOp::MakeTuple,
        args: tuple_fields,
        span,
    };

    Some(prepend_lam_params(tuple, context_binders))
}

/// The expression yielding a superclass dictionary for the head `type_name`.
///
/// `class Eq<a> => Ord<a>` means an `Ord<T>` dictionary carries the `Eq<T>`
/// dictionary for the *same* `T`. There are two places that evidence can come
/// from, and the order matters:
///
/// 1. **This instance's own context.** `instance Middle<Int> => Top<Int>`
///    is handed a `Middle<Int>` dictionary, which is exactly the superclass
///    evidence `Top` needs. Reaching for the global instead would apply a
///    dictionary *constructor* to an already-built dictionary.
/// 2. **A plain instance's global**, when the context does not supply it.
///
/// `None` when neither applies — the caller then skips the dictionary rather
/// than emitting evidence it cannot justify.
fn superclass_evidence_expr(
    class_env: &ClassEnv,
    instance: &crate::types::class_env::InstanceDef,
    superclass: crate::types::class_id::ClassId,
    context_binders: &[CoreBinder],
    interner: &Interner,
) -> Option<CoreExpr> {
    let span = Span::default();

    if let Some(binder) = instance
        .context_class_ids
        .iter()
        .position(|&class_id| class_id == superclass)
        .and_then(|idx| context_binders.get(idx))
    {
        return Some(CoreExpr::bound_var(binder, span));
    }

    let evidence = class_env.superclass_instance(superclass, instance)?;
    let dict_sym = interner.lookup(&crate::types::class_env::dictionary_name(
        superclass,
        &evidence.type_key,
        interner,
    ))?;
    evidence
        .context
        .is_empty()
        .then(|| CoreExpr::external_var(dict_sym, span))
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
        .map(|binder| CoreExpr::bound_var(binder, span))
        .collect();
    args.extend(
        user_params
            .iter()
            .map(|binder| CoreExpr::bound_var(binder, span)),
    );
    CoreExpr::Lam {
        params: user_params,
        param_types: Vec::new(),
        result_ty: None,
        body: Box::new(CoreExpr::App {
            func: Box::new(CoreExpr::external_var(mangled_sym, span)),
            args,
            span,
        }),
        span,
    }
}

/// Prepend extra parameters to the outermost `Lam` of an expression.
/// If the expression is not a `Lam`, wrap it in one.
fn prepend_lam_params(expr: CoreExpr, extra_params: Vec<CoreBinder>) -> CoreExpr {
    match expr {
        CoreExpr::Lam {
            mut params,
            mut param_types,
            result_ty,
            body,
            span,
        } => {
            let mut new_params = extra_params;
            let mut new_param_types = vec![None; new_params.len()];
            new_params.append(&mut params);
            new_param_types.append(&mut param_types);
            CoreExpr::Lam {
                params: new_params,
                param_types: new_param_types,
                result_ty,
                body,
                span,
            }
        }
        other => {
            // Non-lambda constrained def (unlikely, but handle gracefully).
            CoreExpr::Lam {
                params: extra_params,
                param_types: Vec::new(),
                result_ty: None,
                body: Box::new(other),
                span: Span::default(),
            }
        }
    }
}

fn normalize_existing_dict_param_types(expr: CoreExpr, dict_param_count: usize) -> CoreExpr {
    if dict_param_count == 0 {
        return expr;
    }

    match expr {
        CoreExpr::Lam {
            params,
            mut param_types,
            result_ty,
            body,
            span,
        } => {
            if !param_types.is_empty() && param_types.len() + dict_param_count == params.len() {
                let mut normalized = vec![None; dict_param_count];
                normalized.append(&mut param_types);
                param_types = normalized;
            } else if !param_types.is_empty() && param_types.len() != params.len() {
                param_types.clear();
            }

            CoreExpr::Lam {
                params,
                param_types,
                result_ty,
                body,
                span,
            }
        }
        other => other,
    }
}

fn normalize_all_existing_dict_param_types(program: &mut CoreProgram, interner: &Interner) {
    for def in &mut program.defs {
        let dict_param_count = match &def.expr {
            CoreExpr::Lam {
                params,
                param_types,
                ..
            } if !param_types.is_empty() && param_types.len() < params.len() => {
                let named_dict_params = params
                    .iter()
                    .take_while(|param| interner.resolve(param.name).starts_with("__dict_"))
                    .count();
                named_dict_params.max(params.len() - param_types.len())
            }
            CoreExpr::Lam { params, .. } => params
                .iter()
                .take_while(|param| interner.resolve(param.name).starts_with("__dict_"))
                .count(),
            _ => 0,
        };
        if dict_param_count > 0 {
            let expr = std::mem::replace(
                &mut def.expr,
                CoreExpr::Lit(crate::core::CoreLit::Unit, Span::default()),
            );
            def.expr = normalize_existing_dict_param_types(expr, dict_param_count);
        }
    }
}

/// Methods reachable *through* a dictionary for `class_id`, each with the class
/// that declares it and the slot path that reaches it, relative to that
/// dictionary.
///
/// Breadth-first over the superclass graph, so a method declared by the nearest
/// class wins over an inherited one of the same name. Cycles cannot occur —
/// E477 rejects them before this pass runs — but the visited set keeps this
/// total regardless.
///
/// The declaring class is reported because it, not the constraint's class, owns
/// the method signature that says which argument reveals the dispatch type.
pub(crate) fn reachable_methods(
    class_env: &ClassEnv,
    class_id: crate::types::class_id::ClassId,
) -> Vec<(Identifier, crate::types::class_id::ClassId, Vec<usize>)> {
    let mut found: Vec<(Identifier, crate::types::class_id::ClassId, Vec<usize>)> = Vec::new();
    let mut visited = vec![class_id];
    let mut queue = std::collections::VecDeque::from([(class_id, Vec::new())]);

    while let Some((current, prefix)) = queue.pop_front() {
        for (idx, slot) in class_env
            .dictionary_layout(current)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let mut path = prefix.clone();
            path.push(idx);
            match slot {
                DictSlot::Method(method) => {
                    if !found.iter().any(|(seen, _, _)| *seen == method) {
                        found.push((method, current, path));
                    }
                }
                DictSlot::Superclass(superclass) if !visited.contains(&superclass) => {
                    visited.push(superclass);
                    queue.push_back((superclass, path));
                }
                DictSlot::Superclass(_) => {}
            }
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::type_infer::constraint::SchemeConstraint,
        core::{CoreLit, FluxRep},
        syntax::type_expr::TypeExpr,
        types::{
            class_env::{ClassDef, InstanceDef, MethodSig},
            infer_type::InferType,
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
            is_public: false,
            is_builtin: false,
            type_params: vec![a_sym],
            superclasses: vec![],
            superclass_class_ids: vec![],
            associated_types: vec![],
            methods: vec![
                MethodSig {
                    name: eq_method,
                    type_params: vec![],
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_type.clone(), a_type.clone()],
                    return_type: bool_type.clone(),
                    arity: 2,
                    effects: vec![],
                    infer_type: None,
                },
                MethodSig {
                    name: neq_method,
                    type_params: vec![],
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_type.clone(), a_type],
                    return_type: bool_type,
                    arity: 2,
                    effects: vec![],
                    infer_type: None,
                },
            ],
            default_methods: vec![neq_method],
            span: s(),
        };

        let instance_def = InstanceDef {
            origin: crate::types::class_env::InstanceOrigin::Declared,
            class_name: eq_sym,
            class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
            instance_module: crate::types::class_id::ModulePath::EMPTY,
            is_public: false,
            type_key: "Int".to_string(),
            head_modules: Vec::new(),
            type_args: vec![TypeExpr::Named {
                name: int_sym,
                args: vec![],
                span: s(),
            }],
            context: vec![],
            context_class_ids: vec![],
            method_names: vec![eq_method, neq_method],
            method_effects: vec![],
            associated_types: vec![],
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
            is_public: false,
            is_builtin: false,
            type_params: vec![a_sym],
            superclasses: vec![],
            superclass_class_ids: vec![],
            associated_types: vec![],
            methods: vec![MethodSig {
                name: eq_method,
                type_params: vec![],
                param_names: vec![interner.intern("__x0")],
                param_types: vec![],
                return_type: TypeExpr::Named {
                    name: a_sym,
                    args: vec![],
                    span: s(),
                },
                arity: 1,
                effects: vec![],
                infer_type: None,
            }],
            default_methods: vec![],
            span: s(),
        };

        let instance_def = InstanceDef {
            origin: crate::types::class_env::InstanceOrigin::Declared,
            class_name: eq_sym,
            class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
            instance_module: crate::types::class_id::ModulePath::EMPTY,
            is_public: false,
            type_key: "Int".to_string(),
            head_modules: Vec::new(),
            type_args: vec![TypeExpr::Named {
                name: float_sym,
                args: vec![],
                span: s(),
            }],
            context: vec![],
            context_class_ids: vec![],
            method_names: vec![eq_method],
            method_effects: vec![],
            associated_types: vec![],
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

    // ── prepend_lam_params ───────────────────────────────────────────────

    #[test]
    fn prepend_lam_params_adds_to_existing_lam() {
        let mut interner = Interner::new();
        let dict = mk_binder(10, interner.intern("__dict"));
        let x = mk_binder(1, interner.intern("x"));

        let lam = CoreExpr::Lam {
            params: vec![x],
            param_types: vec![],
            result_ty: None,
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

    // ── SchemeConstraint in Scheme ────────────────────────────────────────

    #[test]
    fn scheme_instantiate_substitutes_structured_constraint_args() {
        let mut interner = Interner::new();
        let eq_sym = interner.intern("Eq");

        let scheme = Scheme {
            forall: vec![0],
            constraints: vec![SchemeConstraint {
                class_name: eq_sym,
                class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
                type_args: vec![InferType::Var(0)],
            }],
            infer_type: crate::types::infer_type::InferType::Var(0),
        };

        let mut counter = 100;
        let (_ty, mapping, constraints) = scheme.instantiate(&mut counter);

        assert_eq!(constraints.len(), 1);
        let new_var = mapping.get(&0).cloned().unwrap();
        assert_eq!(constraints[0].type_args, vec![InferType::Var(new_var)]);
        assert_eq!(constraints[0].class_name, eq_sym);
    }

    #[test]
    fn scheme_mono_has_empty_constraints() {
        let scheme = Scheme::mono(crate::types::infer_type::InferType::Con(
            crate::types::type_constructor::TypeConstructor::Int,
        ));
        assert!(scheme.constraints.is_empty());
    }

    /// The evidence path's elaboration only defines the dictionaries the
    /// program names: lowering already gave every body its parameters and
    /// arguments, so nothing in a body is rewritten.
    #[test]
    fn build_referenced_dictionaries_defines_dictionaries_and_leaves_bodies_alone() {
        let mut interner = Interner::new();
        let class_env = build_eq_class_env(&mut interner);
        let eq = interner.lookup("eq").expect("eq interned");
        let dict = interner
            .lookup("__dict_Eq_Int")
            .expect("dictionary interned");
        let x = mk_binder(1, interner.intern("x"));
        let body = CoreExpr::lambda(
            vec![x],
            CoreExpr::App {
                func: Box::new(CoreExpr::external_var(eq, s())),
                args: vec![
                    CoreExpr::external_var(dict, s()),
                    CoreExpr::bound_var(&x, s()),
                    CoreExpr::bound_var(&x, s()),
                ],
                span: s(),
            },
            s(),
        );
        let f = CoreDef::new(mk_binder(0, interner.intern("f")), body.clone(), false, s());
        let mut program = CoreProgram {
            defs: vec![f],
            top_level_items: Vec::new(),
        };
        let mut next_id = 2;

        build_referenced_dictionaries(&mut program, &class_env, &interner, &mut next_id);

        let names: Vec<_> = program.defs.iter().map(|def| def.name).collect();
        assert_eq!(names, [dict, interner.lookup("f").unwrap()]);
        assert!(program.defs[0].is_dict_def);
        assert_eq!(format!("{:?}", program.defs[1].expr), format!("{body:?}"));
    }
}
