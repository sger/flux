//! `ModulePath` and `ClassId` — globally unique identity for type classes.
//!
//! Proposal 0151 introduces module-scoped type classes. To make two classes
//! with the same short name in different modules distinguishable at the
//! semantic and ABI level, every class is identified by a `ClassId` rather
//! than a bare `Identifier`.
//!
//! ## Phase 1a status
//!
//! These types are introduced as a parallel API alongside the existing
//! `Identifier`-keyed `ClassEnv` storage. Phase 1a does not yet flip the
//! storage — every `ClassId` produced today carries `ModulePath::empty()`
//! (the "no owning module" sentinel that means "use the legacy global name
//! table"). Phase 1b switches `ClassEnv` to key on `ClassId` directly and
//! removes the proxy.
//!
//! Keeping the public surface stable in Phase 1a means later phases can
//! migrate call sites incrementally without churning the API.
//!
//! ## Representation
//!
//! Both `ModulePath` and `ClassId` are `Copy` and store interned `Identifier`s
//! (`Symbol`s under the hood), so they participate in `HashMap` keys, equality
//! checks, and value-passing without any heap allocation. The dotted form of
//! a module path (e.g. `Flow.Foldable`) is interned as a single string by
//! [`Interner::intern_join`](crate::syntax::interner::Interner::intern_join),
//! so a `ModulePath` is just the symbol of that joined string.

use crate::syntax::Identifier;

/// A module path, e.g. `Flow.Foldable` or `App.Geometry.Inner`.
///
/// Internally a `ModulePath` is the interner symbol of the dotted form. The
/// special value [`ModulePath::empty`] represents "no owning module" and is
/// used during Phase 1a as a sentinel for legacy top-level declarations and
/// for identifiers that have not yet been associated with a module.
///
/// Phase 1b will phase out the empty sentinel by walking module bodies during
/// class collection and assigning a real path to every declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    ///
    /// In Phase 1a this is the only `ModulePath` produced by the parser.
    /// In Phase 1b it becomes a transitional value that the class collector
    /// stops emitting.
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
/// During Phase 1a, every `ClassId` constructed by the compiler has
/// `module == ModulePath::EMPTY`, so the new API is a strict superset of the
/// existing `Identifier`-keyed lookups (no class name can collide with itself).
/// Phase 1b walks module bodies during class collection and starts producing
/// `ClassId`s with real module paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClassId {
    pub module: ModulePath,
    pub name: Identifier,
}

impl ClassId {
    /// Construct a `ClassId` for a class declared in a specific module.
    pub const fn new(module: ModulePath, name: Identifier) -> Self {
        Self { module, name }
    }

    /// Construct a `ClassId` for a legacy top-level class (no owning module).
    ///
    /// Phase 1a uses this everywhere. Phase 1b replaces most uses with
    /// [`ClassId::new`] once the class collector tracks owning modules.
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
        // Phase 1a invariant: legacy top-level classes with the same short
        // name collapse to the same ClassId. Phase 1b removes this case by
        // walking module bodies and assigning real owning modules.
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
