use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, type_expr::TypeExpr},
};

/// One variant of a `data` declaration.
///
/// Fields are always stored positionally in `fields`. When the variant was
/// declared with named-field syntax (`Point { x: Float, y: Float }`),
/// `field_names` holds one entry per positional field in the same order —
/// `field_names[i]` is the declared name of `fields[i]`. When the variant
/// was declared positionally (`Point(Float, Float)`), `field_names` is
/// `None`.
///
/// Invariant: if `field_names` is `Some(v)`, then `v.len() == fields.len()`.
/// This is enforced at parse time; consumers can rely on it without
/// re-checking.
///
/// See proposal 0152 (Named Fields for Data Types).
#[derive(Debug, Clone)]
pub struct DataVariant {
    pub name: Identifier,
    pub fields: Vec<TypeExpr>,
    pub field_names: Option<Vec<Identifier>>,
    pub span: Span,
}

impl DataVariant {
    /// Returns `true` when this variant was declared with named fields.
    #[inline]
    pub fn is_named(&self) -> bool {
        self.field_names.is_some()
    }

    /// Returns the positional index of a named field, or `None` if the
    /// variant is positional or the name is unknown.
    pub fn field_index(&self, name: Identifier) -> Option<usize> {
        self.field_names.as_ref()?.iter().position(|n| *n == name)
    }
}
