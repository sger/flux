//! Evidence to dictionary argument.
//!
//! The one place that turns the solver's [`Evidence`] into the argument a call
//! site passes. It resolves nothing: every choice was already made when the
//! predicate was discharged, and this is a structural fold over that answer.
//!
//! That is the point. Six places in `core/` and `compiler/` each re-derived
//! which instance a call uses, from the class environment, at a point where the
//! call site's instantiation was no longer available — and they were kept in
//! agreement by hand-written comments saying they "must stay in lockstep".
//!
//! The output is deliberately backend-neutral. A [`DictArg`] names *what* to
//! pass; turning that into a `CoreExpr` is the caller's job, because `core/`
//! may not be a dependency of the type system.
//!
//! **If this module ever needs to consult the class environment to make a
//! common case work, the design is wrong.** Once is a missing solver decision;
//! twice means the solver is not in a position to decide, and the honest
//! response is to find out why rather than add a fallback — that is how
//! `choose_candidate`'s "recovers with the first candidate rather than failing"
//! came to sit in a correctness-critical path.
//!
//! See `docs/proposals/0186_generics_foundations.md`.

use crate::types::{
    class_disposition::{Evidence, InstanceKey},
    infer_type::InferType,
};

/// What a call site passes to discharge one predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictArg {
    /// A named instance dictionary, defined at module scope.
    Global {
        /// The instance whose dictionary this is.
        instance: InstanceKey,
    },
    /// A dictionary the enclosing definition received as a parameter, reached
    /// by projecting `path` from it.
    ///
    /// `path` is empty when the parameter *is* the wanted evidence, and names
    /// superclass slots outermost-first otherwise: `[0, 1]` is slot 1 of the
    /// superclass evidence held in slot 0.
    Param {
        /// Which of the definition's predicates supplies it.
        index: usize,
        path: Vec<usize>,
    },
    /// An instance with a context: its dictionary applied to the evidence for
    /// each predicate in that context, in declaration order.
    Applied {
        instance: InstanceKey,
        context: Vec<DictArg>,
    },
    /// Nothing is passed — a marker class carries no dictionary.
    None,
}

/// Translate one piece of evidence into the argument that discharges it.
///
/// `givens` are the predicates the enclosing definition quantified, in the
/// order it receives their dictionaries, so a [`Evidence::FromGiven`] can name
/// a parameter position rather than a type.
///
/// Returns `None` only when the solver did not record enough to act on:
/// [`Evidence::Unrecorded`], or a structural rule whose components have no
/// dictionary representation yet. A caller must treat that as "cannot build
/// this argument" and leave the site alone, never as "pass nothing".
pub fn evidence_to_arg(
    evidence: &Evidence,
    givens: &[crate::ast::type_infer::constraint::SchemeConstraint],
) -> Option<DictArg> {
    match evidence {
        Evidence::Marker => Some(DictArg::None),

        Evidence::FromInstance {
            instance, context, ..
        } => {
            if context.is_empty() {
                return Some(DictArg::Global {
                    instance: instance.clone(),
                });
            }
            let context = context
                .iter()
                .map(|inner| evidence_to_arg(inner, givens))
                .collect::<Option<Vec<_>>>()?;
            Some(DictArg::Applied {
                instance: instance.clone(),
                context,
            })
        }

        Evidence::FromGiven {
            given,
            superclass_path,
        } => {
            let index = givens.iter().position(|candidate| {
                candidate.class_id == given.class_id && candidate.type_args == given.type_args
            })?;
            Some(DictArg::Param {
                index,
                path: superclass_path.clone(),
            })
        }

        // A structural rule — `Eq<(a, b)>` from `Eq<a>` and `Eq<b>` — has no
        // `InstanceDef` and so no `__dict_*` global to name. Representing one
        // at runtime is an open question (proposal 0186), and answering it by
        // guessing an instance here is exactly what this module must not do.
        Evidence::Structural { .. } => None,

        // The solver established that an instance exists without recording
        // which. Nothing can be built from that.
        Evidence::Unrecorded => None,
    }
}

/// The type arguments an instance's dictionary is named by.
///
/// Exposed so a caller can build the dictionary's symbol without reaching into
/// [`InstanceKey`]'s shape.
pub fn instance_head_args(instance: &InstanceKey) -> &[InferType] {
    &instance.head_type_args
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{DictArg, evidence_to_arg, instance_head_args};
    use crate::{
        ast::type_infer::constraint::SchemeConstraint,
        syntax::interner::Interner,
        types::{
            class_disposition::{Evidence, InstanceKey},
            class_id::ClassId,
            infer_type::InferType,
            type_constructor::TypeConstructor,
        },
    };

    fn key(interner: &mut Interner, class: &str, arg: InferType) -> InstanceKey {
        let name = interner.intern(class);
        InstanceKey {
            class_id: ClassId::from_local_name(name),
            head_type_args: vec![arg],
        }
    }

    #[test]
    fn a_context_free_instance_becomes_its_global_dictionary() {
        let mut interner = Interner::new();
        let instance = key(&mut interner, "Eq", InferType::Con(TypeConstructor::Int));
        let arg = evidence_to_arg(
            &Evidence::FromInstance {
                instance: instance.clone(),
                subst: HashMap::new(),
                context: Vec::new(),
            },
            &[],
        )
        .expect("a context-free instance is nameable");

        assert_eq!(arg, DictArg::Global { instance });
    }

    /// `instance Eq<List<a>> => Eq<a>` is its dictionary *applied* to the
    /// evidence for its context, not a bare global.
    #[test]
    fn an_instance_with_a_context_applies_its_context_evidence() {
        let mut interner = Interner::new();
        let outer = key(
            &mut interner,
            "Eq",
            InferType::App(
                TypeConstructor::List,
                vec![InferType::Con(TypeConstructor::Int)],
            ),
        );
        let inner = key(&mut interner, "Eq", InferType::Con(TypeConstructor::Int));

        let arg = evidence_to_arg(
            &Evidence::FromInstance {
                instance: outer.clone(),
                subst: HashMap::new(),
                context: vec![Evidence::FromInstance {
                    instance: inner.clone(),
                    subst: HashMap::new(),
                    context: Vec::new(),
                }],
            },
            &[],
        )
        .expect("both halves are nameable");

        assert_eq!(
            arg,
            DictArg::Applied {
                instance: outer,
                context: vec![DictArg::Global { instance: inner }],
            }
        );
    }

    /// A predicate the caller already holds names the parameter position it
    /// arrives in — the whole reason `givens` is passed in order.
    #[test]
    fn a_given_names_the_parameter_that_carries_it() {
        let mut interner = Interner::new();
        let show = interner.intern("Show");
        let ord = interner.intern("Ord");
        let givens = vec![
            SchemeConstraint {
                class_name: show,
                class_id: ClassId::from_local_name(show),
                type_args: vec![InferType::Var(0)],
            },
            SchemeConstraint {
                class_name: ord,
                class_id: ClassId::from_local_name(ord),
                type_args: vec![InferType::Var(0)],
            },
        ];

        let arg = evidence_to_arg(
            &Evidence::FromGiven {
                given: givens[1].clone(),
                superclass_path: vec![0],
            },
            &givens,
        )
        .expect("the given is in scope");

        assert_eq!(
            arg,
            DictArg::Param {
                index: 1,
                path: vec![0]
            }
        );
    }

    /// Evidence the solver did not record cannot be turned into an argument —
    /// and must not be turned into "pass nothing", which would build a call of
    /// the wrong arity.
    #[test]
    fn unrecorded_and_structural_evidence_yield_no_argument() {
        assert_eq!(evidence_to_arg(&Evidence::Unrecorded, &[]), None);
        assert_eq!(
            evidence_to_arg(
                &Evidence::Structural {
                    components: Vec::new()
                },
                &[]
            ),
            None
        );
    }

    #[test]
    fn a_marker_class_passes_nothing() {
        let mut interner = Interner::new();
        let _ = key(
            &mut interner,
            "Sendable",
            InferType::Con(TypeConstructor::Int),
        );
        assert_eq!(evidence_to_arg(&Evidence::Marker, &[]), Some(DictArg::None));
    }

    #[test]
    fn head_args_are_reachable_for_naming_a_dictionary() {
        let mut interner = Interner::new();
        let instance = key(&mut interner, "Eq", InferType::Con(TypeConstructor::Int));
        assert_eq!(
            instance_head_args(&instance),
            &[InferType::Con(TypeConstructor::Int)]
        );
    }
}
