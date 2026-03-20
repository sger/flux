//! Aether borrow metadata infrastructure.
//!
//! Phase C replaces the old name-only registry with explicit compiler-owned
//! borrow metadata that survives past inference and is available at every
//! Aether call site. Borrow facts come from three sources:
//! - inferred signatures for user-defined Core definitions
//! - explicit Base/runtime metadata
//! - conservative imported/unknown fallbacks

use std::collections::HashMap;

use crate::core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreVarRef};
use crate::syntax::interner::Interner;
use crate::syntax::Identifier;

use super::analysis::owned_use_count_with_registry;

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
        if let Some(binder) = var.binder {
            return BorrowCallee::Local(binder);
        }

        match self.lookup_name(var.name).map(|sig| sig.provenance) {
            Some(BorrowProvenance::BaseRuntime) => BorrowCallee::BaseRuntime(var.name),
            Some(BorrowProvenance::Imported) => BorrowCallee::Imported(var.name),
            Some(BorrowProvenance::Inferred) => BorrowCallee::Global(var.name),
            Some(BorrowProvenance::Unknown) | None => BorrowCallee::Imported(var.name),
        }
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
pub fn infer_borrow_modes(program: &mut CoreProgram, interner: Option<&Interner>) -> BorrowRegistry {
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

    loop {
        let mut changed = false;
        for def in &program.defs {
            let Some(signature) = infer_def_signature(def, &registry) else {
                continue;
            };
            if registry.upsert_user_signature(def.binder, signature) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for def in &mut program.defs {
        def.borrow_signature = registry.lookup_binder(def.binder.id).cloned();
    }

    registry
}

fn infer_def_signature(def: &CoreDef, registry: &BorrowRegistry) -> Option<BorrowSignature> {
    let (params, body) = extract_lam(&def.expr)?;
    let params = params
        .iter()
        .map(|param| {
            if owned_use_count_with_registry(param.id, body, registry) == 0 {
                BorrowMode::Borrowed
            } else {
                BorrowMode::Owned
            }
        })
        .collect();
    Some(BorrowSignature::new(params, BorrowProvenance::Inferred))
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
            && let Some(metadata) = crate::runtime::base::get_base_borrow_metadata(interner.resolve(name))
        {
            let mode = match metadata.arg_mode {
                crate::runtime::base_function::BaseFunctionArgMode::BorrowedOnly
                | crate::runtime::base_function::BaseFunctionArgMode::BorrowedPreferredWithOwnedFallback => BorrowMode::Borrowed,
                crate::runtime::base_function::BaseFunctionArgMode::OwnedOnly => BorrowMode::Owned,
            };
            registry.insert_named_if_absent(
                name,
                BorrowSignature::all(mode, metadata.arity, BorrowProvenance::BaseRuntime),
            );
            continue;
        }

        registry.insert_named_if_absent(
            name,
            BorrowSignature::all(BorrowMode::Owned, arity, BorrowProvenance::Imported),
        );
    }
}

fn collect_unresolved_callees(expr: &CoreExpr, unresolved: &mut HashMap<Identifier, usize>) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => collect_unresolved_callees(body, unresolved),
        CoreExpr::App { func, args, .. } => {
            if let CoreExpr::Var { var, .. } = func.as_ref() {
                if var.binder.is_none() {
                    unresolved
                        .entry(var.name)
                        .and_modify(|arity| *arity = (*arity).max(args.len()))
                        .or_insert(args.len());
                }
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
    use super::{
        BorrowCallee, BorrowMode, BorrowProvenance, BorrowRegistry, infer_borrow_modes,
    };
    use crate::core::{
        CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram, CoreVarRef,
    };
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

    fn def(binder: CoreBinder, params: Vec<CoreBinder>, body: CoreExpr, is_recursive: bool) -> CoreDef {
        CoreDef::new(binder, CoreExpr::lambda(params, body, span()), is_recursive, span())
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
            registry.lookup_binder(loop_binder.id).expect("loop signature").params,
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
            registry.lookup_name(push).expect("push signature").provenance,
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
    }

    #[test]
    fn indirect_calls_use_unknown_owned_fallback() {
        let registry = BorrowRegistry::default();
        assert!(
            !registry.is_borrowed(BorrowCallee::Unknown, 0),
            "unknown callees must default to owned arguments"
        );
        assert_eq!(
            registry.signature_for_callee(BorrowCallee::Unknown).provenance,
            BorrowProvenance::Unknown
        );
    }
}
