//! Semantic FBIP checking pass (Perceus Section 2.6).
//!
//! Verifies `@fip` / `@fbip` contracts from Aether-transformed Core behavior
//! instead of constructor/reuse counts.

use crate::core::CoreProgram;
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, ErrorType,
};
use crate::syntax::{interner::Interner, statement::FipAnnotation};

use super::fbip_analysis::{FbipFailureReason, FbipOutcome, analyze_program};

#[derive(Debug, Clone)]
pub struct FbipDiagnostic {
    pub function_name: String,
    pub annotation: FipAnnotation,
    pub outcome: FbipOutcome,
    pub reasons: Vec<FbipFailureReason>,
    pub details: Vec<String>,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Default)]
pub struct FbipCheckResult {
    pub warnings: Vec<Diagnostic>,
    pub error: Option<Diagnostic>,
    pub diagnostics: Vec<FbipDiagnostic>,
}

pub fn check_fbip(program: &CoreProgram, interner: &Interner) -> FbipCheckResult {
    let summaries = analyze_program(program);
    let mut result = FbipCheckResult::default();

    for def in &program.defs {
        let Some(annotation) = def.fip else {
            continue;
        };
        let Some(summary) = summaries.get(&def.binder.id) else {
            continue;
        };

        let reasons = summary.reasons.iter().copied().collect::<Vec<_>>();
        let mut details = reasons
            .iter()
            .filter(|reason| **reason != FbipFailureReason::NoConstructors)
            .map(|reason| detail_for_reason(*reason))
            .collect::<Vec<_>>();
        if summary.has_constructors {
            details.retain(|detail| !detail.contains("no heap constructor sites"));
        }
        if details.is_empty() {
            details.push(match summary.outcome {
                FbipOutcome::Fip => "provably zero fresh allocations after Aether".to_string(),
                FbipOutcome::Fbip { bound } => {
                    format!("provably bounded fresh allocations with upper bound {bound}")
                }
                FbipOutcome::NotProvable => "could not prove the requested FBIP contract".to_string(),
            });
        }

        let title = match annotation {
            FipAnnotation::Fip => "FBIP Contract Not Proven",
            FipAnnotation::Fbip => "FBIP Contract Failed",
        };

        let annotation_name = match annotation {
            FipAnnotation::Fip => "fip",
            FipAnnotation::Fbip => "fbip",
        };
        let details_text = details
            .iter()
            .map(|detail| format!("- {detail}"))
            .collect::<Vec<_>>()
            .join("\n");
        let message = format!(
            "@{annotation_name} on `{}` analysis result {:?}:\n{}",
            interner.resolve(def.name),
            summary.outcome,
            details_text
        );

        let diagnostic = match annotation {
            FipAnnotation::Fip => Diagnostic::warning(title)
                .with_category(DiagnosticCategory::Internal)
                .with_error_type(ErrorType::Compiler)
                .with_phase(DiagnosticPhase::Validation)
                .with_span(def.span)
                .with_message(message.clone()),
            FipAnnotation::Fbip => Diagnostic::make_error_dynamic(
                "E999",
                title,
                ErrorType::Compiler,
                message.clone(),
                Some("Annotate only functions whose fresh allocations remain provably bounded.".to_string()),
                "",
                def.span,
            )
            .with_category(DiagnosticCategory::Internal)
            .with_phase(DiagnosticPhase::Validation),
        };

        let fbip_diag = FbipDiagnostic {
            function_name: interner.resolve(def.name).to_string(),
            annotation,
            outcome: summary.outcome.clone(),
            reasons: reasons.clone(),
            details: details.clone(),
            diagnostic: diagnostic.clone(),
        };

        match annotation {
            FipAnnotation::Fip => {
                if !matches!(summary.outcome, FbipOutcome::Fip) {
                    result.warnings.push(diagnostic.clone());
                }
            }
            FipAnnotation::Fbip => {
                if !matches!(summary.outcome, FbipOutcome::Fip | FbipOutcome::Fbip { .. }) {
                    result.error = Some(diagnostic.clone());
                } else if summary.reasons.contains(&FbipFailureReason::NoConstructors) {
                    result.warnings.push(
                        Diagnostic::warning("FBIP Annotation Has No Effect")
                            .with_category(DiagnosticCategory::Internal)
                            .with_error_type(ErrorType::Compiler)
                            .with_phase(DiagnosticPhase::Validation)
                            .with_span(def.span)
                            .with_message(format!(
                                "@fbip on `{}` is vacuous: no heap constructor sites were found",
                                interner.resolve(def.name)
                            )),
                    );
                }
            }
        }

        if annotation == FipAnnotation::Fip
            && summary.reasons.contains(&FbipFailureReason::NoConstructors)
            && matches!(summary.outcome, FbipOutcome::Fip)
        {
            result.warnings.push(
                Diagnostic::warning("FBIP Annotation Has No Effect")
                    .with_category(DiagnosticCategory::Internal)
                    .with_error_type(ErrorType::Compiler)
                    .with_phase(DiagnosticPhase::Validation)
                    .with_span(def.span)
                    .with_message(format!(
                        "@fip on `{}` is vacuous: no heap constructor sites were found",
                        interner.resolve(def.name)
                    )),
            );
        }

        result.diagnostics.push(fbip_diag);
    }

    result
}

fn detail_for_reason(reason: FbipFailureReason) -> String {
    match reason {
        FbipFailureReason::FreshAllocation => {
            "fresh heap allocation remains on at least one path".to_string()
        }
        FbipFailureReason::NonFipCall => {
            "calls a known function whose FBIP contract could not be proved".to_string()
        }
        FbipFailureReason::UnknownCall => {
            "calls an indirect, unknown, or unannotated function".to_string()
        }
        FbipFailureReason::EffectBoundary => {
            "crosses an effect or handler boundary that prevents proof".to_string()
        }
        FbipFailureReason::TokenUnavailable => {
            "constructor rebuild needs a fresh token instead of guaranteed reuse".to_string()
        }
        FbipFailureReason::ControlFlowJoin => {
            "control-flow join loses a precise bound on all paths".to_string()
        }
        FbipFailureReason::NoConstructors => "no heap constructor sites were found".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreTag, CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::{interner::Interner, statement::FipAnnotation};

    use super::check_fbip;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span: Span::default(),
        }
    }

    fn mk_def(
        interner: &mut Interner,
        raw: u32,
        name: &str,
        expr: CoreExpr,
        fip: Option<FipAnnotation>,
    ) -> CoreDef {
        let binder = self::binder(interner, raw, name);
        CoreDef {
            name: binder.name,
            binder,
            expr,
            borrow_signature: None,
            result_ty: None,
            is_anonymous: false,
            is_recursive: false,
            fip,
            span: Span::default(),
        }
    }

    #[test]
    fn semantic_checker_warns_for_fip_fresh_allocations() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let program = CoreProgram {
            defs: vec![mk_def(
                &mut interner,
                1,
                "allocates",
                CoreExpr::Con {
                    tag: CoreTag::Some,
                    fields: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let result = check_fbip(&program, &interner);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0]
            .message()
            .unwrap()
            .contains("fresh heap allocation"));
    }

    #[test]
    fn semantic_checker_errors_for_fbip_unknown_call() {
        let mut interner = Interner::new();
        let f = binder(&mut interner, 2, "f");
        let x = binder(&mut interner, 3, "x");
        let program = CoreProgram {
            defs: vec![mk_def(
                &mut interner,
                1,
                "bounded",
                CoreExpr::App {
                    func: Box::new(var(f)),
                    args: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fbip),
            )],
            top_level_items: Vec::new(),
        };
        let result = check_fbip(&program, &interner);
        assert!(result.error.is_some());
        assert!(result.error.unwrap().message().unwrap().contains("unknown"));
    }
}
