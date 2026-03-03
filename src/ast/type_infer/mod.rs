use std::collections::{HashMap, HashSet};

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
        expression::{Expression, Pattern},
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
        unify_error::{UnifyErrorDetail, UnifyErrorKind, unify_with_span_and_row_var_counter},
    },
};

mod display;
mod unification;
mod effects;
mod adt;
mod statement;
mod function;
mod expression;

// ─────────────────────────────────────────────────────────────────────────────
// Shared type definitions
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AdtConstructorTypeInfo {
    adt_name: Identifier,
    type_params: Vec<Identifier>,
    fields: Vec<TypeExpr>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternFamily {
    Option,
    Either,
    List,
    Tuple(usize),
    Adt(Identifier),
    NonConstraining,
    UnknownOrMixed,
}

// ─────────────────────────────────────────────────────────────────────────────
// Inference context
// ─────────────────────────────────────────────────────────────────────────────

struct InferCtx<'a> {
    env: TypeEnv,
    interner: &'a Interner,
    errors: Vec<Diagnostic>,
    file_path: String,
    /// Accumulated global substitution — grows monotonically as constraints
    /// are solved.  Apply this to any `Ty` retrieved from the env to obtain
    /// its most-resolved form.
    subst: TypeSubst,
    next_expr_id: u32,
    expr_ptr_to_id: HashMap<usize, ExprNodeId>,
    expr_types: HashMap<ExprNodeId, InferType>,
    module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    known_base_names: HashSet<Identifier>,
    base_module_symbol: Identifier,
    adt_constructor_types: HashMap<Identifier, AdtConstructorTypeInfo>,
    effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
    ambient_effect_rows: Vec<InferEffectRow>,
    handled_effects: Vec<Identifier>,
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
        file_path: String,
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
            next_expr_id: 0,
            expr_ptr_to_id: HashMap::new(),
            expr_types: HashMap::new(),
            module_member_schemes: preloaded_module_member_schemes,
            known_base_names,
            base_module_symbol,
            adt_constructor_types: HashMap::new(),
            effect_op_signatures: preloaded_effect_op_signatures,
            ambient_effect_rows: Vec::new(),
            handled_effects: Vec::new(),
        }
    }

    fn node_id_for_expr(&mut self, expr: &Expression) -> ExprNodeId {
        let key = expr as *const Expression as usize;
        if let Some(id) = self.expr_ptr_to_id.get(&key) {
            return *id;
        }
        let id = ExprNodeId(self.next_expr_id);
        self.next_expr_id = self.next_expr_id.saturating_add(1);
        self.expr_ptr_to_id.insert(key, id);
        id
    }

    fn is_concrete_non_any(ty: &InferType) -> bool {
        ty.is_concrete() && !ty.contains_any()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Display helpers (public)
// ─────────────────────────────────────────────────────────────────────────────

pub use display::{display_infer_type, suggest_type_name};

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Run Algorithm W (Hindley-Milner) over the entire program.
///
/// Returns the resulting `TypeEnv` (can be queried for any identifier's
/// inferred scheme) and a list of type-error diagnostics.
///
/// Type errors are **non-fatal**: inference always completes, recovering with
/// `Any` when unification fails.  The compiler can then use the env to enrich
/// its own static type information without gating on type errors.
pub fn infer_program(
    program: &Program,
    interner: &Interner,
    file_path: Option<String>,
    preloaded_base_schemes: HashMap<Identifier, Scheme>,
    preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    known_base_names: HashSet<Identifier>,
    base_module_symbol: Identifier,
    preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
) -> InferProgramResult {
    let file = file_path.unwrap_or_default();
    let mut ctx = InferCtx::new(
        interner,
        file,
        preloaded_base_schemes,
        preloaded_module_member_schemes,
        known_base_names,
        base_module_symbol,
        preloaded_effect_op_signatures,
    );
    ctx.infer_program(program);
    InferProgramResult {
        type_env: ctx.env,
        diagnostics: ctx.errors,
        expr_types: ctx.expr_types,
        expr_ptr_to_id: ctx.expr_ptr_to_id,
    }
}

#[derive(Debug)]
pub struct InferProgramResult {
    pub type_env: TypeEnv,
    pub diagnostics: Vec<Diagnostic>,
    pub expr_types: HashMap<ExprNodeId, InferType>,
    pub expr_ptr_to_id: HashMap<usize, ExprNodeId>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExprNodeId(pub u32);
