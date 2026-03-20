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
mod tail_resumptive;

pub use anf::anf_normalize;
pub use beta::beta_reduce;
pub use case_of_case::case_of_case;
pub use cokc::case_of_known_constructor;
pub use dead_let::elim_dead_let;
pub use evidence::evidence_pass;
pub use inline::inline_trivial_lets;
pub use inliner::inline_lets;

use crate::core::{CoreExpr, CoreLit, CoreProgram};

// ── Pass pipeline ─────────────────────────────────────────────────────────────

/// Run all Core IR passes in order.
///
/// Pass order matters:
/// 1. `beta_reduce`              — eliminate `App(Lam(x, body), arg)` redexes
/// 2. `case_of_case`             — push outer case into inner case arms
/// 3. `case_of_known_constructor` — reduce `Case(Con/Lit, alts)` statically
/// 4. `inline_lets`              — inline dead, single-use, and small let-bindings
///    (subsumes `inline_trivial_lets`; uses occurrence analysis)
/// 5. `elim_dead_let`            — drop unused pure bindings left over
/// 6. `evidence_pass`            — rewrite TR Handle/Perform into evidence passing
/// 7. `anf_normalize`            — flatten nested subexpressions into let-chains
pub fn run_core_passes(program: &mut CoreProgram) {
    // Find the maximum binder ID so passes can allocate fresh IDs above it.
    let mut max_binder_id: u32 = 0;
    for def in &program.defs {
        max_binder_id = max_binder_id.max(def.binder.id.0);
        collect_max_binder_id(&def.expr, &mut max_binder_id);
    }
    let mut next_id = max_binder_id + 1;

    let sentinel = CoreExpr::Lit(CoreLit::Unit, Default::default());

    // Run all Core passes + Aether in a single loop per definition.
    // Borrow inference runs once upfront over the pre-pass program.
    // (Pre-pass bodies are not yet ANF-normalized, so owned_use_count
    // results are approximate — but still correct for the common case
    // of parameters only used in PrimOp/Case/App positions.)
    // Run standard Core passes first (before Aether).
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = beta_reduce(e);
        let e = case_of_case(e);
        let e = case_of_known_constructor(e);
        let e = inline_lets(e);
        let e = elim_dead_let(e);
        let e = evidence_pass(e, &mut next_id);
        let e = anf_normalize(e, &mut next_id);
        def.expr = e;
    }

    // Infer cross-function borrow modes from the ANF-normalized program,
    // then run the Aether pass with the registry.
    let borrow_registry = crate::aether::borrow_infer::infer_borrow_modes(program);
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = crate::aether::run_aether_pass_with_registry(e, &borrow_registry);
        def.expr = e;
    }
}

/// Run FBIP checking on annotated functions after Aether passes.
/// Reports violations as warnings to stderr.
pub fn check_fbip_annotations(
    program: &CoreProgram,
    interner: &crate::syntax::interner::Interner,
) {
    let diags = crate::aether::check_fbip::check_fbip(program, interner);
    for diag in &diags {
        eprintln!("warning: {}", diag);
    }
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
        App { func, args, .. } => {
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

#[cfg(test)]
mod tests;
