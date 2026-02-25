use std::rc::Rc;

use crate::{runtime::closure::Closure, syntax::Identifier};

/// A single compiled handler arm: `op(resume, ...params) -> body`.
///
/// At runtime each arm is a `Closure` with signature
/// `fn(resume_cont, arg0, ..., argN)`.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerArm {
    pub op: Identifier,
    pub closure: Rc<Closure>,
}
