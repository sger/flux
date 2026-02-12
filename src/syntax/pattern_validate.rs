use std::collections::HashSet;

use crate::{
    ast::{Visitor, visit},
    diagnostics::{
        CATCHALL_NOT_LAST, DUPLICATE_PATTERN_BINDING, Diagnostic, EMPTY_MATCH,
        NON_EXHAUSTIVE_MATCH, position::Span,
    },
    syntax::{
        expression::{Expression, MatchArm, Pattern},
        interner::Interner,
        program::Program,
        symbol::Symbol,
    },
};

#[derive(Debug, Clone, Copy)]
pub struct PatternValidationContext<'a> {
    pub file_path: &'a str,
    pub interner: &'a Interner,
}

impl<'a> PatternValidationContext<'a> {
    pub fn new(file_path: &'a str, interner: &'a Interner) -> Self {
        Self {
            file_path,
            interner,
        }
    }
}

struct PatternValidator<'a> {
    ctx: PatternValidationContext<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Match { arms, span, .. } = expr {
            validate_match_arms(arms, *span, &self.ctx, &mut self.diagnostics);

            for arm in arms {
                validate_pattern(&arm.pattern, &self.ctx, &mut self.diagnostics);
            }
        }

        visit::walk_expr(self, expr);
    }
}

pub fn validate_program_patterns(
    program: &Program,
    file_path: &str,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let ctx = PatternValidationContext::new(file_path, interner);
    let mut validator = PatternValidator {
        ctx,
        diagnostics: Vec::new(),
    };
    validator.visit_program(program);
    validator.diagnostics
}

pub fn validate_pattern(
    pattern: &Pattern,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut bindings = HashSet::new();
    validate_pattern_bindings(pattern, ctx, diagnostics, &mut bindings);
}

fn validate_match_arms(
    arms: &[MatchArm],
    match_span: Span,
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
}

fn validate_pattern_bindings(
    pattern: &Pattern,
    ctx: &PatternValidationContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    bindings: &mut HashSet<Symbol>,
) {
    match pattern {
        Pattern::Identifier { name, span } => {
            if !bindings.insert(*name) {
                diagnostics.push(Diagnostic::make_error(
                    &DUPLICATE_PATTERN_BINDING,
                    &[ctx.interner.resolve(*name)],
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
