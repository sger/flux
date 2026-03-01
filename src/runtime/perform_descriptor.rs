use crate::syntax::Identifier;

/// Stored in the constant pool by the compiler.
/// `OpPerform` reads this to identify which effect/op is being performed.
#[derive(Debug, Clone, PartialEq)]
pub struct PerformDescriptor {
    pub effect: Identifier,
    pub op: Identifier,
    /// Human-readable names for runtime error messages resolved at compile time.
    pub effect_name: Box<str>,
    pub op_name: Box<str>,
}
