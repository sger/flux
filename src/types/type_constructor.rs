//! Type-constructor symbols used in inference types.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::syntax::symbol::Symbol;

/// Concrete type constructors (0-argument types or type formers).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeConstructor {
    /// Primitive 64-bit signed integer.
    Int,
    /// Primitive 64-bit floating-point value.
    Float,
    /// Primitive boolean.
    Bool,
    /// Primitive UTF-8 string.
    String,
    /// Primitive immutable byte buffer.
    Bytes,
    /// Unit type (spelled `None` in source-level type annotations).
    Unit,
    /// Bottom type (non-returning computations).
    Never,
    /// `List<T>` type constructor.
    List,
    /// `Array<T>` type constructor.
    Array,
    /// `Map<K, V>` type constructor.
    Map,
    /// `Option<T>` type constructor.
    Option,
    /// `Either<L, R>` type constructor.
    Either,
    /// User-defined ADT, keyed by its interned name symbol ID.
    ///
    /// Displays as `$<symbol_id>` (e.g. `$7`) — the `$` prefix comes from
    /// `Symbol`'s own `Display` impl, which renders the raw interned ID rather
    /// than the resolved string, since display here is diagnostic/debug only.
    Adt(Symbol),
}

impl TypeConstructor {
    /// The kind of this type constructor (Proposal 0123 Phase 5).
    ///
    /// Ground types have kind `Type`, unary constructors have kind `Type -> Type`,
    /// and binary constructors have kind `Type -> Type -> Type`.
    pub fn kind(&self) -> super::kind::Kind {
        use super::kind::Kind;
        match self {
            TypeConstructor::Int
            | TypeConstructor::Float
            | TypeConstructor::Bool
            | TypeConstructor::String
            | TypeConstructor::Bytes
            | TypeConstructor::Unit
            | TypeConstructor::Never => Kind::Type,

            TypeConstructor::List | TypeConstructor::Array | TypeConstructor::Option => {
                Kind::type1()
            }

            TypeConstructor::Map | TypeConstructor::Either => Kind::type2(),

            // User-defined ADTs default to Type. Parameterized ADTs would
            // need kind inference from their data declaration.
            TypeConstructor::Adt(_) => Kind::Type,
        }
    }

    /// Collect all `Symbol`s contained in this constructor.
    pub fn collect_symbols(&self, out: &mut std::collections::HashSet<Symbol>) {
        if let TypeConstructor::Adt(sym) = self {
            out.insert(*sym);
        }
    }

    /// Replace Symbol IDs according to `remap`. Returns a new constructor.
    pub fn remap_symbols(&self, remap: &std::collections::HashMap<Symbol, Symbol>) -> Self {
        match self {
            TypeConstructor::Adt(sym) => TypeConstructor::Adt(*remap.get(sym).unwrap_or(sym)),
            other => other.clone(),
        }
    }
}

impl fmt::Display for TypeConstructor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeConstructor::Int => write!(f, "Int"),
            TypeConstructor::Float => write!(f, "Float"),
            TypeConstructor::Bool => write!(f, "Bool"),
            TypeConstructor::String => write!(f, "String"),
            TypeConstructor::Bytes => write!(f, "Bytes"),
            TypeConstructor::Unit => write!(f, "Unit"),
            TypeConstructor::Never => write!(f, "Never"),
            TypeConstructor::List => write!(f, "List"),
            TypeConstructor::Array => write!(f, "Array"),
            TypeConstructor::Map => write!(f, "Map"),
            TypeConstructor::Option => write!(f, "Option"),
            TypeConstructor::Either => write!(f, "Either"),
            TypeConstructor::Adt(sym) => write!(f, "${sym}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TypeConstructor;
    use crate::syntax::symbol::Symbol;

    #[test]
    fn display_builtin_constructors() {
        assert_eq!(TypeConstructor::Int.to_string(), "Int");
        assert_eq!(TypeConstructor::Float.to_string(), "Float");
        assert_eq!(TypeConstructor::Bool.to_string(), "Bool");
        assert_eq!(TypeConstructor::String.to_string(), "String");
        assert_eq!(TypeConstructor::Bytes.to_string(), "Bytes");
        assert_eq!(TypeConstructor::Unit.to_string(), "Unit");
        assert_eq!(TypeConstructor::Never.to_string(), "Never");
        assert_eq!(TypeConstructor::List.to_string(), "List");
        assert_eq!(TypeConstructor::Array.to_string(), "Array");
        assert_eq!(TypeConstructor::Map.to_string(), "Map");
        assert_eq!(TypeConstructor::Option.to_string(), "Option");
        assert_eq!(TypeConstructor::Either.to_string(), "Either");
    }

    #[test]
    fn display_adt_constructor_uses_symbol() {
        let sym = Symbol::new(7);
        assert_eq!(TypeConstructor::Adt(sym).to_string(), "$$7");
    }

    #[test]
    fn collect_symbols_finds_adt_symbol() {
        let sym = Symbol::new(42);
        let tc = TypeConstructor::Adt(sym);
        let mut out = std::collections::HashSet::new();
        tc.collect_symbols(&mut out);
        assert!(out.contains(&sym));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn collect_symbols_empty_for_builtins() {
        let mut out = std::collections::HashSet::new();
        TypeConstructor::Int.collect_symbols(&mut out);
        TypeConstructor::List.collect_symbols(&mut out);
        TypeConstructor::Map.collect_symbols(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn remap_symbols_rewrites_adt() {
        let old = Symbol::new(5);
        let new = Symbol::new(99);
        let tc = TypeConstructor::Adt(old);
        let remap = std::collections::HashMap::from([(old, new)]);
        assert_eq!(tc.remap_symbols(&remap), TypeConstructor::Adt(new));
    }

    #[test]
    fn remap_symbols_preserves_builtins() {
        let remap = std::collections::HashMap::from([(Symbol::new(1), Symbol::new(2))]);
        assert_eq!(
            TypeConstructor::Int.remap_symbols(&remap),
            TypeConstructor::Int
        );
        assert_eq!(
            TypeConstructor::List.remap_symbols(&remap),
            TypeConstructor::List
        );
    }

    #[test]
    fn remap_symbols_preserves_unmapped_adt() {
        let sym = Symbol::new(10);
        let tc = TypeConstructor::Adt(sym);
        let remap = std::collections::HashMap::new();
        assert_eq!(tc.remap_symbols(&remap), TypeConstructor::Adt(sym));
    }
}
