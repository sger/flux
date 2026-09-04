//! Dispositions for wanted class predicates (Proposal 0179, Stage 3).
//!
//! Proposal 0179 Goal 2 requires that every class obligation is "solved,
//! generalized, represented as a documented stuck predicate, or reported as
//! an error—never silently discarded". Before Stage 3 the solver returned
//! only `Vec<Diagnostic>`, so a predicate that solved, one that was still
//! polymorphic, and one that was skipped for a synthetic span were all
//! indistinguishable at the output — every `continue` in the solve loop was
//! a silent drop.
//!
//! This module gives each [`WantedClassConstraint`] exactly one
//! [`Disposition`]. The enum deliberately has no "dropped" variant: a
//! predicate the solver cannot decide becomes [`Disposition::Stuck`] with a
//! recorded [`StuckReason`], which is a *documented* state rather than an
//! absence.
//!
//! ## Relationship to Haskell
//!
//! The vocabulary follows "Typing Haskell in Haskell" (Jones, 2000), whose
//! `ClassEnv`/`Pred`/`Qual` model maps closely onto Flux's
//! `ClassEnv`/`SchemeConstraint`/`Scheme`. [`Disposition::Generalized`] and
//! [`Disposition::Stuck`] are the two halves of THIH's `split`, and
//! [`Evidence::FromInstance`] retains what THIH's `byInst` returns (the
//! instance context as further subgoals) instead of collapsing it to a
//! boolean.

use std::collections::HashMap;

use crate::{
    ast::type_infer::constraint::{SchemeConstraint, WantedClassConstraint},
    diagnostics::Diagnostic,
    syntax::{Identifier, interner::Interner},
    types::{class_id::ClassId, infer_type::InferType},
};

/// Identifies the instance that discharged a predicate.
///
/// Deliberately an owned key rather than a borrow of the matched
/// `InstanceDef`: evidence travels through `FinalizedBindingClassConstraints`
/// and `InferProgramResult`, and a lifetime there would eventually infect the
/// module cache. Keyed on [`ClassId`] rather than a bare class name so that
/// Stage 4's `ClassId`-aware lookup does not have to re-plumb it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceKey {
    /// Canonical identity of the class this instance implements.
    pub class_id: ClassId,
    /// The instance head's type arguments.
    pub head_type_args: Vec<InferType>,
}

/// How a predicate was discharged.
///
/// Corresponds to THIH's notion of evidence: `FromInstance` is `byInst`
/// (including the instance context as recursive subgoals), and `Structural`
/// covers the compiler's built-in structural rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// Matched a declared `instance`.
    FromInstance {
        /// Which instance matched.
        instance: InstanceKey,
        /// The instance-head substitution produced while matching.
        subst: HashMap<Identifier, InferType>,
        /// Evidence for each predicate in the instance's context, in
        /// declaration order. Empty for a context-free instance.
        context: Vec<Evidence>,
    },
    /// Discharged by a built-in structural rule, such as `Eq<(a, b)>` from
    /// `Eq<a>` and `Eq<b>`.
    Structural {
        /// Evidence for each component the structural rule decomposed into.
        components: Vec<Evidence>,
    },
    /// Discharged by the context the enclosing scope already holds.
    ///
    /// THIH's `bySuper`: inside `fn f<a: Ord>(..)` the predicate `Ord<a>` — and
    /// `Eq<a>`, which `Ord` implies — is evidence the caller supplies, so the
    /// body owes nothing. Nothing is resolved here; the dictionary arrives as a
    /// parameter, and `superclass_path` says which slot to project when the
    /// given is a subclass of what was wanted.
    FromGiven {
        /// The scheme predicate that discharged it.
        given: SchemeConstraint,
        /// Slot path from the given's dictionary to the wanted evidence, empty
        /// when the given *is* the wanted predicate.
        superclass_path: Vec<usize>,
    },
    /// A marker class carries no dictionary, so there is nothing to pass.
    Marker,
    /// An instance was found, but the details were not recorded.
    ///
    /// Stage 3 establishes that a predicate *was* discharged; Stage 4
    /// ("Deterministic evidence resolution") replaces this with the instance
    /// and substitution that matching already computes, so dictionary
    /// elaboration can stop re-resolving. Distinct from [`Evidence::Marker`],
    /// which is a positive claim that no dictionary exists.
    Unrecorded,
}

/// Why a predicate could not be decided at the scope where it was solved.
///
/// A stuck predicate is *not* an error at binding scope — it is handed back
/// to a wider scope. One that is still stuck at whole-program scope is an
/// error, which is what stops this variant from becoming a renamed silent
/// drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StuckReason {
    /// A type argument still mentions a unification variable owned by an
    /// enclosing binding, so this binding may not generalize it.
    OuterScopeVariable,
    /// The class is absent from the environment. Already reported as E441 by
    /// class-environment validation; recorded so the predicate is accounted
    /// for rather than dropped.
    UnknownClass,
    /// Emitted by compiler-generated code (dispatch shims and similar), which
    /// carries a default span and so is not attributable to user source.
    SyntheticOrigin,
    /// A type argument was still polymorphic at whole-program scope, where
    /// nothing remains to generalize it. Distinct from
    /// [`StuckReason::OuterScopeVariable`], which is recoverable by an
    /// enclosing binding.
    UnresolvedAfterGeneralization,
}

/// The outcome assigned to exactly one wanted class predicate.
///
/// There is no "dropped" variant, by design.
#[derive(Debug, Clone)]
pub enum Disposition {
    /// A matching instance was found.
    Solved {
        /// How the predicate was discharged.
        evidence: Evidence,
    },
    /// The predicate mentions a variable the enclosing binding quantifies, so
    /// it becomes a `SchemeConstraint` and its obligation transfers to every
    /// call site. This is the retained half of THIH's `split`.
    Generalized {
        /// The constraint as attached to the binding's scheme.
        scheme_constraint: SchemeConstraint,
    },
    /// Undecidable at this scope; handed back to a wider one. The deferred
    /// half of THIH's `split`.
    Stuck {
        /// Why it could not be decided here.
        reason: StuckReason,
    },
    /// A definite error. The diagnostic is carried so callers need not
    /// re-derive it.
    Diagnosed {
        /// The reported diagnostic.
        diagnostic: Box<Diagnostic>,
    },
}

impl Disposition {
    /// Whether this disposition represents a reported error.
    pub fn is_error(&self) -> bool {
        matches!(self, Disposition::Diagnosed { .. })
    }

    /// The diagnostic carried by an errored disposition, if any.
    pub fn diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            Disposition::Diagnosed { diagnostic } => Some(diagnostic),
            _ => None,
        }
    }
}

/// A wanted predicate paired with the outcome assigned to it.
#[derive(Debug, Clone)]
pub struct DispositionedConstraint {
    /// The predicate as collected by inference.
    pub wanted: WantedClassConstraint,
    /// The outcome assigned to it.
    pub disposition: Disposition,
}

/// The result of solving a set of wanted predicates.
///
/// Upholds the Stage 3 invariant that the number of dispositions equals the
/// number of wanted predicates supplied — the mechanical form of "never
/// silently discarded".
#[derive(Debug, Clone, Default)]
pub struct SolveOutcome {
    /// One entry per wanted predicate, in the order they were supplied.
    pub dispositions: Vec<DispositionedConstraint>,
}

impl SolveOutcome {
    /// Every diagnostic produced, in wanted order.
    ///
    /// Borrows rather than allocating; callers that need owned diagnostics
    /// can `.cloned()`, and callers feeding an existing collection can
    /// `.extend()` without an intermediate `Vec`.
    pub fn diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.dispositions
            .iter()
            .filter_map(|entry| entry.disposition.diagnostic())
    }

    /// Consume the outcome, yielding its diagnostics by value.
    ///
    /// Preferred over `diagnostics().cloned()` where the outcome is no longer
    /// needed: the diagnostics are moved out instead of duplicated.
    pub fn into_diagnostics(self) -> impl Iterator<Item = Diagnostic> {
        self.dispositions
            .into_iter()
            .filter_map(|entry| match entry.disposition {
                Disposition::Diagnosed { diagnostic } => Some(*diagnostic),
                _ => None,
            })
    }

    /// The predicates that could not be decided at this scope.
    /// Print the terminal stuck set to stderr when `FLUX_STUCK_TRACE` is set.
    ///
    /// Proposal 0183 is measured, not argued: escalating a state whose size
    /// nobody knows is how it stayed inert through 0179. Each refactor has to
    /// report what it left behind, and a count nobody can reproduce is not a
    /// measurement — so this ships rather than living in a scratch patch.
    ///
    /// One line per `(reason, class, origin)` group with its count, most
    /// frequent first.
    pub fn trace_stuck(&self, interner: &Interner) {
        if std::env::var_os("FLUX_STUCK_TRACE").is_none() {
            return;
        }
        let mut groups: HashMap<(String, String, String), usize> = HashMap::new();
        for (entry, reason) in self.stuck() {
            let key = (
                format!("{reason:?}"),
                interner.resolve(entry.wanted.class_name).to_string(),
                format!("{:?}", entry.wanted.origin),
            );
            *groups.entry(key).or_default() += 1;
        }
        let mut rows: Vec<_> = groups.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let total: usize = rows.iter().map(|(_, count)| count).sum();
        eprintln!("[stuck] {total} terminal stuck predicate(s)");
        for ((reason, class, origin), count) in rows {
            eprintln!("[stuck] {count:>6}  {reason:<30} {class:<12} {origin}");
        }

        // `FLUX_STUCK_TRACE=full` adds one line per predicate with its type
        // arguments, which is what tells a genuinely undecidable predicate
        // apart from one the classifier merely mis-shelved.
        if std::env::var_os("FLUX_STUCK_TRACE").is_some_and(|v| v == "full") {
            for (entry, reason) in self.stuck() {
                let args = entry
                    .wanted
                    .type_args
                    .iter()
                    .map(|ty| crate::ast::type_infer::display_infer_type(ty, interner))
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!(
                    "[stuck]   {reason:?} {}<{args}> at {}:{}",
                    interner.resolve(entry.wanted.class_name),
                    entry.wanted.span.start.line,
                    entry.wanted.span.start.column,
                );
            }
        }
    }

    pub fn stuck(&self) -> impl Iterator<Item = (&DispositionedConstraint, StuckReason)> {
        self.dispositions
            .iter()
            .filter_map(|entry| match entry.disposition {
                Disposition::Stuck { reason } => Some((entry, reason)),
                _ => None,
            })
    }

    /// The predicates that were discharged, with their evidence.
    pub fn evidence(&self) -> impl Iterator<Item = (&WantedClassConstraint, &Evidence)> {
        self.dispositions
            .iter()
            .filter_map(|entry| match &entry.disposition {
                Disposition::Solved { evidence } => Some((&entry.wanted, evidence)),
                _ => None,
            })
    }

    /// The constraints retained on the enclosing binding's scheme.
    pub fn generalized(&self) -> impl Iterator<Item = &SchemeConstraint> {
        self.dispositions
            .iter()
            .filter_map(|entry| match &entry.disposition {
                Disposition::Generalized { scheme_constraint } => Some(scheme_constraint),
                _ => None,
            })
    }
}

/// The scope a set of predicates is being solved at.
///
/// The two solve call sites ran identical logic before Stage 3, which is why
/// the whole-program pass could only repeat what the per-binding pass had
/// already said. Distinguishing them is what makes ambiguity detection
/// possible: ambiguity is a judgment about a variable that survived
/// generalization, so it can only be made once generalization has had its
/// chance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveScope {
    /// Solving within a single binding, before generalization.
    Binding,
    /// The final whole-program pass. Nothing remains generalizable here.
    WholeProgram,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::type_infer::constraint::WantedClassConstraintOrigin;
    use crate::diagnostics::compiler_errors::NO_INSTANCE;
    use crate::diagnostics::diagnostic_for;
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;
    use crate::types::type_constructor::TypeConstructor;

    fn wanted(class_name: Identifier) -> WantedClassConstraint {
        WantedClassConstraint {
            class_name,
            class_id: crate::types::class_id::ClassId::from_local_name(class_name),
            type_args: vec![InferType::Con(TypeConstructor::Int)],
            span: Span::default(),
            origin: WantedClassConstraintOrigin::ExplicitBound,
        }
    }

    #[test]
    fn outcome_partitions_dispositions_without_loss() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let num = interner.intern("Num");
        let show = interner.intern("Show");
        let ord = interner.intern("Ord");

        let outcome = SolveOutcome {
            dispositions: vec![
                DispositionedConstraint {
                    wanted: wanted(eq),
                    disposition: Disposition::Solved {
                        evidence: Evidence::Marker,
                    },
                },
                DispositionedConstraint {
                    wanted: wanted(num),
                    disposition: Disposition::Generalized {
                        scheme_constraint: SchemeConstraint {
                            class_name: num,
                            class_id: crate::types::class_id::ClassId::from_local_name(num),
                            type_args: vec![InferType::Var(0)],
                        },
                    },
                },
                DispositionedConstraint {
                    wanted: wanted(show),
                    disposition: Disposition::Stuck {
                        reason: StuckReason::OuterScopeVariable,
                    },
                },
                DispositionedConstraint {
                    wanted: wanted(ord),
                    disposition: Disposition::Diagnosed {
                        diagnostic: Box::new(diagnostic_for(&NO_INSTANCE)),
                    },
                },
            ],
        };

        // Every predicate is accounted for by exactly one accessor.
        assert_eq!(outcome.evidence().count(), 1);
        assert_eq!(outcome.generalized().count(), 1);
        assert_eq!(outcome.stuck().count(), 1);
        assert_eq!(outcome.diagnostics().count(), 1);
        assert_eq!(
            outcome.evidence().count()
                + outcome.generalized().count()
                + outcome.stuck().count()
                + outcome.diagnostics().count(),
            outcome.dispositions.len(),
            "every disposition must fall into exactly one category"
        );
    }

    #[test]
    fn stuck_reports_its_reason() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let outcome = SolveOutcome {
            dispositions: vec![DispositionedConstraint {
                wanted: wanted(eq),
                disposition: Disposition::Stuck {
                    reason: StuckReason::UnknownClass,
                },
            }],
        };
        let reasons: Vec<StuckReason> = outcome.stuck().map(|(_, reason)| reason).collect();
        assert_eq!(reasons, vec![StuckReason::UnknownClass]);
    }

    #[test]
    fn into_diagnostics_moves_without_cloning() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let outcome = SolveOutcome {
            dispositions: vec![
                DispositionedConstraint {
                    wanted: wanted(eq),
                    disposition: Disposition::Solved {
                        evidence: Evidence::Unrecorded,
                    },
                },
                DispositionedConstraint {
                    wanted: wanted(eq),
                    disposition: Disposition::Diagnosed {
                        diagnostic: Box::new(diagnostic_for(&NO_INSTANCE)),
                    },
                },
            ],
        };
        let borrowed = outcome.diagnostics().count();
        let owned: Vec<Diagnostic> = outcome.into_diagnostics().collect();
        assert_eq!(borrowed, 1);
        assert_eq!(owned.len(), 1);
    }

    #[test]
    fn only_diagnosed_is_an_error() {
        assert!(
            Disposition::Diagnosed {
                diagnostic: Box::new(diagnostic_for(&NO_INSTANCE))
            }
            .is_error()
        );
        assert!(
            !Disposition::Solved {
                evidence: Evidence::Marker
            }
            .is_error()
        );
        assert!(
            !Disposition::Stuck {
                reason: StuckReason::SyntheticOrigin
            }
            .is_error()
        );
    }
}
