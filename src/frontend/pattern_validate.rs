use std::collections::HashSet;

use crate::frontend::{
    block::Block,
    diagnostics::{
        CATCHALL_NOT_LAST, DUPLICATE_PATTERN_BINDING, Diagnostic, EMPTY_MATCH, NON_EXHAUSTIVE_MATCH,
    },
    expression::{Expression, MatchArm, Pattern, StringPart},
    program::Program,
    statement::Statement,
};

#[derive(Debug, Clone, Copy)]
pub struct PatternValidationContext<'a> {
    pub file_path: &'a str,
}

impl<'a> PatternValidationContext<'a> {
    pub fn new(file_path: &'a str) -> Self {
        Self { file_path }
    }
}

pub fn validate_program_patterns(program: &Program, file_path: &str) -> Vec<Diagnostic> {
    let ctx = PatternValidationContext::new(file_path);
    let mut diagnostics = Vec::new();
    for statement in &program.statements {
        validate_statement_patterns(statement, &ctx, &mut diagnostics);
    }
    diagnostics
}

pub fn validate_pattern(
    pattern: &Pattern,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut bindings = HashSet::new();
    validate_pattern_bindings(pattern, ctx, diagnostics, &mut bindings);
}

fn validate_statement_patterns(
    statement: &Statement,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        Statement::Let { value, .. } | Statement::Assign { value, .. } => {
            validate_expression_patterns(value, ctx, diagnostics);
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                validate_expression_patterns(value, ctx, diagnostics);
            }
        }
        Statement::Expression { expression, .. } => {
            validate_expression_patterns(expression, ctx, diagnostics);
        }
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            validate_block_patterns(body, ctx, diagnostics);
        }
        Statement::Import { .. } => {}
    }
}

fn validate_block_patterns(
    block: &Block,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        validate_statement_patterns(statement, ctx, diagnostics);
    }
}

fn validate_expression_patterns(
    expression: &Expression,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expression {
        Expression::Prefix { right, .. } => {
            validate_expression_patterns(right, ctx, diagnostics);
        }
        Expression::Infix { left, right, .. } => {
            validate_expression_patterns(left, ctx, diagnostics);
            validate_expression_patterns(right, ctx, diagnostics);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            validate_expression_patterns(condition, ctx, diagnostics);
            validate_block_patterns(consequence, ctx, diagnostics);
            if let Some(alt) = alternative {
                validate_block_patterns(alt, ctx, diagnostics);
            }
        }
        Expression::Function { body, .. } => {
            validate_block_patterns(body, ctx, diagnostics);
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            validate_expression_patterns(function, ctx, diagnostics);
            for argument in arguments {
                validate_expression_patterns(argument, ctx, diagnostics);
            }
        }
        Expression::Array { elements, .. } => {
            for element in elements {
                validate_expression_patterns(element, ctx, diagnostics);
            }
        }
        Expression::Index { left, index, .. } => {
            validate_expression_patterns(left, ctx, diagnostics);
            validate_expression_patterns(index, ctx, diagnostics);
        }
        Expression::Hash { pairs, .. } => {
            for (key, value) in pairs {
                validate_expression_patterns(key, ctx, diagnostics);
                validate_expression_patterns(value, ctx, diagnostics);
            }
        }
        Expression::MemberAccess { object, .. } => {
            validate_expression_patterns(object, ctx, diagnostics);
        }
        Expression::Match {
            scrutinee,
            arms,
            span,
        } => {
            validate_expression_patterns(scrutinee, ctx, diagnostics);
            validate_match_arms(arms, *span, ctx, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    validate_expression_patterns(guard, ctx, diagnostics);
                }
                validate_expression_patterns(&arm.body, ctx, diagnostics);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => {
            validate_expression_patterns(value, ctx, diagnostics);
        }
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(expr) = part {
                    validate_expression_patterns(expr, ctx, diagnostics);
                }
            }
        }
        Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::None { .. } => {}
    }
}

fn validate_match_arms(
    arms: &[MatchArm],
    match_span: crate::frontend::position::Span,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if arms.is_empty() {
        diagnostics.push(Diagnostic::make_error(
            &EMPTY_MATCH,
            &[],
            ctx.file_path.to_string(),
            match_span,
        ));
        return;
    }

    if arms.len() > 1 {
        for arm in &arms[..arms.len() - 1] {
            if is_unconditional_catchall_arm(arm) {
                diagnostics.push(Diagnostic::make_error(
                    &CATCHALL_NOT_LAST,
                    &[],
                    ctx.file_path.to_string(),
                    arm.pattern.span(),
                ));
            }
        }
    }

    if let Some(last) = arms.last()
        && !is_unconditional_catchall_arm(last)
    {
        diagnostics.push(Diagnostic::make_error(
            &NON_EXHAUSTIVE_MATCH,
            &[],
            ctx.file_path.to_string(),
            match_span,
        ));
    }

    for arm in arms {
        validate_pattern(&arm.pattern, ctx, diagnostics);
    }
}

fn validate_pattern_bindings(
    pattern: &Pattern,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    bindings: &mut HashSet<String>,
) {
    match pattern {
        Pattern::Identifier { name, span } => {
            if !bindings.insert(name.clone()) {
                diagnostics.push(Diagnostic::make_error(
                    &DUPLICATE_PATTERN_BINDING,
                    &[name],
                    ctx.file_path.to_string(),
                    *span,
                ));
            }
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            validate_pattern_bindings(pattern, ctx, diagnostics, bindings);
        }
        Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
    }
}

fn is_catchall(pattern: &Pattern) -> bool {
    matches!(
        pattern,
        Pattern::Wildcard { .. } | Pattern::Identifier { .. }
    )
}

fn is_unconditional_catchall_arm(arm: &MatchArm) -> bool {
    arm.guard.is_none() && is_catchall(&arm.pattern)
}
