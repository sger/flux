use crate::ast::desugar_named_fields::{
    NamedFieldDesugarCtx, collect_named_field_metadata, collect_named_field_metadata_in_statements,
    desugar_named_fields_in_program,
};
use crate::ast::type_infer::constraint::WantedConstraints;
use crate::ast::type_infer::static_type_validation::{
    StaticTypeValidationCtx, validate_static_types,
};
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::program::Program;
use crate::types::class_disposition::SolveScope;
use crate::types::class_solver::solve_wanted_tree;

use super::super::{Compiler, pipeline::TypeInferenceResult, tag_diagnostics};

impl Compiler {
    /// Phase 3: HM type inference (single or two-phase with type_optimize).
    ///
    /// Two-phase model (when type_optimize=true, proposal 0077):
    ///   Phase 1: infer on the syntactically-optimized AST → TypeEnv for optimization
    ///   type_informed_fold: rewrite AST using TypeEnv (dead branch, const prop, inlining)
    ///   Phase 2: infer on the type-optimized AST → pointer-stable maps for codegen
    ///
    /// Single-phase model (when type_optimize=false):
    ///   Standard single inference pass.
    ///
    /// Invariant: codegen must use the same Program allocation as the final
    /// inference pass so pointer-keyed expression IDs remain stable.
    pub(in crate::compiler) fn phase_type_inference<'a>(
        &mut self,
        program: &'a Program,
    ) -> TypeInferenceResult<'a> {
        let final_inference = self.infer_final_program(program);
        let mut final_program = final_inference.effective_program;
        let hm_final = final_inference.hm_final;
        self.type_env = hm_final.type_env;
        self.hm_expr_types = hm_final.expr_types;
        let module_member_schemes = hm_final.module_member_schemes;
        self.cached_member_schemes
            .extend(module_member_schemes.clone());
        let class_constraints: WantedConstraints = hm_final.class_constraints;
        let instantiated_expr_vars = hm_final.instantiated_expr_vars;
        let resolved_binding_schemes = hm_final.resolved_binding_schemes;

        // REPL mode (proposal 0176): remember this line's top-level binding
        // schemes so a later line's inference can resolve the types of earlier
        // session globals. A failed line is rolled back wholesale by the engine
        // (a pre-line clone of the compiler), which discards these too.
        if self.repl_mode {
            self.repl_session_schemes.extend(
                resolved_binding_schemes
                    .iter()
                    .map(|(k, v)| (*k, v.clone())),
            );
        }

        let mut hm_diagnostics = hm_final.diagnostics;
        tag_diagnostics(&mut hm_diagnostics, DiagnosticPhase::TypeInference);

        // Authoritative static-typing gate: reject any binding whose
        // resolved scheme still contains unresolved fallback type variables.
        let mut strict_diags = validate_static_types(
            final_program.as_ref(),
            &StaticTypeValidationCtx {
                resolved_schemes: &resolved_binding_schemes,
                resolved_binding_schemes_by_span: &hm_final.resolved_binding_schemes_by_span,
                expr_types: &self.hm_expr_types,
                module_member_schemes: &module_member_schemes,
                fallback_vars: &hm_final.fallback_vars,
                instantiated_expr_vars: &instantiated_expr_vars,
                existing_diagnostics: &hm_diagnostics,
                interner: &self.interner,
            },
        );
        tag_diagnostics(&mut strict_diags, DiagnosticPhase::TypeInference);
        hm_diagnostics.extend(strict_diags);

        hm_diagnostics.extend(self.solve_class_constraints(&class_constraints));

        self.has_hm_diagnostics = hm_diagnostics
            .iter()
            .any(|d| d.severity() == crate::diagnostics::Severity::Error);

        // Proposal 0152, Phase 3: desugar named-field AST nodes into their
        // positional equivalents so every downstream phase (AST-fallback
        // bytecode, Core lowering, LLVM) sees only classic AST forms.
        {
            let (mut ctor_field_names, mut adt_variants) =
                collect_named_field_metadata(final_program.as_ref());
            // Proposal 0176: fold in named-field metadata from earlier REPL
            // lines' `data` declarations so a later line's `Point { x: .. }` /
            // `{ ...p, .. }` / `p.x` desugars to its positional form. Empty
            // outside the REPL.
            if !self.repl_session_adt_data.is_empty() {
                let (preloaded_field_names, preloaded_variants) =
                    collect_named_field_metadata_in_statements(&self.repl_session_adt_data);
                ctor_field_names.extend(preloaded_field_names);
                adt_variants.extend(preloaded_variants);
            }
            // fold in field order for constructors declared in
            // *imported* modules, which have no `data` statement in this
            // program. Inserted first-wins order: a locally declared
            // constructor of the same name keeps its own field order, matching
            // the shadowing behaviour everywhere else.
            for (ctor, fields) in &self.preloaded_ctor_field_names {
                ctor_field_names
                    .entry(*ctor)
                    .or_insert_with(|| fields.clone());
            }
            let mut ctx = NamedFieldDesugarCtx {
                ctor_field_names: &ctor_field_names,
                adt_variants: &adt_variants,
                hm_expr_types: &self.hm_expr_types,
            };
            let owned = final_program.to_mut();
            desugar_named_fields_in_program(owned, &mut ctx);
        }

        TypeInferenceResult {
            final_program,
            hm_diagnostics,
        }
    }

    /// Solve a program's class constraints, and make `evidence_map` the
    /// evidence for that program. Returns the solver's diagnostics.
    ///
    /// The map belongs to the program just inferred. A program that skips the
    /// solve raised nothing to answer, and must not keep another program's
    /// evidence: `ExprId`s restart per unit and move when modules are merged,
    /// so a stale entry would answer a different expression.
    pub(in crate::compiler) fn solve_class_constraints(
        &mut self,
        class_constraints: &WantedConstraints,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        self.evidence_map = crate::types::evidence::EvidenceMap::new();
        let mut solver_diags = Vec::new();

        // Type class constraint solving: verify that concrete-type constraints
        // have matching instances in the ClassEnv (Proposal 0145, Step 4).
        if !class_constraints.is_solved() && !self.class_env.classes.is_empty() {
            // Whole-program scope: generalization has already had its chance,
            // so nothing here is generalizable (Proposal 0179 Stage 3). Each
            // definition's scope is solved with the context its signature
            // promises, which is why the tree is passed rather than a list.
            let outcome = solve_wanted_tree(
                class_constraints,
                SolveScope::WholeProgram,
                &self.class_env,
                &self.interner,
            );
            outcome.trace_stuck(&self.interner);
            self.evidence_map = harvest_evidence(&self.class_env, &outcome);
            solver_diags.extend(outcome.into_diagnostics());
            tag_diagnostics(&mut solver_diags, DiagnosticPhase::TypeInference);
        }
        // Outside the solve: a definition whose body raised nothing leaves the
        // tree solved, and still has dictionary parameters.
        self.evidence_map
            .record_definitions(class_constraints, &self.class_env);
        if std::env::var("FLUX_DBG_EVIDENCE").is_ok() {
            self.dump_evidence_map();
        }
        solver_diags
    }
}

/// Collect the solver's evidence, keyed by the site that raised each predicate.
///
/// Only `Solved` predicates carry evidence: a `Generalized` one becomes a
/// dictionary *parameter* of the enclosing definition rather than an argument
/// at this site, and a `Stuck` or `Diagnosed` one has no instance to name.
///
/// The index within a site is the emission order of that site's predicates,
/// which is also the order its dictionaries are passed — so it doubles as the
/// argument position.
///
/// A marker predicate is recorded as `Marker` whether or not it was solved. A
/// marker class has no dictionary, so a site never passes one for it, and
/// lowering needs no answer to know that. Whether the predicate *holds* is the
/// solver's to report, and its diagnostics are not touched here.
fn harvest_evidence(
    class_env: &crate::types::class_env::ClassEnv,
    outcome: &crate::types::class_disposition::SolveOutcome,
) -> crate::types::evidence::EvidenceMap {
    use crate::types::class_disposition::{Disposition, Evidence};
    use crate::types::evidence::EvidenceMap;

    let mut map = EvidenceMap::new();
    for entry in &outcome.dispositions {
        let Some(expr) = entry.wanted.expr else {
            continue;
        };
        let is_marker = class_env
            .lookup_class_by_id(entry.wanted.class_id)
            .is_some_and(|class| class.methods.is_empty());
        // Raise every predicate the expression raised, not just the solved
        // ones. `EvidenceSite.index` is the argument position, so skipping an
        // unsolved predicate would slide every later dictionary one slot left.
        // The map also keeps the count, which is what lets
        // `EvidenceMap::args_for` refuse a partial argument list even when the
        // missing answer is the *last* one.
        let site = map.raise(expr, (&entry.wanted).into());
        if is_marker {
            map.insert(site, Evidence::Marker);
            continue;
        }
        let Disposition::Solved { evidence } = &entry.disposition else {
            continue;
        };
        map.insert(site, evidence.clone());
    }
    map
}

impl crate::compiler::Compiler {
    /// Dump the solver's evidence, one line per predicate, to stderr.
    ///
    /// Enabled by `FLUX_DBG_EVIDENCE`. This is the instrument for 0186 stage 5:
    /// the emitter reads exactly this map, so when a call site gets the wrong
    /// dictionary the first question is whether the solver recorded the wrong
    /// evidence or the emitter mistranslated right evidence, and these lines
    /// answer it directly.
    ///
    /// Sites print in `ExprId` order so two dumps can be diffed, and every
    /// raised position prints — an unanswered one as `(unsolved)` — because a
    /// hole is exactly what this dump exists to show.
    pub(in crate::compiler) fn dump_evidence_map(&self) {
        use crate::types::evidence::EvidenceSite;
        eprintln!(
            "EVIDENCE for {}: {} entries",
            self.file_path,
            self.evidence_map.len()
        );
        for (expr, raised) in self.evidence_map.raised_sites() {
            eprintln!("  site expr={expr:?} raised={raised}");
            for index in 0..raised {
                let site = EvidenceSite::new(expr, index);
                let kind = match self.evidence_map.get(&site) {
                    Some(evidence) => self.describe_evidence(evidence),
                    None => "(unsolved)".to_string(),
                };
                let raised_as = match self.evidence_map.predicate(&site) {
                    Some(p) => format!(
                        "{} {:?} @{}:{}",
                        self.interner.resolve(p.class_name),
                        p.origin,
                        p.span.start.line,
                        p.span.start.column
                    ),
                    None => "?".to_string(),
                };
                eprintln!("    idx={index} {raised_as} -> {kind}");
            }
        }
        for (id, definition) in self.evidence_map.definitions() {
            let params = definition
                .params
                .iter()
                .map(|param| self.interner.resolve(param.class_name).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "  def {id:?}: [{params}] param_of_given={:?}",
                definition.param_of_given
            );
        }
    }

    fn describe_evidence(&self, evidence: &crate::types::class_disposition::Evidence) -> String {
        use crate::types::class_disposition::Evidence;
        match evidence {
            Evidence::FromInstance {
                instance, context, ..
            } => format!(
                "FromInstance dict={:?} ctx={}",
                instance
                    .dict_name(&self.interner)
                    .map(|n| self.interner.resolve(n).to_string()),
                context.len()
            ),
            Evidence::FromGiven {
                given,
                owner,
                superclass_path,
            } => format!(
                "FromGiven class={} owner={:?}#{} path={:?}",
                self.interner.resolve(given.class_name),
                owner.definition,
                owner.index,
                superclass_path
            ),
            Evidence::Structural { .. } => "Structural".to_string(),
            Evidence::Marker => "Marker".to_string(),
            Evidence::Unrecorded => "Unrecorded".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::harvest_evidence;
    use crate::ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin};
    use crate::diagnostics::position::Span;
    use crate::syntax::expression::ExprId;
    use crate::syntax::symbol::Symbol;
    use crate::types::class_disposition::{
        Disposition, DispositionedConstraint, Evidence, InstanceKey, SolveOutcome, StuckReason,
    };
    use crate::types::class_env::ClassEnv;
    use crate::types::class_id::ClassId;
    use crate::types::evidence::EvidenceSite;
    use crate::types::infer_type::InferType;
    use crate::types::translate::DictArg;
    use crate::types::type_constructor::TypeConstructor;

    /// Symbols are only ever compared here, never resolved, so a raw index is
    /// safe — see `Symbol::new`.
    fn wanted(expr: ExprId) -> WantedClassConstraint {
        let name = Symbol::new(0);
        WantedClassConstraint {
            class_name: name,
            class_id: ClassId::from_local_name(name),
            type_args: vec![],
            span: Span::default(),
            expr: Some(expr),
            origin: WantedClassConstraintOrigin::MethodCall,
        }
    }

    fn outcome(dispositions: Vec<Disposition>, expr: ExprId) -> SolveOutcome {
        SolveOutcome {
            dispositions: dispositions
                .into_iter()
                .map(|disposition| DispositionedConstraint {
                    wanted: wanted(expr),
                    disposition,
                })
                .collect(),
        }
    }

    fn solved() -> Disposition {
        Disposition::Solved {
            evidence: Evidence::Structural { components: vec![] },
        }
    }

    fn stuck() -> Disposition {
        Disposition::Stuck {
            reason: StuckReason::OuterScopeVariable,
        }
    }

    /// `Channel.make(5)` on a channel nothing is ever sent on leaves
    /// `Sendable<?a>` unsolved. A marker takes no argument whether or not it
    /// holds, so the site still knows what to pass: nothing.
    #[test]
    fn an_unsolved_marker_is_recorded_as_a_marker() {
        let mut interner = crate::syntax::interner::Interner::new();
        let mut class_env = ClassEnv::new();
        class_env.register_builtins(&mut interner);
        let sendable = interner.lookup("Sendable").expect("Sendable is built in");
        let expr = ExprId(5);
        let marker_wanted = WantedClassConstraint {
            class_name: sendable,
            class_id: ClassId::from_local_name(sendable),
            ..wanted(expr)
        };
        let unsolved = SolveOutcome {
            dispositions: vec![DispositionedConstraint {
                wanted: marker_wanted,
                disposition: stuck(),
            }],
        };

        let map = harvest_evidence(&class_env, &unsolved);

        assert_eq!(
            map.get(&EvidenceSite::new(expr, 0)),
            Some(&Evidence::Marker)
        );
        assert_eq!(map.dict_args_at(expr), Some(Vec::new()));
    }

    #[test]
    fn an_unsolved_predicate_does_not_shift_the_ones_after_it() {
        // `index` is the argument position. Counting only solved predicates
        // would put this evidence at slot 0, which is the slot the *stuck*
        // predicate's dictionary occupies.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&ClassEnv::new(), &outcome(vec![stuck(), solved()], expr));

        assert!(map.get(&EvidenceSite::new(expr, 0)).is_none());
        assert!(map.get(&EvidenceSite::new(expr, 1)).is_some());
    }

    #[test]
    fn a_site_with_a_hole_refuses_to_build_an_argument_list() {
        // `args_for` returns None rather than a short list, so a caller cannot
        // emit a call with the wrong arity. Compacting the indices would defeat
        // that guard by making the list dense.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&ClassEnv::new(), &outcome(vec![stuck(), solved()], expr));

        assert!(map.args_for(expr).is_none());
    }

    #[test]
    fn a_site_with_a_hole_at_the_tail_refuses_to_build_an_argument_list() {
        // The answered predicates alone look like a site that raised one. Only
        // the raised count shows the second is missing; deriving the count
        // from present keys returned `Some([ev0])` here — a short list.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&ClassEnv::new(), &outcome(vec![solved(), stuck()], expr));

        assert_eq!(map.raised(expr), 2);
        assert!(map.args_for(expr).is_none());
    }

    #[test]
    fn a_fully_solved_site_still_yields_every_argument_in_order() {
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&ClassEnv::new(), &outcome(vec![solved(), solved()], expr));

        assert_eq!(map.args_for(expr).map(|args| args.len()), Some(2));
    }

    fn solved_with(evidence: Evidence) -> Disposition {
        Disposition::Solved { evidence }
    }

    fn int_instance() -> InstanceKey {
        InstanceKey {
            class_id: ClassId::from_local_name(Symbol::new(0)),
            head_type_args: vec![InferType::Con(TypeConstructor::Int)],
            dict_type_key: "Int".to_string(),
        }
    }

    #[test]
    fn a_site_that_raised_nothing_passes_no_dictionaries() {
        // Every identifier is asked. Raising nothing is "pass nothing", not
        // "cannot build", which is what `args_for` answers here.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(&ClassEnv::new(), &outcome(vec![], expr));

        assert_eq!(map.dict_args_at(expr), Some(vec![]));
    }

    #[test]
    fn a_marker_takes_an_index_but_no_argument() {
        // A dictionary's position is its index among the non-marker
        // predicates, so the marker at index 0 leaves one argument, not two.
        let expr = ExprId::UNSET;
        let map = harvest_evidence(
            &ClassEnv::new(),
            &outcome(
                vec![
                    solved_with(Evidence::Marker),
                    solved_with(Evidence::FromInstance {
                        instance: int_instance(),
                        subst: HashMap::new(),
                        context: vec![],
                    }),
                ],
                expr,
            ),
        );

        assert_eq!(
            map.dict_args_at(expr),
            Some(vec![DictArg::Global {
                instance: int_instance()
            }])
        );
    }

    #[test]
    fn a_hole_anywhere_yields_no_argument_list() {
        let expr = ExprId::UNSET;
        let marker = || solved_with(Evidence::Marker);
        let tail = harvest_evidence(&ClassEnv::new(), &outcome(vec![marker(), stuck()], expr));
        let head = harvest_evidence(&ClassEnv::new(), &outcome(vec![stuck(), marker()], expr));

        assert_eq!(tail.dict_args_at(expr), None);
        assert_eq!(head.dict_args_at(expr), None);
    }

    #[test]
    fn unbuildable_evidence_yields_no_argument_list() {
        // Every position is answered, so `args_for` succeeds; the fold over
        // the answer is what fails, and that must not become "pass nothing".
        let expr = ExprId::UNSET;
        let map = harvest_evidence(
            &ClassEnv::new(),
            &outcome(vec![solved_with(Evidence::Unrecorded)], expr),
        );

        assert!(map.args_for(expr).is_some());
        assert_eq!(map.dict_args_at(expr), None);
    }
}
