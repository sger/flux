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
    pub effect_name: Box<str>,
    /// Op names in the same order as the closures left on the stack.
    pub ops: Vec<Identifier>,
    pub op_names: Vec<Box<str>>,
    /// Parameterized handlers carry one mutable handler-frame value threaded
    /// through two-argument resume calls.
    pub has_state: bool,
    /// When `true`, all handler arms never use `resume`. `OpPerform` can skip
    /// continuation capture entirely — just unwind and call the arm directly.
    /// (Perceus Section 2.7.1: non-linear control flow safety.)
    pub is_discard: bool,
}
