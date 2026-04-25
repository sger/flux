use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::{
    diagnostics::{
        CONSTRUCTOR_ARITY_MISMATCH, Diagnostic, DiagnosticBuilder,
        compiler_errors::{
            call_arg_type_mismatch, fun_arity_mismatch, fun_param_type_mismatch,
            fun_return_type_mismatch, if_branch_type_mismatch, occurs_check_failure,
            type_unification_error,
        },
        diagnostic_for,
        position::Span,
        text_similarity::levenshtein_distance,
    },
    syntax::{
        Identifier,
        block::Block,
        data_variant::DataVariant,
        effect_expr::EffectExpr,
        expression::{ExprId, Expression, MatchArm},
        interner::Interner,
        program::Program,
        statement::{FunctionTypeParam, Statement},
        type_expr::TypeExpr,
    },
    types::{
        TypeVarId,
        class_defaulting::finalize_binding_class_constraints,
        infer_effect_row::InferEffectRow,
        infer_type::InferType,
        scheme::{Scheme, generalize, generalize_with_constraints},
        type_constructor::TypeConstructor,
        type_env::TypeEnv,
        type_subst::TypeSubst,
        unify::unify_core,
        unify_error::{UnifyErrorDetail, UnifyErrorKind},
    },
};

mod adt;
pub mod boundary;
pub mod constraint;
mod display;
mod effects;
mod expression;
mod function;
mod pattern_coverage;
mod pattern_coverage_adapter;
mod solver;
mod statement;
pub mod static_type_validation;
mod unification;

pub(crate) type BindingSpanKey = (usize, usize, usize, usize);
pub use display::{display_infer_type, render_scheme_canonical};

// ─────────────────────────────────────────────────────────────────────────────
// Shared type definitions
// ─────────────────────────────────────────────────────────────────────────────

// ── ADT metadata ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AdtConstructorTypeInfo {
    adt_name: Identifier,
    type_params: Vec<Identifier>,
    fields: Vec<TypeExpr>,
    /// Field names for named-field variants (Proposal 0152).
    /// `None` for positional variants. When `Some`, length matches `fields`.
    field_names: Option<Vec<Identifier>>,
}

// ── Diagnostic context ───────────────────────────────────────────────────────

/// Reporting mode for HM unification diagnostics.
#[derive(Debug, Clone)]
pub enum ReportContext {
    Plain,
    IfBranch {
        then_span: Span,
        else_span: Span,
    },
    MatchArm {
        first_span: Span,
        arm_span: Span,
        arm_index: usize,
    },
    CallArg {
        fn_name: Option<String>,
        fn_def_span: Option<Span>,
    },
}

// ── Inference input specs ─────────────────────────────────────────────────────

/// Immutable inputs require to infer a named function declaration.
///
/// This bundles syntax nodes captured from a `Statement::Function` so the
/// inference entrypoint can accept one parameter object instead of many
/// positional arguments.
#[derive(Debug, Clone, Copy)]
struct FnInferInput<'a> {
    name: Identifier,
    fn_span: Span,
    type_params: &'a [FunctionTypeParam],
    parameters: &'a [Identifier],
    parameter_types: &'a [Option<TypeExpr>],
    return_type: &'a Option<TypeExpr>,
    effects: &'a [EffectExpr],
    body: &'a Block,
}

/// Immutable inputs required to infer a lambda expression.
#[derive(Debug, Clone, Copy)]
struct LambdaInferInput<'a> {
    parameters: &'a [Identifier],
    parameter_types: &'a [Option<TypeExpr>],
    return_type: &'a Option<TypeExpr>,
    effects: &'a [EffectExpr],
    body: &'a Block,
}

/// Immutable inputs required to infer a call expression.
#[derive(Debug, Clone, Copy)]
struct CallInferInput<'a> {
    function: &'a Expression,
    arguments: &'a [Expression],
    span: Span,
}

/// Immutable inputs required to infer a match expression.
#[derive(Debug, Clone, Copy)]
struct MatchInferInput<'a> {
    scrutinee: &'a Expression,
    arms: &'a [MatchArm],
    span: Span,
}

// ─────────────────────────────────────────────────────────────────────────────
// Inference context
// ─────────────────────────────────────────────────────────────────────────────

struct InferCtx<'a> {
    env: TypeEnv,
    interner: &'a Interner,
    errors: Vec<Diagnostic>,
    file_path: Rc<str>,
    /// Accumulated global substitution — grows monotonically as constraints
    /// are solved.  Apply this to any `Ty` retrieved from the env to obtain
    /// its most-resolved form.
    subst: TypeSubst,
    expr_types: HashMap<ExprId, InferType>,
    module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    binding_schemes_by_span: HashMap<BindingSpanKey, Scheme>,
    known_flow_names: HashSet<Identifier>,
    flow_module_symbol: Identifier,
    adt_constructor_types: HashMap<Identifier, AdtConstructorTypeInfo>,
    /// Reverse index: ADT name → type parameters. Avoids linear scan over
    /// `adt_constructor_types` when only per-ADT metadata is needed.
    adt_type_params: HashMap<Identifier, Vec<Identifier>>,
    effect_op_signatures: HashMap<(Identifier, Identifier), Scheme>,
    effect_row_aliases: HashMap<Identifier, EffectExpr>,
    ambient_effect_rows: Vec<InferEffectRow>,
    handled_effects: Vec<Identifier>,
    /// Deduplication set for unification diagnostics. Keyed by a hash of
    /// (expected_type, actual_type) so the same mismatch is reported at most once.
    seen_error_keys: HashSet<u64>,
    /// Constraint log records every constraint generated during inference.
    /// Currently populated alongside eager solving for observability and
    /// future deferred-solving support.
    contraint_log: Vec<constraint::Constraint>,
    /// Deferred constraints awaiting batch solving. Empty under the current
    /// eager model; used by [`Self::solve_deferred_constraints`].
    deferred_constraints: Vec<constraint::Constraint>,
    /// Type class environment for constraint generation (Proposal 0145).
    class_env: Option<crate::types::class_env::ClassEnv>,
    /// Accumulated type class constraints (e.g., `Num<a>` from `x + y`).
    class_constraints: Vec<constraint::WantedClassConstraint>,
    /// Type variables allocated as fallback after inference failures.
    /// These are "tainted" — if they appear in a binding's resolved type,
    /// the binding has unresolved inference even if the scheme is mono.
    fallback_vars: HashSet<TypeVarId>,
    /// Type variables introduced by `Scheme::instantiate(...)` at expression
    /// use sites. Surviving unresolved vars from this set are expected to be
    /// resolved by later call-site unification and should not trigger E430.
    instantiated_expr_vars: HashSet<TypeVarId>,
    /// Rigid (skolem) type variables introduced by a declared signature
    /// (Proposal 0159). A skolem cannot be unified with anything other than
    /// itself; `unify_core` enforces this inline via the threaded
    /// `skolems` parameter. Unmarked when the declaring function's scope
    /// exits so downstream uses treat them as flexible.
    skolem_vars: HashSet<TypeVarId>,
    /// Source-level name for each skolem (the type parameter identifier),
    /// used to render readable E305 diagnostics.
    skolem_names: HashMap<TypeVarId, Identifier>,
    /// Pre-resolved class name symbols for constraint emission in operators.
    /// `None` if the class is not declared in the current program.
    class_sym_eq: Option<Identifier>,
    class_sym_ord: Option<Identifier>,
    class_sym_num: Option<Identifier>,
    class_sym_semigroup: Option<Identifier>,
}

impl<'a> InferCtx<'a> {
    /// Format an `InferType` for user-facing diagnostics, resolving ADT
    /// symbols to their human-readable names via the interner.
    fn display_type(&self, ty: &InferType) -> String {
        display_infer_type(ty, self.interner)
    }

    /// Construct a fresh inference context for one compilation unit.
    ///
    /// This initializes:
    /// - a new [`TypeEnv`] pre-populated with `preloaded_base_schemes`,
    /// - lookup tables for module-member HM schemes and effect operation
    ///   signatures loaded from earlier compiler phases,
    /// - interner-backed naming context and file-path metadata used by
    ///   diagnostics,
    /// - empty substitution/error/trace state used during inference.
    ///
    /// The resulting context is ready to infer top-level declarations and
    /// expressions while preserving deterministic ids and diagnostics for this
    /// source file.
    ///
    /// Parameters:
    /// - `interner`: shared symbol table used for display and name resolution.
    /// - `file_path`: source path to stamp diagnostics with origin metadata.
    /// - `preloaded_base_schemes`: HM schemes for Flow runtime bindings.
    /// - `preloaded_module_member_schemes`: HM schemes for imported module
    ///   members, keyed by `(module, member)`.
    /// - `known_flow_names`: fast-membership set for names belonging to Flow.
    /// - `flow_module_symbol`: canonical symbol identifying the Flow module.
    /// - `preloaded_effect_op_signatures`: generalized signatures for effect
    ///   operations,
    ///   keyed by `(effect, operation)`.
    fn new(
        interner: &'a Interner,
        file_path: Rc<str>,
        preloaded_base_schemes: HashMap<Identifier, Scheme>,
        preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
        known_flow_names: HashSet<Identifier>,
        flow_module_symbol: Identifier,
        preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), Scheme>,
        effect_row_aliases: HashMap<Identifier, EffectExpr>,
    ) -> Self {
        let mut env = TypeEnv::new();
        advance_counter_past_preloaded_schemes(
            &mut env,
            &preloaded_base_schemes,
            &preloaded_module_member_schemes,
        );
        for (name, scheme) in preloaded_base_schemes {
            env.bind(name, scheme);
        }

        InferCtx {
            env,
            interner,
            errors: Vec::new(),
            file_path,
            subst: TypeSubst::empty(),
            expr_types: HashMap::new(),
            module_member_schemes: preloaded_module_member_schemes,
            binding_schemes_by_span: HashMap::new(),
            known_flow_names,
            flow_module_symbol,
            adt_constructor_types: HashMap::new(),
            adt_type_params: HashMap::new(),
            effect_op_signatures: preloaded_effect_op_signatures,
            effect_row_aliases,
            ambient_effect_rows: Vec::new(),
            handled_effects: Vec::new(),
            seen_error_keys: HashSet::new(),
            contraint_log: Vec::new(),
            deferred_constraints: Vec::new(),
            fallback_vars: HashSet::new(),
            instantiated_expr_vars: HashSet::new(),
            skolem_vars: HashSet::new(),
            skolem_names: HashMap::new(),
            class_env: None,
            class_constraints: Vec::new(),
            class_sym_eq: None,
            class_sym_ord: None,
            class_sym_num: None,
            class_sym_semigroup: None,
        }
    }

    /// Return whether a type is fully concrete (no unresolved type variables).
    fn is_fully_concrete(ty: &InferType) -> bool {
        ty.is_concrete()
    }

    /// Allocate a fresh type variable and mark it as a fallback from an
    /// inference failure. Fallback vars are tracked so the static type
    /// validation pass can distinguish them from legitimately polymorphic
    /// variables.
    fn alloc_fallback_var(&mut self) -> InferType {
        let ty = self.env.alloc_infer_type_var();
        if let InferType::Var(v) = &ty {
            self.fallback_vars.insert(*v);
        }
        ty
    }

    /// Record fresh vars created by expression-site scheme instantiation.
    fn record_instantiated_expr_vars<I>(&mut self, vars: I)
    where
        I: IntoIterator<Item = TypeVarId>,
    {
        self.instantiated_expr_vars.extend(vars);
    }

    /// Record a constraint in the log for observability and future deferred solving.
    fn record_constraint(&mut self, constraint: constraint::Constraint) {
        self.contraint_log.push(constraint);
    }

    /// Emit a type class constraint (e.g., `Num<a>` from `x + y`).
    ///
    /// The constraint is recorded for downstream phases (Step 4: solving).
    /// Currently informational — does not affect type inference behavior.
    fn emit_class_constraint(
        &mut self,
        class_name: Identifier,
        type_arg: InferType,
        span: Span,
        origin: constraint::WantedClassConstraintOrigin,
    ) {
        self.emit_class_constraint_args(class_name, vec![type_arg], span, origin);
    }

    /// Emit a type class constraint with the full resolved class head.
    ///
    /// This is used for multi-parameter classes such as `Convert<a, b>`,
    /// where a method call may constrain more than one type argument.
    fn emit_class_constraint_args(
        &mut self,
        class_name: Identifier,
        type_args: Vec<InferType>,
        span: Span,
        origin: constraint::WantedClassConstraintOrigin,
    ) {
        self.class_constraints
            .push(constraint::WantedClassConstraint {
                class_name,
                type_args: type_args.clone(),
                span,
                origin,
                originated_from_concrete_type: type_args.iter().all(Self::is_fully_concrete),
            });
        self.record_constraint(constraint::Constraint::Class {
            class_name,
            type_args,
            span,
        });
    }

    /// Re-emit instantiated scheme constraints into the current inference state.
    ///
    /// Generalized constraints are attached to schemes at definition sites, and
    /// this helper materializes them again when a constrained binding is used so
    /// downstream solving and dictionary elaboration can see the call-site
    /// obligations.
    fn emit_scheme_constraints(
        &mut self,
        constraints: &[constraint::SchemeConstraint],
        span: Span,
    ) {
        for constraint in constraints {
            let type_args = constraint
                .type_vars
                .iter()
                .map(|v| InferType::Var(*v))
                .collect::<Vec<_>>();
            self.class_constraints
                .push(constraint::WantedClassConstraint {
                    class_name: constraint.class_name,
                    type_args: type_args.clone(),
                    span,
                    origin: constraint::WantedClassConstraintOrigin::SchemeUse,
                    originated_from_concrete_type: true,
                });
            self.record_constraint(constraint::Constraint::Class {
                class_name: constraint.class_name,
                type_args,
                span,
            });
        }
    }

    /// Finalize one binding's type-class obligations before generalization.
    ///
    /// This validates concrete obligations, applies numeric defaulting for
    /// truly ambiguous `Num` variables, and returns the resulting scheme.
    fn finalize_binding_scheme(
        &mut self,
        infer_type: &InferType,
        relevant_constraints: &[constraint::WantedClassConstraint],
        env_free_vars: &HashSet<TypeVarId>,
    ) -> Scheme {
        let finalized = finalize_binding_class_constraints(
            infer_type,
            env_free_vars,
            relevant_constraints,
            &self.subst,
            self.class_env.as_ref(),
            self.interner,
        );
        if !finalized.default_subst.is_empty() {
            self.subst = std::mem::take(&mut self.subst).compose(&finalized.default_subst);
        }
        self.errors.extend(finalized.diagnostics);

        if finalized.scheme_constraints.is_empty() {
            generalize(&finalized.infer_type, env_free_vars)
        } else {
            generalize_with_constraints(
                &finalized.infer_type,
                env_free_vars,
                finalized.scheme_constraints,
            )
        }
    }

    /// Mark a type variable as rigid (skolem) for the duration of a checked
    /// signature (Proposal 0159). While marked, `unify_core` rejects any
    /// attempt to bind `v` to a non-identical type.
    pub(super) fn mark_skolem(&mut self, v: TypeVarId, name: Identifier) {
        self.skolem_vars.insert(v);
        self.skolem_names.insert(v, name);
    }

    /// Unmark a set of skolems; called when leaving the declared-signature
    /// scope so the variables can behave flexibly in downstream contexts.
    pub(super) fn unmark_skolems(&mut self, vs: &[TypeVarId]) {
        for v in vs {
            self.skolem_vars.remove(v);
            self.skolem_names.remove(v);
        }
    }

    /// Check if a name is a known class method. Returns the class name if so.
    fn lookup_class_method(&self, name: Identifier) -> Option<Identifier> {
        let class_env = self.class_env.as_ref()?;
        // Phase 1b Step 3: storage is keyed on ClassId now, but this lookup
        // only needs the class's short name. Iterate values directly.
        for class_def in class_env.classes.values() {
            if class_def.methods.iter().any(|m| m.name == name) {
                return Some(class_def.name);
            }
        }
        None
    }
}

pub use display::suggest_type_name;

/// Pre-loaded data arguments required by [`infer_program`].
///
/// Bundles the 6 data arguments constructed by the compiler before calling
/// into the HM inference pass, keeping the public API narrow and position-safe.
///
/// # Examples
///
/// ```text
/// let cfg = InferProgramConfig {
///     file_path: Some("examples/app.flx".into()),
///     preloaded_base_schemes: HashMap::new(),
///     preloaded_module_member_schemes: HashMap::new(),
///     known_flow_names: HashSet::new(),
///     flow_module_symbol,
///     preloaded_effect_op_signatures: HashMap::new(),
/// };
/// ```
pub struct InferProgramConfig {
    pub file_path: Option<Rc<str>>,
    pub preloaded_base_schemes: HashMap<Identifier, Scheme>,
    pub preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    pub known_flow_names: HashSet<Identifier>,
    pub flow_module_symbol: Identifier,
    pub preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), Scheme>,
    pub effect_row_aliases: HashMap<Identifier, EffectExpr>,
    /// Type class environment for constraint generation.
    pub class_env: Option<crate::types::class_env::ClassEnv>,
}

/// Run Algorithm W (Hindley-Milner) over the entire program.
///
/// Returns the resulting `TypeEnv` (can be queried for any identifier's
/// inferred scheme) and a list of type-error diagnostics.
///
/// Type errors are **non-fatal**: inference always completes, recovering with
/// fresh inference variables when unification fails. The compiler can then use the env to enrich
/// its own static type information without gating on type errors.
///
/// # Examples
///
/// ```text
/// let result = infer_program(&program, &interner, InferProgramConfig {
///     file_path: Some("main.flx".into()),
///     preloaded_base_schemes: base_schemes,
///     preloaded_module_member_schemes: module_member_schemes,
///     known_flow_names,
///     flow_module_symbol,
///     preloaded_effect_op_signatures: effect_op_signatures,
/// });
///
/// // Inference is resilient: diagnostics may be present,
/// // but a type environment is still returned.
/// let _env = result.type_env;
/// ```
pub fn infer_program(
    program: &Program,
    interner: &Interner,
    config: InferProgramConfig,
) -> InferProgramResult {
    let file: Rc<str> = config.file_path.unwrap_or_else(|| "".into());
    let mut ctx = InferCtx::new(
        interner,
        file,
        config.preloaded_base_schemes,
        config.preloaded_module_member_schemes,
        config.known_flow_names,
        config.flow_module_symbol,
        config.preloaded_effect_op_signatures,
        config.effect_row_aliases,
    );
    init_class_env(&mut ctx, config.class_env, interner);
    ctx.infer_program(program);
    ctx.solve_deferred_constraints();
    build_infer_result(ctx)
}

/// Expand the fallback set through the substitution and build resolved binding
/// schemes with `forall = free_vars(resolved) - fallback_vars`.
/// Advance the env's type-var counter past any TypeVarId used in preloaded
/// schemes so freshly-allocated vars in this pass cannot collide with IDs
/// baked into cross-pass scheme bodies (Proposal 0159).
fn advance_counter_past_preloaded_schemes(
    env: &mut TypeEnv,
    base: &HashMap<Identifier, Scheme>,
    module_members: &HashMap<(Identifier, Identifier), Scheme>,
) {
    let max_id = base
        .values()
        .chain(module_members.values())
        .flat_map(|s| {
            let mut ids: Vec<TypeVarId> = s.infer_type.free_vars().into_iter().collect();
            ids.extend(s.forall.iter().copied());
            ids
        })
        .max();
    if let Some(m) = max_id
        && env.counter <= m
    {
        env.counter = m + 1;
    }
}

/// Expand the fallback set through the substitution and build resolved binding
/// schemes with `forall = free_vars(resolved) - fallback_vars`.
fn resolve_binding_schemes(
    env: &TypeEnv,
    subst: &TypeSubst,
    fallback_vars: &HashSet<TypeVarId>,
) -> (HashSet<TypeVarId>, HashMap<Identifier, Scheme>) {
    // Expand fallback set: if a fallback var was unified with another var,
    // the target is also tainted.
    let mut expanded = fallback_vars.clone();
    for &fv in fallback_vars {
        let resolved = InferType::Var(fv).apply_type_subst(subst);
        if let InferType::Var(target) = resolved {
            expanded.insert(target);
        }
    }

    let schemes = env
        .visible_bindings()
        .map(|(name, scheme)| {
            let resolved_type = scheme.infer_type.apply_type_subst(subst);
            let mut forall: Vec<TypeVarId> = resolved_type
                .free_vars()
                .into_iter()
                .filter(|v| !expanded.contains(v))
                .collect();
            forall.sort_unstable();
            forall.dedup();
            (
                name,
                Scheme {
                    forall,
                    constraints: scheme.constraints.clone(),
                    infer_type: resolved_type,
                },
            )
        })
        .collect();

    (expanded, schemes)
}

/// Apply final substitution to all inferred types and build the result.
fn build_infer_result(ctx: InferCtx<'_>) -> InferProgramResult {
    let constraint_count = ctx.contraint_log.len();
    let (expanded_fallback, resolved_binding_schemes) =
        resolve_binding_schemes(&ctx.env, &ctx.subst, &ctx.fallback_vars);
    let resolved_instantiated_expr_vars =
        resolve_instantiated_expr_vars(&ctx.instantiated_expr_vars, &ctx.subst);
    let resolved_schemes = resolve_module_member_schemes(ctx.module_member_schemes, &ctx.subst);
    let resolved_binding_schemes_by_span = resolve_binding_schemes_by_span(
        ctx.binding_schemes_by_span,
        &ctx.subst,
        &expanded_fallback,
    );
    let resolved_expr_types = resolve_expr_types(ctx.expr_types, &ctx.subst);
    let resolved_class_constraints = resolve_class_constraints(ctx.class_constraints, &ctx.subst);

    InferProgramResult {
        type_env: ctx.env,
        diagnostics: ctx.errors,
        expr_types: resolved_expr_types,
        module_member_schemes: resolved_schemes,
        constraint_count,
        class_constraints: resolved_class_constraints,
        fallback_vars: expanded_fallback,
        instantiated_expr_vars: resolved_instantiated_expr_vars,
        resolved_binding_schemes,
        resolved_binding_schemes_by_span,
    }
}

/// Resolve the final set of expression-instantiated type variables through the
/// completed substitution so static validation can reason about final ids.
fn resolve_instantiated_expr_vars(
    instantiated_expr_vars: &HashSet<TypeVarId>,
    subst: &TypeSubst,
) -> HashSet<TypeVarId> {
    instantiated_expr_vars
        .iter()
        .flat_map(|&var| InferType::Var(var).apply_type_subst(subst).free_vars())
        .collect()
}

/// Apply the final substitution to imported/module-member schemes.
fn resolve_module_member_schemes(
    module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    subst: &TypeSubst,
) -> HashMap<(Identifier, Identifier), Scheme> {
    module_member_schemes
        .into_iter()
        .map(|(key, scheme)| {
            let resolved_type = scheme.infer_type.apply_type_subst(subst);
            let mut forall = resolved_type.free_vars().into_iter().collect::<Vec<_>>();
            forall.sort_unstable();
            forall.dedup();
            (
                key,
                Scheme {
                    forall,
                    constraints: Vec::new(),
                    infer_type: resolved_type,
                },
            )
        })
        .collect()
}

/// Apply the final substitution to per-binding schemes while preserving the
/// strict-types rule that fallback-tainted vars are never generalized.
fn resolve_binding_schemes_by_span(
    binding_schemes_by_span: HashMap<BindingSpanKey, Scheme>,
    subst: &TypeSubst,
    fallback_vars: &HashSet<TypeVarId>,
) -> HashMap<BindingSpanKey, Scheme> {
    binding_schemes_by_span
        .into_iter()
        .map(|(span, scheme)| {
            let resolved_type = scheme.infer_type.apply_type_subst(subst);
            let mut forall = resolved_type
                .free_vars()
                .into_iter()
                .filter(|v| !fallback_vars.contains(v))
                .collect::<Vec<_>>();
            forall.sort_unstable();
            forall.dedup();
            (
                span,
                Scheme {
                    forall,
                    constraints: scheme.constraints,
                    infer_type: resolved_type,
                },
            )
        })
        .collect()
}

/// Apply the final substitution to all recorded expression types.
fn resolve_expr_types(
    expr_types: HashMap<ExprId, InferType>,
    subst: &TypeSubst,
) -> HashMap<ExprId, InferType> {
    expr_types
        .into_iter()
        .map(|(id, ty)| (id, ty.apply_type_subst(subst)))
        .collect()
}

/// Apply the final substitution to accumulated class constraints.
fn resolve_class_constraints(
    class_constraints: Vec<constraint::WantedClassConstraint>,
    subst: &TypeSubst,
) -> Vec<constraint::WantedClassConstraint> {
    class_constraints
        .into_iter()
        .map(|mut c| {
            c.type_args = c
                .type_args
                .iter()
                .map(|t| t.apply_type_subst(subst))
                .collect();
            c
        })
        .collect()
}

/// Convert a statement span into a stable hashable key for binding-scheme lookup.
pub(crate) fn binding_span_key(span: Span) -> BindingSpanKey {
    (
        span.start.line,
        span.start.column,
        span.end.line,
        span.end.column,
    )
}

/// Initialize the class environment and pre-resolve well-known class name
/// symbols for constraint emission in operators.
fn init_class_env(
    ctx: &mut InferCtx<'_>,
    class_env: Option<crate::types::class_env::ClassEnv>,
    interner: &Interner,
) {
    ctx.class_env = class_env;
    if let Some(ref env) = ctx.class_env {
        // Phase 1b Step 3: keys are ClassId now; project to the short name.
        for class_id in env.classes.keys() {
            let class_name = class_id.name;
            match interner.resolve(class_name) {
                "Eq" => ctx.class_sym_eq = Some(class_name),
                "Ord" => ctx.class_sym_ord = Some(class_name),
                "Num" => ctx.class_sym_num = Some(class_name),
                "Semigroup" => ctx.class_sym_semigroup = Some(class_name),
                _ => {}
            }
        }
    }
}

#[derive(Debug)]
pub struct InferProgramResult {
    /// Final type environment after all constraints and substitutions are applied.
    pub type_env: TypeEnv,
    /// Non-fatal inference diagnostics collected during the pass.
    pub diagnostics: Vec<Diagnostic>,
    /// Inferred type for each recorded expression, keyed by parser-assigned `ExprId`.
    pub expr_types: HashMap<ExprId, InferType>,
    /// Inferred type schemes for public module members.
    ///
    /// Keyed by `(module_name, member_name)`. Includes both preloaded schemes
    /// from previously-compiled modules and newly-inferred schemes from the
    /// current module's `module { ... }` blocks.
    pub module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    /// Total number of type/effect constraints generated during inference.
    pub constraint_count: usize,
    /// Type class constraints collected during inference (Proposal 0145, Step 3).
    ///
    /// Each entry records a `ClassName<Type>` constraint arising from operator
    /// usage or class method calls. Currently informational — Step 4 (solving)
    /// will resolve these against known instances.
    pub class_constraints: Vec<constraint::WantedClassConstraint>,
    /// Type variables that were allocated as fallback after inference failures.
    /// Used by the static type validation pass to distinguish fallback vars
    /// from legitimately polymorphic vars in mono schemes.
    pub fallback_vars: HashSet<TypeVarId>,
    /// Type variables introduced by expression-site scheme instantiation after
    /// applying the final substitution.
    pub instantiated_expr_vars: HashSet<TypeVarId>,
    /// Resolved binding schemes: each top-level binding's type after applying
    /// the final substitution, with `forall` recomputed as
    /// `free_vars(resolved) - fallback_vars`. This is the authoritative source
    /// for the static type validation pass — `has_unresolved_vars()` on these
    /// schemes correctly identifies bindings with unresolved inference.
    pub resolved_binding_schemes: HashMap<Identifier, Scheme>,
    /// Resolved binding schemes for each statement-level generalization site,
    /// keyed by the `Statement::Function`/`Statement::Let` span.
    pub resolved_binding_schemes_by_span: HashMap<BindingSpanKey, Scheme>,
}

/// Stable identifier for one expression node within a single inference run.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprNodeId(pub u32);
