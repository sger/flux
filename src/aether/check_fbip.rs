//! FBIP checking pass (Perceus Section 2.6).
//!
//! Verifies `@fip` / `@fbip` contracts from semantic Core or backend-only
//! Aether lowering behavior instead of constructor/reuse counts.

use crate::aether::AetherProgram;
use crate::core::CoreProgram;
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, ErrorType,
};
use crate::syntax::{interner::Interner, statement::FipAnnotation};

use super::fbip_analysis::{
    FbipCallDetail, FbipCallKind, FbipCallOutcome, FbipFailureReason, FbipOutcome, analyze_program,
    analyze_program_aether,
};

#[derive(Debug, Clone)]
pub struct FbipDiagnostic {
    pub function_name: String,
    pub annotation: FipAnnotation,
    pub outcome: FbipOutcome,
    pub reasons: Vec<FbipFailureReason>,
    pub details: Vec<String>,
    pub call_details: Vec<FbipCallDetail>,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Default)]
pub struct FbipCheckResult {
    pub warnings: Vec<Diagnostic>,
    pub error: Option<Diagnostic>,
    pub diagnostics: Vec<FbipDiagnostic>,
}

pub fn check_fbip(program: &CoreProgram, interner: &Interner) -> FbipCheckResult {
    let summaries = analyze_program(program, interner);
    build_check_result(&program.defs, summaries, interner)
}

pub fn check_fbip_aether(program: &AetherProgram, interner: &Interner) -> FbipCheckResult {
    let summaries = analyze_program_aether(program, interner);
    build_check_result(&program.defs, summaries, interner)
}

fn build_check_result<D>(
    defs: &[D],
    summaries: std::collections::HashMap<
        crate::core::CoreBinderId,
        super::fbip_analysis::FbipSummary,
    >,
    interner: &Interner,
) -> FbipCheckResult
where
    D: FbipDefView,
{
    let mut result = FbipCheckResult::default();

    for def in defs {
        let Some(annotation) = def.fip_annotation() else {
            continue;
        };
        let Some(summary) = summaries.get(&def.binder().id) else {
            continue;
        };

        let reasons = summary.reasons.iter().copied().collect::<Vec<_>>();
        let mut details = reasons
            .iter()
            .filter(|reason| **reason != FbipFailureReason::NoConstructors)
            .map(|reason| detail_for_reason(*reason, &summary.call_details))
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
                FbipOutcome::NotProvable => {
                    "could not prove the requested FBIP contract".to_string()
                }
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
            interner.resolve(def.name()),
            summary.outcome,
            details_text
        );

        let diagnostic = match annotation {
            FipAnnotation::Fip => Diagnostic::warning(title)
                .with_category(DiagnosticCategory::Internal)
                .with_error_type(ErrorType::Compiler)
                .with_phase(DiagnosticPhase::Validation)
                .with_span(def.span())
                .with_message(message.clone()),
            FipAnnotation::Fbip => Diagnostic::make_error_dynamic(
                "E999",
                title,
                ErrorType::Compiler,
                message.clone(),
                Some(
                    "Annotate only functions whose fresh allocations remain provably bounded."
                        .to_string(),
                ),
                "",
                def.span(),
            )
            .with_category(DiagnosticCategory::Internal)
            .with_phase(DiagnosticPhase::Validation),
        };

        let fbip_diag = FbipDiagnostic {
            function_name: interner.resolve(def.name()).to_string(),
            annotation,
            outcome: summary.outcome.clone(),
            reasons: reasons.clone(),
            details: details.clone(),
            call_details: summary.call_details.clone(),
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
                            .with_span(def.span())
                            .with_message(format!(
                                "@fbip on `{}` is vacuous: no heap constructor sites were found",
                                interner.resolve(def.name())
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
                    .with_span(def.span())
                    .with_message(format!(
                        "@fip on `{}` is vacuous: no heap constructor sites were found",
                        interner.resolve(def.name())
                    )),
            );
        }

        result.diagnostics.push(fbip_diag);
    }

    result
}

trait FbipDefView {
    fn name(&self) -> crate::syntax::Identifier;
    fn binder(&self) -> crate::core::CoreBinder;
    fn span(&self) -> crate::diagnostics::position::Span;
    fn fip_annotation(&self) -> Option<FipAnnotation>;
}

impl FbipDefView for crate::core::CoreDef {
    fn name(&self) -> crate::syntax::Identifier {
        self.name
    }

    fn binder(&self) -> crate::core::CoreBinder {
        self.binder
    }

    fn span(&self) -> crate::diagnostics::position::Span {
        self.span
    }

    fn fip_annotation(&self) -> Option<FipAnnotation> {
        self.fip
    }
}

impl FbipDefView for crate::aether::AetherDef {
    fn name(&self) -> crate::syntax::Identifier {
        self.name
    }

    fn binder(&self) -> crate::core::CoreBinder {
        self.binder
    }

    fn span(&self) -> crate::diagnostics::position::Span {
        self.span
    }

    fn fip_annotation(&self) -> Option<FipAnnotation> {
        self.fip
    }
}

fn detail_for_reason(reason: FbipFailureReason, call_details: &[FbipCallDetail]) -> String {
    match reason {
        FbipFailureReason::FreshAllocation => {
            "fresh heap allocation remains on at least one path".to_string()
        }
        FbipFailureReason::NonFipCall => call_details
            .iter()
            .find(|detail| matches!(detail.outcome, FbipCallOutcome::KnownNotProvable))
            .map(|detail| match detail.kind {
                FbipCallKind::DirectInternal | FbipCallKind::DirectInferredGlobal => {
                    format!(
                        "calls known function `{}` whose FBIP behavior is not yet provable",
                        detail.callee
                    )
                }
                FbipCallKind::DirectImported => {
                    format!(
                        "calls imported or name-only function `{}` whose FBIP behavior is conservative",
                        detail.callee
                    )
                }
                FbipCallKind::Builtin(effect) => format!(
                    "calls Flux builtin `{}` ({}) at a proof boundary",
                    detail.callee,
                    builtin_effect_label(effect)
                ),
                FbipCallKind::Indirect => {
                    "calls a known function whose FBIP contract could not be proved".to_string()
                }
            })
            .unwrap_or_else(|| {
                "calls a known function whose FBIP contract could not be proved".to_string()
            }),
        FbipFailureReason::UnknownCall => call_details
            .iter()
            .find(|detail| matches!(detail.outcome, FbipCallOutcome::UnknownIndirect))
            .map(|detail| {
                format!(
                    "calls indirect or opaque callee `{}` whose FBIP behavior is unknown",
                    detail.callee
                )
            })
            .unwrap_or_else(|| "calls an indirect, unknown, or unannotated function".to_string()),
        FbipFailureReason::BuiltinBoundary => call_details
            .iter()
            .find(|detail| matches!(detail.kind, FbipCallKind::Builtin(_)))
            .map(|detail| match detail.kind {
                FbipCallKind::Builtin(effect) => format!(
                    "calls Flux builtin `{}` ({}) at a conservative proof boundary",
                    detail.callee,
                    builtin_effect_label(effect)
                ),
                _ => unreachable!(),
            })
            .unwrap_or_else(|| {
                "crosses a Flux builtin boundary that remains conservative".to_string()
            }),
        FbipFailureReason::EffectBoundary => {
            "crosses an effect or handler boundary that prevents proof".to_string()
        }
        FbipFailureReason::TokenUnavailable => {
            "constructor rebuild could not reuse an available token".to_string()
        }
        FbipFailureReason::ControlFlowJoin => {
            "control-flow join loses a precise allocation bound across paths".to_string()
        }
        FbipFailureReason::NoConstructors => "no heap constructor sites were found".to_string(),
    }
}

fn builtin_effect_label(effect: crate::aether::AetherBuiltinEffect) -> &'static str {
    match effect {
        crate::aether::AetherBuiltinEffect::Io => crate::syntax::builtin_effects::IO,
        crate::aether::AetherBuiltinEffect::Time => crate::syntax::builtin_effects::TIME,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreTag, CoreVarRef,
    };
    use crate::diagnostics::position::Span;
    use crate::syntax::{interner::Interner, statement::FipAnnotation};
    use std::borrow::Borrow;

    use super::check_fbip;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var<B: Borrow<CoreBinder>>(binder: B) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder.borrow()),
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
        assert!(
            result.warnings[0]
                .message()
                .unwrap()
                .contains("fresh heap allocation")
        );
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
        assert!(
            result
                .error
                .unwrap()
                .message()
                .unwrap()
                .contains("indirect or opaque")
        );
    }

    #[test]
    fn semantic_checker_names_known_nonprovable_callee() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let caller = binder(&mut interner, 3, "caller");
        let callee = mk_def(
            &mut interner,
            4,
            "callee",
            CoreExpr::App {
                func: Box::new(var(x)),
                args: vec![var(x)],
                span: Span::default(),
            },
            Some(FipAnnotation::Fip),
        );
        let program = CoreProgram {
            defs: vec![
                callee.clone(),
                CoreDef {
                    name: caller.name,
                    binder: caller,
                    expr: CoreExpr::App {
                        func: Box::new(CoreExpr::Var {
                            var: CoreVarRef::resolved(&callee.binder),
                            span: Span::default(),
                        }),
                        args: vec![var(x)],
                        span: Span::default(),
                    },
                    borrow_signature: None,
                    result_ty: None,
                    is_anonymous: false,
                    is_recursive: false,
                    fip: Some(FipAnnotation::Fbip),
                    span: Span::default(),
                },
            ],
            top_level_items: Vec::new(),
        };
        let result = check_fbip(&program, &interner);
        let error = result.error.expect("expected hard error");
        let message = error.message().unwrap();
        assert!(message.contains("`callee`"));
        assert!(message.contains("not yet provable"));
    }

    #[test]
    fn semantic_checker_does_not_report_builtin_as_unknown() {
        let mut interner = Interner::new();
        let x = binder(&mut interner, 2, "x");
        let print = interner.intern("print");
        let program = CoreProgram {
            defs: vec![mk_def(
                &mut interner,
                1,
                "log_only",
                CoreExpr::App {
                    func: Box::new(CoreExpr::Var {
                        var: CoreVarRef::unresolved(print),
                        span: Span::default(),
                    }),
                    args: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let result = check_fbip(&program, &interner);
        assert!(
            result.error.is_none(),
            "known Flux builtins should not fail FBIP as opaque unknown calls"
        );
    }

    #[test]
    fn semantic_checker_reports_imported_name_only_fallback_specifically() {
        let mut interner = Interner::new();
        let foreign = interner.intern("foreign_fn");
        let x = binder(&mut interner, 2, "x");
        let program = CoreProgram {
            defs: vec![mk_def(
                &mut interner,
                1,
                "caller",
                CoreExpr::App {
                    func: Box::new(CoreExpr::Var {
                        var: CoreVarRef::unresolved(foreign),
                        span: Span::default(),
                    }),
                    args: vec![var(x)],
                    span: Span::default(),
                },
                Some(FipAnnotation::Fip),
            )],
            top_level_items: Vec::new(),
        };
        let result = check_fbip(&program, &interner);
        assert_eq!(result.warnings.len(), 1);
        let message = result.warnings[0].message().unwrap();
        assert!(message.contains("imported or name-only function `foreign_fn`"));
    }
}
