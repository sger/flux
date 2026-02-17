use crate::runtime::{
    RuntimeContext,
    gc::GcHeap,
    value::Value,
};

use super::value_arena::ValueArena;

/// Execution context for JIT-compiled code.
///
/// Holds the value arena, globals, constants, GC heap, and error state.
/// Passed as a `*mut JitContext` (i64) to all JIT-compiled functions and
/// runtime helpers.
pub struct JitContext {
    pub arena: ValueArena,
    pub globals: Vec<Value>,
    pub constants: Vec<Value>,
    pub gc_heap: GcHeap,
    /// When a runtime helper encounters an error, it stores the message here
    /// and returns NULL to the JIT code.
    pub error: Option<String>,
}

impl JitContext {
    pub fn new() -> Self {
        Self {
            arena: ValueArena::new(),
            globals: vec![Value::None; 65536],
            constants: Vec::new(),
            gc_heap: GcHeap::new(),
            error: None,
        }
    }

    /// Allocate a Value in the arena, returning a stable pointer.
    pub fn alloc(&mut self, value: Value) -> *mut Value {
        self.arena.alloc(value)
    }

    /// Take the stored error message, if any.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

impl RuntimeContext for JitContext {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        use crate::runtime::builtins::get_builtin_by_index;

        match callee {
            Value::Builtin(idx) => {
                let builtin = get_builtin_by_index(idx as usize)
                    .ok_or_else(|| format!("unknown builtin index: {}", idx))?;
                (builtin.func)(self, args)
            }
            _ => Err(format!(
                "JIT invoke_value: cannot call {}",
                callee
            )),
        }
    }

    fn gc_heap(&self) -> &GcHeap {
        &self.gc_heap
    }

    fn gc_heap_mut(&mut self) -> &mut GcHeap {
        &mut self.gc_heap
    }
}
