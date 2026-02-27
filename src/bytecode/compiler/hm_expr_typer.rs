use crate::{
    bytecode::compiler::Compiler,
    diagnostics::{
        Diagnostic, DiagnosticBuilder, ErrorType, compiler_errors::type_unification_error,
    },
    syntax::expression::Expression,
    types::{
        infer_type::InferType, type_constructor::TypeConstructor, unify_error::unify,
    },
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
        let key = expression as *const Expression as usize;
        let Some(node_id) = self.expr_ptr_to_id.get(&key) else {
            debug_assert!(
                false,
                "HM expression id lookup missed in strict path. Invariant: HM pass and typed validation must run against the same Program allocation within one compile invocation."
            );
            return HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType);
        };
        match self.expr_type(*node_id) {
            Some(infer) if Self::is_hm_type_resolved(&infer) => HmExprTypeResult::Known(infer),
            Some(_) => HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType),
            None => {
                debug_assert!(
                    false,
                    "HM expression type map missed known expression id in strict path. Invariant: infer_program must populate ExprTypeMap for every expression visited in codegen validation."
                );
                HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType)
            }
        }
    }

    pub(super) fn expr_type(
        &self,
        node_id: crate::ast::type_infer::ExprNodeId,
    ) -> Option<InferType> {
        self.hm_expr_types.get(&node_id).cloned()
    }

    fn contains_any(infer: &InferType) -> bool {
        match infer {
            InferType::Con(TypeConstructor::Any) => true,
            InferType::App(_, args) | InferType::Tuple(args) => args.iter().any(Self::contains_any),
            InferType::Fun(params, ret, _) => {
                params.iter().any(Self::contains_any) || Self::contains_any(ret)
            }
            InferType::Var(_) => false,
            InferType::Con(_) => false,
        }
    }

    fn is_hm_type_resolved(infer: &InferType) -> bool {
        infer.free_vars().is_empty() && !Self::contains_any(infer)
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

    fn ensure_unify(&self, expected: &InferType, actual: &InferType) -> Result<(), (String, String)> {
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
                diag.hints.push(crate::diagnostics::types::Hint::help(
                    format!("Unknown type `{name}` — {suggestion}"),
                ));
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
        self.check_private_module_member_access_for_expr(expression)?;
        Err(Box::new(
            self.unresolved_boundary_error(expression, unresolved_context),
        ))
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
            None
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
