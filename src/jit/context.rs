use crate::runtime::{
    RuntimeContext, function_contract::FunctionContract, gc::GcHeap, value::Value,
};
use crate::{
    diagnostics::{Diagnostic, DiagnosticsAggregator, RUNTIME_TYPE_ERROR},
    diagnostics::position::{Position, Span},
};
use std::collections::HashMap;

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
    pub named_functions: HashMap<String, usize>,
    pub source_file: Option<String>,
    pub source_text: Option<String>,
    /// When a runtime helper encounters an error, it stores the message here
    /// and returns NULL to the JIT code.
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct JitFunctionEntry {
    pub ptr: *const u8,
    pub num_params: usize,
    pub contract: Option<FunctionContract>,
}

impl JitContext {
    pub(crate) fn render_runtime_type_error(
        &self,
        expected: &str,
        actual: &str,
        span: Option<Span>,
    ) -> String {
        let file = self
            .source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string());
        let span = span.unwrap_or_else(|| Span::new(Position::new(1, 0), Position::new(1, 0)));
        let diag = Diagnostic::make_error(&RUNTIME_TYPE_ERROR, &[expected, actual], file.clone(), span);
        let mut agg =
            DiagnosticsAggregator::new(std::slice::from_ref(&diag)).with_file_headers(false);
        if let Some(src) = &self.source_text {
            agg = agg.with_source(file, src.clone());
        }
        agg.report().rendered
    }

    pub(crate) fn render_runtime_type_error_at(
        &self,
        expected: &str,
        actual: &str,
        line: usize,
        column_1_based: usize,
    ) -> String {
        let col0 = column_1_based.saturating_sub(1);
        let span = Span::new(Position::new(line, col0), Position::new(line, col0));
        self.render_runtime_type_error(expected, actual, Some(span))
    }

    pub(crate) fn check_contract_arg(
        &self,
        function_index: usize,
        arg_index: usize,
        value: &Value,
    ) -> Result<(), (String, String)> {
        let Some(entry) = self.jit_functions.get(function_index) else {
            return Ok(());
        };
        let Some(contract) = entry.contract.as_ref() else {
            return Ok(());
        };
        let Some(expected) = contract.params.get(arg_index).and_then(|t| t.as_ref()) else {
            return Ok(());
        };
        if expected.matches_value(value, self) {
            Ok(())
        } else {
            let expected_name = expected.type_name();
            Err((expected_name, value.type_name().to_string()))
        }
    }

    pub(crate) fn check_contract_return(
        &self,
        function_index: usize,
        value: &Value,
    ) -> Result<(), (String, String)> {
        let Some(entry) = self.jit_functions.get(function_index) else {
            return Ok(());
        };
        let Some(contract) = entry.contract.as_ref() else {
            return Ok(());
        };
        let Some(expected) = contract.ret.as_ref() else {
            return Ok(());
        };
        if expected.matches_value(value, self) {
            Ok(())
        } else {
            let expected_name = expected.type_name();
            Err((expected_name, value.type_name().to_string()))
        }
    }

    pub fn new() -> Self {
        Self {
            arena: ValueArena::new(),
            globals: vec![Value::None; 65536],
            constants: Vec::new(),
            gc_heap: GcHeap::new(),
            jit_functions: Vec::new(),
            named_functions: HashMap::new(),
            source_file: None,
            source_text: None,
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

    pub fn set_named_functions(&mut self, functions: HashMap<String, usize>) {
        self.named_functions = functions;
    }

    pub fn set_source_context(&mut self, file: Option<String>, source: Option<String>) {
        self.source_file = file;
        self.source_text = source;
    }
}

impl Default for JitContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeContext for JitContext {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String> {
        use crate::runtime::base::get_base_function_by_index;

        match callee {
            Value::BaseFunction(idx) => {
                let base = get_base_function_by_index(idx as usize)
                    .ok_or_else(|| format!("unknown Base function index: {}", idx))?;
                (base.func)(self, args)
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
                for (index, arg) in args.iter().enumerate() {
                    if let Err((expected, actual)) =
                        self.check_contract_arg(closure.function_index, index, arg)
                    {
                        return Err(self.render_runtime_type_error(&expected, &actual, None));
                    }
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
                if let Some(err) = self.take_error() {
                    return Err(err);
                }
                let result = unsafe { (*result_ptr).clone() };
                if let Err((expected, actual)) =
                    self.check_contract_return(closure.function_index, &result)
                {
                    return Err(self.render_runtime_type_error(&expected, &actual, None));
                }
                Ok(result)
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
