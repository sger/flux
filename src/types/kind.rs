//! Kind system for higher-kinded types (Proposal 0123 Phase 5).
//!
//! Kinds classify types: `Type` is the kind of value types (Int, String, etc.),
//! and `Arrow(k1, k2)` is the kind of type constructors (List : Type -> Type).
//!
//! This is deliberately simple — no kind polymorphism, no TypeInType,
//! no promoted types. Just `Type` and `->` between kinds.

use std::fmt;

/// The kind of a type or type constructor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The kind of value types: `Int`, `String`, `Bool`, `List<Int>`, etc.
    Type,
    /// The kind of type constructors: `k1 -> k2`.
    /// `List` has kind `Type -> Type`, `Map` has kind `Type -> Type -> Type`.
    Arrow(Box<Kind>, Box<Kind>),
}

impl Kind {
    /// Shorthand for `Type -> Type` (unary type constructors).
    pub fn type1() -> Self {
        Kind::Arrow(Box::new(Kind::Type), Box::new(Kind::Type))
    }

    /// Shorthand for `Type -> Type -> Type` (binary type constructors).
    pub fn type2() -> Self {
        Kind::Arrow(
            Box::new(Kind::Type),
            Box::new(Kind::Arrow(Box::new(Kind::Type), Box::new(Kind::Type))),
        )
    }

    /// Returns the arity (number of type arguments expected).
    pub fn arity(&self) -> usize {
        match self {
            Kind::Type => 0,
            Kind::Arrow(_, rest) => 1 + rest.arity(),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Type => write!(f, "Type"),
            Kind::Arrow(from, to) => {
                if matches!(from.as_ref(), Kind::Arrow(_, _)) {
                    write!(f, "({from}) -> {to}")
                } else {
                    write!(f, "{from} -> {to}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_display() {
        assert_eq!(Kind::Type.to_string(), "Type");
        assert_eq!(Kind::type1().to_string(), "Type -> Type");
        assert_eq!(Kind::type2().to_string(), "Type -> Type -> Type");
    }

    #[test]
    fn kind_arity() {
        assert_eq!(Kind::Type.arity(), 0);
        assert_eq!(Kind::type1().arity(), 1);
        assert_eq!(Kind::type2().arity(), 2);
    }
}
