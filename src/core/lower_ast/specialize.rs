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
//! - **Cloned** (2a): the call sites disagree. Each instantiation that
//!   changes a representation gets its own clone, `f$<key>`, and its call
//!   sites are pointed at it. The generic body stays, for the rest.
//! - **Re-specialised** (2a-2): a clone's body, or an in-place one, is lowered
//!   under its substitution, so the calls inside it are at known
//!   instantiations too, and their callees are cloned in turn.
//! - **Dictionary clones** (2a-3b): a function that takes dictionaries is
//!   cloned where every dictionary a call passes it is known. The clone takes
//!   none: its method calls go straight to the instance's methods, and the
//!   dictionaries it would have passed on are the known ones.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    syntax::{Identifier, expression::ExprId, interner::Interner, program::Program},
    types::{
        TypeVarId, class_env::ClassEnv, evidence::EvidenceMap, infer_type::InferType,
        translate::DictArg, type_env::TypeEnv, type_subst::TypeSubst,
    },
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
    /// Calls that call a clone only inside a specialised body: that body's
    /// definition (a clone's name, or an in-place function's own), the
    /// callee's `ExprId`, and the clone it calls there.
    pub(super) context_targets: Vec<(String, ExprId, String)>,
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
    /// The dictionaries the clone stands for, one per dictionary parameter
    /// of the function, all known. Empty for a function that takes none.
    pub(super) dicts: Vec<DictArg>,
    /// The callee `ExprId` of every call site that uses this clone wherever
    /// it is lowered. A call that uses it only inside a specialised body is in
    /// [`SpecPlan::context_targets`] instead.
    pub(super) sites: Vec<ExprId>,
    /// One instantiation that reaches this clone, for logging.
    pub(super) example: InferType,
}

/// A clone lowering emits: its interned name, its substitution, and the
/// dictionaries it stands for.
#[derive(Clone)]
pub(super) struct SpecClone {
    pub(super) name: Identifier,
    pub(super) subst: TypeSubst,
    pub(super) dicts: Vec<DictArg>,
}

/// A body lowered under a substitution: the function, the name of the
/// definition lowered (a clone's, or the function's own when in place), the
/// substitution and the dictionaries the body is handed.
struct SpecContext {
    function: Identifier,
    name: String,
    subst: TypeSubst,
    dicts: Vec<DictArg>,
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
    class_env: Option<&ClassEnv>,
    evidence: Option<&EvidenceMap>,
    dictionary_clones: bool,
) -> Vec<(Identifier, String)> {
    let mut lowerer = AstLowerer::new(
        hm_expr_types,
        Some(interner),
        Some(type_env),
        None,
        class_env,
        evidence,
    );
    lowerer.dictionary_clones = dictionary_clones;
    lowerer.collect_local_function_names(program);
    lowerer
        .plan_specializations(program)
        .clones
        .into_iter()
        .flat_map(|(function, clones)| clones.into_iter().map(move |clone| (function, clone.name)))
        .collect()
}

/// The key spelling each variable's representation, for the instantiation
/// `subst` of the variables `forall`, and that instantiation restricted to
/// what the key records.
///
/// `Int`, `Float` and `Bool` are unboxed and each its own, and everything
/// else is `Box`. `String` and `List<Int>` therefore share a key, and `Int`
/// and either of them do not.
fn rep_key(forall: &[TypeVarId], subst: &TypeSubst) -> (String, TypeSubst) {
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
    (parts.join("_"), kept)
}

/// [`rep_key`], or `None` when every variable is boxed, which is what the
/// generic body already is.
fn clone_key(forall: &[TypeVarId], subst: &TypeSubst) -> Option<(String, TypeSubst)> {
    let (key, kept) = rep_key(forall, subst);
    (!kept.is_empty()).then_some((key, kept))
}

/// The dictionary `arg` names inside a body of `owner` that was handed
/// `dicts`: `owner`'s parameter becomes the dictionary passed for it,
/// projected along any superclass path. Anything that does not resolve is
/// returned as it was.
pub(super) fn resolve_dict_arg(
    arg: &DictArg,
    owner: ExprId,
    dicts: &[DictArg],
    class_env: Option<&ClassEnv>,
) -> DictArg {
    match arg {
        DictArg::Param {
            owner: param_owner,
            index,
            path,
        } if *param_owner == owner => {
            let Some(mut resolved) = dicts.get(*index).cloned() else {
                return arg.clone();
            };
            for slot in path {
                match class_env.and_then(|env| env.superclass_dict_arg(&resolved, *slot)) {
                    Some(next) => resolved = next,
                    None => return arg.clone(),
                }
            }
            resolved
        }
        DictArg::Applied { instance, context } => DictArg::Applied {
            instance: instance.clone(),
            context: context
                .iter()
                .map(|arg| resolve_dict_arg(arg, owner, dicts, class_env))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Whether `arg` is a dictionary known without any parameter.
fn is_known(arg: &DictArg) -> bool {
    match arg {
        DictArg::Global { .. } => true,
        DictArg::Applied { context, .. } => context.iter().all(is_known),
        DictArg::Param { .. } | DictArg::None => false,
    }
}

/// Whether `arg` still names a parameter of `owner`.
pub(super) fn names_param_of(arg: &DictArg, owner: ExprId) -> bool {
    match arg {
        DictArg::Param {
            owner: param_owner, ..
        } => *param_owner == owner,
        DictArg::Applied { context, .. } => context.iter().any(|arg| names_param_of(arg, owner)),
        DictArg::Global { .. } | DictArg::None => false,
    }
}

/// The part of a clone's name that spells the dictionary `arg`: its class and
/// instance key, then each context dictionary's.
///
/// The class is named bare, without its module. A clone's dictionaries are
/// positional, so two clones of one function differ only in the instances
/// at each position, never in the class there.
fn dict_key(arg: &DictArg, interner: &Interner) -> Option<String> {
    match arg {
        DictArg::Global { instance } => Some(format!(
            "{}_{}",
            interner.try_resolve(instance.class_id.name)?,
            instance.dict_type_key
        )),
        DictArg::Applied { instance, context } => {
            let mut key = dict_key(
                &DictArg::Global {
                    instance: instance.clone(),
                },
                interner,
            )?;
            for arg in context {
                key.push('_');
                key.push_str(&dict_key(arg, interner)?);
            }
            Some(key)
        }
        DictArg::Param { .. } | DictArg::None => None,
    }
}

impl SpecPlan {
    /// The clone `name` of `function`, planned now if it is not yet and the
    /// cap allows: `None` past the cap, otherwise whether it is new.
    fn ensure_clone(
        &mut self,
        function: Identifier,
        name: String,
        subst: TypeSubst,
        dicts: Vec<DictArg>,
        example: &InferType,
    ) -> Option<(bool, &mut PlannedClone)> {
        let index = match self.clones.iter().position(|(f, _)| *f == function) {
            Some(index) => index,
            None => {
                self.clones.push((function, Vec::new()));
                self.clones.len() - 1
            }
        };
        let clones = &mut self.clones[index].1;
        if let Some(at) = clones.iter().position(|clone| clone.name == name) {
            return Some((false, &mut clones[at]));
        }
        if clones.len() >= MAX_CLONES_PER_FUNCTION {
            self.missed.push((function, example.clone()));
            return None;
        }
        clones.push(PlannedClone {
            name,
            subst,
            dicts,
            sites: Vec::new(),
            example: example.clone(),
        });
        clones.last_mut().map(|clone| (true, clone))
    }
}

impl AstLowerer<'_> {
    /// Decide, for every user-written function, whether it is specialised in
    /// place, cloned per instantiation, or left generic.
    ///
    /// **Self-calls do not count.** A recursive occurrence is at the
    /// definition's own type by construction, so it is never a second
    /// instantiation, and counting it would exclude every recursive function.
    /// Each specialised body's self-calls are planned with the rest of its
    /// calls, in [`Self::respecialize`].
    ///
    /// The exception is polymorphic recursion, which a signature allows: a
    /// self-call at another type, `depth([x], n)` inside `depth<a>(x: a, n)`.
    /// Such a function is never specialised in place, since its self-call
    /// would reach a body specialised for the wrong type; it is cloned, and
    /// the self-call keeps the clone or the generic body its own type reaches.
    ///
    /// A function that takes dictionaries is never specialised in place
    /// either: its dictionaries are positional and fixed by its scheme, so it
    /// is cloned where they are known, and the generic body stays.
    pub(super) fn plan_specializations(&self, program: &Program) -> SpecPlan {
        let mut visitor = CallSiteInstantiations {
            hm_expr_types: self.hm_expr_types,
            user_fns: &self.user_function_names,
            enclosing: None,
            outermost: None,
            sites: HashMap::new(),
            inner: HashMap::new(),
            def_ids: HashMap::new(),
            evidence_sites: HashMap::new(),
            params: HashMap::new(),
            constrained: HashSet::new(),
            param_types: HashMap::new(),
        };
        crate::ast::visit::Visitor::visit_program(&mut visitor, program);

        // Only a top-level function is cloned for now: a module member's
        // clone needs a module-qualified name (step 2a-4).
        let top_level: HashSet<Identifier> = program
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
            let takes_dicts = self.dict_param_count(name, &visitor) > 0;
            // A written bound with no evidence to say which dictionaries it
            // takes: the paths that lower without a solve. Never specialised.
            if !takes_dicts && visitor.constrained.contains(&name) {
                continue;
            }
            let first = &sites[0].1;
            if !takes_dicts
                && sites.iter().all(|(_, ty)| ty == first)
                && !self.recurses_polymorphically(name, &visitor)
            {
                if let Some(subst) = self.in_place_subst(name, first, &visitor) {
                    plan.in_place.insert(name, subst);
                }
                continue;
            }
            for (site, call_ty) in sites {
                let dicts = self.site_dicts(*site, None);
                let Some((clone_name, subst, dicts)) =
                    self.instantiation(name, call_ty, dicts, &top_level, &visitor)
                else {
                    continue;
                };
                if let Some((_, clone)) = plan.ensure_clone(name, clone_name, subst, dicts, call_ty)
                {
                    clone.sites.push(*site);
                }
            }
        }
        self.respecialize(&visitor, &top_level, &mut plan);
        plan
    }

    /// 2a-2: plan the clones the calls inside each specialised body reach.
    ///
    /// Every clone, and every in-place definition, is a body lowered under a
    /// substitution (and, for a dictionary clone, handed known dictionaries),
    /// so a call in it that was generic may be at a known instantiation there.
    /// Each body is visited once, and a clone it creates is visited in turn.
    /// That terminates because a function has at most
    /// [`MAX_CLONES_PER_FUNCTION`] clones, and it covers a recursive group:
    /// `f$Int` calling `g` reaches `g$Int`, whose call back to `f` reaches the
    /// `f$Int` that exists.
    fn respecialize(
        &self,
        visitor: &CallSiteInstantiations<'_>,
        top_level: &HashSet<Identifier>,
        plan: &mut SpecPlan,
    ) {
        let mut in_place: Vec<(&Identifier, &TypeSubst)> = plan.in_place.iter().collect();
        in_place.sort_unstable_by_key(|(name, _)| name.as_u32());
        let mut work: VecDeque<SpecContext> = VecDeque::new();
        for (function, subst) in in_place {
            if let Some(name) = self.interner.and_then(|i| i.try_resolve(*function)) {
                work.push_back(SpecContext {
                    function: *function,
                    name: name.to_string(),
                    subst: subst.clone(),
                    dicts: Vec::new(),
                });
            }
        }
        for (function, clones) in &plan.clones {
            for clone in clones {
                work.push_back(SpecContext {
                    function: *function,
                    name: clone.name.clone(),
                    subst: clone.subst.clone(),
                    dicts: clone.dicts.clone(),
                });
            }
        }
        while let Some(context) = work.pop_front() {
            let Some(calls) = visitor.inner.get(&context.function) else {
                continue;
            };
            let owner = visitor.def_ids.get(&context.function).copied();
            for (callee, site, call_ty) in calls {
                let dicts =
                    self.site_dicts(*site, owner.map(|owner| (owner, context.dicts.as_slice())));
                // Ordinary recursion: the body calls itself, a clone its own
                // clone. Its variables are fresh, so the substitution would
                // not reach them; its dictionaries are the body's own, or
                // none when the call raised none.
                if *callee == context.function
                    && self.is_own_type(context.function, call_ty)
                    && (dicts.is_empty() || dicts == context.dicts)
                {
                    if self
                        .interner
                        .and_then(|i| i.try_resolve(context.function))
                        .is_some_and(|own| own != context.name)
                    {
                        plan.context_targets.push((
                            context.name.clone(),
                            *site,
                            context.name.clone(),
                        ));
                    }
                    continue;
                }
                let call_ty = call_ty.apply_type_subst(&context.subst);
                let Some((clone_name, clone_subst, clone_dicts)) =
                    self.instantiation(*callee, &call_ty, dicts, top_level, visitor)
                else {
                    continue;
                };
                // An in-place definition is already this instantiation.
                if plan
                    .in_place
                    .get(callee)
                    .and_then(|subst| self.clone_name(*callee, subst, &[]))
                    .is_some_and(|(in_place, _)| in_place == clone_name)
                {
                    continue;
                }
                let Some((new, clone)) = plan.ensure_clone(
                    *callee,
                    clone_name.clone(),
                    clone_subst,
                    clone_dicts,
                    &call_ty,
                ) else {
                    continue;
                };
                if new {
                    work.push_back(SpecContext {
                        function: *callee,
                        name: clone_name.clone(),
                        subst: clone.subst.clone(),
                        dicts: clone.dicts.clone(),
                    });
                }
                plan.context_targets
                    .push((context.name.clone(), *site, clone_name));
            }
        }
    }

    /// How many dictionary parameters `function` takes, by its evidence.
    fn dict_param_count(
        &self,
        function: Identifier,
        visitor: &CallSiteInstantiations<'_>,
    ) -> usize {
        visitor
            .def_ids
            .get(&function)
            .and_then(|id| self.evidence?.definition(*id))
            .map_or(0, |definition| definition.params.len())
    }

    /// The dictionaries the call whose callee is `site` passes, each resolved
    /// through the body it sits in: `(owner, dicts)` when that body is handed
    /// known dictionaries. A call with no answer passes `DictArg::None`, which
    /// no clone accepts.
    fn site_dicts(&self, site: ExprId, body: Option<(ExprId, &[DictArg])>) -> Vec<DictArg> {
        let Some(evidence) = self.evidence else {
            return Vec::new();
        };
        let Some(args) = evidence.dict_args_at(site) else {
            return vec![DictArg::None];
        };
        match body {
            Some((owner, dicts)) if !dicts.is_empty() => args
                .iter()
                .map(|arg| resolve_dict_arg(arg, owner, dicts, self.class_env))
                .collect(),
            _ => args,
        }
    }

    /// Whether every dictionary the body of `function` reads resolves when
    /// it is handed `dicts`: a superclass the class environment cannot find
    /// statically would leave the clone reading a parameter it does not have.
    fn resolves_in_body(
        &self,
        function: Identifier,
        dicts: &[DictArg],
        visitor: &CallSiteInstantiations<'_>,
    ) -> bool {
        let (Some(evidence), Some(owner)) = (self.evidence, visitor.def_ids.get(&function)) else {
            return false;
        };
        visitor
            .evidence_sites
            .get(&function)
            .into_iter()
            .flatten()
            .filter_map(|site| evidence.dict_args_at(*site))
            .flatten()
            .all(|arg| {
                !names_param_of(
                    &resolve_dict_arg(&arg, *owner, dicts, self.class_env),
                    *owner,
                )
            })
    }

    /// Whether `name` calls itself at a type other than its own.
    fn recurses_polymorphically(
        &self,
        name: Identifier,
        visitor: &CallSiteInstantiations<'_>,
    ) -> bool {
        visitor.inner.get(&name).is_some_and(|calls| {
            calls
                .iter()
                .any(|(callee, _, ty)| *callee == name && !self.is_own_type(name, ty))
        })
    }

    /// Whether `ty`, a self-call's type, is `function`'s own type up to the
    /// names of its variables.
    ///
    /// A self-call is recorded with fresh variables even in ordinary
    /// recursion, so it is compared up to renaming: each quantified variable
    /// must map to a distinct variable. Polymorphic recursion maps one to a
    /// type, `a` to `List<a>`. Without a scheme there is nothing to compare
    /// with, and nothing is cloned either, so the answer is `true`: B1 as
    /// before.
    fn is_own_type(&self, function: Identifier, ty: &InferType) -> bool {
        let Some(scheme) = self.type_env.and_then(|env| env.lookup(function)) else {
            return true;
        };
        let Ok(subst) = crate::types::unify::unify(&scheme.infer_type, ty) else {
            return false;
        };
        let mut seen = HashSet::new();
        scheme.forall.iter().all(|var| match subst.get(*var) {
            None => true,
            Some(InferType::Var(target)) => seen.insert(*target),
            Some(_) => false,
        })
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

    /// The clone of `function` that a call at `call_ty` passing `dicts`
    /// calls: its name, substitution and dictionaries. `None` when the
    /// generic body serves it, or when `function` is not one this step clones.
    ///
    /// `call_ty` need not be ground. A variable it leaves is boxed in the key,
    /// and every boxed type has one representation, so the clone is right for
    /// whatever the variable becomes. `dicts` must be known, one per
    /// dictionary parameter, and resolve everything the body reads.
    fn instantiation(
        &self,
        function: Identifier,
        call_ty: &InferType,
        dicts: Vec<DictArg>,
        top_level: &HashSet<Identifier>,
        visitor: &CallSiteInstantiations<'_>,
    ) -> Option<(String, TypeSubst, Vec<DictArg>)> {
        if !top_level.contains(&function) {
            return None;
        }
        if dicts.len() != self.dict_param_count(function, visitor)
            || (!dicts.is_empty() && !self.dictionary_clones)
            || !dicts.iter().all(is_known)
            || (!dicts.is_empty() && !self.resolves_in_body(function, &dicts, visitor))
        {
            return None;
        }
        let scheme = self.type_env?.lookup(function)?;
        let subst = crate::types::unify::unify(&scheme.infer_type, call_ty).ok()?;
        let (name, subst) = self.clone_name(function, &subst, &dicts)?;
        Some((name, subst, dicts))
    }

    /// The name of `function`'s clone for the instantiation `subst` and the
    /// dictionaries `dicts`, and `subst` restricted to what that clone
    /// records.
    fn clone_name(
        &self,
        function: Identifier,
        subst: &TypeSubst,
        dicts: &[DictArg],
    ) -> Option<(String, TypeSubst)> {
        let scheme = self.type_env?.lookup(function)?;
        if scheme.forall.is_empty() {
            return None;
        }
        let interner = self.interner?;
        let base = interner.try_resolve(function)?;
        if dicts.is_empty() {
            // A scheme with constraints takes dictionaries; with none known
            // here, it is not cloned.
            if !scheme.constraints.is_empty() {
                return None;
            }
            let (key, subst) = clone_key(&scheme.forall, subst)?;
            return Some((format!("{base}${key}"), subst));
        }
        let (key, subst) = rep_key(&scheme.forall, subst);
        let mut name = format!("{base}${key}");
        for arg in dicts {
            name.push('$');
            name.push_str(&dict_key(arg, interner)?);
        }
        Some((name, subst))
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
