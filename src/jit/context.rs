use crate::runtime::{RuntimeContext, gc::GcHeap, value::Value};

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
    pub jit_functions: Vec<JitFunctionEntry>,
    /// When a runtime helper encounters an error, it stores the message here
    /// and returns NULL to the JIT code.
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
pub struct JitFunctionEntry {
    pub ptr: *const u8,
    pub num_params: usize,
}

impl JitContext {
    pub fn new() -> Self {
        Self {
            arena: ValueArena::new(),
            globals: vec![Value::None; 65536],
            constants: Vec::new(),
            gc_heap: GcHeap::new(),
            jit_functions: Vec::new(),
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

    pub fn set_jit_functions(&mut self, functions: Vec<JitFunctionEntry>) {
        self.jit_functions = functions;
    }
}

impl Default for JitContext {
    fn default() -> Self {
        Self::new()
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
            Value::JitClosure(closure) => {
                let entry = self
                    .jit_functions
                    .get(closure.function_index)
                    .ok_or_else(|| {
                        format!("unknown JIT function index: {}", closure.function_index)
                    })?
                    .to_owned();
                if args.len() != entry.num_params {
                    return Err(format!(
                        "wrong number of arguments: want={}, got={}",
                        entry.num_params,
                        args.len()
                    ));
                }

                let mut arg_ptrs: Vec<*mut Value> = Vec::with_capacity(args.len());
                for v in args {
                    arg_ptrs.push(self.alloc(v));
                }
                let mut capture_ptrs: Vec<*mut Value> = Vec::with_capacity(closure.captures.len());
                for v in &closure.captures {
                    capture_ptrs.push(self.alloc(v.clone()));
                }

                let func: unsafe extern "C" fn(
                    *mut JitContext,
                    *const *mut Value,
                    i64,
                    *const *mut Value,
                    i64,
                ) -> *mut Value = unsafe { std::mem::transmute(entry.ptr) };
                let result_ptr = unsafe {
                    func(
                        self as *mut JitContext,
                        arg_ptrs.as_ptr(),
                        arg_ptrs.len() as i64,
                        capture_ptrs.as_ptr(),
                        capture_ptrs.len() as i64,
                    )
                };
                if result_ptr.is_null() {
                    return Err(self
                        .take_error()
                        .unwrap_or_else(|| "unknown JIT call error".to_string()));
                }
                Ok(unsafe { (*result_ptr).clone() })
            }
            _ => Err(format!("JIT invoke_value: cannot call {}", callee)),
        }
    }

    fn gc_heap(&self) -> &GcHeap {
        &self.gc_heap
    }

    fn gc_heap_mut(&mut self) -> &mut GcHeap {
        &mut self.gc_heap
    }
}
