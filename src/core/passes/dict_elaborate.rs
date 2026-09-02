/// Dictionary elaboration pass for type classes (Proposal 0145, Step 5b).
///
/// Transforms type class dispatch from runtime `type_of()` checks to
/// compile-time dictionary passing.
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
use std::collections::{HashMap, HashSet};

use crate::{
    core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CorePrimOp, CoreProgram, CoreType, FluxRep,
    },
    diagnostics::position::Span,
    syntax::{Identifier, interner::Interner},
    types::{
        class_env::{ClassEnv, DictSelection, DictSlot, select_dictionary},
        scheme::Scheme,
        type_env::TypeEnv,
    },
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
    let def_schemes = program
        .defs
        .iter()
        .filter_map(|def| {
            type_env
                .lookup(def.name)
                .cloned()
                .map(|scheme| (def.binder.id, scheme))
        })
        .collect::<HashMap<_, _>>();
    elaborate_dictionaries_with_def_schemes(
        program,
        class_env,
        type_env,
        &def_schemes,
        interner,
        next_id,
    );
}

pub fn elaborate_dictionaries_with_def_schemes(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    type_env: &TypeEnv,
    def_schemes: &HashMap<CoreBinderId, Scheme>,
    interner: &Interner,
    next_id: &mut u32,
) {
    normalize_all_existing_dict_param_types(program, interner);

    if class_env.classes.is_empty() {
        return;
    }

    // Rewrite constrained function bodies when present, then resolve concrete
    // dictionary references introduced by typed class-call lowering. The
    // latter is needed even in an otherwise monomorphic caller: a call such
    // as `encode(Some(42))` is lowered directly to a mangled instance method
    // but still needs its `Encode<Int>` dictionary materialised.
    // Phase 3: Rewrite constrained function bodies.
    rewrite_constrained_functions(program, class_env, def_schemes, interner, next_id);

    // Phase 4: Insert dictionary arguments at call sites.
    insert_dict_args_at_call_sites(program, class_env, type_env, def_schemes, interner);

    let referenced_dicts = collect_referenced_dictionary_names(program, interner);
    if referenced_dicts.is_empty() {
        return;
    }

    // Phase 2: Build only the concrete dictionary CoreDefs this module
    // actually references. Constrained definitions that only thread incoming
    // dictionary parameters do not need local copies of every instance dict.
    let dict_defs = build_instance_dictionaries(class_env, interner, next_id)
        .into_iter()
        .filter(|def| referenced_dicts.contains(&def.name))
        .collect::<Vec<_>>();
    if dict_defs.is_empty() {
        return;
    }

    // Prepend dictionary defs so they are available to all subsequent defs.
    let mut new_defs = dict_defs;
    new_defs.append(&mut program.defs);
    program.defs = new_defs;
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
        let type_name = instance
            .type_args
            .iter()
            .map(|a| a.display_with(interner))
            .collect::<Vec<_>>()
            .join("_");

        // Build the dictionary name: __dict_{Class}_{Type}
        // These names are pre-interned during dispatch generation (Phase 1b).
        let dict_name_str =
            crate::types::class_env::dictionary_name(instance.class_id, &type_name, interner);
        let dict_name = match interner.lookup(&dict_name_str) {
            Some(sym) => sym,
            None => continue, // Not pre-interned — skip this instance.
        };

        let dict_expr = if instance.context.is_empty() {
            let Some(slot_names) =
                class_env.dictionary_slot_names(instance.class_id, &type_name, interner)
            else {
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
    let type_name = instance
        .type_args
        .iter()
        .map(|a| a.display_with(interner))
        .collect::<Vec<_>>()
        .join("_");

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
                &type_name,
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
    type_name: &str,
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

    let dict_sym = interner.lookup(&crate::types::class_env::dictionary_name(
        superclass, type_name, interner,
    ))?;
    let evidence = class_env.instances.iter().find(|candidate| {
        candidate.class_id == superclass
            && candidate
                .type_args
                .iter()
                .map(|arg| arg.display_with(interner))
                .collect::<Vec<_>>()
                .join("_")
                == type_name
    })?;
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

/// Rewrite constrained functions to accept dictionary parameters and
/// extract methods from them instead of calling polymorphic stubs.
/// The constraints of `scheme` that carry a runtime dictionary.
///
/// Marker classes (no methods) have no dictionary tuple, so they contribute
/// neither a parameter nor an argument. Filtering here — once, where the
/// constraint list is read — keeps callee arity and call-site arity derived
/// from the same predicate (Proposal 0179 Stage 2).
fn dictionary_constraints(
    scheme: &Scheme,
    class_env: &ClassEnv,
) -> Vec<crate::ast::type_infer::constraint::SchemeConstraint> {
    scheme
        .constraints
        .iter()
        .filter(|constraint| class_env.constraint_needs_dictionary(constraint))
        .cloned()
        .collect()
}

fn rewrite_constrained_functions(
    program: &mut CoreProgram,
    class_env: &ClassEnv,
    def_schemes: &HashMap<CoreBinderId, Scheme>,
    interner: &Interner,
    next_id: &mut u32,
) {
    for def in &mut program.defs {
        let scheme = match def_schemes.get(&def.binder.id) {
            Some(s) => s,
            None => continue,
        };

        let constraints = dictionary_constraints(scheme, class_env);
        if constraints.is_empty() {
            continue;
        }

        let existing_dict_params = match &def.expr {
            CoreExpr::Lam { params, .. }
                if params.len() >= constraints.len()
                    && params[..constraints.len()]
                        .iter()
                        .all(|binder| interner.resolve(binder.name).starts_with("__dict_")) =>
            {
                params[..constraints.len()].to_vec()
            }
            _ => Vec::new(),
        };

        // Build dictionary parameters and method map for this function.
        let mut dict_params: Vec<CoreBinder> = Vec::new();
        let mut method_map: MethodPaths = HashMap::new();
        // A function can hold several dictionaries for one class, so the
        // parameter names carry a per-class occurrence suffix. The lookups that
        // consume them — `current_context_dictionary` and its AST twin — have
        // always computed `__dict_Enc_1` for a second occurrence; naming every
        // parameter `__dict_Enc` meant the second shadowed the first and every
        // reference resolved to the wrong dictionary (KI-052).
        let mut class_occurrences: HashMap<crate::types::class_id::ClassId, usize> = HashMap::new();

        for (index, constraint) in constraints.iter().enumerate() {
            if class_env.lookup_class_by_id(constraint.class_id).is_none() {
                continue;
            }

            let dict_binder = if let Some(existing) = existing_dict_params.get(index).cloned() {
                existing
            } else {
                let class_str =
                    crate::types::class_env::dictionary_prefix(constraint.class_id, interner);
                let occurrence = class_occurrences.entry(constraint.class_id).or_insert(0);
                let suffix = if *occurrence == 0 {
                    String::new()
                } else {
                    format!("_{occurrence}")
                };
                *occurrence += 1;
                let param_name_str = format!("{class_str}{suffix}");
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

            // Record every method reachable through this dictionary with the
            // slot path that reaches it. The class's own methods are one
            // projection; a superclass method is that superclass's slot
            // followed by the method's slot inside it, which is what lets a
            // `fn f<a: Ord>` call `eq`.
            //
            // Appended rather than inserted: a second constraint on the same
            // class reaches the same methods through a *different* dictionary,
            // and both have to survive for the call site to choose between
            // them.
            let type_args: Vec<CoreType> = constraint
                .type_args
                .iter()
                .map(CoreType::try_from_infer)
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default();
            for (method, declaring_class, path) in reachable_methods(class_env, constraint.class_id)
            {
                method_map.entry(method).or_default().push(MethodCandidate {
                    declaring_class,
                    type_args: type_args.clone(),
                    binder: dict_binder,
                    path,
                });
            }
        }

        // Rewrite the function body to extract methods from dictionaries.
        let old_expr = std::mem::replace(
            &mut def.expr,
            CoreExpr::Lit(crate::core::CoreLit::Unit, Span::default()),
        );
        // Class-method calls in generated instance methods are resolved by the
        // typed AST-to-Core lowering.  In particular, a same-class contextual
        // instance must distinguish a container call (direct self dispatch)
        // from an element call (dictionary extraction).  Dictionary
        // elaboration only performs the latter rewrite here.
        let rewritten = rewrite_body_with_dicts(old_expr, &method_map, class_env);

        if dict_params.is_empty() {
            def.expr = normalize_existing_dict_param_types(rewritten, existing_dict_params.len());
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
    def_schemes: &HashMap<CoreBinderId, Scheme>,
    interner: &Interner,
) {
    // Build a set of function names that have constraints.
    let constrained_fns_by_binder: HashMap<
        CoreBinderId,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    > = program
        .defs
        .iter()
        .filter_map(|def| {
            let scheme = def_schemes.get(&def.binder.id)?;
            let constraints = dictionary_constraints(scheme, class_env);
            (!constraints.is_empty()).then_some((def.binder.id, constraints))
        })
        .collect();
    let constrained_fns_by_name: HashMap<
        Identifier,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    > = program
        .defs
        .iter()
        .filter_map(|def| {
            let scheme = def_schemes
                .get(&def.binder.id)
                .or_else(|| type_env.lookup(def.name))?;
            let constraints = dictionary_constraints(scheme, class_env);
            (!constraints.is_empty()).then_some((def.name, constraints))
        })
        .collect();

    if constrained_fns_by_binder.is_empty() && constrained_fns_by_name.is_empty() {
        return;
    }

    // For each def, build its own dict_param map (for polymorphic forwarding),
    // then walk its body to insert dict args at call sites.
    for def in &mut program.defs {
        // Build the caller's own dict_param map (if it's a constrained function).
        let caller_dicts: CallerDicts = if let Some(scheme) = def_schemes
            .get(&def.binder.id)
            .or_else(|| type_env.lookup(def.name))
        {
            build_caller_dict_map(&def.expr, &dictionary_constraints(scheme, class_env))
        } else {
            CallerDicts::new()
        };

        let old_expr = std::mem::replace(
            &mut def.expr,
            CoreExpr::Lit(crate::core::CoreLit::Unit, Span::default()),
        );
        def.expr = insert_dict_args_expr(
            old_expr,
            &constrained_fns_by_binder,
            &constrained_fns_by_name,
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
/// The dictionary parameters a constrained function received, paired with the
/// predicate each one discharges.
///
/// Keyed by the *whole* predicate rather than the class name. A function can
/// hold several dictionaries for one class — `fn show_all<a: Enc>(xs: List<a>)`
/// takes both `Enc<a>` and `Enc<List<a>>` — and collapsing them to the class
/// name kept only the last, so a call inside the body was handed a dictionary
/// for the wrong type (KI-052). The list is a handful of entries per function,
/// so a linear search costs less than hashing `InferType`s.
type CallerDicts = Vec<(
    crate::ast::type_infer::constraint::SchemeConstraint,
    CoreBinder,
)>;

fn build_caller_dict_map(
    expr: &CoreExpr,
    constraints: &[crate::ast::type_infer::constraint::SchemeConstraint],
) -> CallerDicts {
    let mut map = CallerDicts::new();
    if constraints.is_empty() {
        return map;
    }
    if let CoreExpr::Lam { params, .. } = expr {
        // The first N params are dictionary params (one per constraint).
        for (i, constraint) in constraints.iter().enumerate() {
            if let Some(binder) = params.get(i) {
                map.push((constraint.clone(), *binder));
            }
        }
    }
    map
}

fn insert_dict_args_expr(
    expr: CoreExpr,
    constrained_fns_by_binder: &HashMap<
        CoreBinderId,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    >,
    constrained_fns_by_name: &HashMap<
        Identifier,
        Vec<crate::ast::type_infer::constraint::SchemeConstraint>,
    >,
    caller_dicts: &CallerDicts,
    class_env: &ClassEnv,
    interner: &Interner,
) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            // Check if the callee is a constrained function.
            if let CoreExpr::Var { ref var, .. } = *func
                && let Some(callee_constraints) = var
                    .binder
                    .and_then(|binder| constrained_fns_by_binder.get(&binder))
                    .or_else(|| constrained_fns_by_name.get(&var.name))
            {
                let already_has_dict_args = args.len() >= callee_constraints.len()
                    && args
                        .iter()
                        .take(callee_constraints.len())
                        .all(|arg| match arg {
                            CoreExpr::Var { var, .. } => {
                                interner.resolve(var.name).starts_with("__dict_")
                            }
                            _ => false,
                        });
                if already_has_dict_args {
                    return CoreExpr::App {
                        func,
                        args: args
                            .into_iter()
                            .map(|a| {
                                insert_dict_args_expr(
                                    a,
                                    constrained_fns_by_binder,
                                    constrained_fns_by_name,
                                    caller_dicts,
                                    class_env,
                                    interner,
                                )
                            })
                            .collect(),
                        span,
                    };
                }
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
                        insert_dict_args_expr(
                            a,
                            constrained_fns_by_binder,
                            constrained_fns_by_name,
                            caller_dicts,
                            class_env,
                            interner,
                        )
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
                    constrained_fns_by_binder,
                    constrained_fns_by_name,
                    caller_dicts,
                    class_env,
                    interner,
                )),
                args: args
                    .into_iter()
                    .map(|a| {
                        insert_dict_args_expr(
                            a,
                            constrained_fns_by_binder,
                            constrained_fns_by_name,
                            caller_dicts,
                            class_env,
                            interner,
                        )
                    })
                    .collect(),
                span,
            }
        }

        // Recursive cases — same structure as rewrite_expr but threading
        // different context.
        // A dictionary reference the AST lowerer emitted by name, before the
        // parameter holding it existed. Bind it to that parameter now.
        //
        // `current_context_dictionary` names the dictionary a call needs
        // (`__dict_Enc`, `__dict_Enc_1`) and emits an *unresolved* variable,
        // because at AST-lowering time the enclosing function has no dictionary
        // parameters yet — this pass adds them. Left unresolved, the name
        // escapes to global scope, where no such definition exists, and the
        // call receives `None` (KI-052).
        CoreExpr::Var { ref var, span }
            if var.binder.is_none()
                && caller_dicts.iter().any(|(_, held)| held.name == var.name) =>
        {
            let binder = caller_dicts
                .iter()
                .find(|(_, held)| held.name == var.name)
                .map(|(_, held)| *held)
                .expect("guard just matched this name");
            CoreExpr::bound_var(&binder, span)
        }

        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
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
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns_by_binder,
                constrained_fns_by_name,
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
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns_by_binder,
                constrained_fns_by_name,
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
                            constrained_fns_by_binder,
                            constrained_fns_by_name,
                            caller_dicts,
                            class_env,
                            interner,
                        )),
                    )
                })
                .collect(),
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
            span,
        },

        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(insert_dict_args_expr(
                *scrutinee,
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = insert_dict_args_expr(
                        alt.rhs,
                        constrained_fns_by_binder,
                        constrained_fns_by_name,
                        caller_dicts,
                        class_env,
                        interner,
                    );
                    alt.guard = alt.guard.map(|g| {
                        insert_dict_args_expr(
                            g,
                            constrained_fns_by_binder,
                            constrained_fns_by_name,
                            caller_dicts,
                            class_env,
                            interner,
                        )
                    });
                    alt
                })
                .collect(),
            join_ty,
            span,
        },

        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| {
                    insert_dict_args_expr(
                        f,
                        constrained_fns_by_binder,
                        constrained_fns_by_name,
                        caller_dicts,
                        class_env,
                        interner,
                    )
                })
                .collect(),
            span,
        },

        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| {
                    insert_dict_args_expr(
                        a,
                        constrained_fns_by_binder,
                        constrained_fns_by_name,
                        caller_dicts,
                        class_env,
                        interner,
                    )
                })
                .collect(),
            span,
        },

        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(insert_dict_args_expr(
                *value,
                constrained_fns_by_binder,
                constrained_fns_by_name,
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
                    insert_dict_args_expr(
                        a,
                        constrained_fns_by_binder,
                        constrained_fns_by_name,
                        caller_dicts,
                        class_env,
                        interner,
                    )
                })
                .collect(),
            span,
        },

        CoreExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(insert_dict_args_expr(
                *body,
                constrained_fns_by_binder,
                constrained_fns_by_name,
                caller_dicts,
                class_env,
                interner,
            )),
            effect,
            parameter: parameter.map(|p| {
                Box::new(insert_dict_args_expr(
                    *p,
                    constrained_fns_by_binder,
                    constrained_fns_by_name,
                    caller_dicts,
                    class_env,
                    interner,
                ))
            }),
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = insert_dict_args_expr(
                        h.body,
                        constrained_fns_by_binder,
                        constrained_fns_by_name,
                        caller_dicts,
                        class_env,
                        interner,
                    );
                    h
                })
                .collect(),
            span,
        },

        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(insert_dict_args_expr(
                *object,
                constrained_fns_by_binder,
                constrained_fns_by_name,
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
                constrained_fns_by_binder,
                constrained_fns_by_name,
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
    caller_dicts: &CallerDicts,
    _class_env: &ClassEnv,
    interner: &Interner,
    span: Span,
) -> Option<CoreExpr> {
    // Case 1: the caller already holds a dictionary for exactly this
    // predicate — forward it.
    if let Some((_, binder)) = caller_dicts.iter().find(|(held, _)| held == constraint) {
        return Some(CoreExpr::bound_var(binder, span));
    }

    // Case 2: the caller holds exactly one dictionary for this class and the
    // predicate did not match structurally. Matching by class alone is only
    // safe when there is no second dictionary to confuse it with; with two,
    // picking either is a guess, and guessing wrong is what KI-052 was.
    let mut same_class = caller_dicts
        .iter()
        .filter(|(held, _)| held.class_id == constraint.class_id);
    if let Some((_, binder)) = same_class.next()
        && same_class.next().is_none()
    {
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
/// `method_map` maps class method names to `(dict_param_binder, slot_path)`.
/// One dictionary a class method can be reached through, and how.
///
/// `path` is a slot path applied outermost-first: `[2]` is the dictionary's own
/// slot 2, and `[0, 1]` is slot 1 of the superclass evidence held in slot 0.
/// Superclass entailment is exactly what makes a path longer than one element
/// possible.
#[derive(Debug, Clone)]
pub struct MethodCandidate {
    /// The class that declares the method — the constraint's own class for a
    /// direct method, a superclass for an inherited one. Selection asks *this*
    /// class which argument reveals its type parameter.
    declaring_class: crate::types::class_id::ClassId,
    /// Type arguments of the constraint this candidate was reached through.
    ///
    /// Empty when they could not be expressed as `CoreType`, which simply
    /// fails to match and so never wins a selection.
    type_args: Vec<CoreType>,
    binder: CoreBinder,
    path: Vec<usize>,
}

/// Every dictionary each class method can be reached through, in constraint
/// order.
///
/// A list rather than a single entry: a function constrained twice on one class
/// holds two dictionaries that both reach the same method, and collapsing them
/// to one is KI-057 — every call then dispatched through whichever won.
type MethodPaths = HashMap<Identifier, Vec<MethodCandidate>>;

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

/// Choose which of `method`'s candidate dictionaries this call means.
///
/// One candidate needs no choosing, which is both the common case and what
/// keeps a method dispatched on its result type working unchanged. With two or
/// more, the argument types decide: `dispatch_positions` says which argument
/// reveals each class parameter, and `select_dictionary` matches what it finds
/// against each candidate's constraint.
///
/// An undecidable call has already been reported by inference (E485). A Core
/// pass cannot report, so this recovers with the first candidate rather than
/// failing — lowering stays total and the diagnostic is what the user sees.
fn choose_candidate<'a>(
    class_env: &ClassEnv,
    candidates: &'a [MethodCandidate],
    method: Identifier,
    args: &[CoreExpr],
    binder_types: &HashMap<CoreBinderId, CoreType>,
) -> Option<&'a MethodCandidate> {
    let [_, _, ..] = candidates else {
        return candidates.first();
    };
    let first = &candidates[0];
    let Some(positions) = class_env.dispatch_positions(first.declaring_class, method) else {
        return Some(first);
    };
    // A method whose class parameter appears in no value parameter — `decode`,
    // whose type is fixed by where its result flows — reveals nothing here.
    // Result-directed selection is not in scope (KI-058), so preserve what the
    // name-keyed map did: the last constraint to record the method won.
    if positions.iter().all(Option::is_none) {
        return candidates.last();
    }
    let observed: Vec<Option<CoreType>> = positions
        .iter()
        .map(|position| {
            let arg = args.get((*position)?)?;
            match arg {
                CoreExpr::Var { var, .. } => {
                    var.binder.and_then(|id| binder_types.get(&id).cloned())
                }
                _ => None,
            }
        })
        .collect();
    let indexed: Vec<(usize, Vec<CoreType>)> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| (index, candidate.type_args.clone()))
        .collect();
    match select_dictionary(&indexed, &observed) {
        DictSelection::Unique(index) => candidates.get(index),
        DictSelection::Ambiguous | DictSelection::NoMatch => Some(first),
    }
}

pub fn rewrite_body_with_dicts(
    expr: CoreExpr,
    method_map: &MethodPaths,
    class_env: &ClassEnv,
) -> CoreExpr {
    let mut binder_types = HashMap::new();
    rewrite_expr(expr, method_map, class_env, &mut binder_types)
}

fn rewrite_expr(
    expr: CoreExpr,
    method_map: &MethodPaths,
    class_env: &ClassEnv,
    binder_types: &mut HashMap<CoreBinderId, CoreType>,
) -> CoreExpr {
    match expr {
        // Key case: App where the function is a class method reference.
        // Rewrite: App(Var(eq), args) → App(TupleField(Var(dict), idx), args)
        CoreExpr::App { func, args, span } => {
            if let CoreExpr::Var { ref var, .. } = *func
                && let Some(candidates) = method_map.get(&var.name)
                && let Some(MethodCandidate {
                    binder: dict_binder,
                    path,
                    ..
                }) = choose_candidate(class_env, candidates, var.name, &args, binder_types)
            {
                // Class method reference — project it out of the dictionary.
                // A path longer than one slot walks through superclass
                // evidence on the way (`__dict_Ord.0.0` is `Ord`'s `Eq`
                // dictionary, then `Eq`'s `eq`).
                let method_extract =
                    path.iter()
                        .fold(CoreExpr::bound_var(dict_binder, span), |object, &index| {
                            CoreExpr::TupleField {
                                object: Box::new(object),
                                index,
                                span,
                            }
                        });
                let rewritten_args = args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map, class_env, binder_types))
                    .collect();
                return CoreExpr::App {
                    func: Box::new(method_extract),
                    args: rewritten_args,
                    span,
                };
            }
            // Not a class method — recurse normally.
            CoreExpr::App {
                func: Box::new(rewrite_expr(*func, method_map, class_env, binder_types)),
                args: args
                    .into_iter()
                    .map(|a| rewrite_expr(a, method_map, class_env, binder_types))
                    .collect(),
                span,
            }
        }

        // Recursive cases for all other expression forms.
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => {
            // Record parameter types on the way down: selection reads them at
            // the call sites below. Binder ids are unique within a function, so
            // entries never need removing on the way back up.
            for (binder, ty) in params.iter().zip(param_types.iter()) {
                if let Some(ty) = ty {
                    binder_types.insert(binder.id, ty.clone());
                }
            }
            CoreExpr::Lam {
                params,
                param_types,
                result_ty,
                body: Box::new(rewrite_expr(*body, method_map, class_env, binder_types)),
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
            rhs: Box::new(rewrite_expr(*rhs, method_map, class_env, binder_types)),
            body: Box::new(rewrite_expr(*body, method_map, class_env, binder_types)),
            span,
        },

        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(rewrite_expr(*rhs, method_map, class_env, binder_types)),
            body: Box::new(rewrite_expr(*body, method_map, class_env, binder_types)),
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
                        Box::new(rewrite_expr(*rhs, method_map, class_env, binder_types)),
                    )
                })
                .collect(),
            body: Box::new(rewrite_expr(*body, method_map, class_env, binder_types)),
            span,
        },

        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(rewrite_expr(
                *scrutinee,
                method_map,
                class_env,
                binder_types,
            )),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = rewrite_expr(alt.rhs, method_map, class_env, binder_types);
                    alt.guard = alt
                        .guard
                        .map(|g| rewrite_expr(g, method_map, class_env, binder_types));
                    alt
                })
                .collect(),
            join_ty,
            span,
        },

        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields
                .into_iter()
                .map(|f| rewrite_expr(f, method_map, class_env, binder_types))
                .collect(),
            span,
        },

        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args
                .into_iter()
                .map(|a| rewrite_expr(a, method_map, class_env, binder_types))
                .collect(),
            span,
        },

        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_expr(*value, method_map, class_env, binder_types)),
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
                .map(|a| rewrite_expr(a, method_map, class_env, binder_types))
                .collect(),
            span,
        },

        CoreExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_expr(*body, method_map, class_env, binder_types)),
            effect,
            parameter: parameter
                .map(|p| Box::new(rewrite_expr(*p, method_map, class_env, binder_types))),
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = rewrite_expr(h.body, method_map, class_env, binder_types);
                    h
                })
                .collect(),
            span,
        },

        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(rewrite_expr(*object, method_map, class_env, binder_types)),
            member,
            span,
        },

        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(rewrite_expr(*object, method_map, class_env, binder_types)),
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
            infer_type::InferType,
            scheme::Scheme,
        },
    };

    fn s() -> Span {
        Span::default()
    }

    /// The only candidate for a method, which is what every rewriter test
    /// exercises: with one dictionary in scope there is nothing to choose, so
    /// the class environment is never consulted.
    fn single_candidate(binder: CoreBinder, path: Vec<usize>) -> MethodCandidate {
        MethodCandidate {
            declaring_class: crate::types::class_id::ClassId::new(
                crate::types::class_id::ModulePath::EMPTY,
                crate::syntax::symbol::Symbol::new(0),
            ),
            type_args: Vec::new(),
            binder,
            path,
        }
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
                    default_body: None,
                },
                MethodSig {
                    name: neq_method,
                    type_params: vec![],
                    param_names: vec![interner.intern("__x0"), interner.intern("__x1")],
                    param_types: vec![a_type.clone(), a_type],
                    return_type: bool_type,
                    arity: 2,
                    effects: vec![],
                    default_body: None,
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
                default_body: None,
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
        method_map.insert(
            eq_method,
            vec![single_candidate(dict_binder, vec![0_usize])],
        );

        // Build: App(Var(eq), [Var(x), Var(y)])
        let expr = CoreExpr::App {
            func: Box::new(CoreExpr::Var {
                var: CoreVarRef::unresolved(eq_method),
                span: s(),
            }),
            args: vec![
                CoreExpr::bound_var(&x_binder, s()),
                CoreExpr::bound_var(&y_binder, s()),
            ],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map, &ClassEnv::new());

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
            args: vec![CoreExpr::bound_var(&x_binder, s())],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map, &ClassEnv::new());

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
        method_map.insert(
            eq_method,
            vec![single_candidate(dict_binder, vec![0_usize])],
        );

        // App(Var(eq, binder=99), [Lit(1)]) — bound var with class method name.
        let expr = CoreExpr::App {
            func: Box::new(CoreExpr::bound_var(&local_eq, s())),
            args: vec![CoreExpr::Lit(CoreLit::Int(1), s())],
            span: s(),
        };

        let rewritten = rewrite_body_with_dicts(expr, &method_map, &ClassEnv::new());

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
                is_dict_def: false,
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
                class_id: crate::types::class_id::ClassId::from_local_name(eq_sym),
                type_args: vec![InferType::Var(0)],
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
                    param_types: vec![],
                    result_ty: None,
                    body: Box::new(CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::unresolved(eq_method),
                            span: s(),
                        }),
                        args: vec![
                            CoreExpr::bound_var(&x_binder, s()),
                            CoreExpr::bound_var(&y_binder, s()),
                        ],
                        span: s(),
                    }),
                    span: s(),
                },
                is_dict_def: false,
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

        // No concrete dictionary is referenced at a call site, so the pass
        // only threads the incoming dictionary parameter through `contains`.
        assert_eq!(program.defs.len(), 1);

        let contains_def = &program.defs[0];
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
}
