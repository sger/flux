use super::*;

impl<'a> InferCtx<'a> {
    /// Join two types for branch contexts (if/else, match arms).
    ///
    /// Unlike `unify_reporting`, this does NOT add substitution constraints —
    /// it only compares the already-resolved types.  When the resolved types
    /// agree exactly, the common type is returned.  When they differ, `Any` is
    /// returned without modifying the substitution.
    ///
    /// This models Flux's gradual type system where different branches may
    /// legitimately produce values of different types (union falls back to Any).
    pub(super) fn join_types(&mut self, t1: &InferType, t2: &InferType) -> InferType {
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
    pub(super) fn unify_propagate(&mut self, t1: &InferType, t2: &InferType) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        match unify_with_span_and_row_var_counter(
            &t1_sub,
            &t2_sub,
            Span::default(),
            &mut self.env.counter,
        ) {
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
    pub(super) fn unify_with_context(
        &mut self,
        t1: &InferType,
        t2: &InferType,
        span: Span,
        context: ReportContext,
    ) -> InferType {
        let t1_sub = t1.apply_type_subst(&self.subst);
        let t2_sub = t2.apply_type_subst(&self.subst);
        match unify_with_span_and_row_var_counter(&t1_sub, &t2_sub, span, &mut self.env.counter) {
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
    pub(super) fn unify_reporting(&mut self, t1: &InferType, t2: &InferType, span: Span) -> InferType {
        self.unify_with_context(t1, t2, span, ReportContext::Plain)
    }
}
