use crate::{
    bytecode::compiler::Compiler,
    diagnostics::{
        Diagnostic, DiagnosticBuilder, ErrorType, compiler_errors::type_unification_error,
        position::Span, types::LabelStyle,
    },
    syntax::expression::Expression,
    types::{infer_type::InferType, unify::unify},
};

use crate::ast::type_infer::{display_infer_type, suggest_type_name};

use super::CompileResult;

#[derive(Debug, Clone)]
pub enum UnresolvedReason {
    UnknownExpressionType,
}

#[derive(Debug, Clone)]
pub enum HmExprTypeResult {
    Known(InferType),
    Unresolved(UnresolvedReason),
}

impl Compiler {
    /// Authoritative HM expression typing for typed validation paths.
    /// This path only consults the global HM expression-type map.
    pub(super) fn hm_expr_type_strict_path(&self, expression: &Expression) -> HmExprTypeResult {
        let expr_id = expression.expr_id();
        match self.hm_expr_types.get(&expr_id) {
            Some(infer) if Self::is_hm_type_resolved(infer) => {
                HmExprTypeResult::Known(infer.clone())
            }
            Some(_) => HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType),
            None => HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType),
        }
    }

    fn is_hm_type_resolved(infer: &InferType) -> bool {
        infer.free_vars().is_empty()
    }

    fn format_infer_type(&self, infer: &InferType) -> String {
        display_infer_type(infer, &self.interner)
    }

    fn format_unify_pair(&self, expected: &InferType, actual: &InferType) -> (String, String) {
        let expected_str = self.format_infer_type(expected);
        let actual_str = self.format_infer_type(actual);
        (expected_str, actual_str)
    }

    fn resolved_unify_types(
        expected: &InferType,
        actual: &InferType,
    ) -> Option<(InferType, InferType)> {
        let subst = unify(expected, actual).ok()?;
        Some((
            expected.apply_type_subst(&subst),
            actual.apply_type_subst(&subst),
        ))
    }

    fn ensure_unify(
        &self,
        expected: &InferType,
        actual: &InferType,
    ) -> Result<(), (String, String)> {
        if let Some((resolved_expected, resolved_actual)) =
            Self::resolved_unify_types(expected, actual)
            && resolved_expected == resolved_actual
        {
            return Ok(());
        }
        let (expected_str, actual_str) = self.format_unify_pair(expected, actual);
        Err((expected_str, actual_str))
    }

    fn build_type_mismatch(
        &self,
        expression: &Expression,
        primary_label: &str,
        help: String,
        expected: &str,
        actual: &str,
    ) -> Diagnostic {
        let mut diag =
            type_unification_error(self.file_path.clone(), expression.span(), expected, actual)
                .with_secondary_label(expression.span(), primary_label)
                .with_help(help);
        // Add "did you mean?" hint for likely type name typos
        for name in [expected, actual] {
            if let Some(suggestion) = suggest_type_name(name) {
                diag.hints
                    .push(crate::diagnostics::types::Hint::help(format!(
                        "Unknown type `{name}` — {suggestion}"
                    )));
            }
        }
        diag
    }

    pub(super) fn unresolved_boundary_error(
        &self,
        expression: &Expression,
        unresolved_context: &str,
    ) -> Diagnostic {
        Diagnostic::make_error_dynamic(
            "E425",
            "STRICT UNRESOLVED BOUNDARY TYPE",
            ErrorType::Compiler,
            format!(
                "Strict mode cannot enforce runtime boundary check for unresolved expression type in {}.",
                unresolved_context
            ),
            Some(
                "Use concrete types (or additional annotations) so HM can resolve this expression."
                    .to_string(),
            ),
            self.file_path.clone(),
            expression.span(),
        )
        .with_display_title("Unresolved Boundary Type")
        .with_category(crate::diagnostics::DiagnosticCategory::TypeInference)
        .with_primary_label(
            expression.span(),
            "expression type is unresolved in strict mode",
        )
    }

    fn maybe_unresolved(
        &self,
        expression: &Expression,
        unresolved_context: &str,
        strict_unresolved_error: bool,
    ) -> CompileResult<()> {
        if !strict_unresolved_error || !self.strict_mode {
            return Ok(());
        }
        if self.is_concrete_match_arm_conflict(expression) {
            return Ok(());
        }
        if self.is_concrete_array_literal_conflict(expression) {
            return Ok(());
        }
        if self.type_error_already_reported_for(expression) {
            return Ok(());
        }
        self.check_private_module_member_access_for_expr(expression)?;
        Err(Box::new(
            self.unresolved_boundary_error(expression, unresolved_context),
        ))
    }

    fn is_concrete_array_literal_conflict(&self, expression: &Expression) -> bool {
        let Expression::ArrayLiteral { elements, .. } = expression else {
            return false;
        };
        if elements.len() < 2 {
            return false;
        }
        let mut first_known: Option<InferType> = None;
        for element in elements {
            let HmExprTypeResult::Known(actual) = self.hm_expr_type_strict_path(element) else {
                return false;
            };
            if !actual.free_vars().is_empty() {
                return false;
            }
            if let Some(expected) = &first_known {
                if self.ensure_unify(expected, &actual).is_err() {
                    return true;
                }
            } else {
                first_known = Some(actual);
            }
        }
        false
    }

    fn is_concrete_match_arm_conflict(&self, expression: &Expression) -> bool {
        let Expression::Match { arms, .. } = expression else {
            return false;
        };
        if arms.len() < 2 {
            return false;
        }

        let concrete_arms: Vec<InferType> = arms
            .iter()
            .filter_map(|arm| match self.hm_expr_type_strict_path(&arm.body) {
                HmExprTypeResult::Known(ty) if ty.free_vars().is_empty() => Some(ty),
                _ => None,
            })
            .collect();
        if concrete_arms.len() < 2 {
            return false;
        }

        let pivot = &concrete_arms[0];
        for actual in concrete_arms.iter().skip(1) {
            if self.ensure_unify(pivot, actual).is_err() {
                return true;
            }
        }
        false
    }

    pub(super) fn type_error_already_reported_for_span(&self, expr_span: Span) -> bool {
        self.errors.iter().any(|diag| {
            if diag.code() != Some("E300") {
                return false;
            }
            let diag_file = diag.file().unwrap_or(self.file_path.as_str());
            if diag_file != self.file_path {
                return false;
            }
            if let Some(diag_span) = diag.span()
                && Self::spans_overlap(diag_span, expr_span)
            {
                return true;
            }
            if let Some(diag_span) = diag.span()
                && diag_span.start.line == expr_span.start.line
            {
                return true;
            }
            diag.labels().iter().any(|label| {
                if label.style != LabelStyle::Primary {
                    return false;
                }
                Self::spans_overlap(label.span, expr_span)
                    || label.span.start.line == expr_span.start.line
            })
        })
    }

    pub(super) fn type_error_already_reported_for(&self, expression: &Expression) -> bool {
        self.type_error_already_reported_for_span(expression.span())
    }

    fn check_private_module_member_access_for_expr(
        &self,
        expression: &Expression,
    ) -> CompileResult<()> {
        let (object, member, span) = match expression {
            Expression::MemberAccess { object, member, .. } => {
                (object.as_ref(), *member, expression.span())
            }
            Expression::Call { function, .. } => match function.as_ref() {
                Expression::MemberAccess { object, member, .. } => {
                    (object.as_ref(), *member, function.span())
                }
                _ => return Ok(()),
            },
            _ => return Ok(()),
        };

        let Expression::Identifier { name, .. } = object else {
            return Ok(());
        };

        let module_name = if let Some(target) = self.import_aliases.get(name) {
            Some(*target)
        } else if self.imported_modules.contains(name) || self.current_module_prefix == Some(*name)
        {
            Some(*name)
        } else {
            let short = self.sym(*name);
            self.imported_modules
                .iter()
                .copied()
                .find(|module| self.sym(*module).rsplit('.').next() == Some(short))
                .or_else(|| {
                    self.current_module_prefix
                        .filter(|module| self.sym(*module).rsplit('.').next() == Some(short))
                })
        };
        let Some(module_name) = module_name else {
            return Ok(());
        };

        let member_str = self.sym(member);
        self.check_private_member(member_str, span, Some(self.sym(module_name)))?;
        if self.current_module_prefix != Some(module_name)
            && self.module_member_function_is_public(module_name, member) == Some(false)
        {
            return Err(Box::new(Diagnostic::make_error(
                &crate::diagnostics::PRIVATE_MEMBER,
                &[member_str],
                self.file_path.clone(),
                span,
            )));
        }

        Ok(())
    }

    pub(super) fn validate_expr_expected_type(
        &self,
        expected: &InferType,
        expression: &Expression,
        primary_label: &str,
        help: String,
        unresolved_context: &str,
    ) -> CompileResult<()> {
        self.validate_expr_expected_type_with_policy(
            expected,
            expression,
            primary_label,
            help,
            unresolved_context,
            false,
        )
    }

    pub(super) fn validate_expr_expected_type_with_policy(
        &self,
        expected: &InferType,
        expression: &Expression,
        primary_label: &str,
        help: String,
        unresolved_context: &str,
        strict_unresolved_error: bool,
    ) -> CompileResult<()> {
        match self.hm_expr_type_strict_path(expression) {
            HmExprTypeResult::Known(actual) => {
                if self.ensure_unify(expected, &actual).is_ok() {
                    return Ok(());
                }

                let (expected_str, actual_str) = self.format_unify_pair(expected, &actual);
                Err(Box::new(self.build_type_mismatch(
                    expression,
                    primary_label,
                    help,
                    &expected_str,
                    &actual_str,
                )))
            }
            HmExprTypeResult::Unresolved(_) => {
                self.maybe_unresolved(expression, unresolved_context, strict_unresolved_error)
            }
        }
    }
}
