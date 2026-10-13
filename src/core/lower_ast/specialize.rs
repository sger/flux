//! Which generalized functions are specialised, and how (0187 B1, and the
//! 0.0.8 plan's step 2a).
//!
//! A generalized definition loses the concrete parameter type an
//! instantiation gives it, and with it the `FluxRep` that lets Aether prove a
//! `DropSpecialized` unnecessary (KI-098). The type is still recorded at each
//! call site, so it is read back from there:
//!
//! - **In place** (B1): every call site agrees on one ground instantiation, so
//!   the definition itself is lowered under it.
//! - **Cloned** (2a): the call sites disagree. Each ground instantiation that
//!   changes a representation gets its own clone, `f$<key>`, and its call
//!   sites are pointed at it. The generic body stays, for the rest.

use std::collections::HashMap;

use crate::{
    syntax::{Identifier, expression::ExprId, interner::Interner, program::Program},
    types::{TypeVarId, infer_type::InferType, type_env::TypeEnv, type_subst::TypeSubst},
};

use super::{AstLowerer, CallSiteInstantiations};
use crate::core::FluxRep;

/// At most this many clones of one function (GHC's SpecConstr default).
/// Past it, callers use the generic body.
pub(super) const MAX_CLONES_PER_FUNCTION: usize = 3;

/// What [`AstLowerer::plan_specializations`] decided for a program.
#[derive(Default)]
pub(super) struct SpecPlan {
    /// Functions lowered in place, under their single instantiation.
    pub(super) in_place: HashMap<Identifier, TypeSubst>,
    /// Functions cloned per instantiation, in a stable order.
    pub(super) clones: Vec<(Identifier, Vec<PlannedClone>)>,
    /// Instantiations over [`MAX_CLONES_PER_FUNCTION`], which stay generic.
    pub(super) missed: Vec<(Identifier, InferType)>,
}

/// One clone the plan asks for, before its name is interned.
pub(super) struct PlannedClone {
    /// `f$<key>`.
    pub(super) name: String,
    /// The instantiation, restricted to the variables whose representation
    /// it changes. A boxed variable stays a variable: every boxed type has
    /// the same representation, so the clone is honest for all of them.
    pub(super) subst: TypeSubst,
    /// The callee `ExprId` of every call site that uses this clone.
    pub(super) sites: Vec<ExprId>,
    /// One instantiation that reaches this clone, for logging.
    pub(super) example: InferType,
}

/// A clone lowering emits: its interned name and its substitution.
#[derive(Clone)]
pub(super) struct SpecClone {
    pub(super) name: Identifier,
    pub(super) subst: TypeSubst,
}

/// The clones lowering will emit for `program`: each cloned function and the
/// name of one of its clones.
///
/// Lowering only holds a shared interner, so a clone's name must be interned
/// beforehand, the way `pre_intern_dict_names` does for dictionaries. A name
/// that is not interned is a clone lowering leaves out, and its call sites use
/// the generic body.
pub fn specialization_clone_names(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: &Interner,
    type_env: &TypeEnv,
) -> Vec<(Identifier, String)> {
    let mut lowerer = AstLowerer::new(
        hm_expr_types,
        Some(interner),
        Some(type_env),
        None,
        None,
        None,
    );
    lowerer.collect_local_function_names(program);
    lowerer
        .plan_specializations(program)
        .clones
        .into_iter()
        .flat_map(|(function, clones)| clones.into_iter().map(move |clone| (function, clone.name)))
        .collect()
}

/// The key of a clone for the instantiation `subst` of the variables
/// `forall`, and that instantiation restricted to what the key records.
///
/// The key spells each variable's representation: `Int`, `Float` and `Bool`
/// are unboxed and each its own, and everything else is `Box`. `String` and
/// `List<Int>` therefore share a key, and `Int` and either of them do not.
/// `None` when every variable is boxed, which is what the generic body
/// already is.
fn clone_key(forall: &[TypeVarId], subst: &TypeSubst) -> Option<(String, TypeSubst)> {
    let mut parts = Vec::with_capacity(forall.len());
    let mut kept = TypeSubst::empty();
    for var in forall {
        let ty = subst.get(*var);
        let part = match ty.map(FluxRep::from_infer_type) {
            Some(FluxRep::IntRep) => "Int",
            Some(FluxRep::FloatRep) => "Float",
            Some(FluxRep::BoolRep) => "Bool",
            _ => {
                parts.push("Box");
                continue;
            }
        };
        parts.push(part);
        if let Some(ty) = ty {
            kept.insert(*var, ty.clone());
        }
    }
    (!kept.is_empty()).then(|| (parts.join("_"), kept))
}

impl AstLowerer<'_> {
    /// Decide, for every user-written function, whether it is specialised in
    /// place, cloned per instantiation, or left generic.
    ///
    /// **Self-calls do not count.** A recursive occurrence is at the
    /// definition's own type by construction, so it is never a second
    /// instantiation, and counting it would exclude every recursive function.
    /// A clone's self-calls follow the clone instead.
    pub(super) fn plan_specializations(&self, program: &Program) -> SpecPlan {
        let mut visitor = CallSiteInstantiations {
            hm_expr_types: self.hm_expr_types,
            user_fns: &self.user_function_names,
            enclosing: None,
            sites: HashMap::new(),
            params: HashMap::new(),
            constrained: std::collections::HashSet::new(),
            param_types: HashMap::new(),
        };
        crate::ast::visit::Visitor::visit_program(&mut visitor, program);

        // Only a top-level function is cloned for now: a module member's
        // clone needs a module-qualified name (step 2a-4).
        let top_level: std::collections::HashSet<Identifier> = program
            .statements
            .iter()
            .filter_map(|stmt| match stmt {
                crate::syntax::statement::Statement::Function { name, .. } => Some(*name),
                _ => None,
            })
            .collect();
        let mut plan = SpecPlan::default();
        let mut names: Vec<Identifier> = visitor.sites.keys().copied().collect();
        names.sort_unstable_by_key(|name| name.as_u32());
        for name in names {
            let sites = &visitor.sites[&name];
            // A function that receives dictionaries is never specialised here.
            //
            // Its body's types are what several sites consult to decide which
            // instance a class-method call means, while the dictionaries it is
            // handed are positional and fixed by its signature. Rewriting the
            // former without the latter makes the two disagree — verified: it
            // sends `result_directed_two_dictionaries.flx` to the wrong
            // instance and it prints `7` for a `String`.
            if visitor.constrained.contains(&name) {
                continue;
            }
            let first = &sites[0].1;
            if sites.iter().all(|(_, ty)| ty == first) {
                if let Some(subst) = self.in_place_subst(name, first, &visitor) {
                    plan.in_place.insert(name, subst);
                }
                continue;
            }
            if top_level.contains(&name) {
                self.plan_clones(name, sites, &mut plan);
            }
        }
        plan
    }

    /// B1: the substitution that lowers `name` in place at its one
    /// instantiation `call_ty`, if it buys anything.
    fn in_place_subst(
        &self,
        name: Identifier,
        call_ty: &InferType,
        visitor: &CallSiteInstantiations<'_>,
    ) -> Option<TypeSubst> {
        // Only a fully concrete instantiation is worth anything: a
        // substitution mapping one variable to another buys no rep.
        if !call_ty.free_vars().is_empty() {
            return None;
        }
        let generic = self
            .type_env
            .and_then(|env| env.lookup(name))
            .map(|scheme| scheme.infer_type.clone());
        let subst = match generic {
            // A definition with nothing to quantify is already specialised.
            Some(generic) if generic.free_vars().is_empty() => return None,
            Some(generic) => crate::types::unify::unify(&generic, call_ty).ok(),
            None => {
                Self::subst_from_parameters(name, call_ty, &visitor.params, &visitor.param_types)
            }
        }?;
        (!subst.is_empty()).then_some(subst)
    }

    /// 2a: one clone of `name` per ground instantiation whose key differs,
    /// up to the cap.
    ///
    /// A call site whose instantiation is not ground (it sits in another
    /// generic body) keeps the generic definition.
    fn plan_clones(&self, name: Identifier, sites: &[(ExprId, InferType)], plan: &mut SpecPlan) {
        let (Some(interner), Some(scheme)) = (
            self.interner,
            self.type_env.and_then(|env| env.lookup(name)),
        ) else {
            return;
        };
        // A scheme with constraints takes dictionaries, whether or not its
        // source writes the bound; cloning it means replacing those too
        // (step 2a-3).
        if scheme.forall.is_empty() || !scheme.constraints.is_empty() {
            return;
        }
        let Some(base) = interner.try_resolve(name) else {
            return;
        };
        let mut clones: Vec<PlannedClone> = Vec::new();
        for (site, call_ty) in sites {
            if !call_ty.free_vars().is_empty() {
                continue;
            }
            let Ok(subst) = crate::types::unify::unify(&scheme.infer_type, call_ty) else {
                continue;
            };
            let Some((key, subst)) = clone_key(&scheme.forall, &subst) else {
                continue;
            };
            let clone_name = format!("{base}${key}");
            if let Some(clone) = clones.iter_mut().find(|clone| clone.name == clone_name) {
                clone.sites.push(*site);
            } else if clones.len() < MAX_CLONES_PER_FUNCTION {
                clones.push(PlannedClone {
                    name: clone_name,
                    subst,
                    sites: vec![*site],
                    example: call_ty.clone(),
                });
            } else {
                plan.missed.push((name, call_ty.clone()));
            }
        }
        if !clones.is_empty() {
            plan.clones.push((name, clones));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::type_constructor::TypeConstructor;

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    fn string() -> InferType {
        InferType::Con(TypeConstructor::String)
    }

    fn list(elem: InferType) -> InferType {
        InferType::App(TypeConstructor::List, vec![elem])
    }

    #[test]
    fn an_unboxed_variable_keys_the_clone_and_a_boxed_one_does_not() {
        let mut subst = TypeSubst::empty();
        subst.insert(0, int());
        subst.insert(1, string());
        let (key, kept) = clone_key(&[0, 1], &subst).expect("Int is unboxed");
        assert_eq!(key, "Int_Box");
        assert_eq!(kept.get(0), Some(&int()));
        assert_eq!(kept.get(1), None, "a boxed variable stays a variable");
    }

    #[test]
    fn string_and_a_list_share_a_key_and_need_no_clone() {
        for ty in [string(), list(int())] {
            let mut subst = TypeSubst::empty();
            subst.insert(0, ty);
            assert!(clone_key(&[0], &subst).is_none());
        }
    }
}
