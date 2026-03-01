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
        infer_type::InferType,
        scheme::{Scheme, generalize},
        type_constructor::TypeConstructor,
        type_env::TypeEnv,
        type_subst::TypeSubst,
        unify_error::{UnifyErrorDetail, UnifyErrorKind, unify_with_span},
    },
};

// ─────────────────────────────────────────────────────────────────────────────
// Display helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an `InferType` for user-facing diagnostics, resolving ADT symbols
/// to their human-readable names via the interner. Unresolved type variables
/// display as `_` (unknown type).
pub fn display_infer_type(ty: &InferType, interner: &Interner) -> String {
    match ty {
        InferType::Var(_) => "_".to_string(),
        InferType::Con(c) => display_type_constructor(c, interner),
        InferType::App(c, args) => {
            let base = display_type_constructor(c, interner);
            let args_str: Vec<String> = args
                .iter()
                .map(|a| display_infer_type(a, interner))
                .collect();
            format!("{}<{}>", base, args_str.join(", "))
        }
        InferType::Fun(params, ret, effects) => {
            let params_str: Vec<String> = params
                .iter()
                .map(|p| display_infer_type(p, interner))
                .collect();
            let ret_str = display_infer_type(ret, interner);
            if effects.is_empty() {
                format!("({}) -> {}", params_str.join(", "), ret_str)
            } else {
                let eff_str: Vec<String> = effects
                    .iter()
                    .map(|e| interner.resolve(*e).to_string())
                    .collect();
                format!(
                    "({}) -> {} with {}",
                    params_str.join(", "),
                    ret_str,
                    eff_str.join(", ")
                )
            }
        }
        InferType::Tuple(elems) => {
            let elems_str: Vec<String> = elems
                .iter()
                .map(|e| display_infer_type(e, interner))
                .collect();
            format!("({})", elems_str.join(", "))
        }
    }
}

fn display_type_constructor(c: &TypeConstructor, interner: &Interner) -> String {
    match c {
        TypeConstructor::Adt(sym) => interner.resolve(*sym).to_string(),
        _ => c.to_string(),
    }
}

/// Built-in type names used for "did you mean?" suggestions.
const KNOWN_TYPE_NAMES: &[&str] = &[
    "Int", "Float", "Bool", "String", "Unit", "List", "Map", "Array", "Option", "Either",
];

/// If a type name looks like a typo of a known built-in type, return a
/// suggestion string like `did you mean \`String\`?`.
pub fn suggest_type_name(name: &str) -> Option<String> {
    // Don't suggest for known types or very short names
    if KNOWN_TYPE_NAMES.contains(&name) || name.len() < 2 {
        return None;
    }
    let best = KNOWN_TYPE_NAMES
        .iter()
        .filter_map(|&known| {
            let d = levenshtein_distance(name, known);
            // Allow distance ≤ 2, or prefix match
            if d <= 2 || known.starts_with(name) || name.starts_with(known) {
                Some((d, known))
            } else {
                None
            }
        })
        .min_by_key(|(d, _)| *d);

    best.map(|(_, known)| format!("did you mean `{known}`?"))
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
    adt_constructor_types: HashMap<Identifier, AdtConstructorTypeInfo>,
    effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
}

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

impl<'a> InferCtx<'a> {
    /// Format an `InferType` for user-facing diagnostics, resolving ADT
    /// symbols to their human-readable names via the interner.
    fn display_type(&self, ty: &InferType) -> String {
        display_infer_type(ty, self.interner)
    }

    fn new(
        interner: &'a Interner,
        file_path: String,
        preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
        preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
    ) -> Self {
        InferCtx {
            env: TypeEnv::new(),
            interner,
            errors: Vec::new(),
            file_path,
            subst: TypeSubst::empty(),
            next_expr_id: 0,
            expr_ptr_to_id: HashMap::new(),
            expr_types: HashMap::new(),
            module_member_schemes: preloaded_module_member_schemes,
            adt_constructor_types: HashMap::new(),
            effect_op_signatures: preloaded_effect_op_signatures,
        }
    }

    fn effect_op_signature_types(
        &self,
        effect: Identifier,
        operation: Identifier,
    ) -> Option<(Vec<InferType>, InferType)> {
        let type_expr = self.effect_op_signatures.get(&(effect, operation))?;
        let TypeExpr::Function {
            params,
            ret,
            effects: _,
            span: _,
        } = type_expr
        else {
            return None;
        };
        let tp_map: HashMap<Identifier, TypeVarId> = HashMap::new();
        let param_tys = params
            .iter()
            .map(|p| TypeEnv::infer_type_from_type_expr(p, &tp_map, self.interner))
            .collect::<Option<Vec<_>>>()?;
        let ret_ty = TypeEnv::infer_type_from_type_expr(ret, &tp_map, self.interner)?;
        Some((param_tys, ret_ty))
    }

    fn pattern_family(&self, pattern: &Pattern) -> PatternFamily {
        match pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } | Pattern::Literal { .. } => {
                PatternFamily::NonConstraining
            }
            Pattern::None { .. } | Pattern::Some { .. } => PatternFamily::Option,
            Pattern::Left { .. } | Pattern::Right { .. } => PatternFamily::Either,
            Pattern::EmptyList { .. } | Pattern::Cons { .. } => PatternFamily::List,
            Pattern::Tuple { elements, .. } => PatternFamily::Tuple(elements.len()),
            Pattern::Constructor { name, .. } => self
                .adt_constructor_types
                .get(name)
                .map(|info| PatternFamily::Adt(info.adt_name))
                .unwrap_or(PatternFamily::UnknownOrMixed),
        }
    }

    fn match_constraint_family(
        &self,
        arms: &[crate::syntax::expression::MatchArm],
    ) -> Option<PatternFamily> {
        let mut family: Option<PatternFamily> = None;
        for arm in arms {
            let arm_family = self.pattern_family(&arm.pattern);
            match arm_family {
                PatternFamily::NonConstraining => {}
                PatternFamily::UnknownOrMixed => return None,
                _ => match &family {
                    None => family = Some(arm_family),
                    Some(existing) if *existing == arm_family => {}
                    Some(_) => return None,
                },
            }
        }
        family
    }

    fn family_expected_type(&mut self, family: &PatternFamily) -> Option<InferType> {
        match family {
            PatternFamily::Option => Some(InferType::App(
                TypeConstructor::Option,
                vec![self.env.fresh_infer_type()],
            )),
            PatternFamily::Either => Some(InferType::App(
                TypeConstructor::Either,
                vec![self.env.fresh_infer_type(), self.env.fresh_infer_type()],
            )),
            PatternFamily::List => Some(InferType::App(
                TypeConstructor::List,
                vec![self.env.fresh_infer_type()],
            )),
            PatternFamily::Tuple(arity) => Some(InferType::Tuple(
                (0..*arity).map(|_| self.env.fresh_infer_type()).collect(),
            )),
            PatternFamily::Adt(adt_name) => {
                let info = self
                    .adt_constructor_types
                    .values()
                    .find(|info| info.adt_name == *adt_name)?;
                if info.type_params.is_empty() {
                    Some(InferType::Con(TypeConstructor::Adt(*adt_name)))
                } else {
                    Some(InferType::App(
                        TypeConstructor::Adt(*adt_name),
                        info.type_params
                            .iter()
                            .map(|_| self.env.fresh_infer_type())
                            .collect(),
                    ))
                }
            }
            PatternFamily::NonConstraining | PatternFamily::UnknownOrMixed => None,
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

    // ── Unification with error recovery ──────────────────────────────────────

    /// Join two types for branch contexts (if/else, match arms).
    ///
    /// Unlike `unify_reporting`, this does NOT add substitution constraints —
    /// it only compares the already-resolved types.  When the resolved types
    /// agree exactly, the common type is returned.  When they differ, `Any` is
    /// returned without modifying the substitution.
    ///
    /// This models Flux's gradual type system where different branches may
    /// legitimately produce values of different types (union falls back to Any).
    fn join_types(&mut self, t1: &InferType, t2: &InferType) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        if t1_sub == t2_sub {
            t1_sub
        } else {
            InferType::Con(TypeConstructor::Any)
        }
    }

    /// Unify `t1` with `t2` silently — update the substitution for type
    /// propagation but never emit a diagnostic on failure.
    ///
    /// Used at annotated boundary sites (return type annotations, typed `let`
    /// initializers) where the compiler's boundary checker is the authoritative
    /// error reporter.  HM still needs the substitution side-effect so that
    /// downstream inference sees the annotation constraint.
    fn unify_propagate(&mut self, t1: &InferType, t2: &InferType) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        match unify_with_span(&t1_sub, &t2_sub, Span::default()) {
            Ok(s) => {
                self.subst = std::mem::take(&mut self.subst).compose(&s);
                t1_sub.apply_type_subst(&self.subst)
            }
            Err(_) => {
                // Compiler boundary check will report — return the annotation
                // type so that downstream inference stays consistent with the
                // programmer's declared intent.
                t2_sub.apply_type_subst(&self.subst)
            }
        }
    }

    /// Unify `t1` with `t2`, composing the result into `self.subst` with an
    /// explicit reporting context.
    fn unify_with_context(
        &mut self,
        t1: &InferType,
        t2: &InferType,
        span: Span,
        context: ReportContext,
    ) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        match unify_with_span(&t1_sub, &t2_sub, span) {
            Ok(s) => {
                // Compose the new solution into the global substitution.
                self.subst = std::mem::take(&mut self.subst).compose(&s);
                t1_sub.apply_type_subst(&self.subst)
            }
            Err(e) => {
                // Only emit a diagnostic when both conflicting types are fully
                // concrete (no unresolved type variables) and neither is `Any`.
                //
                // This prevents false positives in gradual / partially-typed code
                // where a fresh variable from an uninferred base-function call
                // collides with a known type — those conflicts resolve to `Any`
                // once the base-function signature is known.
                let should_emit = e.expected.is_concrete()
                    && e.actual.is_concrete()
                    && !e.expected.contains_any()
                    && !e.actual.contains_any();

                if should_emit {
                    let file = self.file_path.clone();
                    let function_detail_diag = || match &e.detail {
                        UnifyErrorDetail::FunArityMismatch { expected, actual } => {
                            Some(fun_arity_mismatch(file.clone(), span, *expected, *actual))
                        }
                        UnifyErrorDetail::FunParamMismatch { index } => {
                            let exp_param = self.display_type(&e.expected);
                            let act_param = self.display_type(&e.actual);
                            Some(fun_param_type_mismatch(
                                file.clone(),
                                span,
                                *index + 1,
                                &exp_param,
                                &act_param,
                            ))
                        }
                        UnifyErrorDetail::FunReturnMismatch => {
                            let exp_ret = self.display_type(&e.expected);
                            let act_ret = self.display_type(&e.actual);
                            Some(fun_return_type_mismatch(
                                file.clone(),
                                span,
                                &exp_ret,
                                &act_ret,
                            ))
                        }
                        UnifyErrorDetail::None => None,
                    };
                    let mut diag = match (context, &e.kind) {
                        (ReportContext::Plain, UnifyErrorKind::OccursCheck(v)) => {
                            let v_str = format!("t{v}");
                            let ty_str = self.display_type(&e.actual);
                            occurs_check_failure(file, span, &v_str, &ty_str)
                        }
                        (ReportContext::Plain, UnifyErrorKind::Mismatch) => {
                            if let Some(diag) = function_detail_diag() {
                                diag
                            } else {
                                let exp_str = self.display_type(&e.expected);
                                let act_str = self.display_type(&e.actual);
                                type_unification_error(file, span, &exp_str, &act_str)
                            }
                        }
                        (
                            ReportContext::IfBranch {
                                then_span,
                                else_span,
                            },
                            UnifyErrorKind::Mismatch,
                        ) => {
                            if let Some(diag) = function_detail_diag() {
                                diag
                            } else {
                                let then_ty = self.display_type(&e.expected);
                                let else_ty = self.display_type(&e.actual);
                                if_branch_type_mismatch(
                                    file, then_span, else_span, &then_ty, &else_ty,
                                )
                            }
                        }
                        (ReportContext::IfBranch { .. }, UnifyErrorKind::OccursCheck(v)) => {
                            let v_str = format!("t{v}");
                            let ty_str = self.display_type(&e.actual);
                            occurs_check_failure(file, span, &v_str, &ty_str)
                        }
                        (
                            ReportContext::MatchArm {
                                first_span,
                                arm_span,
                                arm_index,
                            },
                            UnifyErrorKind::Mismatch,
                        ) => {
                            if let Some(diag) = function_detail_diag() {
                                diag
                            } else {
                                let first_ty = self.display_type(&e.expected);
                                let arm_ty = self.display_type(&e.actual);
                                crate::diagnostics::compiler_errors::match_arm_type_mismatch(
                                    file, first_span, arm_span, &first_ty, &arm_ty, arm_index,
                                )
                            }
                        }
                        (ReportContext::MatchArm { .. }, UnifyErrorKind::OccursCheck(v)) => {
                            let v_str = format!("t{v}");
                            let ty_str = self.display_type(&e.actual);
                            occurs_check_failure(file, span, &v_str, &ty_str)
                        }
                        (
                            ReportContext::CallArg {
                                fn_name,
                                fn_def_span,
                            },
                            UnifyErrorKind::Mismatch,
                        ) => {
                            if let Some(diag) = function_detail_diag() {
                                diag
                            } else {
                                let exp_str = self.display_type(&e.expected);
                                let act_str = self.display_type(&e.actual);
                                // Fallback path is for dynamic/opaque callees where we do
                                // not have per-argument mismatch detail. Keep `1` as a stable
                                // placeholder until/if this path is upgraded with indexed detail.
                                call_arg_type_mismatch(
                                    file,
                                    span,
                                    fn_name.as_deref(),
                                    1,
                                    fn_def_span,
                                    &exp_str,
                                    &act_str,
                                )
                            }
                        }
                        (ReportContext::CallArg { .. }, UnifyErrorKind::OccursCheck(v)) => {
                            let v_str = format!("t{v}");
                            let ty_str = self.display_type(&e.actual);
                            occurs_check_failure(file, span, &v_str, &ty_str)
                        }
                    };
                    // Add "did you mean?" hint for likely type name typos
                    for ty in [&e.expected, &e.actual] {
                        if let InferType::Con(TypeConstructor::Adt(sym)) = ty {
                            let name = self.interner.resolve(*sym);
                            if let Some(suggestion) = suggest_type_name(name) {
                                diag.hints
                                    .push(crate::diagnostics::types::Hint::help(format!(
                                        "Unknown type `{name}` — {suggestion}"
                                    )));
                            }
                        }
                    }
                    self.errors.push(diag);
                }
                InferType::Con(TypeConstructor::Any)
            }
        }
    }

    /// Unify `t1` with `t2`, composing the result into `self.subst`.
    ///
    /// On success, returns the resolved first type.
    /// On failure, emits a diagnostic and returns `Any` so that inference can
    /// continue without cascading errors.
    fn unify_reporting(&mut self, t1: &InferType, t2: &InferType, span: Span) -> InferType {
        self.unify_with_context(t1, t2, span, ReportContext::Plain)
    }

    // ── Program / statement inference ─────────────────────────────────────────

    fn infer_program(&mut self, program: &Program) {
        // Phase A0: predeclare top-level ADT constructors so functions can
        // reference constructors defined later in the file.
        self.predeclare_data_constructors_in_statements(&program.statements);

        // Phase A: pre-declare all top-level function names with a fresh type
        // variable so that mutually-recursive functions can reference each other.
        for stmt in &program.statements {
            if let Statement::Function { name, span, .. } = stmt {
                let v = self.env.fresh_infer_type();
                self.env.bind_with_span(*name, Scheme::mono(v), Some(*span));
            }
        }

        // Phase B: infer each top-level statement.
        for stmt in &program.statements {
            self.infer_stmt(stmt);
        }
    }

    fn infer_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Function {
                name,
                span,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                self.infer_fn(
                    *name,
                    *span,
                    type_params,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                );
            }
            Statement::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                self.infer_let(*name, type_annotation.as_ref(), value);
            }
            Statement::LetDestructure {
                pattern,
                value,
                span,
            } => {
                let val_ty = self.infer_expr(value);
                self.bind_pattern(pattern, &val_ty, *span);
            }
            Statement::Expression { expression, .. } => {
                // Evaluate for side effects; type is discarded.
                self.infer_expr(expression);
            }
            Statement::Assign { value, .. } => {
                self.infer_expr(value);
            }
            Statement::Module { name, body, .. } => self.infer_module(*name, body),
            Statement::Data {
                name,
                type_params,
                variants,
                ..
            } => {
                self.register_data_constructors(*name, type_params, variants);
            }
            // Import, Return at top-level: no HM inference needed.
            _ => {}
        }
    }

    fn predeclare_data_constructors_in_statements(&mut self, statements: &[Statement]) {
        for stmt in statements {
            if let Statement::Data {
                name,
                type_params,
                variants,
                ..
            } = stmt
            {
                self.register_data_constructors(*name, type_params, variants);
            }
        }
    }

    fn register_data_constructors(
        &mut self,
        adt_name: Identifier,
        type_params: &[Identifier],
        variants: &[DataVariant],
    ) {
        for variant in variants {
            self.adt_constructor_types.insert(
                variant.name,
                AdtConstructorTypeInfo {
                    adt_name,
                    type_params: type_params.to_vec(),
                    fields: variant.fields.clone(),
                },
            );

            let Some((field_tys, result_ty)) = self.instantiate_constructor_parts(variant.name)
            else {
                continue;
            };
            let ctor_ty = if field_tys.is_empty() {
                result_ty
            } else {
                InferType::Fun(field_tys, Box::new(result_ty), vec![])
            };
            let scheme = generalize(&ctor_ty, &HashSet::new());
            self.env.bind(variant.name, scheme);
        }
    }

    // ── Function inference ────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn infer_fn(
        &mut self,
        name: Identifier,
        fn_span: Span,
        type_params: &[Identifier],
        parameters: &[Identifier],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &Block,
    ) {
        // Map explicit type parameters (e.g. `T`, `U`) to fresh type variables.
        let tp_map: HashMap<Identifier, TypeVarId> =
            type_params.iter().map(|s| (*s, self.env.fresh())).collect();

        self.env.enter_scope();

        // Bind each parameter to its annotated type (or a fresh variable).
        let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
        for (i, &param) in parameters.iter().enumerate() {
            let ty = parameter_types
                .get(i)
                .and_then(|opt| opt.as_ref())
                .and_then(|te| TypeEnv::infer_type_from_type_expr(te, &tp_map, self.interner))
                .unwrap_or_else(|| self.env.fresh_infer_type());
            param_tys.push(ty.clone());
            self.env.bind(param, Scheme::mono(ty));
        }

        // Infer the body type.
        let body_ty = self.infer_block(body);

        // Propagate the return type annotation constraint silently — the
        // compiler's boundary checker in statement.rs is the authoritative
        // reporter for return type mismatches.
        let mut ret_ty = match return_type {
            Some(ret_ann) => {
                match TypeEnv::infer_type_from_type_expr(ret_ann, &tp_map, self.interner) {
                    Some(ann_ty) => self.unify_propagate(&body_ty, &ann_ty),
                    None => body_ty.apply_type_subst(&self.subst),
                }
            }
            None => body_ty.apply_type_subst(&self.subst),
        };

        // T11 (self-only): run one extra refinement pass for unannotated
        // self-recursive functions so recursive call result types can feed
        // back into the function return slot.
        if return_type.is_none()
            && type_params.is_empty()
            && self.block_contains_self_call(body, name)
        {
            ret_ty = self.refine_unannotated_self_recursive_return(
                name, parameters, &param_tys, effects, body, &ret_ty,
            );
        }

        // Resolve parameter types through the accumulated substitution.
        let final_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|t| t.apply_type_subst(&self.subst))
            .collect();
        let effect_symbols = effects
            .iter()
            .flat_map(EffectExpr::normalized_names)
            .collect();
        let fn_ty = InferType::Fun(final_param_tys, Box::new(ret_ty), effect_symbols);

        self.env.leave_scope();

        // Generalize: quantify over type variables that are free in `fn_ty`
        // but not in the surrounding environment (the let-generalization step).
        // We only generalize functions with explicit type parameters — for
        // implicitly typed functions, we keep the monomorphic type so that
        // unification constraints across call sites are preserved.
        let env_free = self.env.free_vars();
        let scheme = if !type_params.is_empty() {
            generalize(&fn_ty, &env_free)
        } else {
            Scheme::mono(fn_ty)
        };

        // Update the pre-declared entry (from Phase A).
        self.env.bind_with_span(name, scheme, Some(fn_span));
    }

    fn refine_unannotated_self_recursive_return(
        &mut self,
        name: Identifier,
        parameters: &[Identifier],
        param_tys: &[InferType],
        effects: &[EffectExpr],
        body: &Block,
        current_ret: &InferType,
    ) -> InferType {
        self.env.enter_scope();
        let refined_param_tys: Vec<InferType> = param_tys
            .iter()
            .map(|ty| ty.apply_type_subst(&self.subst))
            .collect();
        for (param_name, param_ty) in parameters.iter().zip(refined_param_tys.iter()) {
            self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
        }
        let ret_slot = self.env.fresh_infer_type();
        let effect_symbols = effects
            .iter()
            .flat_map(EffectExpr::normalized_names)
            .collect();
        let self_fn_ty = InferType::Fun(
            refined_param_tys,
            Box::new(ret_slot.clone()),
            effect_symbols,
        );
        self.env.bind(name, Scheme::mono(self_fn_ty));
        let second_body_ty = self.infer_block(body);
        let refined_ret = self.unify_propagate(&second_body_ty, &ret_slot);
        self.env.leave_scope();
        let refined_resolved = refined_ret.apply_type_subst(&self.subst);
        let current_resolved = current_ret.apply_type_subst(&self.subst);
        let current_concrete = Self::is_concrete_non_any(&current_resolved);
        let refined_concrete = Self::is_concrete_non_any(&refined_resolved);

        if current_concrete && !refined_concrete {
            current_resolved
        } else if (refined_concrete && !current_concrete) || current_ret.contains_any() {
            refined_resolved
        } else if refined_resolved.contains_any() {
            // Keep the prior concrete inference when the refinement pass did not
            // increase precision and would otherwise fall back to Any.
            current_resolved
        } else {
            self.unify_propagate(&current_resolved, &refined_resolved)
                .apply_type_subst(&self.subst)
        }
    }

    fn block_contains_self_call(&self, block: &Block, name: Identifier) -> bool {
        block
            .statements
            .iter()
            .any(|stmt| self.statement_contains_self_call(stmt, name))
    }

    fn statement_contains_self_call(&self, stmt: &Statement, name: Identifier) -> bool {
        match stmt {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => self.expression_contains_self_call(value, name),
            Statement::Return {
                value: Some(expr), ..
            }
            | Statement::Expression {
                expression: expr, ..
            } => self.expression_contains_self_call(expr, name),
            Statement::Module { body, .. } => self.block_contains_self_call(body, name),
            _ => false,
        }
    }

    fn expression_contains_self_call(&self, expr: &Expression, name: Identifier) -> bool {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if let Expression::Identifier { name: callee, .. } = function.as_ref()
                    && *callee == name
                {
                    return true;
                }
                self.expression_contains_self_call(function, name)
                    || arguments
                        .iter()
                        .any(|arg| self.expression_contains_self_call(arg, name))
            }
            Expression::Prefix { right, .. } => self.expression_contains_self_call(right, name),
            Expression::Infix { left, right, .. } => {
                self.expression_contains_self_call(left, name)
                    || self.expression_contains_self_call(right, name)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expression_contains_self_call(condition, name)
                    || self.block_contains_self_call(consequence, name)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_contains_self_call(b, name))
            }
            Expression::DoBlock { block, .. } => self.block_contains_self_call(block, name),
            Expression::Function { .. } => false,
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements
                .iter()
                .any(|element| self.expression_contains_self_call(element, name)),
            Expression::Hash { pairs, .. } => pairs.iter().any(|(k, v)| {
                self.expression_contains_self_call(k, name)
                    || self.expression_contains_self_call(v, name)
            }),
            Expression::Cons { head, tail, .. } => {
                self.expression_contains_self_call(head, name)
                    || self.expression_contains_self_call(tail, name)
            }
            Expression::Index { left, index, .. } => {
                self.expression_contains_self_call(left, name)
                    || self.expression_contains_self_call(index, name)
            }
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => {
                self.expression_contains_self_call(object, name)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.expression_contains_self_call(scrutinee, name)
                    || arms.iter().any(|arm| {
                        arm.guard
                            .as_ref()
                            .is_some_and(|g| self.expression_contains_self_call(g, name))
                            || self.expression_contains_self_call(&arm.body, name)
                    })
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.expression_contains_self_call(value, name),
            Expression::Perform { args, .. } => args
                .iter()
                .any(|arg| self.expression_contains_self_call(arg, name)),
            Expression::Handle { expr, arms, .. } => {
                self.expression_contains_self_call(expr, name)
                    || arms
                        .iter()
                        .any(|arm| self.expression_contains_self_call(&arm.body, name))
            }
            _ => false,
        }
    }

    fn infer_let(&mut self, name: Identifier, annotation: Option<&TypeExpr>, value: &Expression) {
        let val_ty = self.infer_expr(value);

        // Propagate the let annotation constraint silently — the compiler's
        // boundary checker in statement.rs is the authoritative reporter for
        // typed-let initializer mismatches.
        let final_ty = match annotation {
            Some(ann) => {
                match TypeEnv::infer_type_from_type_expr(ann, &HashMap::new(), self.interner) {
                    Some(ann_ty) => self.unify_propagate(&val_ty, &ann_ty),
                    None => val_ty.apply_type_subst(&self.subst),
                }
            }
            None => val_ty.apply_type_subst(&self.subst),
        };

        // Generalize the let binding (Hindley-Milner let-polymorphism).
        let env_free = self.env.free_vars();
        let scheme = generalize(&final_ty, &env_free);
        self.env.bind(name, scheme);
    }

    fn infer_module(&mut self, module_name: Identifier, body: &Block) {
        self.env.enter_scope();
        self.predeclare_data_constructors_in_statements(&body.statements);
        for stmt in &body.statements {
            self.infer_stmt(stmt);
            if let Statement::Function {
                is_public: true,
                name,
                ..
            } = stmt
                && let Some(scheme) = self.env.lookup(*name).cloned()
            {
                self.module_member_schemes
                    .insert((module_name, *name), scheme);
            }
        }
        self.env.leave_scope();
    }

    /// Span of the expression that determines a block's value in HM inference.
    /// Falls back to the full block span when the block has no value expression.
    fn block_value_span(&self, block: &Block) -> Span {
        let mut value_span = block.span;
        for stmt in &block.statements {
            match stmt {
                Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                } => {
                    value_span = expression.span();
                }
                Statement::Return {
                    value: Some(expr), ..
                } => {
                    value_span = expr.span();
                }
                _ => {
                    value_span = block.span;
                }
            }
        }
        value_span
    }

    // ── Block inference ───────────────────────────────────────────────────────

    /// Infer the type of a block: the type of the last value-producing
    /// expression (i.e., the last statement without a trailing semicolon).
    /// Returns `Unit` if there is no such expression.
    fn infer_block(&mut self, block: &Block) -> InferType {
        let mut last_ty = InferType::Con(TypeConstructor::Unit);
        for stmt in &block.statements {
            match stmt {
                // The last no-semicolon expression is the block's value.
                Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                } => {
                    last_ty = self.infer_expr(expression);
                }
                // An explicit `return expr` also gives the block's type.
                Statement::Return {
                    value: Some(expr), ..
                } => {
                    last_ty = self.infer_expr(expr);
                }
                _ => {
                    self.infer_stmt(stmt);
                    last_ty = InferType::Con(TypeConstructor::Unit);
                }
            }
        }
        last_ty
    }

    // ── Expression inference ──────────────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expression) -> InferType {
        let node_id = self.node_id_for_expr(expr);
        let inferred = match expr {
            // ── Literals ──────────────────────────────────────────────────────
            Expression::Integer { .. } => InferType::Con(TypeConstructor::Int),
            Expression::Float { .. } => InferType::Con(TypeConstructor::Float),
            Expression::Boolean { .. } => InferType::Con(TypeConstructor::Bool),
            Expression::String { .. } | Expression::InterpolatedString { .. } => {
                InferType::Con(TypeConstructor::String)
            }

            // ── Option / Either constructors ──────────────────────────────────
            Expression::None { .. } => {
                InferType::App(TypeConstructor::Option, vec![self.env.fresh_infer_type()])
            }
            Expression::Some { value, .. } => {
                let inner = self.infer_expr(value);
                InferType::App(TypeConstructor::Option, vec![inner])
            }
            Expression::Left { value, .. } => {
                let inner = self.infer_expr(value);
                let r = self.env.fresh_infer_type();
                InferType::App(TypeConstructor::Either, vec![inner, r])
            }
            Expression::Right { value, .. } => {
                let inner = self.infer_expr(value);
                let l = self.env.fresh_infer_type();
                InferType::App(TypeConstructor::Either, vec![l, inner])
            }

            // ── Identifier lookup ─────────────────────────────────────────────
            Expression::Identifier { name, .. } => {
                if let Some(scheme) = self.env.lookup(*name).cloned() {
                    // Instantiate the scheme with fresh type variables so each
                    // use of a generic function is independent.
                    let (ty, _) = scheme.instantiate(&mut self.env.counter);
                    ty
                } else {
                    // Unknown at this stage (may be a built-in / runtime binding).
                    // Gradual typing: treat as Any without an error.
                    InferType::Con(TypeConstructor::Any)
                }
            }

            // ── Operators ─────────────────────────────────────────────────────
            Expression::Prefix { right, .. } => {
                // Best-effort: return the operand type (covers `-x` and `!x`).
                self.infer_expr(right)
            }
            Expression::Infix {
                left,
                operator,
                right,
                span,
            } => self.infer_infix(left, operator, right, *span),

            // ── Control flow ──────────────────────────────────────────────────
            Expression::If {
                condition,
                consequence,
                alternative,
                span,
            } => {
                let cond_ty = self.infer_expr(condition);
                self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), *span);

                let then_ty = self.infer_block(consequence);
                match alternative {
                    Some(alt) => {
                        let else_ty = self.infer_block(alt);
                        let then_value_span = self.block_value_span(consequence);
                        let else_value_span = self.block_value_span(alt);
                        self.unify_with_context(
                            &then_ty,
                            &else_ty,
                            *span,
                            ReportContext::IfBranch {
                                then_span: then_value_span,
                                else_span: else_value_span,
                            },
                        )
                    }
                    None => then_ty,
                }
            }

            Expression::DoBlock { block, .. } => self.infer_block(block),

            Expression::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                if arms.is_empty() {
                    return InferType::Con(TypeConstructor::Any);
                }
                let propagated_scrutinee_ty = self
                    .match_constraint_family(arms)
                    .and_then(|family| self.family_expected_type(&family))
                    .map(|expected| self.unify_reporting(&scrutinee_ty, &expected, *span))
                    .unwrap_or_else(|| scrutinee_ty.clone());

                // Infer the first arm.
                self.env.enter_scope();
                self.bind_pattern(&arms[0].pattern, &propagated_scrutinee_ty, *span);
                if let Some(guard) = &arms[0].guard {
                    self.infer_expr(guard);
                }
                let first_ty = self.infer_expr(&arms[0].body);
                let first_span = arms[0].body.span();
                self.env.leave_scope();

                // Keep historical arm-join unification behavior so unresolved
                // first-arm variables can still be refined by later concrete arms.
                let mut result_ty = first_ty.clone();
                let mut arm_types: Vec<(InferType, Span, usize)> =
                    vec![(first_ty.clone(), first_span, 1)];
                for (i, arm) in arms.iter().skip(1).enumerate() {
                    self.env.enter_scope();
                    self.bind_pattern(&arm.pattern, &propagated_scrutinee_ty, *span);
                    if let Some(guard) = &arm.guard {
                        self.infer_expr(guard);
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.env.leave_scope();
                    result_ty = self.unify_with_context(
                        &first_ty,
                        &arm_ty,
                        arm.span,
                        ReportContext::MatchArm {
                            first_span,
                            arm_span: arm.body.span(),
                            arm_index: i + 2,
                        },
                    );
                    arm_types.push((arm_ty, arm.body.span(), i + 1));
                }

                // Additional concrete-pivot check only when first arm is not
                // concrete, so ordering does not hide concrete conflicts.
                if !Self::is_concrete_non_any(&first_ty) {
                    let pivot = arm_types
                        .iter()
                        .find(|(ty, _, _)| Self::is_concrete_non_any(ty))
                        .cloned();
                    if let Some((pivot_ty, pivot_span, pivot_index)) = pivot {
                        for (arm_ty, arm_span, arm_index) in &arm_types {
                            if *arm_index == pivot_index {
                                continue;
                            }
                            if Self::is_concrete_non_any(arm_ty) {
                                let _ = self.unify_with_context(
                                    &pivot_ty,
                                    arm_ty,
                                    *arm_span,
                                    ReportContext::MatchArm {
                                        first_span: pivot_span,
                                        arm_span: *arm_span,
                                        arm_index: *arm_index,
                                    },
                                );
                            }
                        }
                    }
                }
                result_ty
            }

            // ── Lambda ────────────────────────────────────────────────────────
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                self.env.enter_scope();

                let mut param_tys: Vec<InferType> = Vec::with_capacity(parameters.len());
                for (i, &param) in parameters.iter().enumerate() {
                    let ty = parameter_types
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .and_then(|te| {
                            TypeEnv::infer_type_from_type_expr(te, &HashMap::new(), self.interner)
                        })
                        .unwrap_or_else(|| self.env.fresh_infer_type());
                    param_tys.push(ty.clone());
                    self.env.bind(param, Scheme::mono(ty));
                }

                let body_ty = self.infer_block(body);
                let ret_ty = match return_type {
                    Some(ret_ann) => {
                        match TypeEnv::infer_type_from_type_expr(
                            ret_ann,
                            &HashMap::new(),
                            self.interner,
                        ) {
                            Some(ann_ty) => self.unify_propagate(&body_ty, &ann_ty),
                            None => body_ty.apply_type_subst(&self.subst),
                        }
                    }
                    None => body_ty.apply_type_subst(&self.subst),
                };

                let final_param_tys: Vec<InferType> = param_tys
                    .iter()
                    .map(|t| t.apply_type_subst(&self.subst))
                    .collect();
                self.env.leave_scope();

                let effect_symbols = effects
                    .iter()
                    .flat_map(EffectExpr::normalized_names)
                    .collect();
                InferType::Fun(final_param_tys, Box::new(ret_ty), effect_symbols)
            }

            // ── Function call ─────────────────────────────────────────────────
            Expression::Call {
                function,
                arguments,
                span,
            } => {
                if let Expression::Identifier { name, .. } = function.as_ref()
                    && self.adt_constructor_types.contains_key(name)
                {
                    self.infer_constructor_call(*name, arguments, *span)
                } else {
                    self.infer_call(function, arguments, *span)
                }
            }

            // ── Collection literals ───────────────────────────────────────────
            Expression::TupleLiteral { elements, .. } => {
                let elem_tys: Vec<InferType> =
                    elements.iter().map(|e| self.infer_expr(e)).collect();
                InferType::Tuple(elem_tys)
            }

            Expression::ListLiteral { elements, span } => {
                if elements.is_empty() {
                    InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()])
                } else {
                    let first = self.infer_expr(&elements[0]);
                    for e in elements.iter().skip(1) {
                        let t = self.infer_expr(e);
                        self.unify_reporting(&first, &t, *span);
                    }
                    InferType::App(
                        TypeConstructor::List,
                        vec![first.apply_type_subst(&self.subst)],
                    )
                }
            }

            Expression::ArrayLiteral { elements, .. } => {
                // Flux arrays (`[| |]`) are heterogeneous at runtime (Array<Any>).
                // Infer each element for substitution side-effects, then return
                // Array<T> when all elements agree on T, or Array<Any> otherwise.
                if elements.is_empty() {
                    InferType::App(TypeConstructor::Array, vec![self.env.fresh_infer_type()])
                } else {
                    let first = self.infer_expr(&elements[0]);
                    let mut homogeneous = true;
                    for e in elements.iter().skip(1) {
                        let t = self.infer_expr(e);
                        let t_r = t.apply_type_subst(&self.subst);
                        let f_r = first.apply_type_subst(&self.subst);
                        if t_r != f_r {
                            homogeneous = false;
                            // Strict-first Any-fallback reduction: emit concrete
                            // mismatches at the literal site when possible so strict
                            // validation doesn't only observe unresolved Array<Any>.
                            self.unify_reporting(&first, &t, e.span());
                        }
                    }
                    let elem_ty = if homogeneous {
                        first.apply_type_subst(&self.subst)
                    } else {
                        InferType::Con(TypeConstructor::Any)
                    };
                    InferType::App(TypeConstructor::Array, vec![elem_ty])
                }
            }

            Expression::EmptyList { .. } => {
                InferType::App(TypeConstructor::List, vec![self.env.fresh_infer_type()])
            }

            Expression::Hash { pairs, .. } => {
                if pairs.is_empty() {
                    let k = self.env.fresh_infer_type();
                    let v = self.env.fresh_infer_type();
                    InferType::App(TypeConstructor::Map, vec![k, v])
                } else {
                    let kt = self.infer_expr(&pairs[0].0);
                    let vt = self.infer_expr(&pairs[0].1);
                    // Infer remaining pairs for side effects (subst updates).
                    for (k, v) in pairs.iter().skip(1) {
                        self.infer_expr(k);
                        self.infer_expr(v);
                    }
                    InferType::App(TypeConstructor::Map, vec![kt, vt])
                }
            }

            Expression::Cons { head, tail, span } => {
                let elem_ty = self.infer_expr(head);
                let list_ty = InferType::App(TypeConstructor::List, vec![elem_ty]);
                let tail_ty = self.infer_expr(tail);
                self.unify_reporting(&list_ty, &tail_ty, *span);
                list_ty.apply_type_subst(&self.subst)
            }

            // ── Member / index access ─────────────────────────────────────────
            Expression::Index { left, index, .. } => {
                let left_ty = self.infer_expr(left);
                let _index_ty = self.infer_expr(index);
                let left_resolved = left_ty.apply_type_subst(&self.subst);
                match left_resolved {
                    InferType::App(TypeConstructor::Array, args)
                    | InferType::App(TypeConstructor::List, args)
                        if args.len() == 1 =>
                    {
                        InferType::App(TypeConstructor::Option, vec![args[0].clone()])
                    }
                    InferType::App(TypeConstructor::Map, args) if args.len() == 2 => {
                        InferType::App(TypeConstructor::Option, vec![args[1].clone()])
                    }
                    InferType::Tuple(elements) => {
                        if let Expression::Integer { value, .. } = index.as_ref()
                            && *value >= 0
                            && let Some(elem) = elements.get(*value as usize)
                        {
                            InferType::App(
                                TypeConstructor::Option,
                                vec![elem.clone().apply_type_subst(&self.subst)],
                            )
                        } else {
                            let joined = elements.iter().skip(1).fold(
                                elements
                                    .first()
                                    .cloned()
                                    .unwrap_or(InferType::Con(TypeConstructor::Any)),
                                |acc, ty| self.join_types(&acc, ty),
                            );
                            InferType::App(TypeConstructor::Option, vec![joined])
                        }
                    }
                    _ => InferType::Con(TypeConstructor::Any),
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                if let Expression::Identifier {
                    name: module_name, ..
                } = object.as_ref()
                    && let Some(scheme) = self
                        .module_member_schemes
                        .get(&(*module_name, *member))
                        .cloned()
                {
                    let (ty, _) = scheme.instantiate(&mut self.env.counter);
                    ty
                } else {
                    self.infer_expr(object);
                    InferType::Con(TypeConstructor::Any)
                }
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                match self.infer_expr(object).apply_type_subst(&self.subst) {
                    InferType::Tuple(elements) => elements
                        .get(*index)
                        .cloned()
                        .unwrap_or(InferType::Con(TypeConstructor::Any)),
                    _ => InferType::Con(TypeConstructor::Any),
                }
            }
            Expression::Perform {
                effect,
                operation,
                args,
                span,
            } => {
                let arg_tys: Vec<InferType> = args.iter().map(|arg| self.infer_expr(arg)).collect();
                if let Some((param_tys, ret_ty)) =
                    self.effect_op_signature_types(*effect, *operation)
                {
                    if arg_tys.len() == param_tys.len() {
                        for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
                            self.unify_reporting(actual, expected, *span);
                        }
                        ret_ty.apply_type_subst(&self.subst)
                    } else {
                        InferType::Con(TypeConstructor::Any)
                    }
                } else {
                    for arg in args {
                        self.infer_expr(arg);
                    }
                    InferType::Con(TypeConstructor::Any)
                }
            }
            Expression::Handle {
                expr,
                effect,
                arms,
                span,
            } => {
                let handled_ty = self.infer_expr(expr);
                let mut arm_result: Option<InferType> = None;
                for arm in arms {
                    self.env.enter_scope();
                    if let Some((param_tys, _ret_ty)) =
                        self.effect_op_signature_types(*effect, arm.operation_name)
                    {
                        for (param_name, param_ty) in arm.params.iter().zip(param_tys.iter()) {
                            self.env.bind(*param_name, Scheme::mono(param_ty.clone()));
                        }
                    }
                    let body_ty = self.infer_expr(&arm.body);
                    self.env.leave_scope();
                    arm_result = Some(match arm_result {
                        Some(prev) => self.join_types(&prev, &body_ty),
                        None => body_ty,
                    });
                }

                let arm_ty = arm_result.unwrap_or(InferType::Con(TypeConstructor::Any));
                let _ = span;
                self.join_types(&handled_ty, &arm_ty)
            }
        };
        let resolved = inferred.apply_type_subst(&self.subst);
        self.expr_types.insert(node_id, resolved.clone());
        resolved
    }

    // ── Infix operator typing ─────────────────────────────────────────────────

    fn infer_infix(
        &mut self,
        left: &Expression,
        op: &str,
        right: &Expression,
        span: Span,
    ) -> InferType {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);
        match op {
            // Arithmetic policy:
            // - `+` supports Int, Float, and String with matching operand types.
            // - `-`, `*`, `/`, `%` support Int/Float with matching operand types.
            // Mixed numeric types are rejected in typed contexts.
            "+" => {
                let resolved = self.unify_reporting(&lt, &rt, span);
                match resolved.apply_type_subst(&self.subst) {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float)
                    | InferType::Con(TypeConstructor::String) => {
                        resolved.apply_type_subst(&self.subst)
                    }
                    InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                        InferType::Con(TypeConstructor::Any)
                    }
                    other => {
                        let expected_numeric = InferType::Con(TypeConstructor::Int);
                        self.unify_reporting(&other, &expected_numeric, span);
                        InferType::Con(TypeConstructor::Any)
                    }
                }
            }
            "-" | "*" | "/" | "%" => {
                let resolved = self.unify_reporting(&lt, &rt, span);
                match resolved.apply_type_subst(&self.subst) {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float) => {
                        resolved.apply_type_subst(&self.subst)
                    }
                    InferType::Con(TypeConstructor::Any) | InferType::Var(_) => {
                        InferType::Con(TypeConstructor::Any)
                    }
                    other => {
                        let expected_numeric = InferType::Con(TypeConstructor::Int);
                        self.unify_reporting(&other, &expected_numeric, span);
                        InferType::Con(TypeConstructor::Any)
                    }
                }
            }
            // Comparisons — operands must agree; result is Bool.
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                self.unify_reporting(&lt, &rt, span);
                InferType::Con(TypeConstructor::Bool)
            }
            // Logical — operands must be Bool.
            "&&" | "||" => {
                let bool_ty = InferType::Con(TypeConstructor::Bool);
                self.unify_reporting(&lt, &bool_ty, span);
                self.unify_reporting(&rt, &bool_ty, span);
                InferType::Con(TypeConstructor::Bool)
            }
            // Concatenation (strings or lists).
            "++" => {
                self.unify_reporting(&lt, &rt, span);
                lt.apply_type_subst(&self.subst)
            }
            // Pipe `|>`: right side is a function applied to left side.
            "|>" => rt,
            _ => InferType::Con(TypeConstructor::Any),
        }
    }

    // ── Call inference ────────────────────────────────────────────────────────

    fn infer_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let fn_ty = self.infer_expr(function);
        let fn_ty_resolved = fn_ty.apply_type_subst(&self.subst);

        let (fn_name, fn_def_span) = match function {
            Expression::Identifier { name, .. } => {
                let fn_name = self.interner.resolve(*name).to_string();
                (Some(fn_name), self.env.lookup_span(*name))
            }
            _ => (None, None),
        };

        if let InferType::Fun(param_tys, ret_ty, _) = fn_ty_resolved.clone() {
            let has_higher_order_params = param_tys
                .iter()
                .map(|t| t.apply_type_subst(&self.subst))
                .any(|t| matches!(t, InferType::Fun(..)));
            if has_higher_order_params {
                // Preserve pre-058 higher-order behavior for now: keep the
                // original whole-function unification path, which is less eager
                // for function-typed argument diagnostics.
                let arg_tys: Vec<InferType> =
                    arguments.iter().map(|a| self.infer_expr(a)).collect();
                let ret_var = self.env.fresh_infer_type();
                let expected_fn_ty = InferType::Fun(arg_tys, Box::new(ret_var.clone()), vec![]);
                self.unify_reporting(&fn_ty, &expected_fn_ty, span);
                return ret_var.apply_type_subst(&self.subst);
            }

            if param_tys.len() != arguments.len() {
                // Keep arity diagnostics in compile pass (E056); do not emit HM arity
                // diagnostics from call inference.
                return ret_ty.apply_type_subst(&self.subst);
            }

            for (index, (arg_expr, expected_param_ty)) in
                arguments.iter().zip(param_tys.iter()).enumerate()
            {
                let arg_ty = self.infer_expr(arg_expr);
                let expected_resolved = expected_param_ty.apply_type_subst(&self.subst);
                let actual_resolved = arg_ty.apply_type_subst(&self.subst);
                let should_emit = expected_resolved.is_concrete()
                    && actual_resolved.is_concrete()
                    && !expected_resolved.contains_any()
                    && !actual_resolved.contains_any();

                match unify_with_span(&expected_resolved, &actual_resolved, arg_expr.span()) {
                    Ok(s) => {
                        self.subst = std::mem::take(&mut self.subst).compose(&s);
                    }
                    Err(_) => {
                        if should_emit {
                            let exp_str = self.display_type(&expected_resolved);
                            let act_str = self.display_type(&actual_resolved);
                            self.errors.push(call_arg_type_mismatch(
                                self.file_path.clone(),
                                arg_expr.span(),
                                fn_name.as_deref(),
                                index + 1,
                                fn_def_span,
                                &exp_str,
                                &act_str,
                            ));
                        }
                    }
                }
            }

            return ret_ty.apply_type_subst(&self.subst);
        }

        // Fallback for dynamic/unknown callees keeps prior behavior.
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expr(a)).collect();
        let ret_var = self.env.fresh_infer_type();
        let expected_fn_ty = InferType::Fun(arg_tys, Box::new(ret_var.clone()), vec![]);
        self.unify_with_context(
            &fn_ty,
            &expected_fn_ty,
            span,
            ReportContext::CallArg {
                fn_name,
                fn_def_span,
            },
        );
        ret_var.apply_type_subst(&self.subst)
    }

    // ── Pattern variable binding ──────────────────────────────────────────────

    /// Bind variables introduced by a pattern, propagating scrutinee type
    /// information when available.
    fn bind_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &InferType, span: Span) {
        let resolved_scrutinee = scrutinee_ty.apply_type_subst(&self.subst);
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.env.bind(*name, Scheme::mono(resolved_scrutinee));
            }
            Pattern::Wildcard { .. } => {
                // Nothing to bind.
            }
            Pattern::Literal { expression, .. } => {
                let literal_ty = self.infer_expr(expression);
                self.unify_reporting(&resolved_scrutinee, &literal_ty, span);
            }
            Pattern::None { .. } => {
                let inner = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Option, vec![inner]);
                self.unify_reporting(&resolved_scrutinee, &expected, span);
            }
            Pattern::Some { pattern, .. } => {
                let inner = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Option, vec![inner.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let inner_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Option, args) if args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => inner.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &inner_ty, span);
            }
            Pattern::Left { pattern, .. } => {
                let left = self.env.fresh_infer_type();
                let right = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Either, vec![left.clone(), right]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let left_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[0].clone()
                    }
                    _ => left.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &left_ty, span);
            }
            Pattern::Right { pattern, .. } => {
                let left = self.env.fresh_infer_type();
                let right = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::Either, vec![left, right.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &expected, span);
                let right_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::Either, args) if args.len() == 2 => {
                        args[1].clone()
                    }
                    _ => right.apply_type_subst(&self.subst),
                };
                self.bind_pattern(pattern, &right_ty, span);
            }
            Pattern::EmptyList { .. } => {
                let elem = self.env.fresh_infer_type();
                let expected = InferType::App(TypeConstructor::List, vec![elem]);
                self.unify_reporting(&resolved_scrutinee, &expected, span);
            }
            Pattern::Cons { head, tail, .. } => {
                let elem = self.env.fresh_infer_type();
                let list_ty = InferType::App(TypeConstructor::List, vec![elem.clone()]);
                let unified = self.unify_reporting(&resolved_scrutinee, &list_ty, span);
                let element_ty = match unified.apply_type_subst(&self.subst) {
                    InferType::App(TypeConstructor::List, args) if args.len() == 1 => {
                        args[0].clone()
                    }
                    _ => elem.apply_type_subst(&self.subst),
                };
                self.bind_pattern(head, &element_ty, span);
                self.bind_pattern(tail, &list_ty, span);
            }
            Pattern::Tuple { elements, .. } => {
                let tuple_shape = InferType::Tuple(
                    elements
                        .iter()
                        .map(|_| self.env.fresh_infer_type())
                        .collect(),
                );
                let unified = self.unify_reporting(&resolved_scrutinee, &tuple_shape, span);
                if let InferType::Tuple(component_types) = unified.apply_type_subst(&self.subst) {
                    for (elem, elem_ty) in elements.iter().zip(component_types.iter()) {
                        self.bind_pattern(elem, elem_ty, span);
                    }
                } else {
                    if Self::is_concrete_non_any(&resolved_scrutinee) {
                        let expected =
                            self.display_type(&tuple_shape.apply_type_subst(&self.subst));
                        let actual = self.display_type(&resolved_scrutinee);
                        self.errors.push(type_unification_error(
                            self.file_path.clone(),
                            span,
                            &expected,
                            &actual,
                        ));
                    }
                    for elem in elements {
                        let fallback = self.env.fresh_infer_type();
                        self.bind_pattern(elem, &fallback, span);
                    }
                }
            }
            Pattern::Constructor { fields, .. } => {
                if let Pattern::Constructor { name, .. } = pattern
                    && let Some((field_tys, result_ty)) = self.instantiate_constructor_parts(*name)
                {
                    self.unify_reporting(&resolved_scrutinee, &result_ty, span);
                    if field_tys.len() == fields.len() {
                        for (field, field_ty) in fields.iter().zip(field_tys.iter()) {
                            self.bind_pattern(field, field_ty, span);
                        }
                    } else {
                        for field in fields {
                            self.bind_pattern(field, &InferType::Con(TypeConstructor::Any), span);
                        }
                    }
                } else {
                    for field in fields {
                        self.bind_pattern(field, &InferType::Con(TypeConstructor::Any), span);
                    }
                }
            }
        }
    }

    fn is_concrete_non_any(ty: &InferType) -> bool {
        ty.is_concrete() && !ty.contains_any()
    }

    fn instantiate_constructor_parts(
        &mut self,
        constructor: Identifier,
    ) -> Option<(Vec<InferType>, InferType)> {
        let info = self.adt_constructor_types.get(&constructor)?.clone();
        let mut type_param_map: HashMap<Identifier, TypeVarId> = HashMap::new();
        for type_param in &info.type_params {
            type_param_map.insert(*type_param, self.env.fresh());
        }

        let field_tys: Vec<InferType> = info
            .fields
            .iter()
            .map(|field| TypeEnv::infer_type_from_type_expr(field, &type_param_map, self.interner))
            .collect::<Option<Vec<_>>>()?;

        let result_ty = if info.type_params.is_empty() {
            InferType::Con(TypeConstructor::Adt(info.adt_name))
        } else {
            let mut args = Vec::with_capacity(info.type_params.len());
            for type_param in &info.type_params {
                let var = type_param_map.get(type_param)?;
                args.push(InferType::Var(*var));
            }
            InferType::App(TypeConstructor::Adt(info.adt_name), args)
        };

        Some((field_tys, result_ty))
    }

    fn infer_constructor_call(
        &mut self,
        constructor: Identifier,
        arguments: &[Expression],
        span: Span,
    ) -> InferType {
        let arg_tys: Vec<InferType> = arguments.iter().map(|a| self.infer_expr(a)).collect();
        let Some((param_tys, result_ty)) = self.instantiate_constructor_parts(constructor) else {
            return InferType::Con(TypeConstructor::Any);
        };
        if arg_tys.len() != param_tys.len() {
            let name_str = self.interner.resolve(constructor).to_string();
            self.errors.push(
                diag_enhanced(&CONSTRUCTOR_ARITY_MISMATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Constructor `{}` expects {} argument(s) but got {}.",
                        name_str,
                        param_tys.len(),
                        arg_tys.len()
                    ))
                    .with_file(self.file_path.clone()),
            );
            return InferType::Con(TypeConstructor::Any);
        }
        for (actual, expected) in arg_tys.iter().zip(param_tys.iter()) {
            self.unify_reporting(actual, expected, span);
        }
        result_ty.apply_type_subst(&self.subst)
    }
}

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
    preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    preloaded_effect_op_signatures: HashMap<(Identifier, Identifier), TypeExpr>,
) -> InferProgramResult {
    let file = file_path.unwrap_or_default();
    let mut ctx = InferCtx::new(
        interner,
        file,
        preloaded_module_member_schemes,
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
