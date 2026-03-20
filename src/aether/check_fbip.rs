//! FBIP checking pass (Perceus Section 2.6).
//!
//! Verifies that functions annotated with `@fip` or `@fbip` meet
//! their FBIP constraints after all Aether passes have run.
//!
//! - `@fip`: zero unreused heap allocations (every Con has a matching Reuse)
//! - `@fbip`: finite (bounded) allocations — reported but not rejected
//!
//! Violations are reported as compiler warnings.

use crate::core::{CoreDef, CoreProgram};
use crate::syntax::statement::FipAnnotation;

use super::{collect_stats, FbipStatus};

/// A diagnostic from FBIP checking.
#[derive(Debug)]
pub struct FbipDiagnostic {
    pub function_name: String,
    pub annotation: FipAnnotation,
    pub violation: FbipViolation,
}

/// The kind of FBIP violation.
#[derive(Debug)]
pub enum FbipViolation {
    /// @fip function has unreused heap allocations.
    UnreusedAllocations { count: usize },
    /// @fip function has no constructor sites (annotation is unnecessary).
    NoConstructors,
}

impl std::fmt::Display for FbipDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let annotation = match self.annotation {
            FipAnnotation::Fip => "fip",
            FipAnnotation::Fbip => "fbip",
        };
        match &self.violation {
            FbipViolation::UnreusedAllocations { count } => {
                write!(
                    f,
                    "@{} function `{}` has {} unreused heap allocation{} \
                     (not all constructors are reused in-place on the unique path)",
                    annotation,
                    self.function_name,
                    count,
                    if *count == 1 { "" } else { "s" }
                )
            }
            FbipViolation::NoConstructors => {
                write!(
                    f,
                    "@{} annotation on `{}` has no effect — \
                     function contains no heap constructor allocations",
                    annotation, self.function_name
                )
            }
        }
    }
}

/// Check all @fip/@fbip annotated functions in a CoreProgram.
/// Returns diagnostics for any violations found.
pub fn check_fbip(program: &CoreProgram, interner: &crate::syntax::interner::Interner) -> Vec<FbipDiagnostic> {
    let mut diags = Vec::new();
    for def in &program.defs {
        if let Some(annotation) = def.fip {
            check_function(def, annotation, interner, &mut diags);
        }
    }
    diags
}

fn check_function(
    def: &CoreDef,
    annotation: FipAnnotation,
    interner: &crate::syntax::interner::Interner,
    diags: &mut Vec<FbipDiagnostic>,
) {
    let stats = collect_stats(&def.expr);
    let status = stats.fbip_status();
    let name = interner.resolve(def.name).to_string();

    match annotation {
        FipAnnotation::Fip => match status {
            FbipStatus::Fip => {} // All good — zero unreused allocations
            FbipStatus::Fbip(n) => {
                diags.push(FbipDiagnostic {
                    function_name: name,
                    annotation,
                    violation: FbipViolation::UnreusedAllocations { count: n },
                });
            }
            FbipStatus::NotApplicable => {
                diags.push(FbipDiagnostic {
                    function_name: name,
                    annotation,
                    violation: FbipViolation::NoConstructors,
                });
            }
        },
        FipAnnotation::Fbip => {
            // @fbip allows allocations — we report but don't warn.
            // Future: could warn if allocation count exceeds a threshold.
            if status == FbipStatus::NotApplicable {
                diags.push(FbipDiagnostic {
                    function_name: name,
                    annotation,
                    violation: FbipViolation::NoConstructors,
                });
            }
        }
    }
}
