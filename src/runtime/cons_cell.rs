use std::rc::Rc;

use crate::runtime::value::Value;

/// Rc-based cons cell for persistent linked lists.
///
/// Replaces `HeapObject::Cons` (GC-managed) with reference-counted cells
/// as part of the Aether memory model (proposal 0084, Phase 1).
#[derive(Debug, Clone, PartialEq)]
pub struct ConsCell {
    pub head: Value,
    pub tail: Value,
}

impl ConsCell {
    pub fn new(head: Value, tail: Value) -> Self {
        Self { head, tail }
    }

    /// Construct a `Value::Cons` from head and tail.
    pub fn cons(head: Value, tail: Value) -> Value {
        Value::Cons(Rc::new(Self { head, tail }))
    }
}

impl Drop for ConsCell {
    fn drop(&mut self) {
        // Iterative drop to prevent stack overflow on deep lists.
        // Without this, a list of 10K+ elements would recursively drop
        // Rc<ConsCell> nodes and overflow the stack.
        let mut cur = std::mem::replace(&mut self.tail, Value::EmptyList);
        loop {
            match cur {
                Value::Cons(rc) => match Rc::try_unwrap(rc) {
                    Ok(mut cell) => {
                        cur = std::mem::replace(&mut cell.tail, Value::EmptyList);
                    }
                    Err(_) => break, // shared tail — other references exist, stop
                },
                _ => break,
            }
        }
    }
}
