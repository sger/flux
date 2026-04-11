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
use std::collections::{HashMap, HashSet};

use crate::{
    ast::free_vars::collect_free_vars_in_function_body,
    diagnostics::position::Span,
    syntax::{
        Identifier, block::Block, expression::ExprId, program::Program, statement::Statement,
    },
    types::infer_type::InferType,
};

use super::{CoreAlt, CoreBinder, CoreDef, CoreExpr, CoreLit, CoreProgram, CoreTopLevelItem};

mod binder_resolution;
mod expression;
mod pattern;

use binder_resolution::{resolve_program_binders, validate_program_binders};

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
    let mut lowerer = AstLowerer::new(hm_expr_types, interner, type_env, effect_op_sigs, class_env);
    let mut defs = Vec::new();
    let mut top_level_items = Vec::new();
    for stmt in &program.statements {
        lowerer.lower_top_level(stmt, &mut defs, &mut top_level_items);
    }
    let mut core = CoreProgram {
        defs,
        top_level_items,
    };
    resolve_program_binders(&mut core);
    assert!(
        validate_program_binders(&core),
        "Core binder resolution invariant failed after AST→Core lowering"
    );
    core
}

// ── Lowerer ───────────────────────────────────────────────────────────────────

pub(super) struct AstLowerer<'a> {
    /// HM-inferred types keyed by ExprId — used for type-directed code selection.
    pub(super) hm_expr_types: &'a HashMap<ExprId, InferType>,
    /// Counter for synthesizing fresh binding names.
    pub(super) fresh: u32,
    pub(super) next_binder_id: u32,
    /// Optional interner for resolving source-level type annotations.
    pub(super) interner: Option<&'a crate::syntax::interner::Interner>,
    /// Optional TypeEnv for looking up function parameter types (Phase 7).
    type_env: Option<&'a crate::types::type_env::TypeEnv>,
    /// Optional effect op signatures for typed handler binders (Phase 7f).
    pub(super) effect_op_sigs: Option<&'a EffectOpSigs>,
    /// Optional ClassEnv for compile-time class method dispatch (Phase 4 Step 5).
    pub(super) class_env: Option<&'a crate::types::class_env::ClassEnv>,
}

impl<'a> AstLowerer<'a> {
    fn new(
        hm_expr_types: &'a HashMap<ExprId, InferType>,
        interner: Option<&'a crate::syntax::interner::Interner>,
        type_env: Option<&'a crate::types::type_env::TypeEnv>,
        effect_op_sigs: Option<&'a EffectOpSigs>,
        class_env: Option<&'a crate::types::class_env::ClassEnv>,
    ) -> Self {
        Self {
            hm_expr_types,
            fresh: 0,
            next_binder_id: 0,
            interner,
            type_env,
            effect_op_sigs,
            class_env,
        }
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
        CoreBinder::with_rep(id, name, rep)
    }

    /// Get the `FluxRep` for an expression from HM type info.
    pub(super) fn rep_for_expr(&self, id: ExprId) -> super::FluxRep {
        self.hm_expr_types
            .get(&id)
            .map(super::FluxRep::from_infer_type)
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

    /// Create typed binders for lambda parameters by extracting param types
    /// from the lambda's HM-inferred function type.
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
                return parameters
                    .iter()
                    .zip(param_types)
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

    /// Try to resolve a class method call to a mangled instance function.
    ///
    /// If `name` is a known class method, and the first argument's type is
    /// known (concrete, not a type variable), resolves to `__tc_{Class}_{Type}_{method}`.
    /// Returns `None` if resolution fails (unknown type, no instance, no ClassEnv).
    pub(super) fn try_resolve_class_call(
        &self,
        name: Identifier,
        arguments: &[crate::syntax::expression::Expression],
    ) -> Option<Identifier> {
        let class_env = self.class_env?;
        let interner = self.interner?;

        // Check if this name is a class method.
        let (class_name, _class_def) = class_env.method_to_class(name)?;

        // Try compile-time resolution: if the first argument's type is concrete,
        // find the matching instance and build the mangled name from all of
        // the instance's type args (supporting multi-param classes).
        if let Some(first_arg) = arguments.first()
            && let Some(first_arg_type) = self.hm_expr_types.get(&first_arg.expr_id())
            && let Some((instance, _concrete_type_args)) = class_env
                .resolve_method_call_instance_from_first_arg(
                    class_name,
                    first_arg_type,
                    interner,
                )
        {
            // Build mangled name from the instance head exactly as dispatch
            // generation does. This preserves higher-kinded heads such as
            // `Functor<List>` while still allowing first-argument instance
            // selection for multi-parameter classes like `Convert<Int, String>`.
            let type_key = instance
                .type_args
                .iter()
                .map(|a| a.display_with(interner))
                .collect::<Vec<_>>()
                .join("_");
            let class_str = interner.resolve(class_name);
            let method_str = interner.resolve(name);
            let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
            if let Some(sym) = interner.lookup(&mangled) {
                return Some(sym);
            }
        }

        // No compile-time resolution possible — return None.
        // Dictionary elaboration handles polymorphic calls via dict params.
        None
    }

    pub(super) fn try_resolve_class_call_expr(
        &self,
        function: &crate::syntax::expression::Expression,
        arguments: &[crate::syntax::expression::Expression],
    ) -> Option<Identifier> {
        match function {
            crate::syntax::expression::Expression::Identifier { name, .. } => {
                self.try_resolve_class_call(*name, arguments)
            }
            crate::syntax::expression::Expression::MemberAccess { object, member, .. } => {
                let crate::syntax::expression::Expression::Identifier { .. } = object.as_ref()
                else {
                    return None;
                };
                self.try_resolve_class_call(*member, arguments)
            }
            _ => None,
        }
    }

    pub(super) fn resolve_direct_class_call_dict_args(
        &self,
        method_name: Identifier,
        arguments: &[crate::syntax::expression::Expression],
    ) -> Vec<CoreExpr> {
        let (class_env, interner) = match (self.class_env, self.interner) {
            (Some(class_env), Some(interner)) => (class_env, interner),
            _ => return Vec::new(),
        };
        let Some((class_name, _)) = class_env.method_to_class(method_name) else {
            return Vec::new();
        };
        let Some(first_arg) = arguments.first() else {
            return Vec::new();
        };
        let Some(first_arg_type) = self.hm_expr_types.get(&first_arg.expr_id()) else {
            return Vec::new();
        };

        class_env
            .resolve_instance_context_dictionaries(
                class_name,
                std::slice::from_ref(first_arg_type),
                interner,
            )
            .map(|dicts| dicts.iter().map(Self::lower_dictionary_ref).collect())
            .unwrap_or_default()
    }

    /// Resolve concrete dictionary arguments for a call to a constrained function.
    ///
    /// Looks up the callee's `Scheme` in the type environment. If it has class
    /// constraints, determines which concrete dictionaries to pass by examining
    /// the HM-inferred types at the call site.
    ///
    /// Returns a (possibly empty) vector of `CoreExpr::Var(__dict_{Class}_{Type})`
    /// for each constraint that could be resolved to a concrete dictionary.
    pub(super) fn resolve_dict_args_for_call(
        &self,
        callee_name: Identifier,
        call_id: ExprId,
        arguments: &[crate::syntax::expression::Expression],
    ) -> Vec<CoreExpr> {
        let (type_env, class_env, interner) = match (self.type_env, self.class_env, self.interner) {
            (Some(te), Some(ce), Some(int)) => (te, ce, int),
            _ => return Vec::new(),
        };

        let scheme = match type_env.lookup(callee_name) {
            Some(s) if !s.constraints.is_empty() => s,
            _ => return Vec::new(),
        };

        // For each constraint on the callee, try to determine the concrete type
        // by looking at the argument types at this call site.
        let mut dict_args = Vec::new();
        for constraint in &scheme.constraints {
            if let Some(actual_type_args) =
                self.resolve_constraint_type_args(constraint, scheme, call_id, arguments)
                && let Some(dict_ref) = class_env.resolve_dictionary_ref(
                    constraint.class_name,
                    &actual_type_args,
                    interner,
                )
            {
                dict_args.push(Self::lower_dictionary_ref(&dict_ref));
                continue;
            }

            // Could not resolve — don't partially apply dictionaries.
            return Vec::new();
        }

        dict_args
    }

    /// Try to determine the concrete type for a constraint's type variable
    /// by examining the argument types at a call site.
    ///
    /// For `fn contains<a: Eq>(xs: List<a>, elem: a)` called with `([1,2,3], 2)`,
    /// the constraint `Eq<a>` has `type_var` matching `a`. We look at the arguments'
    /// HM types and find `a = Int`.
    fn resolve_constraint_type_args(
        &self,
        constraint: &crate::ast::type_infer::constraint::SchemeConstraint,
        scheme: &crate::types::scheme::Scheme,
        call_id: ExprId,
        arguments: &[crate::syntax::expression::Expression],
    ) -> Option<Vec<InferType>> {
        if let InferType::Fun(param_tys, ret_ty, _) = &scheme.infer_type {
            let param_offset = param_tys.len().saturating_sub(arguments.len());
            let call_result_ty = self.hm_expr_types.get(&call_id);
            let mut resolved = Vec::with_capacity(constraint.type_vars.len());
            for type_var in &constraint.type_vars {
                let mut found = None;
                for (i, param_ty) in param_tys.iter().enumerate().skip(param_offset) {
                    if let Some(arg) = arguments.get(i - param_offset)
                        && let Some(arg_ty) = self.hm_expr_types.get(&arg.expr_id())
                        && let Some(actual) =
                            Self::match_constraint_type_var(param_ty, arg_ty, *type_var)
                    {
                        found = Some(actual);
                        break;
                    }
                }
                if found.is_none()
                    && let Some(actual_ret_ty) = call_result_ty
                    && let Some(actual) =
                        Self::match_constraint_type_var(ret_ty, actual_ret_ty, *type_var)
                {
                    found = Some(actual);
                }
                resolved.push(found?);
            }
            return Some(resolved);
        }

        None
    }

    fn match_constraint_type_var(
        pattern: &InferType,
        actual: &InferType,
        target: crate::types::TypeVarId,
    ) -> Option<InferType> {
        match pattern {
            InferType::Var(var) if *var == target => Some(actual.clone()),
            InferType::App(pattern_ctor, pattern_args) => {
                let InferType::App(actual_ctor, actual_args) = actual else {
                    return None;
                };
                if pattern_ctor != actual_ctor || pattern_args.len() != actual_args.len() {
                    return None;
                }
                pattern_args
                    .iter()
                    .zip(actual_args.iter())
                    .find_map(|(pattern_arg, actual_arg)| {
                        Self::match_constraint_type_var(pattern_arg, actual_arg, target)
                    })
            }
            InferType::Tuple(pattern_elems) => {
                let InferType::Tuple(actual_elems) = actual else {
                    return None;
                };
                if pattern_elems.len() != actual_elems.len() {
                    return None;
                }
                pattern_elems.iter().zip(actual_elems.iter()).find_map(
                    |(pattern_elem, actual_elem)| {
                        Self::match_constraint_type_var(pattern_elem, actual_elem, target)
                    },
                )
            }
            _ => None,
        }
    }

    fn lower_dictionary_ref(dict_ref: &crate::types::class_env::ResolvedDictionaryRef) -> CoreExpr {
        let span = crate::diagnostics::position::Span::default();
        if dict_ref.context_args.is_empty() {
            return CoreExpr::external_var(dict_ref.dict_name, span);
        }

        CoreExpr::App {
            func: Box::new(CoreExpr::external_var(dict_ref.dict_name, span)),
            args: dict_ref
                .context_args
                .iter()
                .map(Self::lower_dictionary_ref)
                .collect(),
            span,
        }
    }

    /// Convert an HM-inferred expression type to a `CoreType`, if available.
    fn infer_core_type(&self, id: ExprId) -> Option<super::CoreType> {
        self.hm_expr_types.get(&id).map(super::CoreType::from_infer)
    }

    /// Convert a source-level type annotation to a `CoreType`, if the interner
    /// is available and the annotation is a simple named type (Int, Float, etc.).
    fn core_type_from_type_expr(
        &self,
        type_expr: &crate::syntax::type_expr::TypeExpr,
    ) -> Option<super::CoreType> {
        let interner = self.interner?;
        match type_expr {
            crate::syntax::type_expr::TypeExpr::Named { name, args, .. } if args.is_empty() => {
                match interner.resolve(*name) {
                    "Int" => Some(super::CoreType::Int),
                    "Float" => Some(super::CoreType::Float),
                    "Bool" => Some(super::CoreType::Bool),
                    "String" => Some(super::CoreType::String),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // ── Top-level statements ─────────────────────────────────────────────────

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
                span,
                return_type,
                ..
            } => {
                let binder = self.bind_name(*name);
                let params = self.bind_fn_params(*name, parameters);
                let body_expr = self.lower_block(body);
                // Always wrap in Lam, even for parameterless functions — the
                // Core→IR lowerer uses the Lam marker to distinguish function
                // definitions from value bindings.  We construct Lam directly
                // instead of using CoreExpr::lambda() which elides empty params.
                let expr = CoreExpr::Lam {
                    params,
                    body: Box::new(body_expr),
                    span: *span,
                };
                // For functions, build a Function CoreType from body's last expression type.
                // The function itself doesn't have a single ExprId, but the body block does.
                let mut def = CoreDef::new(binder, expr, true, *span);
                def.fip = *fip;
                // Try to get the type from the last expression in the block.
                if let Some(
                    Statement::Expression { expression, .. }
                    | Statement::Return {
                        value: Some(expression),
                        ..
                    },
                ) = body.statements.last()
                {
                    def.result_ty = self.infer_core_type(expression.expr_id());
                }
                // If HM didn't resolve the result type, fall back to the
                // source return-type annotation.  This is critical for
                // recursive functions where HM may leave the return type as
                // an unresolved variable.
                if (def.result_ty.is_none() || matches!(def.result_ty, Some(super::CoreType::Any)))
                    && let Some(rt) = return_type
                    && let Some(annotated_ty) = self.core_type_from_type_expr(rt)
                {
                    def.result_ty = Some(annotated_ty);
                }
                out.push(def);
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
            Statement::Module { body, .. } => {
                self.lower_functions_in_module(&body.statements, out);
            }

            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::Class { .. }
            | Statement::Instance { .. } => {}
        }
    }

    fn lower_functions_in_module(&mut self, stmts: &[Statement], out: &mut Vec<CoreDef>) {
        for stmt in stmts {
            match stmt {
                Statement::Function {
                    name,
                    parameters,
                    body,
                    span,
                    ..
                } => {
                    let binder = self.bind_name(*name);
                    let params = self.bind_fn_params(*name, parameters);
                    let body_expr = self.lower_block(body);
                    let expr = CoreExpr::Lam {
                        params,
                        body: Box::new(body_expr),
                        span: *span,
                    };
                    out.push(CoreDef::new(binder, expr, true, *span));
                }
                Statement::Module { body, .. } => {
                    self.lower_functions_in_module(&body.statements, out);
                }
                _ => {}
            }
        }
    }

    fn lower_decl_item(&mut self, stmt: &Statement) -> Option<CoreTopLevelItem> {
        match stmt {
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
                span,
            } => Some(CoreTopLevelItem::Instance {
                class_name: *class_name,
                type_args: type_args.clone(),
                context: context.clone(),
                methods: methods.clone(),
                span: *span,
            }),
            Statement::Let { .. }
            | Statement::LetDestructure { .. }
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
    /// Detects runs of consecutive function statements that form mutual
    /// recursion groups (any function references a sibling defined later)
    /// and emits `LetRecGroup` for those instead of nested `LetRec`s.
    fn prepend_stmts(&mut self, stmts: &[Statement], body: CoreExpr, span: Span) -> CoreExpr {
        // Process statements right-to-left, but handle mutual recursion
        // groups using SCC (Strongly Connected Component) analysis.
        let mut result = body;
        let mut i = stmts.len();
        while i > 0 {
            i -= 1;
            if matches!(stmts[i], Statement::Function { .. }) {
                // Found a function. Scan backward for a contiguous run.
                let run_end = i + 1;
                let mut run_start = i;
                while run_start > 0 && matches!(stmts[run_start - 1], Statement::Function { .. }) {
                    run_start -= 1;
                }
                let fn_run = &stmts[run_start..run_end];
                if fn_run.len() >= 2 {
                    // Compute SCCs to partition into minimal binding groups.
                    result = self.lower_fn_run_with_scc(fn_run, result, span);
                } else {
                    result = self.prepend_one_stmt(&stmts[run_start], result, span);
                }
                i = run_start;
            } else {
                result = self.prepend_one_stmt(&stmts[i], result, span);
            }
        }
        result
    }

    /// Partition a contiguous run of function definitions into minimal
    /// binding groups using Tarjan's SCC algorithm, then lower each group.
    ///
    /// This replaces the conservative "group all if any forward reference"
    /// strategy with precise dependency analysis. Functions that don't
    /// participate in cycles become individual `LetRec` bindings that
    /// downstream passes (inliner, dead code elimination) can optimize.
    fn lower_fn_run_with_scc(
        &mut self,
        fn_stmts: &[Statement],
        tail: CoreExpr,
        span: Span,
    ) -> CoreExpr {
        // Step 1: Collect function names and their dependencies on siblings.
        let mut names: Vec<crate::syntax::Identifier> = Vec::new();
        let mut stmt_by_name: HashMap<crate::syntax::Identifier, &Statement> = HashMap::new();
        let mut deps: HashMap<crate::syntax::Identifier, HashSet<crate::syntax::Identifier>> =
            HashMap::new();

        let name_set: HashSet<crate::syntax::Identifier> = fn_stmts
            .iter()
            .filter_map(|s| {
                if let Statement::Function { name, .. } = s {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        for stmt in fn_stmts {
            if let Statement::Function {
                name,
                parameters,
                body,
                ..
            } = stmt
            {
                names.push(*name);
                stmt_by_name.insert(*name, stmt);
                let fv = collect_free_vars_in_function_body(parameters, body);
                // Only keep dependencies on siblings in this run.
                let sibling_deps: HashSet<crate::syntax::Identifier> =
                    fv.into_iter().filter(|v| name_set.contains(v)).collect();
                deps.insert(*name, sibling_deps);
            }
        }

        // Step 2: Compute SCCs via Tarjan's algorithm.
        let sccs = tarjan_scc(&names, &deps);

        // Step 3: Emit bindings in dependency order (SCCs are returned
        // in reverse topological order — dependencies come first).
        // We process right-to-left to build nested lets.
        let mut result = tail;
        for scc in sccs.iter().rev() {
            if scc.len() == 1 {
                // Single function — emit as individual LetRec.
                let name = scc[0];
                let stmt = stmt_by_name[&name];
                result = self.prepend_one_stmt(stmt, result, span);
            } else {
                // Multiple functions in a cycle — emit as LetRecGroup.
                let group_stmts: Vec<&Statement> = scc.iter().map(|n| stmt_by_name[n]).collect();
                result = self.lower_scc_group(&group_stmts, result);
            }
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

        let bindings: Vec<_> = stmts
            .iter()
            .map(|stmt| {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    span: s,
                    ..
                } = stmt
                else {
                    unreachable!("lower_scc_group called with non-function statement");
                };
                let binder = self.bind_name(*name);
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let body_expr = self.lower_block(body);
                (
                    binder,
                    Box::new(CoreExpr::Lam {
                        params,
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
                    span: s,
                    ..
                } = stmt
                else {
                    unreachable!();
                };
                let binder = self.bind_name(*name);
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let body_expr = self.lower_block(body);
                CoreExpr::LetRec {
                    var: binder,
                    rhs: Box::new(CoreExpr::Lam {
                        params,
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
            } => {
                // Bind the scrutinee to a tmp var and use Case to extract fields.
                let rhs = self.lower_expr(value);
                let core_pat = self.lower_pattern(pattern);
                // Build: Let($tmp, rhs, Case($tmp, [core_pat → tail]))
                // We use a special sentinel name for the tmp var.
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 1_000_000);
                self.fresh += 1;
                let tmp_binder = self.fresh_binder(tmp_name);
                let tmp_var = CoreExpr::bound_var(tmp_binder, *s);
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

// ── Tarjan's SCC algorithm ─────────────────────────────────────────────────

/// Compute strongly connected components of a function dependency graph
/// using Tarjan's algorithm. Returns SCCs in reverse topological order
/// (dependencies before dependents).
///
/// Each function name maps to a set of sibling function names it references.
/// Single-element SCCs that are not self-referencing represent non-recursive
/// functions; multi-element SCCs represent true mutual recursion.
fn tarjan_scc(
    names: &[crate::syntax::Identifier],
    deps: &HashMap<crate::syntax::Identifier, HashSet<crate::syntax::Identifier>>,
) -> Vec<Vec<crate::syntax::Identifier>> {
    let n = names.len();
    let name_to_idx: HashMap<crate::syntax::Identifier, usize> =
        names.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    let mut index_counter: usize = 0;
    let mut indices = vec![usize::MAX; n];
    let mut lowlinks = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut result: Vec<Vec<crate::syntax::Identifier>> = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        v: usize,
        names: &[crate::syntax::Identifier],
        deps: &HashMap<crate::syntax::Identifier, HashSet<crate::syntax::Identifier>>,
        name_to_idx: &HashMap<crate::syntax::Identifier, usize>,
        index_counter: &mut usize,
        indices: &mut [usize],
        lowlinks: &mut [usize],
        on_stack: &mut [bool],
        stack: &mut Vec<usize>,
        result: &mut Vec<Vec<crate::syntax::Identifier>>,
    ) {
        indices[v] = *index_counter;
        lowlinks[v] = *index_counter;
        *index_counter += 1;
        stack.push(v);
        on_stack[v] = true;

        // Visit successors.
        if let Some(v_deps) = deps.get(&names[v]) {
            for dep in v_deps {
                if let Some(&w) = name_to_idx.get(dep) {
                    if indices[w] == usize::MAX {
                        // w not yet visited — recurse.
                        strongconnect(
                            w,
                            names,
                            deps,
                            name_to_idx,
                            index_counter,
                            indices,
                            lowlinks,
                            on_stack,
                            stack,
                            result,
                        );
                        lowlinks[v] = lowlinks[v].min(lowlinks[w]);
                    } else if on_stack[w] {
                        // w is on the stack — part of current SCC.
                        lowlinks[v] = lowlinks[v].min(indices[w]);
                    }
                }
            }
        }

        // If v is a root node, pop the SCC.
        if lowlinks[v] == indices[v] {
            let mut scc = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack[w] = false;
                scc.push(names[w]);
                if w == v {
                    break;
                }
            }
            result.push(scc);
        }
    }

    for i in 0..n {
        if indices[i] == usize::MAX {
            strongconnect(
                i,
                names,
                deps,
                &name_to_idx,
                &mut index_counter,
                &mut indices,
                &mut lowlinks,
                &mut on_stack,
                &mut stack,
                &mut result,
            );
        }
    }

    result
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
                known_flow_names: std::collections::HashSet::new(),
                flow_module_symbol: flow_sym,
                preloaded_effect_op_signatures: HashMap::new(),
                class_env: None,
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
            CoreExpr::Handle { body, handlers, .. } => {
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
        // This test verifies that untyped (Any) additions stay as generic Add.
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

    // ── Tarjan SCC unit tests ──────────────────────────────────────────

    fn sym(id: u32) -> crate::syntax::symbol::Symbol {
        crate::syntax::symbol::Symbol::new(id)
    }

    #[test]
    fn tarjan_scc_chain_produces_separate_sccs() {
        // a→b→c (no cycle) → three separate SCCs
        let names = vec![sym(0), sym(1), sym(2)];
        let mut deps = HashMap::new();
        deps.insert(sym(0), HashSet::from([sym(1)])); // a depends on b
        deps.insert(sym(1), HashSet::from([sym(2)])); // b depends on c
        deps.insert(sym(2), HashSet::new()); // c depends on nothing

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs.len(), 3, "chain should produce 3 SCCs: {sccs:?}");
        // Each SCC has exactly one element.
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    #[test]
    fn tarjan_scc_mutual_recursion_produces_single_group() {
        // a↔b (cycle) → one SCC with both
        let names = vec![sym(0), sym(1)];
        let mut deps = HashMap::new();
        deps.insert(sym(0), HashSet::from([sym(1)])); // a depends on b
        deps.insert(sym(1), HashSet::from([sym(0)])); // b depends on a

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(
            sccs.len(),
            1,
            "mutual recursion should produce 1 SCC: {sccs:?}"
        );
        assert_eq!(sccs[0].len(), 2);
    }

    #[test]
    fn tarjan_scc_mixed_cycle_and_independent() {
        // a↔b, c independent → two SCCs: {c} and {a,b}
        let names = vec![sym(0), sym(1), sym(2)];
        let mut deps = HashMap::new();
        deps.insert(sym(0), HashSet::from([sym(1)])); // a depends on b
        deps.insert(sym(1), HashSet::from([sym(0)])); // b depends on a
        deps.insert(sym(2), HashSet::new()); // c independent

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs.len(), 2, "should produce 2 SCCs: {sccs:?}");
        let cycle_scc = sccs
            .iter()
            .find(|s| s.len() == 2)
            .expect("one SCC with 2 elements");
        assert!(cycle_scc.contains(&sym(0)));
        assert!(cycle_scc.contains(&sym(1)));
    }

    #[test]
    fn tarjan_scc_three_way_cycle() {
        // a→b→c→a (triangle cycle)
        let names = vec![sym(0), sym(1), sym(2)];
        let mut deps = HashMap::new();
        deps.insert(sym(0), HashSet::from([sym(1)]));
        deps.insert(sym(1), HashSet::from([sym(2)]));
        deps.insert(sym(2), HashSet::from([sym(0)]));

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs.len(), 1, "triangle cycle should be one SCC: {sccs:?}");
        assert_eq!(sccs[0].len(), 3);
    }

    #[test]
    fn tarjan_scc_no_dependencies() {
        // a, b, c all independent → three separate SCCs
        let names = vec![sym(0), sym(1), sym(2)];
        let deps = HashMap::from([
            (sym(0), HashSet::new()),
            (sym(1), HashSet::new()),
            (sym(2), HashSet::new()),
        ]);

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs.len(), 3);
    }

    #[test]
    fn tarjan_scc_self_recursive_single() {
        // a→a (self-recursive) → one SCC with one element
        let names = vec![sym(0)];
        let deps = HashMap::from([(sym(0), HashSet::from([sym(0)]))]);

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 1);
        assert_eq!(sccs[0][0], sym(0));
    }

    #[test]
    fn tarjan_scc_reverse_topological_order() {
        // a→b→c: SCCs should come out as [c], [b], [a] (deps first)
        let names = vec![sym(0), sym(1), sym(2)];
        let mut deps = HashMap::new();
        deps.insert(sym(0), HashSet::from([sym(1)]));
        deps.insert(sym(1), HashSet::from([sym(2)]));
        deps.insert(sym(2), HashSet::new());

        let sccs = tarjan_scc(&names, &deps);
        assert_eq!(sccs[0][0], sym(2), "c should come first (no deps)");
        assert_eq!(sccs[1][0], sym(1), "b should come second");
        assert_eq!(sccs[2][0], sym(0), "a should come last (depends on b)");
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
}
