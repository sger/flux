//! Type-constructor symbols used in inference types.

use std::fmt;

use crate::syntax::symbol::Symbol;

/// Concrete type constructors (0-argument types or type formers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeConstructor {
    /// Primitive 64-bit signed integer.
    Int,
    /// Primitive 64-bit floating-point value.
    Float,
    /// Primitive boolean.
    Bool,
    /// Primitive UTF-8 string.
    String,
    /// Unit type (spelled `None` in source-level type annotations).
    Unit,
    /// Bottom type (non-returning computations).
    Never,
    /// The gradual-typing escape: compatible with everything.
    Any,
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

impl fmt::Display for TypeConstructor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeConstructor::Int => write!(f, "Int"),
            TypeConstructor::Float => write!(f, "Float"),
            TypeConstructor::Bool => write!(f, "Bool"),
            TypeConstructor::String => write!(f, "String"),
            TypeConstructor::Unit => write!(f, "Unit"),
            TypeConstructor::Never => write!(f, "Never"),
            TypeConstructor::Any => write!(f, "Any"),
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
        assert_eq!(TypeConstructor::Unit.to_string(), "Unit");
        assert_eq!(TypeConstructor::Never.to_string(), "Never");
        assert_eq!(TypeConstructor::Any.to_string(), "Any");
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
}
