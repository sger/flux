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
        diag_enhanced,
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
        statement::Statement,
        type_expr::TypeExpr,
    },
    types::{
        TypeVarId,
        infer_effect_row::InferEffectRow,
        infer_type::InferType,
        scheme::{Scheme, generalize},
        type_constructor::TypeConstructor,
        type_env::TypeEnv,
        type_subst::TypeSubst,
        unify::unify_core,
        unify_error::{UnifyErrorDetail, UnifyErrorKind},
    },
};

mod adt;
mod constraint;
mod display;
mod effects;
mod expression;
mod function;
mod solver;
mod statement;
mod unification;

// ─────────────────────────────────────────────────────────────────────────────
// Shared type definitions
// ─────────────────────────────────────────────────────────────────────────────

// ── ADT metadata ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AdtConstructorTypeInfo {
    adt_name: Identifier,
    type_params: Vec<Identifier>,
    fields: Vec<TypeExpr>,
}

// ── Diagnostic context ───────────────────────────────────────────────────────

/// Reporting mode for HM unification diagnostics.
#[derive(Debug, Clone)]
enum ReportContext {
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
    type_params: &'a [Identifier],
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
    known_base_names: HashSet<Identifier>,
    base_module_symbol: Identifier,
    adt_constructor_types: HashMap<Identifier, AdtConstructorTypeInfo>,
    /// Reverse index: ADT name → type parameters. Avoids linear scan over
    /// `adt_constructor_types` when only per-ADT metadata is needed.
    adt_type_params: HashMap<Identifier, Vec<Identifier>>,
    effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
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
    /// - `preloaded_base_schemes`: HM schemes for Base runtime bindings.
    /// - `preloaded_module_member_schemes`: HM schemes for imported module
    ///   members, keyed by `(module, member)`.
    /// - `known_base_names`: fast-membership set for names belonging to Base.
    /// - `base_module_symbol`: canonical symbol identifying the Base module.
    /// - `preloaded_effect_op_signatures`: signatures for effect operations,
    ///   keyed by `(effect, operation)`.
    fn new(
        interner: &'a Interner,
        file_path: Rc<str>,
        preloaded_base_schemes: HashMap<Identifier, Scheme>,
        preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
        known_base_names: HashSet<Identifier>,
        base_module_symbol: Identifier,
        preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
    ) -> Self {
        let mut env = TypeEnv::new();
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
            known_base_names,
            base_module_symbol,
            adt_constructor_types: HashMap::new(),
            adt_type_params: HashMap::new(),
            effect_op_signatures: preloaded_effect_op_signatures,
            ambient_effect_rows: Vec::new(),
            handled_effects: Vec::new(),
            seen_error_keys: HashSet::new(),
            contraint_log: Vec::new(),
            deferred_constraints: Vec::new(),
        }
    }

    /// Return `true` when `ty` is concrete and does not contain gradual `Any`.
    fn is_concrete_non_any(ty: &InferType) -> bool {
        ty.is_concrete() && !ty.contains_any()
    }

    /// Record a constraint in the log for observability and future deferred solving.
    fn record_constraint(&mut self, constraint: constraint::Constraint) {
        self.contraint_log.push(constraint);
    }
}

pub use display::{display_infer_type, suggest_type_name};

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
///     known_base_names: HashSet::new(),
///     base_module_symbol,
///     preloaded_effect_op_signatures: HashMap::new(),
/// };
/// ```
pub struct InferProgramConfig {
    pub file_path: Option<Rc<str>>,
    pub preloaded_base_schemes: HashMap<Identifier, Scheme>,
    pub preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    pub known_base_names: HashSet<Identifier>,
    pub base_module_symbol: Identifier,
    pub preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
}

/// Run Algorithm W (Hindley-Milner) over the entire program.
///
/// Returns the resulting `TypeEnv` (can be queried for any identifier's
/// inferred scheme) and a list of type-error diagnostics.
///
/// Type errors are **non-fatal**: inference always completes, recovering with
/// `Any` when unification fails.  The compiler can then use the env to enrich
/// its own static type information without gating on type errors.
///
/// # Examples
///
/// ```text
/// let result = infer_program(&program, &interner, InferProgramConfig {
///     file_path: Some("main.flx".into()),
///     preloaded_base_schemes: base_schemes,
///     preloaded_module_member_schemes: module_member_schemes,
///     known_base_names,
///     base_module_symbol,
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
        config.known_base_names,
        config.base_module_symbol,
        config.preloaded_effect_op_signatures,
    );
    ctx.infer_program(program);
    // Solve any deferred constraints (no-op under current eager model).
    ctx.solve_deferred_constraints();
    let constraint_count = ctx.contraint_log.len();
    InferProgramResult {
        type_env: ctx.env,
        diagnostics: ctx.errors,
        expr_types: ctx.expr_types,
        constraint_count,
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
    /// Total number of type/effect constraints generated during inference.
    pub constraint_count: usize,
}

/// Stable identifier for one expression node within a single inference run.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprNodeId(pub u32);
