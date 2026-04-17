use std::hash::{Hash, Hasher};

use crate::{
    ast::type_infer::constraint::Constraint,
    diagnostics::{
        Hint, match_arm_type_mismatch,
        quality::{TypeOriginNote, with_type_origin_notes},
    },
    types::unify_error::UnifyError,
};

use super::*;

impl<'a> InferCtx<'a> {
    /// Build source labels and explanatory notes for the type origins involved
    /// in a unification failure under the given reporting context.
    fn type_origin_notes_for_context(
        &self,
        context: &ReportContext,
        span: Span,
    ) -> Vec<TypeOriginNote> {
        match context {
            ReportContext::Plain => vec![
                TypeOriginNote::new(span, "this type mismatch is reported here")
                    .with_note("the conflicting types meet at this expression"),
            ],
            ReportContext::IfBranch {
                then_span,
                else_span,
            } => vec![
                TypeOriginNote::new(*then_span, "this branch established the expected type")
                    .with_note("the `then` branch fixes the type expected from the whole `if`"),
                TypeOriginNote::new(*else_span, "this branch conflicts with that type")
                    .with_note("the `else` branch produces the competing type"),
            ],
            ReportContext::MatchArm {
                first_span,
                arm_span,
                arm_index,
            } => vec![
                TypeOriginNote::new(*first_span, "this arm established the expected type")
                    .with_note("the first arm fixes the type expected from the whole `match`"),
                TypeOriginNote::new(
                    *arm_span,
                    format!("arm {arm_index} conflicts with that type"),
                )
                .with_note("this arm produces the competing type"),
            ],
            ReportContext::CallArg { fn_def_span, .. } => {
                let mut notes = Vec::new();
                if let Some(fn_def_span) = fn_def_span {
                    notes.push(
                        TypeOriginNote::new(
                            *fn_def_span,
                            "this function signature established the expected type",
                        )
                        .with_note("the call argument is checked against this parameter type"),
                    );
                }
                notes.push(
                    TypeOriginNote::new(span, "this call argument produces the conflicting type")
                        .with_note("the actual argument type is inferred from this expression"),
                );
                notes
            }
        }
    }

    /// Unify `t1` with `t2` silently — update the substitution for type
    /// propagation but never emit a diagnostic on failure.
    ///
    /// Used at annotated boundary sites (return type annotations, typed `let`
    /// initializers) where the compiler's boundary checker is the authoritative
    /// error reporter.  HM still needs the substitution side-effect so that
    /// downstream inference sees the annotation constraint.
    pub(super) fn unify_silent(&mut self, t1: &InferType, t2: &InferType) -> InferType {
        // Lazy substitution: pass &self.subst into unification for on-demand
        // variable resolution instead of pre-resolving both types upfront.
        match unify_core(
            t1,
            t2,
            &self.subst,
            Span::default(),
            &mut self.env.counter,
            &self.skolem_vars,
        ) {
            Ok(s) => {
                self.subst = std::mem::take(&mut self.subst).compose(&s);
                t1.apply_type_subst(&self.subst)
            }
            Err(_) => {
                // Compiler boundary check will report — return the annotation
                // type so that downstream inference stays consistent with the
                // programmer's declared intent.
                t2.apply_type_subst(&self.subst)
            }
        }
    }

    /// Try to unify two types and compose the resulting substitution into context state.
    ///
    /// Behavior:
    /// - Passes `&self.subst` as a context substitution for lazy variable
    ///   resolution (R4). Unification resolves variables on-demand via
    ///   `resolve_head` instead of requiring pre-resolved inputs.
    /// - On success, composes solved bindings into `self.subst`.
    ///
    /// Side effects:
    /// - Mutates `self.subst` only on successful unification.
    ///
    /// Returns:
    /// - `Ok(resolved_type)` on success where the return value is `t1` resolved
    ///   under the updated substitution.
    /// - `Err(UnifyError)` when unification fails.
    #[allow(clippy::result_large_err)]
    fn try_unify_and_compose_subst(
        &mut self,
        t1: &InferType,
        t2: &InferType,
        span: Span,
    ) -> Result<InferType, UnifyError> {
        let solved = unify_core(
            t1,
            t2,
            &self.subst,
            span,
            &mut self.env.counter,
            &self.skolem_vars,
        )?;
        self.subst = std::mem::take(&mut self.subst).compose(&solved);
        Ok(t1.apply_type_subst(&self.subst))
    }

    /// Return whether a unification error should be emitted as a diagnostic.
    ///
    /// Checks the concrete/non-fallback guard, then deduplicates by (expected, actual)
    /// hash so the same type-pair mismatch is reported at most once per inference run.
    fn should_emit_unitfication_diagnostic(&mut self, error: &UnifyError) -> bool {
        // RigidBind errors report a skolem escape; the skolem appears as
        // `InferType::Var(_)` on the expected side, which fails the concrete
        // guard. Allow them through explicitly — they are always actionable.
        let is_rigid = matches!(error.kind, UnifyErrorKind::RigidBind(_));
        if !is_rigid && (!error.expected.is_concrete() || !error.actual.is_concrete()) {
            return false;
        }

        let key = {
            let mut hasher = std::hash::DefaultHasher::new();
            self.display_type(&error.expected).hash(&mut hasher);
            self.display_type(&error.actual).hash(&mut hasher);
            hasher.finish()
        };
        self.seen_error_keys.insert(key)
    }

    /// Build function-detail mismatch diagnostics when available.
    ///
    /// Returns:
    /// - `Some(diagnostic)` for function arity/param/return mismatches.
    /// - `None` when no function-specific detail applies.
    fn build_function_detail_diagnostic(
        &self,
        error: &UnifyError,
        span: Span,
    ) -> Option<Diagnostic> {
        let file = self.file_path.clone();
        match &error.detail {
            UnifyErrorDetail::FunArityMismatch { expected, actual } => {
                Some(fun_arity_mismatch(file, span, *expected, *actual))
            }
            UnifyErrorDetail::FunParamMismatch { index } => {
                let exp_param = self.display_type(&error.expected);
                let act_param = self.display_type(&error.actual);
                Some(fun_param_type_mismatch(
                    file,
                    span,
                    *index + 1,
                    &exp_param,
                    &act_param,
                ))
            }
            UnifyErrorDetail::FunReturnMismatch => {
                let exp_ret = self.display_type(&error.expected);
                let act_ret = self.display_type(&error.actual);
                Some(fun_return_type_mismatch(file, span, &exp_ret, &act_ret))
            }
            UnifyErrorDetail::None => None,
        }
    }

    /// Build diagnostics for plain report context.
    fn build_plain_context_diagnostic(&self, error: &UnifyError, span: Span) -> Diagnostic {
        if let Some(diag) = self.build_function_detail_diagnostic(error, span) {
            return diag;
        }
        let file = self.file_path.clone();
        match &error.kind {
            UnifyErrorKind::OccursCheck(v) => {
                let v_str = format!("t{v}");
                let ty_str = self.display_type(&error.actual);
                occurs_check_failure(file, span, &v_str, &ty_str)
            }
            UnifyErrorKind::RigidBind(v) => self.build_rigid_bind_diagnostic(*v, error, span),
            UnifyErrorKind::Mismatch => {
                let exp_str = self.display_type(&error.expected);
                let act_str = self.display_type(&error.actual);
                type_unification_error(file, span, &exp_str, &act_str)
            }
        }
    }

    /// Build diagnostics for if-branch report context.
    fn build_if_branch_context_diagnostic(
        &self,
        then_span: Span,
        else_span: Span,
        error: &UnifyError,
        span: Span,
    ) -> Diagnostic {
        if let Some(diag) = self.build_function_detail_diagnostic(error, span) {
            return diag;
        }
        let file = self.file_path.clone();
        match &error.kind {
            UnifyErrorKind::Mismatch => {
                let then_ty = self.display_type(&error.expected);
                let else_ty = self.display_type(&error.actual);
                if_branch_type_mismatch(file, then_span, else_span, &then_ty, &else_ty)
            }
            UnifyErrorKind::OccursCheck(v) => {
                let v_str = format!("t{v}");
                let ty_str = self.display_type(&error.actual);
                occurs_check_failure(file, span, &v_str, &ty_str)
            }
            UnifyErrorKind::RigidBind(v) => self.build_rigid_bind_diagnostic(*v, error, span),
        }
    }

    /// Build diagnostics for match-arm report context.
    fn build_match_arm_context_diagnostic(
        &self,
        first_span: Span,
        arm_span: Span,
        arm_index: usize,
        error: &UnifyError,
        span: Span,
    ) -> Diagnostic {
        if let Some(diag) = self.build_function_detail_diagnostic(error, span) {
            return diag;
        }
        let file = self.file_path.clone();
        match &error.kind {
            UnifyErrorKind::Mismatch => {
                let first_ty = self.display_type(&error.expected);
                let arm_ty = self.display_type(&error.actual);
                match_arm_type_mismatch(file, first_span, arm_span, &first_ty, &arm_ty, arm_index)
            }
            UnifyErrorKind::OccursCheck(v) => {
                let v_str = format!("t{v}");
                let ty_str = self.display_type(&error.actual);
                occurs_check_failure(file, span, &v_str, &ty_str)
            }
            UnifyErrorKind::RigidBind(v) => self.build_rigid_bind_diagnostic(*v, error, span),
        }
    }

    /// Build diagnostics for call-argument report context.
    fn build_call_arg_context_diagnostic(
        &self,
        fn_name: &Option<String>,
        fn_def_span: Option<Span>,
        error: &UnifyError,
        span: Span,
    ) -> Diagnostic {
        if let Some(diag) = self.build_function_detail_diagnostic(error, span) {
            return diag;
        }
        let file = self.file_path.clone();
        match &error.kind {
            UnifyErrorKind::Mismatch => {
                let exp_str = self.display_type(&error.expected);
                let act_str = self.display_type(&error.actual);
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
            UnifyErrorKind::OccursCheck(v) => {
                let v_str = format!("t{v}");
                let ty_str = self.display_type(&error.actual);
                occurs_check_failure(file, span, &v_str, &ty_str)
            }
            UnifyErrorKind::RigidBind(v) => self.build_rigid_bind_diagnostic(*v, error, span),
        }
    }

    /// Build one diagnostic according to the report context and unification error kind.
    fn build_diagnostic_for_report_context(
        &self,
        context: &ReportContext,
        error: &UnifyError,
        span: Span,
    ) -> Diagnostic {
        let diag = match context {
            ReportContext::Plain => self.build_plain_context_diagnostic(error, span),
            ReportContext::IfBranch {
                then_span,
                else_span,
            } => self.build_if_branch_context_diagnostic(*then_span, *else_span, error, span),
            ReportContext::MatchArm {
                first_span,
                arm_span,
                arm_index,
            } => self.build_match_arm_context_diagnostic(
                *first_span,
                *arm_span,
                *arm_index,
                error,
                span,
            ),
            ReportContext::CallArg {
                fn_name,
                fn_def_span,
            } => self.build_call_arg_context_diagnostic(fn_name, *fn_def_span, error, span),
        };
        with_type_origin_notes(diag, self.type_origin_notes_for_context(context, span))
    }

    /// Build a rigid-variable-escape (E305) diagnostic, using the declared
    /// skolem source name when known so the user sees the type parameter
    /// they wrote rather than a synthetic slot identifier.
    fn build_rigid_bind_diagnostic(
        &self,
        v: TypeVarId,
        error: &UnifyError,
        span: Span,
    ) -> Diagnostic {
        let name = self
            .skolem_names
            .get(&v)
            .map(|id| self.interner.resolve(*id).to_string())
            .unwrap_or_else(|| format!("t{v}"));
        let bound = self.display_type(&error.actual);
        crate::diagnostics::diagnostic_for(&crate::diagnostics::compiler_errors::RIGID_VAR_ESCAPE)
            .with_file(self.file_path.clone())
            .with_span(span)
            .with_message(format!(
                "Rigid type variable `{name}` cannot be unified with `{bound}`."
            ))
            .with_primary_label(span, format!("forces `{name}` to become `{bound}`"))
    }

    /// Append type-name typo suggestion hints onto an existing diagnostic.
    ///
    /// Side effects:
    /// - Mutates `diagnostic.hints` when ADT names are close to known type names.
    fn append_type_name_suggestions(&self, diagnostic: &mut Diagnostic, error: &UnifyError) {
        for ty in [&error.expected, &error.actual] {
            if let InferType::Con(TypeConstructor::Adt(sym)) = ty {
                let name = self.interner.resolve(*sym);
                if let Some(suggestion) = suggest_type_name(name) {
                    diagnostic
                        .hints
                        .push(Hint::help(format!("Unknown type `{name}` — {suggestion}")));
                }
            }
        }
    }

    /// Unify `t1` with `t2`, composing the result into `self.subst` with an
    /// explicit reporting context.
    ///
    /// Behavior:
    /// - Performs row-aware unification.
    /// - Emits contextual diagnostics only when both sides are concrete and non-fallback.
    ///
    /// Side effects:
    /// - Mutates `self.subst` on success.
    /// - Pushes diagnostics into `self.errors` on eligible failures.
    ///
    /// Invariants:
    /// - Maintains diagnostic context mapping used by snapshot tests.
    pub(super) fn unify_with_context(
        &mut self,
        t1: &InferType,
        t2: &InferType,
        span: Span,
        context: ReportContext,
    ) -> InferType {
        self.record_constraint(Constraint::Unify {
            t1: t1.clone(),
            t2: t2.clone(),
            span,
            context: context.clone(),
        });
        match self.try_unify_and_compose_subst(t1, t2, span) {
            Ok(infer_type) => infer_type,
            Err(error) => {
                if self.should_emit_unitfication_diagnostic(&error) {
                    let mut diagnostic =
                        self.build_diagnostic_for_report_context(&context, &error, span);
                    self.append_type_name_suggestions(&mut diagnostic, &error);
                    self.errors.push(diagnostic);
                }
                // R13: recover with the expected type (t1) when it is already
                // concrete so downstream inference keeps useful information
                // instead of collapsing through a legacy dynamic sink.
                let t1_resolved = t1.apply_type_subst(&self.subst);
                if t1_resolved.is_concrete() {
                    t1_resolved
                } else {
                    self.alloc_fallback_var()
                }
            }
        }
    }

    /// Unify `t1` with `t2`, composing the result into `self.subst`.
    ///
    /// On success, returns the resolved first type.
    /// On failure, emits a diagnostic and returns a fresh inference variable so
    /// inference can continue without reintroducing gradual fallback semantics.
    pub(super) fn unify_reporting(
        &mut self,
        t1: &InferType,
        t2: &InferType,
        span: Span,
    ) -> InferType {
        self.unify_with_context(t1, t2, span, ReportContext::Plain)
    }
}
