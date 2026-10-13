/// Direct AST → Core IR lowering (runs immediately after HM type inference).
///
/// This is the primary Core IR entry point. By operating directly on the typed
/// AST rather than the intermediate `IrTopLevelItem` representation, it:
///
/// 1. Puts Core IR in the right pipeline position:
///    `HM inference → Core IR → passes → backend lowering`
///
/// 2. Gives every binder access to its HM-inferred type (via `hm_expr_types`),
///    enabling type-driven passes such as:
///    - Emitting `IAdd`/`IDiv` instead of generic `Add`/`Div`
///    - Deciding which values need reference-count operations
///    - Detecting integer vs float arithmetic
///
/// All surface Flux constructs (sugar, n-ary functions, multi-argument calls,
/// pattern matching, effects) are desugared into the ~12-variant `CoreExpr`.
use std::collections::HashMap;

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier, block::Block, expression::ExprId, program::Program, statement::Statement,
    },
    types::{infer_type::InferType, type_subst::TypeSubst},
};

use super::{
    CoreAlt, CoreBinder, CoreDef, CoreExpr, CoreLit, CorePrimOp, CoreProgram, CoreTopLevelItem,
};

mod binder_resolution;
mod dict_params;
mod emit;
mod expression;
mod pattern;
mod specialize;

use binder_resolution::{resolve_program_binders, validate_program_binders};
pub use specialize::specialization_clone_names;

/// Pre-resolved effect operation signatures: `(effect, operation) → (param_types, return_type)`.
/// Used by the lowerer to assign `FluxRep` to handler arm binders.
pub type EffectOpSigs = HashMap<(Identifier, Identifier), (Vec<InferType>, InferType)>;

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a typed Flux `Program` into a `CoreProgram`.
///
/// `hm_expr_types` maps every `ExprId` (assigned by the parser) to its
/// HM-inferred type. The lowering uses this to guide type-directed decisions.
pub fn lower_program_ast(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
) -> CoreProgram {
    lower_program_ast_with_interner(program, hm_expr_types, None)
}

/// Lower with an optional interner for resolving source-level type annotations
/// on function return types.  When the interner is available, annotated return
/// types (e.g. `-> Int`) can fill in `CoreDef::result_ty` even when HM
/// inference leaves the type unresolved (common for recursive functions).
pub fn lower_program_ast_with_interner(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
) -> CoreProgram {
    lower_program_ast_full(program, hm_expr_types, interner, None)
}

/// Lower with both interner and TypeEnv for typed binder creation.
/// When the TypeEnv is available, function parameters get their FluxRep
/// from HM-inferred types instead of defaulting to TaggedRep.
pub fn lower_program_ast_full(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
    type_env: Option<&crate::types::type_env::TypeEnv>,
) -> CoreProgram {
    lower_program_ast_complete(program, hm_expr_types, interner, type_env, None)
}

/// Lower with interner, TypeEnv, and effect op signatures for fully typed binders.
/// This is the most complete entry point — all optional context is available.
pub fn lower_program_ast_complete(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
    type_env: Option<&crate::types::type_env::TypeEnv>,
    effect_op_sigs: Option<&EffectOpSigs>,
) -> CoreProgram {
    lower_program_ast_with_class_env(
        program,
        hm_expr_types,
        interner,
        type_env,
        effect_op_sigs,
        None,
    )
}

/// Lower with all context including ClassEnv for compile-time class method dispatch.
/// When ClassEnv is available, calls to class methods are resolved to mangled
/// instance functions at compile time, eliminating runtime `type_of()` dispatch.
pub fn lower_program_ast_with_class_env(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
    type_env: Option<&crate::types::type_env::TypeEnv>,
    effect_op_sigs: Option<&EffectOpSigs>,
    class_env: Option<&crate::types::class_env::ClassEnv>,
) -> CoreProgram {
    lower_program_ast_with_evidence(
        program,
        hm_expr_types,
        interner,
        type_env,
        effect_op_sigs,
        class_env,
        None,
    )
    .0
}

/// The complete entry point: everything above, plus the solver's evidence.
///
/// `evidence` is the whole-program [`EvidenceMap`](crate::types::evidence::EvidenceMap).
/// Only the compiler has one; every other entry point passes `None`, and then
/// no dictionaries are passed at all. Dictionary parameters, arguments and
/// method calls come from it alone (0.0.8 plan, step 1e), and a site it gives
/// no usable answer for is one of the returned internal errors.
pub fn lower_program_ast_with_evidence(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
    type_env: Option<&crate::types::type_env::TypeEnv>,
    effect_op_sigs: Option<&EffectOpSigs>,
    class_env: Option<&crate::types::class_env::ClassEnv>,
    evidence: Option<&crate::types::evidence::EvidenceMap>,
) -> (CoreProgram, Vec<crate::diagnostics::Diagnostic>) {
    let mut lowerer = AstLowerer::new(
        hm_expr_types,
        interner,
        type_env,
        effect_op_sigs,
        class_env,
        evidence,
    );
    lowerer.collect_ctor_field_names(program);
    lowerer.collect_local_function_names(program);
    lowerer.collect_specializations(program);
    let mut defs = Vec::new();
    let mut top_level_items = Vec::new();
    for stmt in &program.statements {
        // Binder ids are handed out from one counter, so everything lowered
        // from this statement occupies a contiguous range. That is what lets
        // each definition carry its own binder types rather than a copy of
        // the whole program's.
        let first_def = defs.len();
        let binders = lowerer.next_binder_id;
        lowerer.lower_top_level(stmt, &mut defs, &mut top_level_items);
        let range = binders..lowerer.next_binder_id;
        if !range.is_empty() {
            let owned: HashMap<super::CoreBinderId, super::CoreType> = lowerer
                .binder_types
                .iter()
                .filter(|(id, _)| range.contains(&id.0))
                .map(|(id, ty)| (*id, ty.clone()))
                .collect();
            for def in &mut defs[first_def..] {
                def.binder_types.clone_from(&owned);
            }
        }
    }
    let mut core = CoreProgram {
        defs,
        top_level_items,
    };
    let is_dict = |name: Identifier| {
        interner
            .and_then(|interner| interner.try_resolve(name))
            .is_some_and(crate::types::class_env::is_dictionary_name)
    };
    resolve_program_binders(&mut core, &is_dict);
    assert!(
        validate_program_binders(&core, &is_dict),
        "Core binder resolution invariant failed after AST→Core lowering"
    );
    (core, lowerer.errors)
}

/// Put a definition's dictionary parameters ahead of its own, as
/// elaboration's `prepend_lam_params` would. Parameter types stay empty when
/// the definition had none, rather than becoming a list too short for the
/// parameters.
fn with_dict_params(
    mut dict_params: Vec<CoreBinder>,
    params: Vec<CoreBinder>,
    param_types: Vec<Option<super::CoreType>>,
) -> (Vec<CoreBinder>, Vec<Option<super::CoreType>>) {
    if dict_params.is_empty() {
        return (params, param_types);
    }
    let param_types = if param_types.is_empty() {
        param_types
    } else {
        let mut padded = vec![None; dict_params.len()];
        padded.extend(param_types);
        padded
    };
    dict_params.extend(params);
    (dict_params, param_types)
}

/// Collects, for each user-written function, the instantiated type its call
/// sites give it — ignoring self-calls, which are at the definition's own type
/// by construction and so are never a second instantiation.
struct CallSiteInstantiations<'a> {
    hm_expr_types: &'a HashMap<ExprId, InferType>,
    user_fns: &'a std::collections::HashSet<Identifier>,
    enclosing: Option<Identifier>,
    /// Each call site's callee `ExprId` and the type it instantiates the
    /// callee at, in source order.
    sites: HashMap<Identifier, Vec<(ExprId, InferType)>>,
    /// Parameter names of each function, in declaration order.
    params: HashMap<Identifier, Vec<Identifier>>,
    /// Functions that declare a constrained type parameter, and so receive
    /// dictionaries.
    constrained: std::collections::HashSet<Identifier>,
    /// The *generalized* type of each parameter, read from an occurrence of it
    /// inside its own function's body.
    ///
    /// This is what makes the substitution computable without a `TypeEnv`: the
    /// generic side of the unification is recoverable from the body, so the
    /// specialisation happens on every lowering path rather than only the one
    /// that happens to carry a scheme table.
    param_types: HashMap<(Identifier, Identifier), InferType>,
}

impl<'ast> crate::ast::visit::Visitor<'ast> for CallSiteInstantiations<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::Function {
            name,
            parameters,
            type_params,
            ..
        } = stmt
        {
            self.params.insert(*name, parameters.clone());
            if type_params
                .iter()
                .any(|param| !param.constraints.is_empty())
            {
                self.constrained.insert(*name);
            }
            let outer = self.enclosing.replace(*name);
            crate::ast::visit::walk_stmt(self, stmt);
            self.enclosing = outer;
            return;
        }
        crate::ast::visit::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast crate::syntax::expression::Expression) {
        use crate::syntax::expression::Expression;
        if let Expression::Call { function, .. } = expr
            && let Expression::Identifier { name, .. } = function.as_ref()
            && self.user_fns.contains(name)
            && Some(*name) != self.enclosing
            && let Some(callee_ty) = self.hm_expr_types.get(&function.expr_id())
        {
            self.sites
                .entry(*name)
                .or_default()
                .push((function.expr_id(), callee_ty.clone()));
        }
        if let Expression::Identifier { name, .. } = expr
            && let Some(enclosing) = self.enclosing
            && self
                .params
                .get(&enclosing)
                .is_some_and(|ps| ps.contains(name))
            && let Some(ty) = self.hm_expr_types.get(&expr.expr_id())
        {
            self.param_types
                .entry((enclosing, *name))
                .or_insert_with(|| ty.clone());
        }
        crate::ast::visit::walk_expr(self, expr);
    }
}

/// The parts of a `Statement::Function` that lowering a definition reads.
struct FunctionStmt<'s> {
    name: Identifier,
    parameters: &'s [Identifier],
    body: &'s Block,
    fip: Option<crate::syntax::statement::FipAnnotation>,
    intrinsic: Option<CorePrimOp>,
    span: Span,
    return_type: Option<&'s crate::syntax::type_expr::TypeExpr>,
    id: ExprId,
}

// ── Lowerer ───────────────────────────────────────────────────────────────────

pub(super) struct AstLowerer<'a> {
    /// HM-inferred types keyed by ExprId — used for type-directed code selection.
    pub(super) hm_expr_types: &'a HashMap<ExprId, InferType>,
    /// Counter for synthesizing fresh binding names.
    pub(super) fresh: u32,
    pub(super) next_binder_id: u32,
    /// HM-inferred type of each binder created with a known expression type.
    ///
    /// Core keeps only a `FluxRep` on a binder, which cannot tell one rigid
    /// parameter from another. Dictionary elaboration needs the real type to
    /// pick between two dictionaries for one class, so it is recorded here and
    /// handed to each `CoreDef` (Proposal 0179 Stage 4).
    pub(super) binder_types: HashMap<super::CoreBinderId, super::CoreType>,
    /// Optional interner for resolving source-level type annotations.
    pub(super) interner: Option<&'a crate::syntax::interner::Interner>,
    /// Optional TypeEnv for looking up function parameter types (Phase 7).
    type_env: Option<&'a crate::types::type_env::TypeEnv>,
    /// Optional effect op signatures for typed handler binders (Phase 7f).
    pub(super) effect_op_sigs: Option<&'a EffectOpSigs>,
    /// Optional ClassEnv for compile-time class method dispatch (Phase 4 Step 5).
    pub(super) class_env: Option<&'a crate::types::class_env::ClassEnv>,
    pub(super) local_function_names: std::collections::HashSet<Identifier>,
    /// The names this unit binds to a *user-written* top-level or module-level
    /// function.
    ///
    /// A subset of [`Self::local_function_names`], excluding the functions
    /// dispatch generation synthesises — instance bodies and the panicking
    /// per-method stub, which carry `Span::default()` where a parsed
    /// declaration carries a real one. Used to keep a bare call to a name the
    /// program itself defines from being rewritten into a class-method
    /// dispatch; see docs/known_issues.md#ki-085.
    pub(super) user_function_names: std::collections::HashSet<Identifier>,
    /// The definitions with evidence parameters whose bodies are being
    /// lowered, innermost last. A call to one of them that raised no
    /// predicate passes it nothing, which in `Emit` mode is an internal error
    /// (KI-101).
    pub(super) constrained_bodies: Vec<Identifier>,
    /// Set by [`Self::evidence_dict_params`] for the definition whose body
    /// is lowered next.
    pub(super) next_body_constrained: Option<Identifier>,
    pub(super) current_function_name: Option<Identifier>,
    current_module_name: Option<Identifier>,
    /// Proposal 0152: variant name → declared field names in declaration
    /// order. Used to desugar named-field constructor expressions, spread
    /// expressions, and named-field patterns into positional form.
    pub(super) ctor_field_names:
        std::collections::HashMap<crate::syntax::Identifier, Vec<crate::syntax::Identifier>>,
    /// Proposal 0152: ADT name → list of variant names. Used when
    /// desugaring dot access on multi-variant ADTs.
    pub(super) adt_variants:
        std::collections::HashMap<crate::syntax::Identifier, Vec<crate::syntax::Identifier>>,
    /// 0187 B1: for each generalized function whose call sites all agree on one
    /// concrete instantiation, the substitution that reaches it.
    ///
    /// Generalizing an unannotated helper replaces a concrete parameter type
    /// with a variable, and a variable has no `FluxRep` — so `IntRep` is lost
    /// and Aether must emit a `DropSpecialized` it previously proved
    /// unnecessary. The type is still known: it is recorded at the *call site*,
    /// which is why this is computed here and not in a `core/` pass, where
    /// `CoreExpr` carries no `ExprId` to look one up by.
    specializations: HashMap<Identifier, TypeSubst>,
    /// 0.0.8 plan step 2a: the clones of each function used at several
    /// instantiations, emitted next to its generic definition.
    clones: HashMap<Identifier, Vec<specialize::SpecClone>>,
    /// The clone each call site calls, by the callee's `ExprId`.
    call_targets: HashMap<ExprId, Identifier>,
    /// While a clone's body is lowered, the function it clones and the
    /// clone's name, so its self-calls call the clone.
    active_clone: Option<(Identifier, Identifier)>,
    /// The substitution in force while lowering one specialised body.
    active_subst: Option<TypeSubst>,
    /// The solver's evidence, by the site that raised each predicate
    /// (0186 stage 5).
    ///
    /// Lowering is the last place an `ExprId` is in hand, so it is the only
    /// place this map can be read: `CoreExpr` carries no id to look a site up
    /// by. `None` on the paths that lower without a whole-program solve.
    pub(super) evidence: Option<&'a crate::types::evidence::EvidenceMap>,
    /// The binder of each dictionary parameter, by its owner and its index
    /// among the owner's evidence parameters (0.0.8 plan, step 1d).
    dict_param_binders: HashMap<(ExprId, u16), CoreBinder>,
    /// Internal errors: sites lowering found no usable evidence for.
    errors: Vec<crate::diagnostics::Diagnostic>,
}

impl<'a> AstLowerer<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        hm_expr_types: &'a HashMap<ExprId, InferType>,
        interner: Option<&'a crate::syntax::interner::Interner>,
        type_env: Option<&'a crate::types::type_env::TypeEnv>,
        effect_op_sigs: Option<&'a EffectOpSigs>,
        class_env: Option<&'a crate::types::class_env::ClassEnv>,
        evidence: Option<&'a crate::types::evidence::EvidenceMap>,
    ) -> Self {
        Self {
            hm_expr_types,
            fresh: 0,
            next_binder_id: 0,
            binder_types: HashMap::new(),
            interner,
            type_env,
            effect_op_sigs,
            class_env,
            local_function_names: std::collections::HashSet::new(),
            user_function_names: std::collections::HashSet::new(),
            constrained_bodies: Vec::new(),
            next_body_constrained: None,
            current_function_name: None,
            current_module_name: None,
            ctor_field_names: std::collections::HashMap::new(),
            adt_variants: std::collections::HashMap::new(),
            specializations: HashMap::new(),
            clones: HashMap::new(),
            call_targets: HashMap::new(),
            active_clone: None,
            active_subst: None,
            evidence,
            dict_param_binders: HashMap::new(),
            errors: Vec::new(),
        }
    }

    fn lower_function_body_for_name(
        &mut self,
        name: Identifier,
        intrinsic: Option<CorePrimOp>,
        params: &[CoreBinder],
        body: &Block,
        span: Span,
    ) -> CoreExpr {
        let previous = self.current_function_name.replace(name);
        let constrained = self.next_body_constrained.take() == Some(name);
        if constrained {
            self.constrained_bodies.push(name);
        }
        let body_expr = self.lower_function_body(intrinsic, params, body, span);
        if constrained {
            self.constrained_bodies.pop();
        }
        self.current_function_name = previous;
        body_expr
    }

    fn collect_local_function_names(&mut self, program: &Program) {
        fn walk(
            stmts: &[Statement],
            out: &mut std::collections::HashSet<Identifier>,
            user: &mut std::collections::HashSet<Identifier>,
        ) {
            for stmt in stmts {
                match stmt {
                    Statement::Function { name, span, .. } => {
                        out.insert(*name);
                        // A generated instance body or dispatch stub is built
                        // with `Span::default()`; anything parsed from source
                        // has a real one. Type inference separates the two the
                        // same way, in `class_method_call_info`.
                        if *span != Span::default() {
                            user.insert(*name);
                        }
                    }
                    Statement::Module { body, .. } => walk(&body.statements, out, user),
                    _ => {}
                }
            }
        }
        walk(
            &program.statements,
            &mut self.local_function_names,
            &mut self.user_function_names,
        );
    }

    /// 0187 B1 and 0.0.8 plan step 2a: decide which functions are
    /// specialised, in place or by cloning (see [`specialize`]).
    ///
    /// A clone whose name was not interned before lowering is left out, and
    /// its call sites keep the generic definition.
    pub(super) fn collect_specializations(&mut self, program: &Program) {
        let plan = self.plan_specializations(program);
        let debug = std::env::var("FLUX_DBG_SPEC").is_ok();
        let shown = |name: Identifier| {
            self.interner
                .map(|i| i.resolve(name).to_string())
                .unwrap_or_else(|| format!("{name:?}"))
        };
        if debug {
            for (name, subst) in &plan.in_place {
                eprintln!("SPEC: {} specialised in place by {subst:?}", shown(*name));
            }
            for (name, call_ty) in &plan.missed {
                eprintln!(
                    "SPEC-MISS: {} at {call_ty}: over the cap of {} clones",
                    shown(*name),
                    specialize::MAX_CLONES_PER_FUNCTION
                );
            }
        }
        let mut clones: HashMap<Identifier, Vec<specialize::SpecClone>> = HashMap::new();
        for (name, planned) in plan.clones {
            for clone in planned {
                let Some(symbol) = self.interner.and_then(|i| i.lookup(&clone.name)) else {
                    if debug {
                        eprintln!("SPEC-MISS: {}: name not interned", clone.name);
                    }
                    continue;
                };
                if debug {
                    eprintln!("SPEC: {} cloned at {}", clone.name, clone.example);
                }
                for site in clone.sites {
                    self.call_targets.insert(site, symbol);
                }
                clones.entry(name).or_default().push(specialize::SpecClone {
                    name: symbol,
                    subst: clone.subst,
                });
            }
        }
        self.specializations = plan.in_place;
        self.clones = clones;
    }

    /// Derive a specialisation substitution by matching each parameter's
    /// generalized type against the type the call site gives it.
    ///
    /// Used when no `TypeEnv` is available — the function's own type is not
    /// recorded anywhere keyed by name, but each parameter's is, at the
    /// occurrences inside its body. A parameter the body never mentions
    /// contributes nothing and is skipped rather than treated as a failure.
    fn subst_from_parameters(
        name: Identifier,
        call_ty: &InferType,
        params: &HashMap<Identifier, Vec<Identifier>>,
        param_types: &HashMap<(Identifier, Identifier), InferType>,
    ) -> Option<TypeSubst> {
        let InferType::Fun(call_params, _, _) = call_ty else {
            return None;
        };
        let declared = params.get(&name)?;
        if declared.len() != call_params.len() {
            return None;
        }
        let mut subst = TypeSubst::empty();
        for (param, concrete) in declared.iter().zip(call_params.iter()) {
            let Some(generic) = param_types.get(&(name, *param)) else {
                continue;
            };
            let generic = generic.apply_type_subst(&subst);
            if let Ok(step) = crate::types::unify::unify(&generic, concrete) {
                subst = subst.compose(&step);
            }
        }
        (!subst.is_empty()).then_some(subst)
    }

    /// Proposal 0152: populate `ctor_field_names` from a program's `data`
    /// declarations. Walks both top-level and module-nested statements.
    pub(super) fn collect_ctor_field_names(&mut self, program: &Program) {
        fn walk(
            stmts: &[Statement],
            field_names: &mut std::collections::HashMap<Identifier, Vec<Identifier>>,
            adt_variants: &mut std::collections::HashMap<Identifier, Vec<Identifier>>,
        ) {
            for stmt in stmts {
                match stmt {
                    Statement::Data { name, variants, .. } => {
                        adt_variants.insert(*name, variants.iter().map(|v| v.name).collect());
                        for variant in variants {
                            if let Some(names) = variant.field_names.as_ref() {
                                field_names.insert(variant.name, names.clone());
                            }
                        }
                    }
                    Statement::Module { body, .. } => {
                        walk(&body.statements, field_names, adt_variants);
                    }
                    _ => {}
                }
            }
        }
        walk(
            &program.statements,
            &mut self.ctor_field_names,
            &mut self.adt_variants,
        );
    }

    pub(super) fn bind_name(&mut self, name: crate::syntax::Identifier) -> CoreBinder {
        let id = super::CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        CoreBinder::new(id, name)
    }

    /// Create a binder with a known runtime representation from HM type info.
    pub(super) fn bind_name_with_expr_type(
        &mut self,
        name: crate::syntax::Identifier,
        expr_id: ExprId,
    ) -> CoreBinder {
        let id = super::CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        let rep = self.rep_for_expr(expr_id);
        // Keep the full type too: `rep` collapses every boxed type to
        // `TaggedRep`, which cannot tell one rigid parameter from another.
        if let Some(ty) = self
            .hm_type(expr_id)
            .as_ref()
            .and_then(super::CoreType::try_from_infer)
        {
            self.binder_types.insert(id, ty);
        }
        CoreBinder::with_rep(id, name, rep)
    }

    /// The HM type recorded for an expression, under any specialisation in
    /// force.
    ///
    /// Every type-directed lowering decision must read types through here
    /// rather than from `hm_expr_types` directly: inside a specialised body the
    /// recorded type still names the generalized variable, and the concrete
    /// type is only reached by applying the substitution.
    pub(super) fn hm_type(&self, id: ExprId) -> Option<InferType> {
        let ty = self.hm_expr_types.get(&id)?;
        Some(match self.active_subst.as_ref() {
            Some(subst) => ty.apply_type_subst(subst),
            None => ty.clone(),
        })
    }

    /// Get the `FluxRep` for an expression from HM type info.
    pub(super) fn rep_for_expr(&self, id: ExprId) -> super::FluxRep {
        self.hm_type(id)
            .map(|ty| super::FluxRep::from_infer_type(&ty))
            .unwrap_or(super::FluxRep::TaggedRep)
    }

    pub(super) fn fresh_binder(&mut self, name: crate::syntax::Identifier) -> CoreBinder {
        self.bind_name(name)
    }

    /// Create a binder with a rep derived from a known `InferType`.
    pub(super) fn bind_name_with_type(
        &mut self,
        name: crate::syntax::Identifier,
        ty: &InferType,
    ) -> CoreBinder {
        let id = super::CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        let rep = super::FluxRep::from_infer_type(ty);
        CoreBinder::with_rep(id, name, rep)
    }

    /// Create binders whose runtime representation is derived from HM param types.
    pub(super) fn bind_lambda_params(
        &mut self,
        parameters: &[crate::syntax::Identifier],
        lambda_expr_id: crate::syntax::expression::ExprId,
    ) -> Vec<CoreBinder> {
        if let Some(fn_ty) = self.hm_expr_types.get(&lambda_expr_id) {
            let param_types = fn_ty.param_types();
            if param_types.len() == parameters.len() && !param_types.is_empty() {
                return parameters
                    .iter()
                    .zip(param_types)
                    .map(|(&p, ty)| self.bind_name_with_type(p, ty))
                    .collect();
            }
        }
        parameters.iter().map(|&p| self.bind_name(p)).collect()
    }

    /// Look up a function's parameter types from the TypeEnv and create
    /// typed binders. Falls back to untyped binders if TypeEnv is unavailable.
    fn bind_fn_params(
        &mut self,
        fn_name: crate::syntax::Identifier,
        parameters: &[crate::syntax::Identifier],
    ) -> Vec<CoreBinder> {
        // Try to get parameter types from the function's HM scheme.
        if let Some(scheme) = self.type_env.and_then(|env| env.lookup(fn_name)) {
            let param_types = scheme.infer_type.param_types();
            if param_types.len() == parameters.len() && !param_types.is_empty() {
                let param_types: Vec<InferType> = match self.active_subst.as_ref() {
                    Some(subst) => param_types
                        .iter()
                        .map(|ty| ty.apply_type_subst(subst))
                        .collect(),
                    None => param_types.to_vec(),
                };
                return parameters
                    .iter()
                    .zip(param_types.iter())
                    .map(|(&p, ty)| self.bind_name_with_type(p, ty))
                    .collect();
            }
        }
        // Fallback: untyped binders
        parameters.iter().map(|&p| self.bind_name(p)).collect()
    }

    /// Allocate a fresh synthetic `Identifier` for compiler-generated bindings.
    /// Used by future passes that need to introduce new named temporaries.
    #[allow(dead_code)]
    pub(super) fn fresh_name(
        &mut self,
        interner: &mut crate::syntax::interner::Interner,
    ) -> crate::syntax::Identifier {
        let id = self.fresh;
        self.fresh += 1;
        interner.intern(&format!("$tmp{id}"))
    }

    /// Look up the HM-inferred type for an expression, if available.
    /// Intended for type-driven passes (e.g. emitting `IAdd` vs `Add`).
    #[allow(dead_code)]
    pub(super) fn expr_type(&self, id: ExprId) -> Option<&InferType> {
        self.hm_expr_types.get(&id)
    }

    /// Convert an HM-inferred expression type to a `CoreType`, if available.
    fn infer_core_type(&self, id: ExprId) -> Option<super::CoreType> {
        self.hm_expr_types
            .get(&id)
            .and_then(super::CoreType::try_from_infer)
    }

    fn lambda_signature_from_infer_type(
        &self,
        ty: Option<&InferType>,
    ) -> (Vec<Option<super::CoreType>>, Option<super::CoreType>) {
        match ty {
            Some(InferType::Fun(params, ret, _)) => (
                params.iter().map(super::CoreType::try_from_infer).collect(),
                super::CoreType::try_from_infer(ret),
            ),
            _ => (Vec::new(), None),
        }
    }

    fn lambda_signature_from_expr_id(
        &self,
        id: ExprId,
    ) -> (Vec<Option<super::CoreType>>, Option<super::CoreType>) {
        self.lambda_signature_from_infer_type(self.hm_expr_types.get(&id))
    }

    fn lambda_signature_from_function_name(
        &self,
        fn_name: crate::syntax::Identifier,
    ) -> (Vec<Option<super::CoreType>>, Option<super::CoreType>) {
        self.lambda_signature_from_infer_type(
            self.type_env
                .and_then(|env| env.lookup(fn_name))
                .map(|scheme| &scheme.infer_type),
        )
    }

    fn full_function_core_type_from_name(
        &self,
        fn_name: crate::syntax::Identifier,
    ) -> Option<super::CoreType> {
        self.type_env
            .and_then(|env| env.lookup(fn_name))
            .and_then(|scheme| super::CoreType::try_from_infer(&scheme.infer_type))
    }

    /// Convert a source-level type annotation to a `CoreType`, if the interner
    /// is available. This preserves container/tuple/function structure and
    /// keeps source-level abstract names explicit instead of dropping them.
    fn core_type_from_type_expr(
        &self,
        type_expr: &crate::syntax::type_expr::TypeExpr,
    ) -> Option<super::CoreType> {
        let interner = self.interner?;
        match type_expr {
            crate::syntax::type_expr::TypeExpr::Named { name, args, .. } => {
                let args = args
                    .iter()
                    .map(|arg| self.core_type_from_type_expr(arg))
                    .collect::<Option<Vec<_>>>()?;
                let resolved = interner.resolve(*name);
                Some(match resolved {
                    "Int" if args.is_empty() => super::CoreType::Int,
                    "Float" if args.is_empty() => super::CoreType::Float,
                    "Bool" if args.is_empty() => super::CoreType::Bool,
                    "String" if args.is_empty() => super::CoreType::String,
                    "Unit" if args.is_empty() => super::CoreType::Unit,
                    "Never" if args.is_empty() => super::CoreType::Never,
                    "List" if args.len() == 1 => super::CoreType::List(Box::new(args[0].clone())),
                    "Array" if args.len() == 1 => super::CoreType::Array(Box::new(args[0].clone())),
                    "Option" if args.len() == 1 => {
                        super::CoreType::Option(Box::new(args[0].clone()))
                    }
                    "Either" if args.len() == 2 => super::CoreType::Either(
                        Box::new(args[0].clone()),
                        Box::new(args[1].clone()),
                    ),
                    "Map" if args.len() == 2 => {
                        super::CoreType::Map(Box::new(args[0].clone()), Box::new(args[1].clone()))
                    }
                    _ if resolved.chars().next().is_some_and(|ch| ch.is_lowercase()) => {
                        super::CoreType::Abstract(super::CoreAbstractType::Named(*name, args))
                    }
                    _ => super::CoreType::Adt(*name, args),
                })
            }
            crate::syntax::type_expr::TypeExpr::Tuple { elements, .. } => {
                Some(super::CoreType::Tuple(
                    elements
                        .iter()
                        .map(|elem| self.core_type_from_type_expr(elem))
                        .collect::<Option<Vec<_>>>()?,
                ))
            }
            crate::syntax::type_expr::TypeExpr::Function { params, ret, .. } => {
                Some(super::CoreType::Function(
                    params
                        .iter()
                        .map(|param| self.core_type_from_type_expr(param))
                        .collect::<Option<Vec<_>>>()?,
                    Box::new(self.core_type_from_type_expr(ret)?),
                ))
            }
        }
    }

    // ── Top-level statements ─────────────────────────────────────────────────

    fn lower_function_body(
        &mut self,
        intrinsic: Option<CorePrimOp>,
        params: &[CoreBinder],
        body: &Block,
        span: Span,
    ) -> CoreExpr {
        if let Some(op) = intrinsic {
            CoreExpr::PrimOp {
                op,
                args: params
                    .iter()
                    .map(|param| CoreExpr::bound_var(param, span))
                    .collect(),
                span,
            }
        } else {
            self.lower_block(body)
        }
    }

    /// Lower a named function to the definition `def_name`, under `subst`.
    ///
    /// `def_name` is the function's own name, or one of its clones' (step
    /// 2a); everything read from the type environment is read under the
    /// function's own name.
    fn lower_function_def(
        &mut self,
        function: &FunctionStmt<'_>,
        def_name: Identifier,
        subst: Option<TypeSubst>,
    ) -> CoreDef {
        let name = function.name;
        let span = function.span;
        // Set before the parameters are bound — their reps are the whole
        // point.
        let restore_subst = std::mem::replace(&mut self.active_subst, subst);
        let binder = self.bind_name(def_name);
        let params = self.bind_fn_params(name, function.parameters);
        let (mut param_types, inferred_result_ty) = self.lambda_signature_from_function_name(name);
        // Guard: param_types must match params length. If the scheme's
        // Fun type has a different arity (e.g., from cross-function
        // inlining or module-scoped name shadowing), discard param_types
        // to prevent LamParamTypeMismatch.
        if !param_types.is_empty() && param_types.len() != params.len() {
            param_types = Vec::new();
        }
        // Before the body: its method calls read these binders.
        let dict_params = self.evidence_dict_params(function.id, name, &params, span);
        let body_expr = self.lower_function_body_for_name(
            name,
            function.intrinsic,
            &params,
            function.body,
            span,
        );
        let (params, param_types) = with_dict_params(dict_params, params, param_types);
        // Always wrap in Lam, even for parameterless functions — the
        // Core→IR lowerer uses the Lam marker to distinguish function
        // definitions from value bindings.  We construct Lam directly
        // instead of using CoreExpr::lambda() which elides empty params.
        let expr = CoreExpr::Lam {
            params,
            param_types,
            result_ty: inferred_result_ty.clone(),
            body: Box::new(body_expr),
            span,
        };
        // For functions, build a Function CoreType from body's last expression type.
        // The function itself doesn't have a single ExprId, but the body block does.
        let mut def = CoreDef::new(binder, expr, true, span);
        def.fip = function.fip;
        // Preserve the full inferred function type for semantic/debug
        // Core dumps. Downstream lowering can peel `forall` wrappers
        // when it needs concrete parameter/return metadata.
        def.result_ty = self
            .full_function_core_type_from_name(name)
            .or_else(|| inferred_result_ty.clone());
        // If HM didn't resolve the result type, fall back to the
        // source return-type annotation.  This is critical for
        // recursive functions where HM may leave the return type as
        // an unresolved variable.
        if def.result_ty.is_none()
            && let Some(rt) = function.return_type
            && let Some(annotated_ty) = self.core_type_from_type_expr(rt)
        {
            def.result_ty = Some(annotated_ty);
        }
        self.active_subst = restore_subst;
        def
    }

    fn lower_top_level(
        &mut self,
        stmt: &Statement,
        out: &mut Vec<CoreDef>,
        top_level_items: &mut Vec<CoreTopLevelItem>,
    ) {
        if let Some(item) = self.lower_decl_item(stmt) {
            top_level_items.push(item);
        }
        match stmt {
            // Named function definition → CoreDef wrapping a curried Lam.
            Statement::Function {
                name,
                parameters,
                body,
                fip,
                intrinsic,
                span,
                return_type,
                id,
                ..
            } => {
                let function = FunctionStmt {
                    name: *name,
                    parameters,
                    body,
                    fip: *fip,
                    intrinsic: *intrinsic,
                    span: *span,
                    return_type: return_type.as_ref(),
                    id: *id,
                };
                // 0187 B1: lower this body under its single instantiation, if
                // it has one.
                let subst = self.specializations.get(name).cloned();
                out.push(self.lower_function_def(&function, *name, subst));
                // 2a: and once more per clone, under the clone's instantiation.
                for clone in self.clones.get(name).cloned().unwrap_or_default() {
                    let outer = self.active_clone.replace((*name, clone.name));
                    out.push(self.lower_function_def(&function, clone.name, Some(clone.subst)));
                    self.active_clone = outer;
                }
            }

            // Value binding → CoreDef for the RHS expression.
            Statement::Let {
                name, value, span, ..
            } => {
                let result_ty = self.infer_core_type(value.expr_id());
                let binder = self.bind_name_with_expr_type(*name, value.expr_id());
                let mut def = CoreDef::new(binder, self.lower_expr(value), false, *span);
                def.result_ty = result_ty;
                out.push(def);
            }

            // Assignment (mutable rebind) → treat as a new CoreDef.
            Statement::Assign { name, value, span } => {
                let result_ty = self.infer_core_type(value.expr_id());
                let binder = self.bind_name_with_expr_type(*name, value.expr_id());
                let mut def = CoreDef::new(binder, self.lower_expr(value), false, *span);
                def.result_ty = result_ty;
                out.push(def);
            }

            // Expression statement → anonymous CoreDef (evaluated for effects).
            Statement::Expression {
                expression, span, ..
            } => {
                let result_ty = self.infer_core_type(expression.expr_id());
                let mut def = CoreDef::new_anonymous(
                    self.bind_name(crate::syntax::symbol::Symbol::new(0)),
                    self.lower_expr(expression),
                    false,
                    *span,
                );
                def.result_ty = result_ty;
                out.push(def);
            }

            // Destructuring let → synthetic tmp var + Case alt for the pattern.
            Statement::LetDestructure {
                pattern,
                value,
                span,
                ..
            } => {
                let rhs = self.lower_expr(value);
                let core_pat = self.lower_pattern(pattern);
                // Wrap: let $destructure = value in case $destructure of { pat -> () }
                // At the top level we emit this as one def per bound variable by
                // expanding the pattern into field accesses where possible.
                self.expand_destructure_top_level(core_pat, rhs, *span, out);
            }

            // Return at top level is unusual but syntactically valid in some contexts.
            Statement::Return { value, span } => {
                if let Some(val) = value {
                    out.push(CoreDef::new_anonymous(
                        self.bind_name(crate::syntax::symbol::Symbol::new(0)),
                        self.lower_expr(val),
                        false,
                        *span,
                    ));
                }
            }

            // Declarations that don't produce runtime values — skip.
            Statement::Module { name, body, .. } => {
                let previous_module = self.current_module_name.replace(*name);
                self.lower_module_defs(&body.statements, out);
                self.current_module_name = previous_module;
            }

            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::EffectAlias { .. }
            | Statement::TypeAlias(_)
            | Statement::Class { .. }
            | Statement::Instance { .. } => {}
        }
    }

    fn lower_module_defs(&mut self, stmts: &[Statement], out: &mut Vec<CoreDef>) {
        for stmt in stmts {
            match stmt {
                Statement::Let {
                    name, value, span, ..
                } => {
                    let result_ty = self.infer_core_type(value.expr_id());
                    let binder = self.bind_name_with_expr_type(*name, value.expr_id());
                    let mut def = CoreDef::new(binder, self.lower_expr(value), false, *span);
                    def.result_ty = result_ty;
                    out.push(def);
                }
                Statement::Function {
                    name,
                    parameters,
                    body,
                    intrinsic,
                    span,
                    id,
                    ..
                } => {
                    let binder = self.bind_name(*name);
                    let params = self.bind_fn_params(*name, parameters);
                    let (mut param_types, result_ty) =
                        self.lambda_signature_from_function_name(*name);
                    if !param_types.is_empty() && param_types.len() != params.len() {
                        param_types = Vec::new();
                    }
                    // Before the body: its method calls read these binders.
                    let dict_params = self.evidence_dict_params(*id, *name, &params, *span);
                    let body_expr =
                        self.lower_function_body_for_name(*name, *intrinsic, &params, body, *span);
                    let (params, param_types) = with_dict_params(dict_params, params, param_types);
                    let expr = CoreExpr::Lam {
                        params,
                        param_types,
                        result_ty,
                        body: Box::new(body_expr),
                        span: *span,
                    };
                    let def = CoreDef::new(binder, expr, true, *span);
                    out.push(def);
                }
                Statement::Module { name, body, .. } => {
                    let previous_module = self.current_module_name.replace(*name);
                    self.lower_module_defs(&body.statements, out);
                    self.current_module_name = previous_module;
                }
                _ => {}
            }
        }
    }

    fn lower_decl_item(&mut self, stmt: &Statement) -> Option<CoreTopLevelItem> {
        match stmt {
            Statement::Let {
                is_public,
                name,
                span,
                ..
            } => Some(CoreTopLevelItem::Let {
                is_public: *is_public,
                name: *name,
                span: *span,
            }),
            Statement::Function {
                is_public,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body: _,
                span,
                fip: _,
                intrinsic: _,
                id: _,
            } => Some(CoreTopLevelItem::Function {
                is_public: *is_public,
                name: *name,
                type_params: Statement::function_type_param_names(type_params),
                parameters: parameters.clone(),
                parameter_types: parameter_types.clone(),
                return_type: return_type.clone(),
                effects: effects.clone(),
                span: *span,
            }),
            Statement::Module { name, body, span } => Some(CoreTopLevelItem::Module {
                name: *name,
                body: body
                    .statements
                    .iter()
                    .filter_map(|item| self.lower_decl_item(item))
                    .collect(),
                span: *span,
            }),
            Statement::Import {
                name,
                alias,
                except,
                exposing,
                span,
            } => Some(CoreTopLevelItem::Import {
                name: *name,
                alias: *alias,
                except: except.clone(),
                exposing: exposing.clone(),
                span: *span,
            }),
            Statement::Data {
                // Proposal 0151: ADT visibility is enforced at the class
                // visibility walker; Core IR is visibility-blind.
                is_public: _,
                name,
                type_params,
                variants,
                span,
                deriving: _,
            } => Some(CoreTopLevelItem::Data {
                name: *name,
                type_params: type_params.clone(),
                variants: variants.clone(),
                span: *span,
            }),
            Statement::EffectDecl { name, ops, span } => Some(CoreTopLevelItem::EffectDecl {
                name: *name,
                ops: ops.clone(),
                span: *span,
            }),
            // Effect aliases (Proposal 0161 B1) are a compile-time-only
            // construct — they are consumed by the Compiler's alias table
            // before Core lowering runs, so the Core IR never sees them.
            Statement::EffectAlias { .. } => None,
            Statement::TypeAlias(_) => None,
            Statement::Class {
                // Proposal 0151: Core IR is currently visibility-blind. Phase
                // 2 will revisit whether `CoreTopLevelItem::Class` needs to
                // carry visibility for `.flxi` serialization; until then we
                // drop the field at the AST→Core boundary.
                is_public: _,
                name,
                type_params,
                superclasses,
                methods,
                span,
                ..
            } => Some(CoreTopLevelItem::Class {
                name: *name,
                type_params: type_params.clone(),
                superclasses: superclasses.clone(),
                methods: methods.clone(),
                span: *span,
            }),
            Statement::Instance {
                is_public: _,
                class_name,
                type_args,
                context,
                methods,
                // Type-level only: every application is reduced before Core.
                associated_types: _,
                span,
                name_span: _,
            } => Some(CoreTopLevelItem::Instance {
                class_name: *class_name,
                type_args: type_args.clone(),
                context: context.clone(),
                methods: methods.clone(),
                span: *span,
            }),
            Statement::LetDestructure { .. }
            | Statement::Return { .. }
            | Statement::Expression { .. }
            | Statement::Assign { .. } => None,
        }
    }

    // ── Block lowering ───────────────────────────────────────────────────────

    /// Lower a `Block` (sequence of statements) into a single `CoreExpr`.
    ///
    /// Each statement is desugared:
    /// - `let x = e; rest` → `Let(x, e, rest)`
    /// - `fn f(p) { b }; rest` → `LetRec(f, Lam(p, b), rest)`
    /// - `e; rest` → `Let($_, e, rest)`   (sequence — result is discarded)
    /// - last expression → the block's return value
    pub(super) fn lower_block(&mut self, block: &Block) -> CoreExpr {
        self.lower_stmts(&block.statements, block.span)
    }

    fn lower_stmts(&mut self, stmts: &[Statement], span: Span) -> CoreExpr {
        // Find where the "value" of the block comes from.
        // It's the last statement if it's a non-semicolon expression,
        // otherwise the block returns unit.
        match stmts {
            [] => CoreExpr::Lit(CoreLit::Unit, span),

            [single] => self.lower_stmt_as_expr(single, span),

            [rest @ .., last] => {
                let tail = self.lower_stmts(std::slice::from_ref(last), span);
                self.prepend_stmt(&rest[rest.len() - 1..], rest, tail, span)
            }
        }
    }

    /// Lower the full statement slice by prepending one statement at a time.
    ///
    /// Function definitions are grouped by mutual reference rather than by
    /// adjacency — see [`crate::binding_groups::plan_block`] — so a
    /// statement standing between two mutually recursive definitions no longer
    /// splits them into separate, wrongly nested bindings.
    fn prepend_stmts(&mut self, stmts: &[Statement], body: CoreExpr, span: Span) -> CoreExpr {
        let plan = crate::binding_groups::plan_block(stmts);

        // Right-to-left, so each item is prepended around the tail already built.
        let mut result = body;
        for item in plan.iter().rev() {
            result = match item {
                crate::binding_groups::PlanItem::Group { members, .. } => {
                    match members.as_slice() {
                        [(_, single)] => self.prepend_one_stmt(single, result, span),
                        many => {
                            let stmts: Vec<&Statement> = many.iter().map(|(_, s)| *s).collect();
                            self.lower_scc_group(&stmts, result)
                        }
                    }
                }
                crate::binding_groups::PlanItem::Other(_, stmt) => {
                    self.prepend_one_stmt(stmt, result, span)
                }
            };
        }
        result
    }

    /// Lower a multi-function SCC as a `LetRecGroup`.
    fn lower_scc_group(&mut self, stmts: &[&Statement], tail: CoreExpr) -> CoreExpr {
        let span = stmts
            .first()
            .map(|s| match s {
                Statement::Function { span, .. } => *span,
                _ => Span::default(),
            })
            .unwrap_or_default();

        // Every member's name must be bound before any body is lowered: that is
        // what makes the group recursive. Binding and lowering in one pass left
        // the first member's body referring to a name the later members had not
        // introduced yet.
        let binders: Vec<_> = stmts
            .iter()
            .map(|stmt| {
                let Statement::Function { name, .. } = stmt else {
                    unreachable!("lower_scc_group called with non-function statement");
                };
                self.bind_name(*name)
            })
            .collect();

        let bindings: Vec<_> = stmts
            .iter()
            .zip(binders)
            .map(|(stmt, binder)| {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    intrinsic,
                    span: s,
                    id,
                    ..
                } = stmt
                else {
                    unreachable!("lower_scc_group called with non-function statement");
                };
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let (mut param_types, result_ty) = self.lambda_signature_from_function_name(*name);
                if !param_types.is_empty() && param_types.len() != params.len() {
                    param_types = Vec::new();
                }
                let dict_params = self.evidence_dict_params(*id, *name, &params, *s);
                let body_expr =
                    self.lower_function_body_for_name(*name, *intrinsic, &params, body, *s);
                let (params, param_types) = with_dict_params(dict_params, params, param_types);
                (
                    binder,
                    Box::new(CoreExpr::Lam {
                        params,
                        param_types,
                        result_ty,
                        body: Box::new(body_expr),
                        span: *s,
                    }),
                )
            })
            .collect();

        CoreExpr::LetRecGroup {
            bindings,
            body: Box::new(tail),
            span,
        }
    }

    fn prepend_stmt(
        &mut self,
        _one: &[Statement],
        all_but_last: &[Statement],
        tail: CoreExpr,
        span: Span,
    ) -> CoreExpr {
        self.prepend_stmts(all_but_last, tail, span)
    }

    /// Wrap `tail` with the binding/effect introduced by a single statement.
    fn prepend_one_stmt(&mut self, stmt: &Statement, tail: CoreExpr, _span: Span) -> CoreExpr {
        match stmt {
            Statement::Let {
                name,
                value,
                span: s,
                ..
            } => CoreExpr::Let {
                var: self.bind_name_with_expr_type(*name, value.expr_id()),
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::Function { .. } => {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    intrinsic,
                    span: s,
                    id,
                    ..
                } = stmt
                else {
                    unreachable!();
                };
                let binder = self.bind_name(*name);
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let (mut param_types, result_ty) = self.lambda_signature_from_function_name(*name);
                if !param_types.is_empty() && param_types.len() != params.len() {
                    param_types = Vec::new();
                }
                let dict_params = self.evidence_dict_params(*id, *name, &params, *s);
                let body_expr =
                    self.lower_function_body_for_name(*name, *intrinsic, &params, body, *s);
                let (params, param_types) = with_dict_params(dict_params, params, param_types);
                CoreExpr::LetRec {
                    var: binder,
                    rhs: Box::new(CoreExpr::Lam {
                        params,
                        param_types,
                        result_ty,
                        body: Box::new(body_expr),
                        span: *s,
                    }),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Assign {
                name,
                value,
                span: s,
            } => CoreExpr::Let {
                var: self.bind_name_with_expr_type(*name, value.expr_id()),
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::LetDestructure {
                pattern,
                value,
                span: s,
                ..
            } => {
                // Bind the scrutinee to a tmp var and use Case to extract fields.
                let rhs = self.lower_expr(value);
                let core_pat = self.lower_pattern(pattern);
                // Build: Let($tmp, rhs, Case($tmp, [core_pat → tail]))
                // We use a special sentinel name for the tmp var.
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 1_000_000);
                self.fresh += 1;
                let tmp_binder = self.fresh_binder(tmp_name);
                let tmp_var = CoreExpr::bound_var(&tmp_binder, *s);
                let alt = CoreAlt {
                    pat: core_pat,
                    guard: None,
                    rhs: tail,
                    span: *s,
                };
                CoreExpr::Let {
                    var: tmp_binder,
                    rhs: Box::new(rhs),
                    body: Box::new(CoreExpr::Case {
                        scrutinee: Box::new(tmp_var),
                        alts: vec![alt],
                        join_ty: None,
                        span: *s,
                    }),
                    span: *s,
                }
            }

            // Expression statement: evaluate for its effect, discard result.
            Statement::Expression {
                expression,
                has_semicolon: true,
                span: s,
            } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp_name),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            // Expression without semicolon in a non-last position is unusual.
            // Treat as sequencing (result discarded).
            Statement::Expression {
                expression,
                has_semicolon: false,
                span: s,
            } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp_name),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Return { value, span: s } => {
                let ret_value = match value {
                    Some(v) => self.lower_expr(v),
                    None => CoreExpr::Lit(CoreLit::Unit, *s),
                };
                CoreExpr::Return {
                    value: Box::new(ret_value),
                    span: *s,
                }
            }

            // Declarations don't contribute to block value.
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::EffectAlias { .. }
            | Statement::TypeAlias(_)
            | Statement::Module { .. }
            | Statement::Class { .. }
            | Statement::Instance { .. } => tail,
        }
    }

    /// Lower the last (or only) statement in a block as an expression.
    fn lower_stmt_as_expr(&mut self, stmt: &Statement, span: Span) -> CoreExpr {
        match stmt {
            Statement::Expression {
                expression,
                has_semicolon: false,
                ..
            } => self.lower_expr(expression),
            // Semicolon-terminated expression discards its value — evaluate
            // for effect, then return unit.
            Statement::Expression {
                expression,
                has_semicolon: true,
                span: s,
            } => {
                let tmp = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(CoreExpr::Lit(CoreLit::Unit, *s)),
                    span: *s,
                }
            }
            Statement::Return { value, span: s } => match value {
                Some(v) => CoreExpr::Return {
                    value: Box::new(self.lower_expr(v)),
                    span: *s,
                },
                None => CoreExpr::Return {
                    value: Box::new(CoreExpr::Lit(CoreLit::Unit, *s)),
                    span: *s,
                },
            },
            // A `let` or `fn` as the last statement returns unit.
            other => {
                let unit = CoreExpr::Lit(CoreLit::Unit, span);
                self.prepend_one_stmt(other, unit, span)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::type_infer::{InferProgramConfig, infer_program},
        core::{CoreExpr, CorePrimOp},
        syntax::{interner::Interner, lexer::Lexer, parser::Parser},
    };

    fn parse_and_infer(
        src: &str,
    ) -> (
        crate::syntax::program::Program,
        HashMap<ExprId, InferType>,
        Interner,
    ) {
        let mut parser = Parser::new(Lexer::new(src));
        let program = parser.parse_program();
        let mut interner = parser.take_interner();
        let flow_sym = interner.intern("Flow");
        let hm = infer_program(
            &program,
            &interner,
            InferProgramConfig {
                file_path: None,

                preloaded_base_schemes: HashMap::new(),
                preloaded_module_member_schemes: HashMap::new(),
                task_module_bindings: std::collections::HashSet::new(),
                task_spawn_exposed: false,
                known_flow_names: std::collections::HashSet::new(),
                flow_module_symbol: flow_sym,
                preloaded_effect_op_signatures: HashMap::new(),
                effect_row_aliases: HashMap::new(),
                class_env: None,
                preloaded_adt_data: Vec::new(),
            },
        );
        let types = hm.expr_types;
        (program, types, interner)
    }

    fn collect_primops(
        program: &crate::syntax::program::Program,
        types: &HashMap<ExprId, InferType>,
    ) -> Vec<CorePrimOp> {
        let core = lower_program_ast(program, types);
        let mut ops = Vec::new();
        for def in &core.defs {
            collect_ops_in_expr(&def.expr, &mut ops);
        }
        ops
    }

    fn collect_ops_in_expr(expr: &CoreExpr, out: &mut Vec<CorePrimOp>) {
        match expr {
            CoreExpr::PrimOp { op, args, .. } => {
                out.push(*op);
                for a in args {
                    collect_ops_in_expr(a, out);
                }
            }
            CoreExpr::Lam { body, .. } => collect_ops_in_expr(body, out),
            CoreExpr::App { func, args, .. } => {
                collect_ops_in_expr(func, out);
                for a in args {
                    collect_ops_in_expr(a, out);
                }
            }
            CoreExpr::Return { value, .. } => collect_ops_in_expr(value, out),
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_ops_in_expr(rhs, out);
                collect_ops_in_expr(body, out);
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                for (_, rhs) in bindings {
                    collect_ops_in_expr(rhs, out);
                }
                collect_ops_in_expr(body, out);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                collect_ops_in_expr(scrutinee, out);
                for alt in alts {
                    collect_ops_in_expr(&alt.rhs, out);
                }
            }
            _ => {}
        }
    }

    fn collect_var_refs<'a>(expr: &'a CoreExpr, out: &mut Vec<&'a crate::core::CoreVarRef>) {
        match expr {
            CoreExpr::Var { var, .. } => out.push(var),
            CoreExpr::Lam { body, .. } => collect_var_refs(body, out),
            CoreExpr::App { func, args, .. } => {
                collect_var_refs(func, out);
                for arg in args {
                    collect_var_refs(arg, out);
                }
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_var_refs(rhs, out);
                collect_var_refs(body, out);
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                for (_, rhs) in bindings {
                    collect_var_refs(rhs, out);
                }
                collect_var_refs(body, out);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                collect_var_refs(scrutinee, out);
                for alt in alts {
                    if let Some(guard) = &alt.guard {
                        collect_var_refs(guard, out);
                    }
                    collect_var_refs(&alt.rhs, out);
                }
            }
            CoreExpr::Con { fields, .. } => {
                for field in fields {
                    collect_var_refs(field, out);
                }
            }
            CoreExpr::Return { value, .. } => collect_var_refs(value, out),
            CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
                for arg in args {
                    collect_var_refs(arg, out);
                }
            }
            CoreExpr::Handle {
                body,
                parameter,
                handlers,
                ..
            } => {
                if let Some(parameter) = parameter {
                    collect_var_refs(parameter, out);
                }
                collect_var_refs(body, out);
                for handler in handlers {
                    collect_var_refs(&handler.body, out);
                }
            }
            CoreExpr::Lit(_, _) => {}
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                collect_var_refs(object, out);
            }
        }
    }

    #[test]
    fn integer_add_emits_iadd() {
        let src = "fn add(x: Int, y: Int) -> Int { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::IAdd),
            "expected IAdd for Int addition, got: {:?}",
            ops
        );
        assert!(
            !ops.contains(&CorePrimOp::Add),
            "should not emit generic Add for typed Int addition"
        );
    }

    #[test]
    fn integer_mul_emits_imul() {
        let src = "fn mul(x: Int, y: Int) -> Int { x * y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::IMul),
            "expected IMul, got: {:?}",
            ops
        );
    }

    #[test]
    fn float_add_emits_fadd() {
        let src = "fn fadd(x: Float, y: Float) -> Float { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::FAdd),
            "expected FAdd, got: {:?}",
            ops
        );
    }

    #[test]
    fn generic_add_stays_generic_without_type_info() {
        // Unannotated polymorphic add — HM resolves to Int here, so we get IAdd.
        // This test verifies that untyped additions stay as generic Add.
        // We use a String + String expression so HM infers String, not Int/Float,
        // which means no typed variant should be emitted — just generic Add.
        let src = r#"fn cat(a: String, b: String) -> String { a + b }"#;
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        // String + String should emit generic Add (no IAdd/FAdd)
        assert!(
            ops.contains(&CorePrimOp::Add),
            "expected generic Add for String addition, got: {:?}",
            ops
        );
        assert!(
            !ops.contains(&CorePrimOp::IAdd),
            "should not emit IAdd for String addition"
        );
        assert!(
            !ops.contains(&CorePrimOp::FAdd),
            "should not emit FAdd for String addition"
        );
    }

    #[test]
    fn lower_ast_resolves_shadowed_let_to_inner_binder() {
        let src = r#"
fn main() {
    let x = 1;
    let x = 2;
    x
}
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let main_name = interner.intern("main");
        let core = lower_program_ast(&prog, &types);
        let main = core.defs.iter().find(|def| def.name == main_name).unwrap();
        let mut refs = Vec::new();
        collect_var_refs(&main.expr, &mut refs);
        let x_refs: Vec<_> = refs
            .into_iter()
            .filter(|var| interner.try_resolve(var.name) == Some("x"))
            .collect();
        let last = x_refs.last().unwrap();
        assert!(
            last.binder.is_some(),
            "expected lexical x reference to be resolved"
        );
    }

    #[test]
    fn lower_ast_leaves_external_name_unbound() {
        let src = r#"
fn main() {
    print("ok")
}
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let main_name = interner.intern("main");
        let core = lower_program_ast(&prog, &types);
        let main = core.defs.iter().find(|def| def.name == main_name).unwrap();
        let mut refs = Vec::new();
        collect_var_refs(&main.expr, &mut refs);
        let print_ref = refs
            .into_iter()
            .find(|var| interner.try_resolve(var.name) == Some("print"))
            .expect("expected print reference");
        assert_eq!(
            print_ref.binder, None,
            "external name should stay unresolved in Core"
        );
    }

    #[test]
    fn lower_ast_preserves_module_data_and_effect_declarations() {
        let src = r#"
module Demo {
    fn value() { 1 }
}

data MaybeInt {
    SomeInt(Int)
    NoneInt
}

effect Console {
    print: String -> Unit
}
"#;
        let (prog, types, _) = parse_and_infer(src);
        let core = lower_program_ast(&prog, &types);

        assert!(matches!(
            core.top_level_items.first(),
            Some(CoreTopLevelItem::Module { .. })
        ));
        assert!(matches!(
            core.top_level_items.get(1),
            Some(CoreTopLevelItem::Data { .. })
        ));
        assert!(matches!(
            core.top_level_items.get(2),
            Some(CoreTopLevelItem::EffectDecl { .. })
        ));

        let module_body = match &core.top_level_items[0] {
            CoreTopLevelItem::Module { body, .. } => body,
            other => panic!("expected module item, got {other:?}"),
        };
        assert!(matches!(
            module_body.first(),
            Some(CoreTopLevelItem::Function { .. })
        ));
    }

    #[test]
    fn lower_ast_does_not_drop_symbol_zero_named_global_bindings() {
        let src = r#"
let f = len
f("flux")
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let f_name = interner.intern("f");
        assert_eq!(
            f_name.as_u32(),
            0,
            "test requires first identifier to be Symbol(0)"
        );

        let core = lower_program_ast(&prog, &types);
        let f_def = core
            .defs
            .iter()
            .find(|def| def.name == f_name && !def.is_anonymous())
            .expect("expected named top-level def for f");
        let anon_def = core
            .defs
            .iter()
            .find(|def| def.is_anonymous())
            .expect("expected anonymous top-level expression def");

        let mut refs = Vec::new();
        collect_var_refs(&anon_def.expr, &mut refs);
        let f_ref = refs
            .into_iter()
            .find(|var| var.name == f_name)
            .expect("expected call through top-level f binding");
        assert_eq!(
            f_ref.binder,
            Some(f_def.binder.id),
            "top-level Symbol(0) binding should still resolve lexically"
        );
    }

    // ── SCC integration tests (full lowering) ─────────────────────────

    /// Count LetRecGroup nodes and individual LetRec nodes in the body
    /// of the main function's Core IR.
    fn count_binding_kinds(src: &str) -> (usize, usize) {
        let (program, types, _interner) = parse_and_infer(src);
        let core = lower_program_ast(&program, &types);
        let main_def = core.defs.iter().find(|d| !d.is_anonymous()).unwrap();
        let mut groups = 0;
        let mut singles = 0;
        count_bindings_in_expr(&main_def.expr, &mut groups, &mut singles);
        (groups, singles)
    }

    fn count_bindings_in_expr(expr: &CoreExpr, groups: &mut usize, singles: &mut usize) {
        match expr {
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                *groups += 1;
                for (_, rhs) in bindings {
                    count_bindings_in_expr(rhs, groups, singles);
                }
                count_bindings_in_expr(body, groups, singles);
            }
            CoreExpr::LetRec { rhs, body, .. } => {
                *singles += 1;
                count_bindings_in_expr(rhs, groups, singles);
                count_bindings_in_expr(body, groups, singles);
            }
            CoreExpr::Let { rhs, body, .. } => {
                count_bindings_in_expr(rhs, groups, singles);
                count_bindings_in_expr(body, groups, singles);
            }
            CoreExpr::Lam { body, .. } => count_bindings_in_expr(body, groups, singles),
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                count_bindings_in_expr(scrutinee, groups, singles);
                for alt in alts {
                    count_bindings_in_expr(&alt.rhs, groups, singles);
                }
            }
            CoreExpr::App { func, args, .. } => {
                count_bindings_in_expr(func, groups, singles);
                for a in args {
                    count_bindings_in_expr(a, groups, singles);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn scc_chain_produces_individual_letrecs() {
        let (groups, singles) =
            count_binding_kinds("fn main() { fn a() { b() } fn b() { c() } fn c() { 42 } a() }");
        assert_eq!(groups, 0, "chain a→b→c should produce no LetRecGroup");
        assert_eq!(singles, 3, "chain should produce 3 individual LetRecs");
    }

    #[test]
    fn scc_mutual_recursion_produces_one_group() {
        let (groups, _singles) = count_binding_kinds(
            "fn main() { fn f(n) { if n <= 0 { 1 } else { g(n - 1) } } fn g(n) { if n <= 0 { 2 } else { f(n - 1) } } f(5) }",
        );
        assert_eq!(groups, 1, "mutual recursion should produce 1 LetRecGroup");
    }

    #[test]
    fn scc_separates_independent_from_cycle() {
        let (groups, singles) = count_binding_kinds(
            "fn main() { fn f(n) { g(n) } fn g(n) { f(n) } fn h() { 42 } f(1) + h() }",
        );
        assert_eq!(groups, 1, "f↔g should be 1 group");
        assert!(singles >= 1, "h should be a separate LetRec");
    }

    /// Each kind of evidence renders as the Core the old paths build for it:
    /// a global, a global applied to its context, or a parameter's binder
    /// followed through the superclass slots.
    #[test]
    fn render_dict_arg_builds_each_kind_of_dictionary() {
        use crate::types::{
            class_disposition::InstanceKey, class_id::ClassId, translate::DictArg,
            type_constructor::TypeConstructor,
        };

        let mut interner = Interner::new();
        let show = interner.intern("Show");
        let eq = interner.intern("Eq");
        let show_int = interner.intern("__dict_Show_Int");
        let eq_list = interner.intern("__dict_Eq_List");
        let eq_int = interner.intern("__dict_Eq_Int");
        let param = interner.intern("__dict_Ord");
        let key = |class, ty: &str| InstanceKey {
            class_id: ClassId::from_local_name(class),
            head_type_args: vec![InferType::Con(TypeConstructor::Int)],
            dict_type_key: ty.to_string(),
        };
        let types = HashMap::new();
        let mut lowerer = AstLowerer::new(&types, Some(&interner), None, None, None, None);
        let owner = ExprId(7);
        let binder = CoreBinder::new(crate::core::CoreBinderId(3), param);
        lowerer.dict_param_binders.insert((owner, 0), binder);
        let span = Span::default();
        let var = |expr: &CoreExpr| match expr {
            CoreExpr::Var { var, .. } => (var.name, var.binder),
            other => panic!("expected a variable, got {other:?}"),
        };

        let global = lowerer
            .render_dict_arg(
                &DictArg::Global {
                    instance: key(show, "Int"),
                },
                span,
            )
            .expect("a global");
        assert_eq!(var(&global), (show_int, None));

        let applied = lowerer
            .render_dict_arg(
                &DictArg::Applied {
                    instance: key(eq, "List"),
                    context: vec![DictArg::Global {
                        instance: key(eq, "Int"),
                    }],
                },
                span,
            )
            .expect("an applied dictionary");
        let CoreExpr::App { func, args, .. } = &applied else {
            panic!("expected an application, got {applied:?}");
        };
        assert_eq!(var(func), (eq_list, None));
        assert_eq!(args.iter().map(var).collect::<Vec<_>>(), [(eq_int, None)]);

        let projected = lowerer
            .render_dict_arg(
                &DictArg::Param {
                    owner,
                    index: 0,
                    path: vec![0],
                },
                span,
            )
            .expect("a parameter's superclass");
        let CoreExpr::TupleField {
            object, index: 0, ..
        } = &projected
        else {
            panic!("expected slot 0 of the parameter, got {projected:?}");
        };
        assert_eq!(var(object), (param, Some(binder.id)));

        let unknown_owner = DictArg::Param {
            owner: ExprId(8),
            index: 0,
            path: Vec::new(),
        };
        assert!(lowerer.render_dict_arg(&unknown_owner, span).is_none());
    }
}
