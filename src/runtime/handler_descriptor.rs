use crate::syntax::Identifier;

/// Stored in the constant pool by the compiler.
/// `OpHandle` reads this together with pre-built closures on the stack
/// to create a `HandlerFrame` at runtime.
///
/// The compiler emits one `OpClosure` per arm (in order) before `OpHandle`.
/// `OpHandle` pops `ops.len()` closures from the stack to build `HandlerArm`s.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerDescriptor {
    pub effect: Identifier,
    /// Op names in the same order as the closures left on the stack.
    pub ops: Vec<Identifier>,
}
