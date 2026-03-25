/// Core IR optimization passes.
///
/// These passes operate on `CoreExpr` / `CoreProgram` before backend lowering.
/// Passes run after `lower_ast::lower_program_ast` produces a `CoreProgram`.
mod anf;
mod beta;
mod case_of_case;
mod cokc;
mod dead_let;
mod evidence;
mod helpers;
mod inline;
mod inliner;
mod primop_promote;
mod tail_resumptive;

pub use anf::anf_normalize;
pub use beta::beta_reduce;
pub use case_of_case::case_of_case;
pub use cokc::case_of_known_constructor;
pub use dead_let::elim_dead_let;
pub use evidence::evidence_pass;
pub use inline::inline_trivial_lets;
pub use inliner::inline_lets;
pub use primop_promote::promote_builtins;

use crate::core::{CoreExpr, CoreLit, CoreProgram};
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, ErrorType,
};
use crate::syntax::interner::Interner;

// ── Pass pipeline ─────────────────────────────────────────────────────────────

/// Run all Core IR passes in order.
///
/// Simplification passes (iterated when `optimize` is true):
/// 1. `beta_reduce`              — eliminate `App(Lam(x, body), arg)` redexes
/// 2. `case_of_case`             — push outer case into inner case arms
/// 3. `case_of_known_constructor` — reduce `Case(Con/Lit, alts)` statically
/// 4. `inline_lets`              — inline dead, single-use, and small let-bindings
///    (subsumes `inline_trivial_lets`; uses occurrence analysis)
/// 5. `elim_dead_let`            — drop unused pure bindings left over
///
/// Normalization passes (run once after simplification):
/// 6. `evidence_pass`            — rewrite TR Handle/Perform into evidence passing
/// 7. `anf_normalize`            — flatten nested subexpressions into let-chains
///
/// When `optimize` is true, simplification passes run up to 3 rounds (fixed-
/// point iteration inspired by GHC's Core simplifier).  Each additional round
/// can discover opportunities exposed by the previous round (e.g. beta
/// reduction exposing a known constructor for COKC).
#[allow(clippy::result_large_err)]
pub fn run_core_passes(program: &mut CoreProgram) -> Result<(), Diagnostic> {
    run_core_passes_with_optional_interner(program, None, false).map(|_| ())
}

#[allow(clippy::result_large_err)]
pub fn run_core_passes_with_interner(
    program: &mut CoreProgram,
    interner: &Interner,
    optimize: bool,
) -> Result<(), Diagnostic> {
    run_core_passes_with_interner_and_warnings(program, interner, optimize).map(|_| ())
}

#[allow(clippy::result_large_err)]
pub fn run_core_passes_with_interner_and_warnings(
    program: &mut CoreProgram,
    interner: &Interner,
    optimize: bool,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    run_core_passes_with_optional_interner(program, Some(interner), optimize)
}

/// Maximum number of simplification rounds when `-O` is enabled.
const MAX_SIMPLIFIER_ROUNDS: usize = 3;

#[allow(clippy::result_large_err)]
fn run_core_passes_with_optional_interner(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
    optimize: bool,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut warnings = Vec::new();
    // Find the maximum binder ID so passes can allocate fresh IDs above it.
    let mut max_binder_id: u32 = 0;
    for def in &program.defs {
        max_binder_id = max_binder_id.max(def.binder.id.0);
        collect_max_binder_id(&def.expr, &mut max_binder_id);
    }
    let mut next_id = max_binder_id + 1;

    // ── Stage 0: Promote known builtin calls to PrimOp ─────────────────
    // Must run after binder resolution (so `binder: None` is reliable) and
    // before simplification (so Core passes see PrimOp, not App).
    if let Some(interner) = interner {
        promote_builtins(program, interner);
    }

    let sentinel = CoreExpr::Lit(CoreLit::Unit, Default::default());

    // ── Stage 1: Simplification passes (iterated when optimize=true) ─────
    let max_rounds = if optimize { MAX_SIMPLIFIER_ROUNDS } else { 1 };

    for round in 0..max_rounds {
        // Measure total program size before this round to detect changes.
        let size_before: usize = program
            .defs
            .iter()
            .map(|d| helpers::expr_size(&d.expr))
            .sum();

        for def in &mut program.defs {
            let e = std::mem::replace(&mut def.expr, sentinel.clone());
            let e = beta_reduce(e);
            verify_aether_contract_stage(def, &e, "beta_reduce")?;
            let e = case_of_case(e);
            verify_aether_contract_stage(def, &e, "case_of_case")?;
            let e = case_of_known_constructor(e);
            verify_aether_contract_stage(def, &e, "case_of_known_constructor")?;
            let e = inline_lets(e);
            verify_aether_contract_stage(def, &e, "inline_lets")?;
            let e = elim_dead_let(e);
            verify_aether_contract_stage(def, &e, "elim_dead_let")?;
            def.expr = e;
        }

        // After the first round, check whether anything changed.
        // If the total node count is the same, no pass fired — stop early.
        if round > 0 {
            let size_after: usize = program
                .defs
                .iter()
                .map(|d| helpers::expr_size(&d.expr))
                .sum();
            if size_after == size_before {
                break;
            }
        }
    }

    // ── Stage 2: Normalization passes (run once) ─────────────────────────
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = evidence_pass(e, &mut next_id);
        verify_aether_contract_stage(def, &e, "evidence_pass")?;
        let e = anf_normalize(e, &mut next_id);
        verify_aether_contract_stage(def, &e, "anf_normalize")?;
        def.expr = e;
    }

    // Infer cross-function borrow modes from the ANF-normalized program,
    // then run the Aether pass with the registry.
    let borrow_registry = crate::aether::borrow_infer::infer_borrow_modes(program, interner);
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = crate::aether::run_aether_pass_with_registry(e, &borrow_registry);
        verify_aether_contract_stage(def, &e, "run_aether_pass")?;
        def.expr = e;
    }

    if let Some(interner) = interner {
        let fbip_result = check_fbip_annotations(program, interner);
        warnings.extend(fbip_result.warnings);
        if let Some(error) = fbip_result.error {
            return Err(error);
        }
    }

    Ok(warnings)
}

/// Run semantic FBIP checking on annotated functions after Aether passes.
pub fn check_fbip_annotations(
    program: &CoreProgram,
    interner: &crate::syntax::interner::Interner,
) -> crate::aether::check_fbip::FbipCheckResult {
    crate::aether::check_fbip::check_fbip(program, interner)
}

/// Walk an expression tree to find the maximum `CoreBinderId` in use.
fn collect_max_binder_id(expr: &CoreExpr, max: &mut u32) {
    use crate::core::CoreExpr::*;
    match expr {
        Var { .. } | Lit(_, _) => {}
        Lam { params, body, .. } => {
            for p in params {
                *max = (*max).max(p.id.0);
            }
            collect_max_binder_id(body, max);
        }
        App { func, args, .. } | AetherCall { func, args, .. } => {
            collect_max_binder_id(func, max);
            for a in args {
                collect_max_binder_id(a, max);
            }
        }
        Let { var, rhs, body, .. } | LetRec { var, rhs, body, .. } => {
            *max = (*max).max(var.id.0);
            collect_max_binder_id(rhs, max);
            collect_max_binder_id(body, max);
        }
        Case {
            scrutinee, alts, ..
        } => {
            collect_max_binder_id(scrutinee, max);
            for alt in alts {
                collect_max_binder_id(&alt.rhs, max);
                if let Some(g) = &alt.guard {
                    collect_max_binder_id(g, max);
                }
            }
        }
        Con { fields, .. } => {
            for f in fields {
                collect_max_binder_id(f, max);
            }
        }
        PrimOp { args, .. } => {
            for a in args {
                collect_max_binder_id(a, max);
            }
        }
        Return { value, .. } => collect_max_binder_id(value, max),
        Perform { args, .. } => {
            for a in args {
                collect_max_binder_id(a, max);
            }
        }
        Handle { body, handlers, .. } => {
            collect_max_binder_id(body, max);
            for h in handlers {
                *max = (*max).max(h.resume.id.0);
                for p in &h.params {
                    *max = (*max).max(p.id.0);
                }
                collect_max_binder_id(&h.body, max);
            }
        }
        Dup { body, .. } | Drop { body, .. } => {
            collect_max_binder_id(body, max);
        }
        Reuse { fields, .. } => {
            for f in fields {
                collect_max_binder_id(f, max);
            }
        }
        DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_max_binder_id(unique_body, max);
            collect_max_binder_id(shared_body, max);
        }
    }
}

#[allow(clippy::result_large_err)]
fn verify_aether_contract_stage(
    def: &crate::core::CoreDef,
    expr: &CoreExpr,
    stage: &'static str,
) -> Result<(), Diagnostic> {
    if let Err(errors) = crate::aether::verify::verify_contract(expr) {
        return Err(aether_contract_error(def, stage, &errors));
    }
    Ok(())
}

fn aether_contract_error(
    def: &crate::core::CoreDef,
    stage: &'static str,
    errors: &[crate::aether::verify::AetherError],
) -> Diagnostic {
    let bullet_lines = errors
        .iter()
        .map(|error| format!("- {:?}", error.kind))
        .collect::<Vec<_>>()
        .join("\n");
    let details = errors
        .iter()
        .map(|error| format!("- {}", error.message))
        .collect::<Vec<_>>()
        .join("\n");
    Diagnostic::make_error_dynamic(
        "E999",
        "AETHER VERIFICATION FAILED",
        ErrorType::Compiler,
        format!(
            "definition `{}` emitted malformed Aether after `{}` and cannot be lowered:\n{}\n\n{}",
            def.name.as_u32(),
            stage,
            bullet_lines,
            details
        ),
        Some("Fix the Aether transform in src/aether/ before lowering to CFG.".to_string()),
        "",
        def.span,
    )
    .with_display_title("Aether Verification Failed")
    .with_category(DiagnosticCategory::Internal)
    .with_phase(DiagnosticPhase::Validation)
    .with_primary_label(def.span, "malformed Aether emitted here")
}

#[cfg(test)]
mod tests;
