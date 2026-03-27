//! Aether borrow metadata infrastructure.
//!
//! Phase C replaces the old name-only registry with explicit compiler-owned
//! borrow metadata that survives past inference and is available at every
//! Aether call site. Borrow facts come from three sources:
//! - inferred signatures for user-defined Core definitions
//! - explicit Flow/runtime metadata
//! - conservative imported/unknown fallbacks

use std::collections::{HashMap, HashSet};

use crate::core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreVarRef};
use crate::syntax::Identifier;
use crate::syntax::interner::Interner;

use super::callee::{AetherCalleeClassification, classify_direct_var_ref};

/// How a function parameter is used — does it need ownership or just a reference?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowMode {
    /// Parameter is consumed (stored in ADT, returned, captured by closure).
    Owned,
    /// Parameter is only read (PrimOp operand, Case scrutinee, passed to
    /// another borrowed param). Caller can skip Rc::clone.
    Borrowed,
}

/// Where a borrow signature came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowProvenance {
    Inferred,
    BaseRuntime,
    Imported,
    Unknown,
}

/// Ordered parameter borrow metadata for a callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowSignature {
    pub params: Vec<BorrowMode>,
    pub provenance: BorrowProvenance,
}

impl BorrowSignature {
    pub fn new(params: Vec<BorrowMode>, provenance: BorrowProvenance) -> Self {
        Self { params, provenance }
    }

    pub fn all(mode: BorrowMode, arity: usize, provenance: BorrowProvenance) -> Self {
        Self {
            params: vec![mode; arity],
            provenance,
        }
    }

    pub fn is_borrowed(&self, param_index: usize) -> bool {
        self.params.get(param_index).copied() == Some(BorrowMode::Borrowed)
    }
}

/// Stable callee identity for borrow lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BorrowCallee {
    Local(CoreBinderId),
    Global(Identifier),
    BaseRuntime(Identifier),
    Imported(Identifier),
    Unknown,
}

/// Registry of borrow signatures available to the Aether pass.
#[derive(Debug, Clone)]
pub struct BorrowRegistry {
    pub by_binder: HashMap<CoreBinderId, BorrowSignature>,
    pub by_name: HashMap<Identifier, BorrowSignature>,
    pub unknown_callee_signature: BorrowSignature,
}

impl Default for BorrowRegistry {
    fn default() -> Self {
        Self {
            by_binder: HashMap::new(),
            by_name: HashMap::new(),
            unknown_callee_signature: BorrowSignature::new(Vec::new(), BorrowProvenance::Unknown),
        }
    }
}

impl BorrowRegistry {
    pub fn lookup_binder(&self, binder: CoreBinderId) -> Option<&BorrowSignature> {
        self.by_binder.get(&binder)
    }

    pub fn lookup_name(&self, name: Identifier) -> Option<&BorrowSignature> {
        self.by_name.get(&name)
    }

    pub fn signature_for_callee(&self, callee: BorrowCallee) -> &BorrowSignature {
        match callee {
            BorrowCallee::Local(binder) => self
                .lookup_binder(binder)
                .unwrap_or(&self.unknown_callee_signature),
            BorrowCallee::Global(name)
            | BorrowCallee::BaseRuntime(name)
            | BorrowCallee::Imported(name) => self
                .lookup_name(name)
                .unwrap_or(&self.unknown_callee_signature),
            BorrowCallee::Unknown => &self.unknown_callee_signature,
        }
    }

    pub fn is_borrowed(&self, callee: BorrowCallee, param_index: usize) -> bool {
        self.signature_for_callee(callee).is_borrowed(param_index)
    }

    pub fn resolve_var_ref(&self, var: &CoreVarRef) -> BorrowCallee {
        self.classify_var_ref(var).borrow_callee
    }

    pub fn classify_var_ref(&self, var: &CoreVarRef) -> AetherCalleeClassification {
        classify_direct_var_ref(
            var,
            |binder| self.by_binder.contains_key(&binder),
            |name| self.lookup_name(name).map(|sig| sig.provenance),
        )
    }

    fn insert_named_if_absent(&mut self, name: Identifier, signature: BorrowSignature) {
        self.by_name.entry(name).or_insert(signature);
    }

    fn upsert_user_signature(&mut self, binder: CoreBinder, signature: BorrowSignature) -> bool {
        let binder_changed = self.by_binder.get(&binder.id) != Some(&signature);
        let name_changed = self.by_name.get(&binder.name) != Some(&signature);
        self.by_binder.insert(binder.id, signature.clone());
        self.by_name.insert(binder.name, signature);
        binder_changed || name_changed
    }
}

/// Infer and persist borrow metadata for all Core definitions.
pub fn infer_borrow_modes(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
) -> BorrowRegistry {
    let mut registry = BorrowRegistry::default();
    register_explicit_named_fallbacks(program, &mut registry, interner);

    for def in &program.defs {
        if let Some((params, _)) = extract_lam(&def.expr) {
            registry.upsert_user_signature(
                def.binder,
                BorrowSignature::all(BorrowMode::Owned, params.len(), BorrowProvenance::Inferred),
            );
        }
    }

    let defs_by_binder: HashMap<CoreBinderId, &CoreDef> = program
        .defs
        .iter()
        .map(|def| (def.binder.id, def))
        .collect();
    let recursive_groups = compute_recursive_groups(program);

    const MAX_BORROW_INFERENCE_ROUNDS: usize = 10;
    for _round in 0..MAX_BORROW_INFERENCE_ROUNDS {
        let mut changed_any = false;
        for group in &recursive_groups {
            let group_set: HashSet<_> = group.iter().copied().collect();
            let constraints =
                infer_group_constraints(group, &defs_by_binder, &registry, &group_set);
            let solved = solve_group_modes(group, &constraints);

            let mut group_changed = false;
            for binder in group {
                let Some(def) = defs_by_binder.get(binder) else {
                    continue;
                };
                let Some((params, _)) = extract_lam(&def.expr) else {
                    continue;
                };
                let params = solved
                    .get(binder)
                    .cloned()
                    .unwrap_or_else(|| vec![BorrowMode::Owned; params.len()]);
                let signature = BorrowSignature::new(params, BorrowProvenance::Inferred);
                if registry.upsert_user_signature(def.binder, signature) {
                    group_changed = true;
                }
            }

            if group_changed {
                changed_any = true;
            }
        }

        if !changed_any {
            break;
        }
    }

    for def in &mut program.defs {
        def.borrow_signature = registry.lookup_binder(def.binder.id).cloned();
    }

    registry
}

fn extract_lam(expr: &CoreExpr) -> Option<(&[CoreBinder], &CoreExpr)> {
    match expr {
        CoreExpr::Lam { params, body, .. } => Some((params, body)),
        _ => None,
    }
}

fn register_explicit_named_fallbacks(
    program: &CoreProgram,
    registry: &mut BorrowRegistry,
    interner: Option<&Interner>,
) {
    let mut unresolved_callees = HashMap::<Identifier, usize>::new();
    for def in &program.defs {
        collect_unresolved_callees(&def.expr, &mut unresolved_callees);
    }

    for (name, arity) in unresolved_callees {
        if let Some(interner) = interner
            && let Some((primop_arity, borrows)) =
                crate::core::CorePrimOp::resolve_borrow_info(interner.resolve(name))
        {
            let mode = if borrows {
                BorrowMode::Borrowed
            } else {
                BorrowMode::Owned
            };
            registry.insert_named_if_absent(
                name,
                BorrowSignature::all(mode, primop_arity, BorrowProvenance::BaseRuntime),
            );
            continue;
        }

        registry.insert_named_if_absent(
            name,
            BorrowSignature::all(BorrowMode::Owned, arity, BorrowProvenance::Imported),
        );
    }
}

#[derive(Debug, Clone, Default)]
struct ParamConstraint {
    force_owned: bool,
    deps: Vec<(CoreBinderId, usize)>,
}

fn compute_recursive_groups(program: &CoreProgram) -> Vec<Vec<CoreBinderId>> {
    let def_ids: HashSet<_> = program.defs.iter().map(|def| def.binder.id).collect();
    let adjacency: HashMap<CoreBinderId, Vec<CoreBinderId>> = program
        .defs
        .iter()
        .map(|def| {
            let mut callees = HashSet::new();
            collect_local_callees(&def.expr, &def_ids, &mut callees);
            (def.binder.id, callees.into_iter().collect())
        })
        .collect();

    let mut index = 0usize;
    let mut stack = Vec::new();
    let mut on_stack = HashSet::new();
    let mut indices = HashMap::<CoreBinderId, usize>::new();
    let mut lowlinks = HashMap::<CoreBinderId, usize>::new();
    let mut components = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        v: CoreBinderId,
        adjacency: &HashMap<CoreBinderId, Vec<CoreBinderId>>,
        index: &mut usize,
        stack: &mut Vec<CoreBinderId>,
        on_stack: &mut HashSet<CoreBinderId>,
        indices: &mut HashMap<CoreBinderId, usize>,
        lowlinks: &mut HashMap<CoreBinderId, usize>,
        components: &mut Vec<Vec<CoreBinderId>>,
    ) {
        indices.insert(v, *index);
        lowlinks.insert(v, *index);
        *index += 1;
        stack.push(v);
        on_stack.insert(v);

        for w in adjacency.get(&v).into_iter().flatten().copied() {
            if !indices.contains_key(&w) {
                strongconnect(
                    w, adjacency, index, stack, on_stack, indices, lowlinks, components,
                );
                let low_v = *lowlinks.get(&v).expect("lowlink for current node");
                let low_w = *lowlinks.get(&w).expect("lowlink for child");
                lowlinks.insert(v, low_v.min(low_w));
            } else if on_stack.contains(&w) {
                let low_v = *lowlinks.get(&v).expect("lowlink for current node");
                let idx_w = *indices.get(&w).expect("index for child");
                lowlinks.insert(v, low_v.min(idx_w));
            }
        }

        if indices.get(&v) == lowlinks.get(&v) {
            let mut component = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack.remove(&w);
                component.push(w);
                if w == v {
                    break;
                }
            }
            components.push(component);
        }
    }

    for def in &program.defs {
        if !indices.contains_key(&def.binder.id) {
            strongconnect(
                def.binder.id,
                &adjacency,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlinks,
                &mut components,
            );
        }
    }

    components
}

fn collect_local_callees(
    expr: &CoreExpr,
    def_ids: &HashSet<CoreBinderId>,
    out: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => collect_local_callees(body, def_ids, out),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            if let CoreExpr::Var { var, .. } = func.as_ref()
                && let Some(binder) = var.binder
                && def_ids.contains(&binder)
            {
                out.insert(binder);
            }
            collect_local_callees(func, def_ids, out);
            for arg in args {
                collect_local_callees(arg, def_ids, out);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_local_callees(rhs, def_ids, out);
            collect_local_callees(body, def_ids, out);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_local_callees(scrutinee, def_ids, out);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_local_callees(guard, def_ids, out);
                }
                collect_local_callees(&alt.rhs, def_ids, out);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                collect_local_callees(field, def_ids, out);
            }
        }
        CoreExpr::Return { value, .. } => collect_local_callees(value, def_ids, out),
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_local_callees(arg, def_ids, out);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_local_callees(body, def_ids, out);
            for handler in handlers {
                collect_local_callees(&handler.body, def_ids, out);
            }
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            collect_local_callees(body, def_ids, out)
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_local_callees(object, def_ids, out)
        }
        CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                collect_local_callees(field, def_ids, out);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_local_callees(unique_body, def_ids, out);
            collect_local_callees(shared_body, def_ids, out);
        }
    }
}

fn infer_group_constraints(
    group: &[CoreBinderId],
    defs_by_binder: &HashMap<CoreBinderId, &CoreDef>,
    registry: &BorrowRegistry,
    group_set: &HashSet<CoreBinderId>,
) -> HashMap<(CoreBinderId, usize), ParamConstraint> {
    let mut constraints = HashMap::new();

    for binder in group {
        let Some(def) = defs_by_binder.get(binder) else {
            continue;
        };
        let Some((params, body)) = extract_lam(&def.expr) else {
            continue;
        };
        for (param_index, param) in params.iter().enumerate() {
            let mut constraint = ParamConstraint::default();
            collect_param_constraints(param.id, body, registry, group_set, &mut constraint);
            constraints.insert((*binder, param_index), constraint);
        }
    }

    constraints
}

fn solve_group_modes(
    group: &[CoreBinderId],
    constraints: &HashMap<(CoreBinderId, usize), ParamConstraint>,
) -> HashMap<CoreBinderId, Vec<BorrowMode>> {
    let mut solved = HashMap::<CoreBinderId, Vec<BorrowMode>>::new();
    for binder in group {
        let mut arity = 0usize;
        while constraints.contains_key(&(*binder, arity)) {
            arity += 1;
        }
        solved.insert(*binder, vec![BorrowMode::Borrowed; arity]);
    }

    loop {
        let mut changed = false;
        for binder in group {
            let Some(current_modes) = solved.get(binder).cloned() else {
                continue;
            };
            let mut next_modes = current_modes.clone();
            for (param_index, mode) in next_modes.iter_mut().enumerate() {
                let Some(constraint) = constraints.get(&(*binder, param_index)) else {
                    continue;
                };
                let next = if constraint.force_owned
                    || constraint.deps.iter().any(|(callee, arg_index)| {
                        solved
                            .get(callee)
                            .and_then(|callee_modes| callee_modes.get(*arg_index))
                            .copied()
                            .unwrap_or(BorrowMode::Owned)
                            == BorrowMode::Owned
                    }) {
                    BorrowMode::Owned
                } else {
                    BorrowMode::Borrowed
                };
                if *mode != next {
                    *mode = next;
                    changed = true;
                }
            }
            solved.insert(*binder, next_modes);
        }
        if !changed {
            break;
        }
    }

    solved
}

fn collect_param_constraints(
    target: CoreBinderId,
    expr: &CoreExpr,
    registry: &BorrowRegistry,
    group_set: &HashSet<CoreBinderId>,
    constraint: &mut ParamConstraint,
) {
    if constraint.force_owned {
        return;
    }

    match expr {
        CoreExpr::Var { var, .. } => {
            if var.binder == Some(target) {
                constraint.force_owned = true;
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::PrimOp { args, .. } => {
            for arg in args {
                collect_param_constraints_skip_direct(target, arg, registry, group_set, constraint);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_param_constraints_skip_direct(
                target, scrutinee, registry, group_set, constraint,
            );
            for alt in alts {
                if !pat_binds(target, &alt.pat) {
                    if let Some(guard) = &alt.guard {
                        collect_param_constraints(target, guard, registry, group_set, constraint);
                    }
                    collect_param_constraints(target, &alt.rhs, registry, group_set, constraint);
                }
            }
        }
        CoreExpr::App { func, args, .. } => {
            collect_call_param_constraints(
                target, func, args, None, registry, group_set, constraint,
            );
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            ..
        } => {
            collect_call_param_constraints(
                target,
                func,
                args,
                Some(arg_modes),
                registry,
                group_set,
                constraint,
            );
        }
        CoreExpr::Con { fields, .. }
        | CoreExpr::Reuse { fields, .. }
        | CoreExpr::Perform { args: fields, .. } => {
            for field in fields {
                collect_param_constraints(target, field, registry, group_set, constraint);
            }
        }
        CoreExpr::Return { value, .. } => {
            collect_param_constraints(target, value, registry, group_set, constraint);
        }
        CoreExpr::Lam { params, body, .. } => {
            if !params.iter().any(|p| p.id == target)
                && super::analysis::owned_use_count_with_registry(target, body, registry) > 0
            {
                constraint.force_owned = true;
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            collect_param_constraints(target, rhs, registry, group_set, constraint);
            if var.id != target {
                collect_param_constraints(target, body, registry, group_set, constraint);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            if var.id != target {
                collect_param_constraints(target, rhs, registry, group_set, constraint);
                collect_param_constraints(target, body, registry, group_set, constraint);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_param_constraints(target, body, registry, group_set, constraint);
            for handler in handlers {
                if handler.resume.id == target || handler.params.iter().any(|p| p.id == target) {
                    continue;
                }
                collect_param_constraints(target, &handler.body, registry, group_set, constraint);
            }
        }
        CoreExpr::Dup { var, body, .. } => {
            if var.binder == Some(target) {
                constraint.force_owned = true;
            }
            collect_param_constraints(target, body, registry, group_set, constraint);
        }
        CoreExpr::Drop { body, .. } => {
            collect_param_constraints(target, body, registry, group_set, constraint);
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_param_constraints(target, object, registry, group_set, constraint);
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_param_constraints(target, unique_body, registry, group_set, constraint);
            collect_param_constraints(target, shared_body, registry, group_set, constraint);
        }
    }
}

fn collect_call_param_constraints(
    target: CoreBinderId,
    func: &CoreExpr,
    args: &[CoreExpr],
    explicit_modes: Option<&[BorrowMode]>,
    registry: &BorrowRegistry,
    group_set: &HashSet<CoreBinderId>,
    constraint: &mut ParamConstraint,
) {
    collect_param_constraints_skip_direct(target, func, registry, group_set, constraint);

    let local_callee = match func {
        CoreExpr::Var { var, .. } => var.binder.filter(|binder| group_set.contains(binder)),
        _ => None,
    };

    for (index, arg) in args.iter().enumerate() {
        if let Some(callee) = local_callee
            && matches!(arg, CoreExpr::Var { var, .. } if var.binder == Some(target))
        {
            constraint.deps.push((callee, index));
            continue;
        }

        let borrowed = if let Some(modes) = explicit_modes {
            modes.get(index).copied() == Some(BorrowMode::Borrowed)
        } else if let CoreExpr::Var {
            var: callee_var, ..
        } = func
        {
            registry.is_borrowed(registry.resolve_var_ref(callee_var), index)
        } else {
            false
        };

        if borrowed {
            collect_param_constraints_skip_direct(target, arg, registry, group_set, constraint);
        } else {
            collect_param_constraints(target, arg, registry, group_set, constraint);
        }
    }
}

fn collect_param_constraints_skip_direct(
    target: CoreBinderId,
    expr: &CoreExpr,
    registry: &BorrowRegistry,
    group_set: &HashSet<CoreBinderId>,
    constraint: &mut ParamConstraint,
) {
    if matches!(expr, CoreExpr::Var { var, .. } if var.binder == Some(target)) {
        return;
    }
    collect_param_constraints(target, expr, registry, group_set, constraint);
}

fn pat_binds(var: CoreBinderId, pat: &crate::core::CorePat) -> bool {
    match pat {
        crate::core::CorePat::Var(binder) => binder.id == var,
        crate::core::CorePat::Con { fields, .. } | crate::core::CorePat::Tuple(fields) => {
            fields.iter().any(|field| pat_binds(var, field))
        }
        crate::core::CorePat::Lit(_)
        | crate::core::CorePat::Wildcard
        | crate::core::CorePat::EmptyList => false,
    }
}

fn collect_unresolved_callees(expr: &CoreExpr, unresolved: &mut HashMap<Identifier, usize>) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => collect_unresolved_callees(body, unresolved),
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            if let CoreExpr::Var { var, .. } = func.as_ref()
                && var.binder.is_none()
            {
                unresolved
                    .entry(var.name)
                    .and_modify(|arity| *arity = (*arity).max(args.len()))
                    .or_insert(args.len());
            }
            collect_unresolved_callees(func, unresolved);
            for arg in args {
                collect_unresolved_callees(arg, unresolved);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_unresolved_callees(rhs, unresolved);
            collect_unresolved_callees(body, unresolved);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_unresolved_callees(scrutinee, unresolved);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_unresolved_callees(guard, unresolved);
                }
                collect_unresolved_callees(&alt.rhs, unresolved);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                collect_unresolved_callees(field, unresolved);
            }
        }
        CoreExpr::Return { value, .. } => collect_unresolved_callees(value, unresolved),
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_unresolved_callees(arg, unresolved);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_unresolved_callees(body, unresolved);
            for handler in handlers {
                collect_unresolved_callees(&handler.body, unresolved);
            }
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            collect_unresolved_callees(body, unresolved)
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            collect_unresolved_callees(object, unresolved)
        }
        CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                collect_unresolved_callees(field, unresolved);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_unresolved_callees(unique_body, unresolved);
            collect_unresolved_callees(shared_body, unresolved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BorrowCallee, BorrowMode, BorrowProvenance, BorrowRegistry, infer_borrow_modes};
    use crate::core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreVarRef};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    fn span() -> Span {
        Span::default()
    }

    fn binder(id: u32, name: crate::syntax::Identifier) -> CoreBinder {
        CoreBinder::new(CoreBinderId(id), name)
    }

    fn var_ref(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span: span(),
        }
    }

    fn ext_var(name: crate::syntax::Identifier) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::unresolved(name),
            span: span(),
        }
    }

    fn def(
        binder: CoreBinder,
        params: Vec<CoreBinder>,
        body: CoreExpr,
        is_recursive: bool,
    ) -> CoreDef {
        CoreDef::new(
            binder,
            CoreExpr::lambda(params, body, span()),
            is_recursive,
            span(),
        )
    }

    #[test]
    fn direct_local_call_uses_inferred_borrow_signature() {
        let mut interner = Interner::new();
        let read_name = interner.intern("read");
        let main_name = interner.intern("main");
        let read_binder = binder(1, read_name);
        let main_binder = binder(2, main_name);
        let x = binder(3, interner.intern("x"));
        let xs = binder(4, interner.intern("xs"));

        let read_def = def(
            read_binder,
            vec![x],
            CoreExpr::Case {
                scrutinee: Box::new(var_ref(x)),
                alts: Vec::new(),
                span: span(),
            },
            false,
        );
        let main_def = def(
            main_binder,
            vec![xs],
            CoreExpr::App {
                func: Box::new(var_ref(read_binder)),
                args: vec![var_ref(xs)],
                span: span(),
            },
            false,
        );

        let mut program = CoreProgram {
            defs: vec![read_def, main_def],
            top_level_items: Vec::new(),
        };

        let registry = infer_borrow_modes(&mut program, Some(&interner));
        let sig = registry
            .lookup_binder(read_binder.id)
            .expect("read signature should exist");
        assert_eq!(sig.params, vec![BorrowMode::Borrowed]);
        assert_eq!(program.defs[0].borrow_signature.as_ref(), Some(sig));
    }

    #[test]
    fn recursive_defs_converge_to_deterministic_signature() {
        let mut interner = Interner::new();
        let loop_name = interner.intern("loop");
        let loop_binder = binder(1, loop_name);
        let xs = binder(2, interner.intern("xs"));

        let loop_def = def(
            loop_binder,
            vec![xs],
            CoreExpr::App {
                func: Box::new(var_ref(loop_binder)),
                args: vec![var_ref(xs)],
                span: span(),
            },
            true,
        );

        let mut program = CoreProgram {
            defs: vec![loop_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(loop_binder.id)
                .expect("loop signature")
                .params,
            vec![BorrowMode::Borrowed]
        );
    }

    #[test]
    fn self_recursive_borrowed_parameter_stays_borrowed() {
        let mut interner = Interner::new();
        let loop_name = interner.intern("loop");
        let loop_binder = binder(1, loop_name);
        let xs = binder(2, interner.intern("xs"));
        let n = binder(3, interner.intern("n"));
        let len = interner.intern("len");

        let loop_def = def(
            loop_binder,
            vec![xs, n],
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::PrimOp {
                    op: crate::core::CorePrimOp::Eq,
                    args: vec![
                        var_ref(n),
                        CoreExpr::Lit(crate::core::CoreLit::Int(0), span()),
                    ],
                    span: span(),
                }),
                alts: vec![
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Lit(crate::core::CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(ext_var(len)),
                            args: vec![var_ref(xs)],
                            span: span(),
                        },
                        span: span(),
                    },
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(var_ref(loop_binder)),
                            args: vec![
                                var_ref(xs),
                                CoreExpr::PrimOp {
                                    op: crate::core::CorePrimOp::Sub,
                                    args: vec![
                                        var_ref(n),
                                        CoreExpr::Lit(crate::core::CoreLit::Int(1), span()),
                                    ],
                                    span: span(),
                                },
                            ],
                            span: span(),
                        },
                        span: span(),
                    },
                ],
                span: span(),
            },
            true,
        );

        let mut program = CoreProgram {
            defs: vec![loop_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(loop_binder.id)
                .expect("loop signature")
                .params,
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed]
        );
    }

    #[test]
    fn self_recursive_higher_order_forwarding_parameter_stays_borrowed() {
        let mut interner = Interner::new();
        let loop_binder = binder(1, interner.intern("loop"));
        let f = binder(2, interner.intern("f"));
        let n = binder(3, interner.intern("n"));

        let loop_def = def(
            loop_binder,
            vec![f, n],
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::PrimOp {
                    op: crate::core::CorePrimOp::Eq,
                    args: vec![
                        var_ref(n),
                        CoreExpr::Lit(crate::core::CoreLit::Int(0), span()),
                    ],
                    span: span(),
                }),
                alts: vec![
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Lit(crate::core::CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::Lit(crate::core::CoreLit::Int(0), span()),
                        span: span(),
                    },
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(var_ref(loop_binder)),
                            args: vec![
                                var_ref(f),
                                CoreExpr::PrimOp {
                                    op: crate::core::CorePrimOp::Sub,
                                    args: vec![
                                        var_ref(n),
                                        CoreExpr::Lit(crate::core::CoreLit::Int(1), span()),
                                    ],
                                    span: span(),
                                },
                            ],
                            span: span(),
                        },
                        span: span(),
                    },
                ],
                span: span(),
            },
            true,
        );

        let mut program = CoreProgram {
            defs: vec![loop_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(loop_binder.id)
                .expect("loop signature")
                .params,
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed]
        );
    }

    #[test]
    fn mutually_recursive_borrowed_parameter_converges() {
        let mut interner = Interner::new();
        let even_binder = binder(1, interner.intern("even"));
        let odd_binder = binder(2, interner.intern("odd"));
        let xs_even = binder(3, interner.intern("xs"));
        let n_even = binder(4, interner.intern("n"));
        let xs_odd = binder(5, interner.intern("ys"));
        let n_odd = binder(6, interner.intern("m"));
        let len = interner.intern("len");

        let even_def = def(
            even_binder,
            vec![xs_even, n_even],
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::PrimOp {
                    op: crate::core::CorePrimOp::Eq,
                    args: vec![
                        var_ref(n_even),
                        CoreExpr::Lit(crate::core::CoreLit::Int(0), span()),
                    ],
                    span: span(),
                }),
                alts: vec![
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Lit(crate::core::CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(ext_var(len)),
                            args: vec![var_ref(xs_even)],
                            span: span(),
                        },
                        span: span(),
                    },
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(var_ref(odd_binder)),
                            args: vec![
                                var_ref(xs_even),
                                CoreExpr::PrimOp {
                                    op: crate::core::CorePrimOp::Sub,
                                    args: vec![
                                        var_ref(n_even),
                                        CoreExpr::Lit(crate::core::CoreLit::Int(1), span()),
                                    ],
                                    span: span(),
                                },
                            ],
                            span: span(),
                        },
                        span: span(),
                    },
                ],
                span: span(),
            },
            true,
        );

        let odd_def = def(
            odd_binder,
            vec![xs_odd, n_odd],
            CoreExpr::Case {
                scrutinee: Box::new(CoreExpr::PrimOp {
                    op: crate::core::CorePrimOp::Eq,
                    args: vec![
                        var_ref(n_odd),
                        CoreExpr::Lit(crate::core::CoreLit::Int(0), span()),
                    ],
                    span: span(),
                }),
                alts: vec![
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Lit(crate::core::CoreLit::Bool(true)),
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(ext_var(len)),
                            args: vec![var_ref(xs_odd)],
                            span: span(),
                        },
                        span: span(),
                    },
                    crate::core::CoreAlt {
                        pat: crate::core::CorePat::Wildcard,
                        guard: None,
                        rhs: CoreExpr::App {
                            func: Box::new(var_ref(even_binder)),
                            args: vec![
                                var_ref(xs_odd),
                                CoreExpr::PrimOp {
                                    op: crate::core::CorePrimOp::Sub,
                                    args: vec![
                                        var_ref(n_odd),
                                        CoreExpr::Lit(crate::core::CoreLit::Int(1), span()),
                                    ],
                                    span: span(),
                                },
                            ],
                            span: span(),
                        },
                        span: span(),
                    },
                ],
                span: span(),
            },
            true,
        );

        let mut program = CoreProgram {
            defs: vec![even_def, odd_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(even_binder.id)
                .expect("even signature")
                .params,
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed]
        );
        assert_eq!(
            registry
                .lookup_binder(odd_binder.id)
                .expect("odd signature")
                .params,
            vec![BorrowMode::Borrowed, BorrowMode::Borrowed]
        );
    }

    #[test]
    fn read_only_closure_capture_keeps_parameter_borrowed() {
        let mut interner = Interner::new();
        let outer_binder = binder(1, interner.intern("outer"));
        let xs = binder(2, interner.intern("xs"));
        let thunk = binder(3, interner.intern("thunk"));
        let len = interner.intern("len");

        let outer_def = def(
            outer_binder,
            vec![xs],
            CoreExpr::Let {
                var: thunk,
                rhs: Box::new(CoreExpr::Lam {
                    params: vec![],
                    body: Box::new(CoreExpr::App {
                        func: Box::new(ext_var(len)),
                        args: vec![var_ref(xs)],
                        span: span(),
                    }),
                    span: span(),
                }),
                body: Box::new(CoreExpr::App {
                    func: Box::new(var_ref(thunk)),
                    args: vec![],
                    span: span(),
                }),
                span: span(),
            },
            false,
        );

        let mut program = CoreProgram {
            defs: vec![outer_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(outer_binder.id)
                .expect("outer signature")
                .params,
            vec![BorrowMode::Borrowed]
        );
    }

    #[test]
    fn escaping_closure_capture_forces_parameter_owned() {
        let mut interner = Interner::new();
        let make_binder = binder(1, interner.intern("make"));
        let xs = binder(2, interner.intern("xs"));

        let make_def = def(
            make_binder,
            vec![xs],
            CoreExpr::Lam {
                params: vec![],
                body: Box::new(var_ref(xs)),
                span: span(),
            },
            false,
        );

        let mut program = CoreProgram {
            defs: vec![make_def],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));
        assert_eq!(
            registry
                .lookup_binder(make_binder.id)
                .expect("make signature")
                .params,
            vec![BorrowMode::Owned]
        );
    }

    #[test]
    fn base_runtime_entries_use_explicit_metadata() {
        let mut interner = Interner::new();
        let len = interner.intern("len");
        let push = interner.intern("push");
        let main = binder(1, interner.intern("main"));
        let xs = binder(2, interner.intern("xs"));
        let push_main = binder(3, interner.intern("push_main"));
        let ys = binder(4, interner.intern("ys"));
        let value = binder(5, interner.intern("value"));
        let mut program = CoreProgram {
            defs: vec![
                def(
                    main,
                    vec![xs],
                    CoreExpr::App {
                        func: Box::new(ext_var(len)),
                        args: vec![var_ref(xs)],
                        span: span(),
                    },
                    false,
                ),
                def(
                    push_main,
                    vec![ys, value],
                    CoreExpr::App {
                        func: Box::new(ext_var(push)),
                        args: vec![var_ref(ys), var_ref(value)],
                        span: span(),
                    },
                    false,
                ),
            ],
            top_level_items: Vec::new(),
        };
        let registry = infer_borrow_modes(&mut program, Some(&interner));

        assert_eq!(
            registry.lookup_name(len).expect("len signature").provenance,
            BorrowProvenance::BaseRuntime
        );
        assert_eq!(
            registry
                .lookup_name(push)
                .expect("push signature")
                .provenance,
            BorrowProvenance::BaseRuntime
        );
        assert!(
            registry.is_borrowed(BorrowCallee::BaseRuntime(len), 0),
            "len should borrow its argument"
        );
        assert!(
            !registry.is_borrowed(BorrowCallee::BaseRuntime(push), 0),
            "push should own at least its array argument"
        );
    }

    #[test]
    fn unresolved_direct_callee_gets_explicit_owned_fallback() {
        let mut interner = Interner::new();
        let ext_name = interner.intern("foreign_fn");
        let main_name = interner.intern("main");
        let main_binder = binder(1, main_name);
        let xs = binder(2, interner.intern("xs"));

        let mut program = CoreProgram {
            defs: vec![def(
                main_binder,
                vec![xs],
                CoreExpr::App {
                    func: Box::new(ext_var(ext_name)),
                    args: vec![var_ref(xs)],
                    span: span(),
                },
                false,
            )],
            top_level_items: Vec::new(),
        };

        let registry = infer_borrow_modes(&mut program, Some(&interner));
        let sig = registry
            .lookup_name(ext_name)
            .expect("explicit imported fallback should be recorded");
        assert_eq!(sig.provenance, BorrowProvenance::Imported);
        assert_eq!(sig.params, vec![BorrowMode::Owned]);
        let classified = registry.classify_var_ref(&CoreVarRef::unresolved(ext_name));
        assert_eq!(classified.provenance, BorrowProvenance::Imported);
        assert_eq!(classified.borrow_callee, BorrowCallee::Imported(ext_name));
    }

    #[test]
    fn indirect_calls_use_unknown_owned_fallback() {
        let mut interner = Interner::new();
        let registry = BorrowRegistry::default();
        assert!(
            !registry.is_borrowed(BorrowCallee::Unknown, 0),
            "unknown callees must default to owned arguments"
        );
        assert_eq!(
            registry
                .signature_for_callee(BorrowCallee::Unknown)
                .provenance,
            BorrowProvenance::Unknown
        );
        let unknown_name = interner.intern("totally_unknown");
        let classified = registry.classify_var_ref(&CoreVarRef::unresolved(unknown_name));
        assert_eq!(classified.provenance, BorrowProvenance::Unknown);
        assert_eq!(classified.borrow_callee, BorrowCallee::Unknown);
    }
}
