use crate::runtime::{
    RuntimeContext, base::list_ops::format_value, function_contract::FunctionContract, gc::GcHeap,
    value::Value,
};
use crate::{
    diagnostics::position::{Position, Span},
    diagnostics::{
        Diagnostic, DiagnosticPhase, ErrorType, render_runtime_diagnostic, runtime_type_error,
    },
};
use std::collections::HashMap;

use super::value_arena::ValueArena;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitCallAbi {
    Array,
    Reg1,
    Reg2,
    Reg3,
    Reg4,
}

impl JitCallAbi {
    pub fn from_arity(arity: usize) -> Self {
        match arity {
            1 => Self::Reg1,
            2 => Self::Reg2,
            3 => Self::Reg3,
            4 => Self::Reg4,
            _ => Self::Array,
        }
    }

    pub fn uses_array_args(self) -> bool {
        matches!(self, Self::Array)
    }

    pub fn direct_arg_count(self) -> usize {
        match self {
            Self::Array => 0,
            Self::Reg1 => 1,
            Self::Reg2 => 2,
            Self::Reg3 => 3,
            Self::Reg4 => 4,
        }
    }

    pub fn captures_param_index(self) -> usize {
        1 + self.direct_arg_count() + usize::from(self.uses_array_args()) * 2
    }

    pub fn ncaptures_param_index(self) -> usize {
        self.captures_param_index() + 1
    }
}

/// A single arm of a JIT handler: maps an effect operation symbol ID to its arm closure.
#[derive(Clone)]
pub struct JitHandlerArm {
    /// Symbol ID of the operation name (e.g. `Console.print` → symbol of `print`).
    pub op: u32,
    /// Pre-compiled arm closure value (`Value::JitClosure`).
    pub closure: Value,
}

/// An active handler pushed onto `JitContext::handler_stack` by `rt_push_handler`.
#[derive(Clone)]
pub struct JitHandlerFrame {
    /// Symbol ID of the effect name (e.g. symbol of `Console`).
    pub effect: u32,
    pub arms: Vec<JitHandlerArm>,
}

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
    /// Active effect handlers pushed by `rt_push_handler` / popped by `rt_pop_handler`.
    pub handler_stack: Vec<JitHandlerFrame>,
    /// Explicit GC shadow roots pushed around helper safepoints.
    pub shadow_roots: Vec<*mut Value>,
    pub shadow_frames: Vec<usize>,
    /// Function index of the JIT-compiled identity closure used as the `resume`
    /// value passed to handler arms (shallow handlers: resume returns its argument).
    pub identity_fn_index: usize,
}

#[derive(Clone)]
pub struct JitFunctionEntry {
    pub ptr: *const u8,
    pub num_params: usize,
    pub call_abi: JitCallAbi,
    pub contract: Option<FunctionContract>,
}

impl JitContext {
    fn span_from_1_based(
        &self,
        start_line: usize,
        start_column_1_based: usize,
        end_line: usize,
        end_column_1_based: usize,
    ) -> Span {
        let start_col0 = start_column_1_based.saturating_sub(1);
        let end_col0 = end_column_1_based.saturating_sub(1);
        Span::new(
            Position::new(start_line, start_col0),
            Position::new(end_line, end_col0),
        )
    }

    pub(crate) fn render_runtime_type_error(
        &self,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
        span: Option<Span>,
    ) -> String {
        let file = self
            .source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string());
        let span = span.unwrap_or_else(|| Span::new(Position::new(1, 0), Position::new(1, 0)));
        let diag = runtime_type_error(expected, actual, value_preview, file.clone(), span);
        render_runtime_diagnostic(&diag, &file, self.source_text.as_deref(), &[])
    }

    pub(crate) fn render_runtime_type_error_at(
        &self,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
        start_line: usize,
        start_column_1_based: usize,
        end_line: usize,
        end_column_1_based: usize,
    ) -> String {
        let span = self.span_from_1_based(
            start_line,
            start_column_1_based,
            end_line,
            end_column_1_based,
        );
        self.render_runtime_type_error(expected, actual, value_preview, Some(span))
    }

    /// Render a generic runtime error through the diagnostics system.
    /// `line` is 1-based; `column` is 1-based.
    /// Produces the same formatted output (colour, source snippet) as VM runtime errors.
    pub(crate) fn render_runtime_error(
        &self,
        code: &str,
        title: &str,
        message: &str,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    ) -> String {
        let file = self
            .source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string());
        let span = self.span_from_1_based(start_line, start_column, end_line, end_column);
        let diag = Diagnostic::make_error_dynamic(
            code,
            title,
            ErrorType::Runtime,
            message,
            None,
            file.clone(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime);
        render_runtime_diagnostic(&diag, &file, self.source_text.as_deref(), &[])
    }

    pub(crate) fn render_runtime_error_message(
        &self,
        code: &str,
        message: &str,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    ) -> String {
        let (title, details) = split_first_line(message);
        self.render_runtime_error(
            code,
            title.trim(),
            details.trim(),
            start_line,
            start_column,
            end_line,
            end_column,
        )
    }

    pub(crate) fn render_runtime_error_from_string(
        &self,
        message: &str,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    ) -> String {
        if is_rendered_runtime_diagnostic(message) {
            return message.to_string();
        }

        let (message, hint) = split_hint(message);
        let (title, details) = split_first_line(message);
        if let Some(actual) = title.strip_prefix("not callable: ") {
            return self.render_runtime_error(
                "E1001",
                "Not A Function",
                &format!("Cannot call non-function value (got {}).", actual.trim()),
                start_line,
                start_column,
                end_line,
                end_column,
            );
        }
        let code = classify_runtime_error_code(title);

        let file = self
            .source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string());
        let span = self.span_from_1_based(start_line, start_column, end_line, end_column);
        let diag = Diagnostic::make_error_dynamic(
            code,
            title.trim(),
            ErrorType::Runtime,
            details.trim(),
            hint.map(|h| h.trim().to_string()),
            file.clone(),
            span,
        )
        .with_phase(DiagnosticPhase::Runtime);
        render_runtime_diagnostic(&diag, &file, self.source_text.as_deref(), &[])
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
            handler_stack: Vec::new(),
            shadow_roots: Vec::new(),
            shadow_frames: Vec::new(),
            identity_fn_index: usize::MAX,
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

    pub fn push_gc_roots(&mut self, ptrs: &[*mut Value]) {
        self.shadow_frames.push(self.shadow_roots.len());
        self.shadow_roots.extend_from_slice(ptrs);
    }

    pub fn pop_gc_roots(&mut self) {
        if let Some(start) = self.shadow_frames.pop() {
            self.shadow_roots.truncate(start);
        }
    }

    pub fn collect_gc(&mut self) {
        let mut roots: Vec<Value> = Vec::with_capacity(
            self.shadow_roots.len()
                + self.globals.len()
                + self.constants.len()
                + self.handler_stack.len(),
        );
        for ptr in &self.shadow_roots {
            if let Some(value) = unsafe { ptr.as_ref() } {
                roots.push(value.clone());
            }
        }
        roots.extend(self.globals.iter().cloned());
        roots.extend(self.constants.iter().cloned());
        for frame in &self.handler_stack {
            for arm in &frame.arms {
                roots.push(arm.closure.clone());
            }
        }
        self.gc_heap.collect_roots(roots.iter());
    }
}

fn split_first_line(message: &str) -> (&str, &str) {
    if let Some((title, rest)) = message.split_once('\n') {
        (title, rest)
    } else {
        (message, "")
    }
}

fn split_hint(message: &str) -> (&str, Option<&str>) {
    if let Some((body, hint)) = message.split_once("\n\nHint:\n") {
        (body, Some(hint))
    } else {
        (message, None)
    }
}

fn is_rendered_runtime_diagnostic(message: &str) -> bool {
    message.starts_with("• ") || message.starts_with("Error[") || message.starts_with("error[")
}

fn classify_runtime_error_code(title: &str) -> &'static str {
    if title.contains("wrong number of arguments") {
        "E1000"
    } else if title.contains("division by zero") {
        "E1008"
    } else if title.contains("not a function") || title.contains("not callable") {
        "E1001"
    } else if title.contains("expected") || title.contains("expects") {
        "E1004"
    } else {
        "E1009"
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
                base.call_owned(self, args)
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
                        let preview = format_value(self, arg);
                        return Err(self.render_runtime_type_error(
                            &expected,
                            &actual,
                            Some(&preview),
                            None,
                        ));
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

                let result_ptr = unsafe {
                    match entry.call_abi {
                        JitCallAbi::Array => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *const *mut Value,
                                i64,
                                *const *mut Value,
                                i64,
                            )
                                -> *mut Value = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_ptrs.as_ptr(),
                                arg_ptrs.len() as i64,
                                capture_ptrs.as_ptr(),
                                capture_ptrs.len() as i64,
                            )
                        }
                        JitCallAbi::Reg1 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *mut Value,
                                *const *mut Value,
                                i64,
                            )
                                -> *mut Value = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_ptrs[0],
                                capture_ptrs.as_ptr(),
                                capture_ptrs.len() as i64,
                            )
                        }
                        JitCallAbi::Reg2 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *mut Value,
                                *mut Value,
                                *const *mut Value,
                                i64,
                            )
                                -> *mut Value = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_ptrs[0],
                                arg_ptrs[1],
                                capture_ptrs.as_ptr(),
                                capture_ptrs.len() as i64,
                            )
                        }
                        JitCallAbi::Reg3 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *mut Value,
                                *mut Value,
                                *mut Value,
                                *const *mut Value,
                                i64,
                            )
                                -> *mut Value = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_ptrs[0],
                                arg_ptrs[1],
                                arg_ptrs[2],
                                capture_ptrs.as_ptr(),
                                capture_ptrs.len() as i64,
                            )
                        }
                        JitCallAbi::Reg4 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *mut Value,
                                *mut Value,
                                *mut Value,
                                *mut Value,
                                *const *mut Value,
                                i64,
                            )
                                -> *mut Value = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_ptrs[0],
                                arg_ptrs[1],
                                arg_ptrs[2],
                                arg_ptrs[3],
                                capture_ptrs.as_ptr(),
                                capture_ptrs.len() as i64,
                            )
                        }
                    }
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
                    let preview = format_value(self, &result);
                    return Err(self.render_runtime_type_error(
                        &expected,
                        &actual,
                        Some(&preview),
                        None,
                    ));
                }
                Ok(result)
            }
            _ => Err(format!("not callable: {}", callee.type_name())),
        }
    }

    fn invoke_base_function_borrowed(
        &mut self,
        base_fn_index: usize,
        args: &[&Value],
    ) -> Result<Value, String> {
        use crate::runtime::base::get_base_function_by_index;

        let base = get_base_function_by_index(base_fn_index)
            .ok_or_else(|| format!("unknown Base function index: {}", base_fn_index))?;
        base.call_borrowed(self, args)
    }

    fn gc_heap(&self) -> &GcHeap {
        &self.gc_heap
    }

    fn gc_heap_mut(&mut self) -> &mut GcHeap {
        &mut self.gc_heap
    }

    fn callable_contract<'a>(&'a self, callee: &'a Value) -> Option<&'a FunctionContract> {
        match callee {
            Value::JitClosure(closure) => self
                .jit_functions
                .get(closure.function_index)
                .and_then(|entry| entry.contract.as_ref()),
            Value::Closure(closure) => closure.function.contract.as_ref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::JitContext;
    use crate::runtime::{
        gc::heap_object::HeapObject,
        value::{AdtFields, Value},
    };

    #[test]
    fn collect_gc_preserves_shadow_rooted_gc_adt() {
        let mut ctx = JitContext::new();
        let list = ctx.gc_heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        let adt = ctx.gc_heap.alloc(HeapObject::Adt {
            constructor: Rc::from("Node"),
            fields: AdtFields::from_vec(vec![Value::Gc(list)]),
        });
        let root = ctx.alloc(Value::GcAdt(adt));
        ctx.push_gc_roots(&[root]);

        ctx.gc_heap.alloc(HeapObject::Cons {
            head: Value::Integer(99),
            tail: Value::None,
        });

        ctx.collect_gc();
        assert_eq!(ctx.gc_heap.live_count(), 2);
        assert_eq!(unsafe { &*root }.adt_constructor(&ctx.gc_heap), Some("Node"));

        ctx.pop_gc_roots();
        ctx.collect_gc();
        assert_eq!(ctx.gc_heap.live_count(), 0);
    }
}
