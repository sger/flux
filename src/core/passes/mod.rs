mod algebraic;
/// Core IR optimization passes.
///
/// These passes operate on `CoreExpr` / `CoreProgram` before backend lowering.
/// Passes run after `lower_ast::lower_program_ast` produces a `CoreProgram`.
mod anf;
mod beta;
mod canonicalize;
mod case_of_case;
mod cokc;
mod const_fold;
mod dead_let;
pub mod dict_elaborate;
mod disciplined_inline;
mod evidence;
mod handle_frame;
mod helpers;
mod inline;
mod inliner;
mod keep;
mod limits;
pub mod lint;
mod primop_promote;
mod specialize;
pub mod static_contract;
mod tail_resumptive;

pub(crate) use helpers::is_primop_pure;

pub use algebraic::algebraic_simplify;
pub use anf::{anf_normalize, primop_result_rep};
pub use beta::beta_reduce;
pub use canonicalize::call_and_case_canonicalize;
pub use case_of_case::case_of_case;
pub use cokc::case_of_known_constructor;
pub use const_fold::constant_fold;
pub use dead_let::elim_dead_let;
pub use dict_elaborate::build_referenced_dictionaries;
pub(crate) use dict_elaborate::reachable_methods;
pub use disciplined_inline::disciplined_inline;
pub use evidence::evidence_pass;
pub use handle_frame::handle_body_frames;
pub(crate) use helpers::expr_size;
pub use inline::inline_trivial_lets;
pub use inliner::inline_lets;
pub use primop_promote::promote_builtins;
pub use specialize::specialize_known_shapes;

use std::collections::HashSet;

use crate::core::{CoreExpr, CoreLit, CorePat, CoreProgram};
use crate::diagnostics::compiler_errors::CORE_LINT_FAILURE;
use crate::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, ErrorType, diagnostic_for,
};
use crate::shared::opt_bisect;
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
/// 7. `handle_body_frames`       — run a handle's body in a frame of its own
/// 8. `anf_normalize`            — flatten nested subexpressions into let-chains
///
/// When `optimize` is true, simplification passes run up to 3 rounds (fixed-
/// point iteration inspired by GHC's Core simplifier).  Each additional round
/// can discover opportunities exposed by the previous round (e.g. beta
/// reduction exposing a known constructor for COKC).
#[allow(clippy::result_large_err)]
pub fn run_core_passes(program: &mut CoreProgram) -> Result<(), Diagnostic> {
    run_semantic_core_passes_with_optional_interner(program, None, false).map(|_| ())
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
pub fn run_core_passes_with_interner_and_registry(
    program: &mut CoreProgram,
    interner: &Interner,
    optimize: bool,
    preloaded_registry: crate::aether::borrow_infer::BorrowRegistry,
) -> Result<(), Diagnostic> {
    run_semantic_core_passes_with_optional_interner(program, Some(interner), optimize)
        .map(|_| ())?;
    run_aether_passes_with_optional_interner(program, Some(interner), preloaded_registry)
        .map(|_| ())
}

/// As [`run_core_passes_with_interner`], for a debug build (proposal 0188
/// S6, S9c): every pass runs, but no binder the program wrote is substituted
/// away — a `let`, a lambda's parameter or a pattern variable it named stays,
/// bound by a `let` where a pass would have inlined it — so that a native
/// debugger shows each variable and stops on each line, as the VM debugger
/// does. Returns the lambdas the simplifier inlined, for debug information.
#[allow(clippy::result_large_err)]
pub fn run_core_passes_keeping_variables(
    program: &mut CoreProgram,
    interner: &Interner,
    optimize: bool,
) -> Result<Vec<crate::core::InlineSite>, Diagnostic> {
    let kept = keep::source_binders(program, interner);
    let (result, sites) = keep::keeping(kept, || {
        run_semantic_core_passes_with_optional_interner(program, Some(interner), optimize)
    });
    result.map(|_| sites)
}

#[allow(clippy::result_large_err)]
pub fn run_core_passes_with_interner_and_warnings(
    program: &mut CoreProgram,
    interner: &Interner,
    optimize: bool,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    run_semantic_core_passes_with_optional_interner(program, Some(interner), optimize)
}

/// Maximum number of simplification rounds when `-O` is enabled.
const MAX_SIMPLIFIER_ROUNDS: usize = 3;

#[allow(clippy::result_large_err)]
fn run_semantic_core_passes_with_optional_interner(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
    optimize: bool,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut warnings: Vec<Diagnostic> = Vec::new();
    // Find the maximum binder ID so passes can allocate fresh IDs above it.
    let mut next_id = next_fresh_binder_id(program);

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
        //
        // Limitation: `expr_size` counts nodes. A pass that rewrites without
        // changing node count (e.g. `call_and_case_canonicalize` normalising
        // the shape of a call without adding or removing subterms) will look
        // like a no-op to this fixed-point detector and may prevent a later
        // round from running even when one would still make progress. In
        // practice every simplification pass today either shrinks the tree
        // or exposes a reduction that does. If that ever stops being true,
        // replace this heuristic with a per-pass `changed: bool` flag.
        let size_before: usize = program.defs.iter().map(|d| helpers::expr_size(&d.expr)).sum();

        let scope = lint_scope(program);
        // Definitions are simplified one at a time, so their order must not
        // matter; `--shuffle-seed` varies it to check (0190 §9).
        for index in crate::shared::shuffle::visit_order(program.defs.len(), "core-simplify") {
            let def = &mut program.defs[index];
            opt_bisect::set_def(|| opt_bisect::def_name(def.name, interner));
            let e = std::mem::replace(&mut def.expr, sentinel.clone());
            let e = beta_reduce(e);
            check_pass(def, &e, "beta_reduce", scope.as_ref(), interner)?;
            let e = case_of_case(e);
            check_pass(def, &e, "case_of_case", scope.as_ref(), interner)?;
            let e = case_of_known_constructor(e);
            check_pass(def, &e, "case_of_known_constructor", scope.as_ref(), interner)?;
            let e = call_and_case_canonicalize(e);
            check_pass(def, &e, "call_and_case_canonicalize", scope.as_ref(), interner)?;
            let e = specialize_known_shapes(e);
            check_pass(def, &e, "specialize_known_shapes", scope.as_ref(), interner)?;
            let e = algebraic_simplify(e);
            check_pass(def, &e, "algebraic_simplify", scope.as_ref(), interner)?;
            let e = constant_fold(e);
            check_pass(def, &e, "constant_fold", scope.as_ref(), interner)?;
            let e = disciplined_inline(e);
            check_pass(def, &e, "disciplined_inline", scope.as_ref(), interner)?;
            let e = inline_lets(e);
            check_pass(def, &e, "inline_lets", scope.as_ref(), interner)?;
            let e = elim_dead_let(e);
            check_pass(def, &e, "elim_dead_let", scope.as_ref(), interner)?;
            def.expr = e;
        }

        normalize_lam_param_type_lengths(program, interner);

        // Verify Core invariants after each simplification round.
        core_lint_stage(program, "simplification", interner)?;

        // After the first round, check whether anything changed.
        // If the total node count is the same, no pass fired — stop early.
        if round > 0 {
            let size_after: usize = program.defs.iter().map(|d| helpers::expr_size(&d.expr)).sum();
            if size_after == size_before {
                break;
            }
        }
    }

    // `cargo xtask bisect-opt` diffs one definition between two limits
    // (0190 §13).
    if let Some(interner) = interner
        && opt_bisect::is_active()
    {
        for def in &program.defs {
            if opt_bisect::shows(&opt_bisect::def_name(def.name, Some(interner))) {
                eprintln!(
                    "opt-bisect-show: core {}\n{}\nopt-bisect-show: end",
                    opt_bisect::def_name(def.name, Some(interner)),
                    crate::core::display::display_expr_readable(&def.expr, interner)
                );
            }
        }
    }

    // ── Stage 1b: Static typing contract validation (Proposal 0167 Part 7) ─
    //
    // Spot-check each Core definition's HM-resolved result type for illegal
    // unresolved residue *before* normalization settles representations.
    //
    // Violations emit at `Severity::Error` (see `build_violation` in
    // `static_contract.rs`). The AST-level strict validator is the primary
    // gate; this pass catches residue the AST pass missed — by definition
    // a real contract violation.
    //
    // The diagnostics still travel through the `warnings` return channel
    // because the surrounding pass infrastructure uses that channel as a
    // general "non-fatal diagnostics" bag. Callers (the type-inference
    // pass) treat Error-severity entries as fatal via `has_hm_diagnostics`.
    //
    // `FLUX_CORE_CONTRACT_SILENT` exists as a temporary escape hatch in
    // case a real user program trips a false-positive during rollout. It
    // can be removed once we have confidence in the pass.
    if let Some(interner) = interner
        && std::env::var_os("FLUX_CORE_CONTRACT_SILENT").is_none()
    {
        let report = static_contract::validate_core_static_contract(program, interner);
        warnings.extend(report.violations);
    }

    // ── Stage 2: Normalization passes (run once) ─────────────────────────
    let scope = lint_scope(program);
    // The order may change the fresh binders `next_id` hands out, never the
    // program (0190 §9).
    for index in crate::shared::shuffle::visit_order(program.defs.len(), "core-normalize") {
        let def = &mut program.defs[index];
        let e = std::mem::replace(&mut def.expr, sentinel.clone());
        let e = evidence_pass(e, &mut next_id);
        check_pass(def, &e, "evidence_pass", scope.as_ref(), interner)?;
        let e = handle_body_frames(e);
        check_pass(def, &e, "handle_body_frames", scope.as_ref(), interner)?;
        let e = anf_normalize(e, &mut next_id);
        check_pass(def, &e, "anf_normalize", scope.as_ref(), interner)?;
        def.expr = e;
    }
    // After the evidence pass, so an arity counts the dictionaries it adds.
    let result_reps = anf::scalar_result_reps(program);
    for def in &mut program.defs {
        anf::type_call_results(&mut def.expr, &result_reps);
    }

    // Verify Core invariants after normalization.
    core_lint_stage(program, "normalization", interner)?;

    limits::check_field_counts(program)?;

    Ok(warnings)
}

fn normalize_lam_param_type_lengths(program: &mut CoreProgram, _interner: Option<&Interner>) {
    for def in &mut program.defs {
        normalize_lam_param_type_lengths_expr(&mut def.expr);
    }
}

fn normalize_lam_param_type_lengths_expr(expr: &mut CoreExpr) {
    match expr {
        CoreExpr::Lam { params, param_types, body, .. } => {
            if !param_types.is_empty() && param_types.len() < params.len() {
                let missing = params.len() - param_types.len();
                let mut normalized = vec![None; missing];
                normalized.append(param_types);
                *param_types = normalized;
            }
            normalize_lam_param_type_lengths_expr(body);
        }
        CoreExpr::App { func, args, .. } => {
            normalize_lam_param_type_lengths_expr(func);
            for arg in args {
                normalize_lam_param_type_lengths_expr(arg);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            normalize_lam_param_type_lengths_expr(rhs);
            normalize_lam_param_type_lengths_expr(body);
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            for (_, rhs) in bindings {
                normalize_lam_param_type_lengths_expr(rhs);
            }
            normalize_lam_param_type_lengths_expr(body);
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            normalize_lam_param_type_lengths_expr(scrutinee);
            for alt in alts {
                if let Some(guard) = &mut alt.guard {
                    normalize_lam_param_type_lengths_expr(guard);
                }
                normalize_lam_param_type_lengths_expr(&mut alt.rhs);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                normalize_lam_param_type_lengths_expr(field);
            }
        }
        CoreExpr::Return { value, .. }
        | CoreExpr::MemberAccess { object: value, .. }
        | CoreExpr::TupleField { object: value, .. } => {
            normalize_lam_param_type_lengths_expr(value);
        }
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                normalize_lam_param_type_lengths_expr(arg);
            }
        }
        CoreExpr::Handle { body, parameter, handlers, .. } => {
            normalize_lam_param_type_lengths_expr(body);
            if let Some(parameter) = parameter {
                normalize_lam_param_type_lengths_expr(parameter);
            }
            for handler in handlers {
                normalize_lam_param_type_lengths_expr(&mut handler.body);
            }
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
    }
}

#[allow(clippy::result_large_err)]
pub fn run_aether_passes_with_interner_and_registry(
    program: &mut CoreProgram,
    interner: &Interner,
    preloaded_registry: crate::aether::borrow_infer::BorrowRegistry,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    run_aether_passes_with_optional_interner(program, Some(interner), preloaded_registry)
}

#[allow(clippy::result_large_err)]
pub fn run_aether_passes(program: &mut CoreProgram) -> Result<Vec<Diagnostic>, Diagnostic> {
    run_aether_passes_with_optional_interner(
        program,
        None,
        crate::aether::borrow_infer::BorrowRegistry::default(),
    )
}

#[allow(clippy::result_large_err)]
fn run_aether_passes_with_optional_interner(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
    preloaded_registry: crate::aether::borrow_infer::BorrowRegistry,
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let (aether_program, warnings) =
        crate::aether::lower_core_to_aether_program(program, interner, preloaded_registry)?;
    verify_aether_program_contract(&aether_program, interner)?;
    *program = aether_program.into_core();
    Ok(warnings)
}

/// Checks every definition of a lowered program against the fatal Aether
/// contract.
#[allow(clippy::result_large_err)]
pub(crate) fn verify_aether_program_contract(
    program: &crate::aether::AetherProgram,
    interner: Option<&Interner>,
) -> Result<(), Diagnostic> {
    for def in program.defs() {
        verify_aether_contract_stage_aether(
            def,
            &def.expr,
            "lower_core_to_aether_program",
            interner,
        )?;
    }
    Ok(())
}

/// Run semantic FBIP checking on annotated functions after Aether passes.
pub fn check_fbip_annotations(
    program: &CoreProgram,
    interner: &crate::syntax::interner::Interner,
) -> crate::aether::check_fbip::FbipCheckResult {
    crate::aether::check_fbip::check_fbip(program, interner)
}

/// Walk an expression tree to find the maximum `CoreBinderId` in use.
pub(crate) fn next_fresh_binder_id(program: &CoreProgram) -> u32 {
    let mut max_binder_id: u32 = 0;
    for def in &program.defs {
        max_binder_id = max_binder_id.max(def.binder.id.0);
        collect_max_binder_id(&def.expr, &mut max_binder_id);
    }
    max_binder_id + 1
}

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
        LetRecGroup { bindings, body, .. } => {
            for (b, rhs) in bindings {
                *max = (*max).max(b.id.0);
                collect_max_binder_id(rhs, max);
            }
            collect_max_binder_id(body, max);
        }
        Case { scrutinee, alts, .. } => {
            collect_max_binder_id(scrutinee, max);
            for alt in alts {
                collect_max_pattern_binder_id(&alt.pat, max);
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
        MemberAccess { object, .. } | TupleField { object, .. } => {
            collect_max_binder_id(object, max);
        }
    }
}

fn collect_max_pattern_binder_id(pat: &CorePat, max: &mut u32) {
    match pat {
        CorePat::Var(binder) => *max = (*max).max(binder.id.0),
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                collect_max_pattern_binder_id(field, max);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}

/// Verify Core IR structural invariants for the entire program.
///
/// Fatal: any lint violation produces an `E998 CORE_LINT_FAILURE` diagnostic
/// and aborts compilation. Called after every simplification round and after
/// normalization; see [`compiler_errors::CORE_LINT_FAILURE`]. The previous
/// warning-only (`W998`) mode was promoted to an error under Proposal 0155 so
/// malformed Core produced by a Core pass cannot silently flow into the
/// backend. Compare with GHC's `-dcore-lint`, which is opt-in but also fatal
/// when enabled.
#[allow(clippy::result_large_err)]
pub(crate) fn core_lint_stage(
    program: &CoreProgram,
    stage: &'static str,
    interner: Option<&Interner>,
) -> Result<(), Diagnostic> {
    if let Err(errors) = lint::lint_core_program(program) {
        return Err(core_lint_failure(&errors, &format!("`{stage}`"), interner));
    }
    Ok(())
}

fn core_lint_failure(
    errors: &[lint::CoreLintError],
    after: &str,
    interner: Option<&Interner>,
) -> Diagnostic {
    let detail = errors
        .iter()
        .map(|e| {
            let def = e
                .def_name
                .map(|n| {
                    let resolved = interner.and_then(|i| i.try_resolve(n)).unwrap_or("?");
                    format!(" in `{resolved}`")
                })
                .unwrap_or_default();
            format!("  [{:?}]{def} {}", e.kind, e.message)
        })
        .collect::<Vec<_>>()
        .join("\n");
    diagnostic_for(&CORE_LINT_FAILURE)
        .with_message(format!(
            "Core IR has {} structural violation(s) after {after}:\n{detail}",
            errors.len()
        ))
        .with_phase(DiagnosticPhase::Validation)
}

/// The top-level binders, all in scope in every definition, when
/// `--verify-each` lints each definition after each pass (0190 §14).
fn lint_scope(program: &CoreProgram) -> Option<HashSet<crate::core::CoreBinderId>> {
    crate::shared::verify_each::is_active()
        .then(|| program.defs.iter().map(|def| def.binder.id).collect())
}

/// After each Core pass on one definition: Aether's contract, and under
/// `--verify-each` (`scope` is `Some`) Core lint as well, naming the pass.
#[allow(clippy::result_large_err)]
fn check_pass(
    def: &crate::core::CoreDef,
    expr: &CoreExpr,
    pass: &'static str,
    scope: Option<&HashSet<crate::core::CoreBinderId>>,
    interner: Option<&Interner>,
) -> Result<(), Diagnostic> {
    verify_aether_contract_stage(def, expr, pass)?;
    if let Some(scope) = scope
        && let Err(errors) = lint::lint_core_expr(expr, scope)
    {
        let name = interner.and_then(|i| i.try_resolve(def.name)).unwrap_or("?");
        return Err(core_lint_failure(&errors, &format!("`{pass}` in `{name}`"), interner));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn verify_aether_contract_stage(
    def: &crate::core::CoreDef,
    expr: &CoreExpr,
    stage: &'static str,
) -> Result<(), Diagnostic> {
    if let Err(errors) = crate::aether::verify::verify_contract(expr) {
        return Err(aether_contract_error_core(def, stage, &errors));
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn verify_aether_contract_stage_aether(
    def: &crate::aether::AetherDef,
    expr: &crate::aether::AetherExpr,
    stage: &'static str,
    interner: Option<&Interner>,
) -> Result<(), Diagnostic> {
    if let Err(errors) =
        crate::aether::verify::verify_def_contract_aether(expr, def.borrow_signature.as_ref())
    {
        return Err(aether_contract_error_aether(def, stage, &errors, interner));
    }
    Ok(())
}

fn aether_contract_error_core(
    def: &crate::core::CoreDef,
    stage: &'static str,
    errors: &[crate::aether::verify::AetherError],
) -> Diagnostic {
    aether_contract_error_common("this definition", def.span, stage, errors)
}

pub(crate) fn aether_contract_error_aether(
    def: &crate::aether::AetherDef,
    stage: &'static str,
    errors: &[crate::aether::verify::AetherError],
    interner: Option<&Interner>,
) -> Diagnostic {
    // A top-level expression's `name` is a placeholder symbol, which resolves to
    // whatever was interned first.
    let subject = if def.is_anonymous {
        "a top-level expression".to_string()
    } else {
        interner
            .and_then(|interner| interner.try_resolve(def.name))
            .map_or_else(|| "this definition".to_string(), |name| format!("`{name}`"))
    };
    aether_contract_error_common(&subject, def.span, stage, errors)
}

pub(crate) fn aether_contract_error_common(
    subject: &str,
    def_span: crate::diagnostics::position::Span,
    stage: &'static str,
    errors: &[crate::aether::verify::AetherError],
) -> Diagnostic {
    let bullet_lines =
        errors.iter().map(|error| format!("- {:?}", error.kind)).collect::<Vec<_>>().join("\n");
    let details =
        errors.iter().map(|error| format!("- {}", error.message)).collect::<Vec<_>>().join("\n");
    Diagnostic::make_error_dynamic(
        "E999",
        "AETHER VERIFICATION FAILED",
        ErrorType::Compiler,
        format!(
            "This is a compiler bug, not a problem in your code: the ownership plan for {} is \
             unsound, so it was not compiled.\n\nFor the bug report, after `{}`:\n{}\n\n{}",
            subject, stage, bullet_lines, details
        ),
        Some(
            "Please report this at: https://github.com/flux-lang/flux/issues\nSetting \
             FLUX_AETHER_VERIFY=0 builds it anyway, but the program may then crash or print \
             wrong results."
                .to_string(),
        ),
        "",
        def_span,
    )
    .with_display_title("Aether Verification Failed")
    .with_category(DiagnosticCategory::Internal)
    .with_phase(DiagnosticPhase::Validation)
    .with_primary_label(def_span, "the plan for this definition was rejected")
}

#[cfg(test)]
mod tests;
