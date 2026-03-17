/// Core IR optimization passes.
///
/// These passes operate on `CoreExpr` / `CoreProgram` before backend lowering.
/// Passes run after `lower_ast::lower_program_ast` produces a `CoreProgram`.
mod beta;
mod cokc;
mod dead_let;
mod helpers;
mod inline;

pub use beta::beta_reduce;
pub use cokc::case_of_known_constructor;
pub use dead_let::elim_dead_let;
pub use inline::inline_trivial_lets;

use crate::core::{CoreExpr, CoreLit, CoreProgram};

// ── Pass pipeline ─────────────────────────────────────────────────────────────

/// Run all Core IR passes in order.
///
/// Pass order matters:
/// 1. `beta_reduce`              — eliminate `App(Lam(x, body), arg)` redexes
/// 2. `case_of_known_constructor` — reduce `Case(Con/Lit, alts)` statically
/// 3. `inline_trivial_lets`      — substitute literal/variable let-bindings
///    (COKC creates field-binding lets like `let x = Lit(n)` that this collapses)
/// 4. `elim_dead_let`            — drop unused pure bindings left over
pub fn run_core_passes(program: &mut CoreProgram) {
    let sentinel = CoreExpr::Lit(CoreLit::Unit, Default::default());
    for def in &mut program.defs {
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = beta_reduce(e);
        let e = case_of_known_constructor(e);
        let e = inline_trivial_lets(e);
        let e = elim_dead_let(e);
        def.expr = e;
    }
}

#[cfg(test)]
mod tests;
