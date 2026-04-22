use super::*;
use crate::ast::type_infer::expression::patterns::PatternFamily;
use crate::ast::type_infer::pattern_coverage::check_match;
use crate::ast::type_infer::pattern_coverage_adapter::{
    AdtResolver, pat_of, ty_shape_of,
};
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory,
    compiler_errors::{EMPTY_MATCH, NON_EXHAUSTIVE_MATCH},
    diagnostic_for,
};
use crate::types::type_constructor::TypeConstructor;

/// Snapshot of the `InferCtx` ADT tables for coverage queries.
struct InferAdtResolver<'a> {
    adt_type_params:
        &'a std::collections::HashMap<crate::syntax::Identifier, Vec<crate::syntax::Identifier>>,
    adt_constructor_types:
        &'a std::collections::HashMap<crate::syntax::Identifier, super::super::AdtConstructorTypeInfo>,
    interner: &'a crate::syntax::interner::Interner,
}

impl<'a> AdtResolver for InferAdtResolver<'a> {
    /// Resolve an ADT symbol to its sorted list of `(constructor_name,
    /// field_type_exprs)`, or `None` if the symbol is not a
    /// registered ADT.
    fn lookup_adt(
        &self,
        adt: crate::syntax::Identifier,
    ) -> Option<Vec<(String, Vec<crate::syntax::type_expr::TypeExpr>)>> {
        if !self.adt_type_params.contains_key(&adt) {
            return None;
        }
        let mut ctors: Vec<(String, Vec<crate::syntax::type_expr::TypeExpr>)> = self
            .adt_constructor_types
            .iter()
            .filter(|(_, info)| info.adt_name == adt)
            .map(|(name, info)| {
                (self.interner.resolve(*name).to_string(), info.fields.clone())
            })
            .collect();
        ctors.sort_by(|a, b| a.0.cmp(&b.0));
        Some(ctors)
    }
}

impl<'a> InferCtx<'a> {
    /// Infer `if` expressions, constraining condition to `Bool` and joining branch results.
    pub(super) fn infer_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
        span: Span,
    ) -> InferType {
        let cond_ty = self.infer_expression(condition);
        self.unify_reporting(&cond_ty, &InferType::Con(TypeConstructor::Bool), span);

        let then_ty = self.infer_block(consequence);
        match alternative {
            Some(alt) => {
                let else_ty = self.infer_block(alt);
                let then_value_span = self.block_value_span(consequence);
                let else_value_span = self.block_value_span(alt);
                self.unify_with_context(
                    &then_ty,
                    &else_ty,
                    span,
                    ReportContext::IfBranch {
                        then_span: then_value_span,
                        else_span: else_value_span,
                    },
                )
            }
            None => then_ty,
        }
    }

    /// Infer `match` expressions while preserving existing arm-order diagnostics.
    pub(super) fn infer_match_expression(&mut self, input: MatchInferInput<'_>) -> InferType {
        let scrutinee_ty = self.infer_expression(input.scrutinee);
        if input.arms.is_empty() {
            self.errors.push(
                diagnostic_for(&EMPTY_MATCH)
                    .with_file(self.file_path.clone())
                    .with_span(input.span)
                    .with_category(DiagnosticCategory::TypeInference),
            );
            return self.alloc_fallback_var();
        }
        let shared_family = self.shared_pattern_family(input.arms);
        let has_nonconstraining_arm = self.match_has_nonconstraining_arm(input.arms);
        let isolate_arm_scrutinees = self.should_isolate_match_arm_scrutinees(
            input.arms,
            &scrutinee_ty,
            shared_family.as_ref(),
        );
        let propagated_scrutinee = self.propagate_match_scrutinee_constraint(
            &scrutinee_ty,
            &shared_family,
            has_nonconstraining_arm,
            &input,
        );
        let (first_ty, first_span) = self.infer_first_match_arm(
            input.arms,
            &propagated_scrutinee,
            isolate_arm_scrutinees,
            input.span,
        );
        let (result_ty, arm_types) = self.infer_remaining_match_arms(
            input.arms,
            &propagated_scrutinee,
            isolate_arm_scrutinees,
            input.span,
            &first_ty,
            first_span,
        );
        self.infer_match_concrete_pivot_conflicts(&first_ty, &arm_types);
        self.experimental_coverage_check(&scrutinee_ty, input.arms, input.span);
        result_ty
    }

    /// Matrix-based coverage check (Proposal 0166).
    ///
    /// Emits warning-severity diagnostics (Non-Exhaustive Match,
    /// Redundant Match Arm) alongside the existing ad-hoc error
    /// checks. Opt-out via `FLUX_COVERAGE_WARN=0`.
    fn experimental_coverage_check(
        &mut self,
        scrutinee_ty: &InferType,
        arms: &[MatchArm],
        span: Span,
    ) {
        if self.coverage_check_suppressed(scrutinee_ty) {
            return;
        }
        let resolver = InferAdtResolver {
            adt_type_params: &self.adt_type_params,
            adt_constructor_types: &self.adt_constructor_types,
            interner: self.interner,
        };
        let resolved = scrutinee_ty.apply_type_subst(&self.subst);
        let ty = ty_shape_of(&resolved, &resolver, self.interner);
        let arm_entries: Vec<_> = arms
            .iter()
            .map(|arm| (pat_of(&arm.pattern, self.interner), arm.guard.is_some()))
            .collect();
        let cov = check_match(&ty, &arm_entries);
        if !cov.is_exhaustive() {
            self.emit_coverage_non_exhaustive(cov.missing.len(), span);
        }
        for idx in cov.redundant {
            if let Some(arm) = arms.get(idx) {
                self.emit_coverage_redundant(arm.span);
            }
        }
    }

    /// Returns true when the coverage check should skip this match.
    /// Suppression rules:
    /// - explicit opt-out via `FLUX_COVERAGE_WARN=0`
    /// - scrutinee type did not resolve (inference failure upstream)
    fn coverage_check_suppressed(&self, scrutinee_ty: &InferType) -> bool {
        if std::env::var("FLUX_COVERAGE_WARN").ok().as_deref() == Some("0") {
            return true;
        }
        matches!(scrutinee_ty.apply_type_subst(&self.subst), InferType::Var(_))
    }

    /// Push an `E015: Non-Exhaustive Match` error at `span`.
    fn emit_coverage_non_exhaustive(&mut self, missing_count: usize, span: Span) {
        let msg = if missing_count == 1 {
            "Match is not exhaustive: 1 missing pattern.".to_string()
        } else {
            format!("Match is not exhaustive: {missing_count} missing patterns.")
        };
        self.errors.push(
            diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                .with_file(self.file_path.clone())
                .with_span(span)
                .with_message(msg)
                .with_hint_text(
                    "Add the missing pattern(s) or an unguarded `_ -> ...` catch-all."
                        .to_string(),
                )
                .with_category(DiagnosticCategory::TypeInference),
        );
    }

    /// Push a `Redundant Match Arm` warning at `span`.
    fn emit_coverage_redundant(&mut self, span: Span) {
        self.errors.push(
            Diagnostic::warning("Redundant Match Arm")
                .with_file(self.file_path.clone())
                .with_span(span)
                .with_message(
                    "this arm is unreachable — earlier arms already cover it"
                        .to_string(),
                )
                .with_category(DiagnosticCategory::TypeInference),
        );
    }

    /// Infer optional family constraint for a match scrutinee.
    fn propagate_match_scrutinee_constraint(
        &mut self,
        scrutinee_ty: &InferType,
        shared_family: &Option<PatternFamily>,
        has_nonconstraining_arm: bool,
        input: &MatchInferInput<'_>,
    ) -> InferType {
        if has_nonconstraining_arm {
            return scrutinee_ty.clone();
        }
        shared_family
            .as_ref()
            .and_then(|family| self.expected_type_for_pattern_family(family))
            .map(|expected| self.unify_reporting(scrutinee_ty, &expected, input.span))
            .unwrap_or_else(|| scrutinee_ty.clone())
    }

    /// Infer the first arm of a match expression under pattern bindings.
    fn infer_first_match_arm(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &InferType,
        isolate_arm_scrutinees: bool,
        span: Span,
    ) -> (InferType, Span) {
        self.env.enter_scope();
        let arm_scrutinee = self.arm_pattern_scrutinee_ty(scrutinee_ty, isolate_arm_scrutinees);
        self.bind_pattern_variables(&arms[0].pattern, &arm_scrutinee, span);
        if let Some(guard) = &arms[0].guard {
            self.infer_expression(guard);
        }
        let first_ty = self.infer_expression(&arms[0].body);
        let first_span = arms[0].body.span();
        self.env.leave_scope();
        (first_ty, first_span)
    }

    /// Infer all remaining match arms and collect arm type metadata.
    fn infer_remaining_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &InferType,
        isolate_arm_scrutinees: bool,
        span: Span,
        first_ty: &InferType,
        first_span: Span,
    ) -> (InferType, Vec<(InferType, Span, usize)>) {
        let mut result_ty = first_ty.clone();
        let mut arm_types: Vec<(InferType, Span, usize)> = vec![(first_ty.clone(), first_span, 1)];
        for (i, arm) in arms.iter().skip(1).enumerate() {
            self.env.enter_scope();
            let arm_scrutinee = self.arm_pattern_scrutinee_ty(scrutinee_ty, isolate_arm_scrutinees);
            self.bind_pattern_variables(&arm.pattern, &arm_scrutinee, span);
            if let Some(guard) = &arm.guard {
                self.infer_expression(guard);
            }
            let arm_ty = self.infer_expression(&arm.body);
            self.env.leave_scope();
            result_ty = self.unify_with_context(
                first_ty,
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
        (result_ty, arm_types)
    }

    /// Choose the scrutinee type that pattern binding should see for one arm.
    ///
    /// When match arms mix incompatible built-in constructor families, each
    /// arm receives a fresh fallback scrutinee so constructors like `Some` and
    /// `Left` do not accidentally constrain one another through the original
    /// scrutinee slot. Mixed concrete ADT families keep the shared scrutinee so
    /// incompatible user-defined constructors still report a mismatch.
    fn arm_pattern_scrutinee_ty(
        &mut self,
        scrutinee_ty: &InferType,
        isolate_arm_scrutinees: bool,
    ) -> InferType {
        if isolate_arm_scrutinees {
            self.alloc_fallback_var()
        } else {
            scrutinee_ty.clone()
        }
    }

    /// Decide whether mixed match arms should bind against isolated scrutinee variables.
    fn should_isolate_match_arm_scrutinees(
        &self,
        arms: &[MatchArm],
        scrutinee_ty: &InferType,
        shared_family: Option<&PatternFamily>,
    ) -> bool {
        let has_nonconstraining_arm = self.match_has_nonconstraining_arm(arms);
        if has_nonconstraining_arm {
            if let Some(family) = shared_family
                && self.concrete_scrutinee_matches_family(scrutinee_ty, family)
            {
                return false;
            }
            return arms.iter().any(|arm| {
                !matches!(
                    self.pattern_family(&arm.pattern),
                    PatternFamily::NonConstraining
                )
            });
        }
        if shared_family.is_some() {
            return false;
        }
        let constraining_families: Vec<PatternFamily> = arms
            .iter()
            .map(|arm| self.pattern_family(&arm.pattern))
            .filter(|family| !matches!(family, PatternFamily::NonConstraining))
            .collect();
        !constraining_families.is_empty()
            && !constraining_families
                .iter()
                .all(|family| matches!(family, PatternFamily::Adt(_)))
    }

    /// Return true when a fully resolved scrutinee already matches the shared pattern family.
    fn concrete_scrutinee_matches_family(
        &self,
        scrutinee_ty: &InferType,
        family: &PatternFamily,
    ) -> bool {
        if !scrutinee_ty.free_vars().is_empty() {
            return false;
        }
        match (scrutinee_ty, family) {
            (InferType::App(TypeConstructor::Option, args), PatternFamily::Option) => {
                args.len() == 1
            }
            (InferType::App(TypeConstructor::Either, args), PatternFamily::Either) => {
                args.len() == 2
            }
            (InferType::App(TypeConstructor::List, args), PatternFamily::List) => args.len() == 1,
            (InferType::Tuple(elements), PatternFamily::Tuple(arity)) => elements.len() == *arity,
            (InferType::Con(TypeConstructor::Adt(name)), PatternFamily::Adt(expected)) => {
                name == expected
            }
            (InferType::App(TypeConstructor::Adt(name), _), PatternFamily::Adt(expected)) => {
                name == expected
            }
            _ => false,
        }
    }

    /// Return whether a match includes any arm that does not constrain the scrutinee family.
    fn match_has_nonconstraining_arm(&self, arms: &[MatchArm]) -> bool {
        arms.iter().any(|arm| {
            matches!(
                self.pattern_family(&arm.pattern),
                PatternFamily::NonConstraining
            )
        })
    }

    /// Emit additional conflicts between concrete match arms when first arm is unresolved.
    fn infer_match_concrete_pivot_conflicts(
        &mut self,
        first_ty: &InferType,
        arm_types: &[(InferType, Span, usize)],
    ) {
        if Self::is_fully_concrete(first_ty) {
            return;
        }
        let pivot = arm_types
            .iter()
            .find(|(ty, _, _)| Self::is_fully_concrete(ty))
            .cloned();
        let Some((pivot_ty, pivot_span, pivot_index)) = pivot else {
            return;
        };
        for (arm_ty, arm_span, arm_index) in arm_types {
            if *arm_index == pivot_index {
                continue;
            }
            if Self::is_fully_concrete(arm_ty) {
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
