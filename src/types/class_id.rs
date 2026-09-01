//! `ModulePath` and `ClassId` — globally unique identity for type classes.
//!
//! Proposal 0151 introduces module-scoped type classes. To make two classes
//! with the same short name in different modules distinguishable at the
//! semantic and ABI level, every class is identified by a `ClassId` rather
//! than a bare `Identifier`.
//!
//! ClassId is the semantic identity carried through inference, solving,
//! dispatch, dictionary elaboration, and serialized module interfaces. The
//! parser remains textual; source resolution is the boundary that produces a
//! ClassId.
//!
//! ## Representation
//!
//! Both `ModulePath` and `ClassId` are `Copy` and store interned `Identifier`s
//! (`Symbol`s under the hood), so they participate in `HashMap` keys, equality
//! checks, and value-passing without any heap allocation. The dotted form of
//! a module path (e.g. `Flow.Foldable`) is interned as a single string by
//! [`Interner::intern_join`](crate::syntax::interner::Interner::intern_join),
//! so a `ModulePath` is just the symbol of that joined string.

use std::collections::HashMap;

use crate::syntax::{Identifier, symbol::Symbol};
use serde::{Deserialize, Serialize};

/// A module path, e.g. `Flow.Foldable` or `App.Geometry.Inner`.
///
/// Internally a `ModulePath` is the interner symbol of the dotted form. The
/// special value [`ModulePath::empty`] represents "no owning module" and is
/// used for legacy top-level declarations and prelude classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModulePath(Identifier);

impl ModulePath {
    /// Construct a `ModulePath` from an interned dotted name.
    ///
    /// The caller is responsible for having already produced `name` via
    /// [`Interner::intern`](crate::syntax::interner::Interner::intern) or
    /// [`Interner::intern_join`](crate::syntax::interner::Interner::intern_join).
    pub const fn from_identifier(name: Identifier) -> Self {
        Self(name)
    }

    /// The empty path sentinel — `(ModulePath::EMPTY, name)` is interpreted
    /// as a legacy top-level declaration with no owning module.
    pub const EMPTY: ModulePath = ModulePath(Identifier::SENTINEL);

    /// Construct the empty-path sentinel. Equivalent to `ModulePath::EMPTY`.
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    /// Returns true if this is the empty-path sentinel.
    pub const fn is_empty(self) -> bool {
        self.0.as_u32() == Identifier::SENTINEL.as_u32()
    }

    /// Access the underlying interner symbol of the dotted form.
    ///
    /// Returns `None` for the empty sentinel, which has no resolvable string.
    pub const fn as_identifier(self) -> Option<Identifier> {
        if self.is_empty() { None } else { Some(self.0) }
    }
}

/// A globally-unique class identity: `(owning module, class name)`.
///
/// Two classes with the same short name in different modules are distinct
/// `ClassId`s and produce distinct mangled symbols, distinct dictionary
/// globals, and distinct `.flxi` entries.
///
/// Top-level/prelude classes use the empty module for legacy ABI names;
/// module-scoped declarations carry their actual dotted owning path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClassId {
    pub module: ModulePath,
    pub name: Identifier,
}

impl ClassId {
    /// Construct a `ClassId` for a class declared in a specific module.
    pub const fn new(module: ModulePath, name: Identifier) -> Self {
        Self { module, name }
    }

    /// Construct a `ClassId` for a legacy top-level or prelude class.
    pub const fn from_local_name(name: Identifier) -> Self {
        Self {
            module: ModulePath::EMPTY,
            name,
        }
    }

    /// Returns true if this class identity has no owning module (the legacy
    /// top-level case).
    pub const fn is_local(self) -> bool {
        self.module.is_empty()
    }

    /// Collect the interned symbols used by this identity for interface
    /// serialization/remapping.
    pub fn collect_symbols(self, out: &mut std::collections::HashSet<Symbol>) {
        out.insert(self.name);
        if let Some(module) = self.module.as_identifier() {
            out.insert(module);
        }
    }

    /// Remap both components of this identity through an interface symbol
    /// table.
    pub fn remap_symbols(self, remap: &HashMap<Symbol, Symbol>) -> Self {
        let module = self
            .module
            .as_identifier()
            .map(|id| remap.get(&id).copied().unwrap_or(id))
            .map(ModulePath::from_identifier)
            .unwrap_or(ModulePath::EMPTY);
        Self {
            module,
            name: remap.get(&self.name).copied().unwrap_or(self.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::interner::Interner;

    #[test]
    fn module_path_empty_is_empty() {
        let p = ModulePath::empty();
        assert!(p.is_empty());
        assert_eq!(p.as_identifier(), None);
    }

    #[test]
    fn module_path_from_real_identifier_is_not_empty() {
        let mut interner = Interner::new();
        let sym = interner.intern("Flow.Foldable");
        let p = ModulePath::from_identifier(sym);
        assert!(!p.is_empty());
        assert_eq!(p.as_identifier(), Some(sym));
    }

    #[test]
    fn class_id_from_local_name_has_empty_module() {
        let mut interner = Interner::new();
        let class_name = interner.intern("Eq");
        let id = ClassId::from_local_name(class_name);
        assert!(id.is_local());
        assert_eq!(id.name, class_name);
        assert_eq!(id.module, ModulePath::EMPTY);
    }

    #[test]
    fn class_id_with_module_is_not_local() {
        let mut interner = Interner::new();
        let module_sym = interner.intern("Flow.Foldable");
        let class_name = interner.intern("Foldable");
        let id = ClassId::new(ModulePath::from_identifier(module_sym), class_name);
        assert!(!id.is_local());
        assert_eq!(id.name, class_name);
        assert_eq!(id.module.as_identifier(), Some(module_sym));
    }

    #[test]
    fn two_class_ids_with_same_name_different_modules_are_distinct() {
        let mut interner = Interner::new();
        let class_name = interner.intern("Foldable");
        let mod_a = interner.intern("Flow.Foldable");
        let mod_b = interner.intern("App.Foldable");

        let id_a = ClassId::new(ModulePath::from_identifier(mod_a), class_name);
        let id_b = ClassId::new(ModulePath::from_identifier(mod_b), class_name);

        assert_ne!(id_a, id_b);
    }

    #[test]
    fn two_class_ids_with_same_name_and_empty_module_are_equal() {
        // Legacy top-level classes deliberately share the empty-module
        // identity; module-scoped declarations use their owning module.
        let mut interner = Interner::new();
        let class_name = interner.intern("Eq");
        let id_a = ClassId::from_local_name(class_name);
        let id_b = ClassId::from_local_name(class_name);
        assert_eq!(id_a, id_b);
    }

    #[test]
    fn class_id_is_copy_and_hashable() {
        // Compile-time assertions that the type satisfies the trait bounds
        // we need for HashMap<ClassId, _> and pass-by-value usage.
        fn assert_copy<T: Copy>() {}
        fn assert_hash<T: std::hash::Hash + Eq>() {}
        assert_copy::<ClassId>();
        assert_hash::<ClassId>();
        assert_copy::<ModulePath>();
        assert_hash::<ModulePath>();
    }
}
