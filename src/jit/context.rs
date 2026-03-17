use crate::runtime::{
    RuntimeContext, base::list_ops::format_value, function_contract::FunctionContract, gc::GcHeap,
    value::Value,
};
use crate::runtime::nanbox::NanBox;
use crate::{
    diagnostics::position::{Position, Span},
    diagnostics::{
        Diagnostic, DiagnosticPhase, ErrorType, render_runtime_diagnostic, runtime_type_error,
    },
};
use std::collections::HashMap;

use super::value_arena::ValueArena;

pub const JIT_TAG_INT: i64 = 1;
pub const JIT_TAG_FLOAT: i64 = 2;
pub const JIT_TAG_BOOL: i64 = 3;
pub const JIT_TAG_PTR: i64 = 4;
pub const JIT_TAG_THUNK: i64 = 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JitTaggedValue {
    pub tag: i64,
    pub payload: i64,
}

impl JitTaggedValue {
    pub const fn int(value: i64) -> Self {
        Self {
            tag: JIT_TAG_INT,
            payload: value,
        }
    }

    pub const fn float_bits(bits: i64) -> Self {
        Self {
            tag: JIT_TAG_FLOAT,
            payload: bits,
        }
    }

    pub const fn bool(value: bool) -> Self {
        Self {
            tag: JIT_TAG_BOOL,
            payload: value as i64,
        }
    }

    pub fn ptr(ptr: *mut Value) -> Self {
        Self {
            tag: JIT_TAG_PTR,
            payload: ptr as i64,
        }
    }

    pub fn none() -> Self {
        Self::ptr(std::ptr::null_mut())
    }

    pub fn as_ptr(self) -> *mut Value {
        self.payload as *mut Value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitCallAbi {
    Array,
    Reg1,
    Reg2,
    Reg3,
    Reg4,
}

impl JitCallAbi {
    pub fn from_arity(_arity: usize) -> Self {
        // Always use Array ABI for a uniform calling convention.
        // This avoids ABI mismatches when struct-typed arguments
        // spill to the stack on aarch64.
        Self::Array
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
        if self.uses_array_args() {
            3
        } else {
            1 + self.direct_arg_count() * 2
        }
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

/// Target and arguments for a deferred mutual tail call.
/// Set by `rt_set_thunk`; consumed by the trampoline loop in `jit_execute`.
pub struct JitThunk {
    pub fn_index: usize,
    pub args: Vec<JitTaggedValue>,
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
    pub globals: Vec<NanBox>,
    pub constants: Vec<NanBox>,
    pub gc_heap: GcHeap,
    pub jit_functions: Vec<JitFunctionEntry>,
    pub named_functions: HashMap<String, usize>,
    pub source_file: Option<String>,
    pub source_text: Option<String>,
    /// When a runtime helper encounters a user-facing runtime error, it stores
    /// the structured diagnostic here and returns NULL to the JIT code.
    pub runtime_error: Option<Diagnostic>,
    /// Raw internal helper failures that are not yet mapped to a runtime diagnostic.
    pub error: Option<String>,
    /// Active effect handlers pushed by `rt_push_handler` / popped by `rt_pop_handler`.
    pub handler_stack: Vec<JitHandlerFrame>,
    /// Explicit GC shadow roots pushed around helper safepoints.
    pub shadow_roots: Vec<*mut Value>,
    pub shadow_frames: Vec<usize>,
    /// Function index of the JIT-compiled identity closure used as the `resume`
    /// value passed to handler arms (shallow handlers: resume returns its argument).
    pub identity_fn_index: usize,
    /// Pending mutual-tail-call thunk. `None` unless the last JIT return was
    /// `JIT_TAG_THUNK`, in which case the trampoline loop re-invokes this.
    pub pending_thunk: Option<JitThunk>,
    /// Cache of unit (nullary) ADT values keyed by constructor name.
    /// Each name is allocated exactly once; subsequent lookups return the same pointer.
    pub unit_adts: HashMap<String, *mut Value>,
}

#[derive(Clone)]
pub struct JitFunctionEntry {
    pub ptr: *const u8,
    pub num_params: usize,
    pub call_abi: JitCallAbi,
    pub contract: Option<FunctionContract>,
    pub return_span: Option<Span>,
}

impl JitContext {
    /// Drop bulk runtime state eagerly when the caller no longer needs the
    /// execution context, avoiding expensive process-exit teardown for large
    /// JIT runs.
    pub fn clear_runtime_state(&mut self) {
        self.globals.clear();
        self.constants.clear();
        self.handler_stack.clear();
        self.shadow_roots.clear();
        self.shadow_frames.clear();
        self.pending_thunk = None;
        self.runtime_error = None;
        self.error = None;
        self.unit_adts.clear();
        self.gc_heap = GcHeap::new();
        self.arena.reset();
    }

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

    fn default_source_file(&self) -> String {
        self.source_file
            .clone()
            .unwrap_or_else(|| "<jit>".to_string())
    }

    pub(crate) fn runtime_type_error_diagnostic(
        &self,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
        span: Option<Span>,
    ) -> Diagnostic {
        let file = self.default_source_file();
        let span = span.unwrap_or_else(|| Span::new(Position::new(1, 0), Position::new(1, 0)));
        runtime_type_error(expected, actual, value_preview, file, span)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn runtime_type_error_diagnostic_at(
        &self,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
        start_line: usize,
        start_column_1_based: usize,
        end_line: usize,
        end_column_1_based: usize,
    ) -> Diagnostic {
        let span = self.span_from_1_based(
            start_line,
            start_column_1_based,
            end_line,
            end_column_1_based,
        );
        self.runtime_type_error_diagnostic(expected, actual, value_preview, Some(span))
    }

    /// Build a generic runtime error diagnostic with the provided span.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn runtime_error_diagnostic(
        &self,
        code: &str,
        title: &str,
        message: &str,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    ) -> Diagnostic {
        let file = self.default_source_file();
        let span = self.span_from_1_based(start_line, start_column, end_line, end_column);
        Diagnostic::make_error_dynamic(code, title, ErrorType::Runtime, message, None, file, span)
            .with_phase(DiagnosticPhase::Runtime)
    }

    pub(crate) fn render_runtime_diagnostic(&self, diag: &Diagnostic) -> String {
        let file = diag.file().unwrap_or("<jit>");
        render_runtime_diagnostic(diag, file, self.source_text.as_deref(), &[])
    }

    pub(crate) fn set_runtime_error_diag(&mut self, diag: Diagnostic) {
        self.runtime_error = Some(diag);
        self.error = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_runtime_error_code(
        &mut self,
        code: &str,
        title: &str,
        message: &str,
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
    ) {
        let diag = self.runtime_error_diagnostic(
            code,
            title,
            message,
            start_line,
            start_column,
            end_line,
            end_column,
        );
        self.set_runtime_error_diag(diag);
    }

    pub(crate) fn set_internal_error(&mut self, message: impl Into<String>) {
        self.runtime_error = None;
        self.error = Some(message.into());
    }

    pub fn take_runtime_error(&mut self) -> Option<Diagnostic> {
        self.runtime_error.take()
    }

    pub fn take_internal_error(&mut self) -> Option<String> {
        self.error.take()
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

    pub(crate) fn contract_return_error_diagnostic(
        &self,
        function_index: usize,
        expected: &str,
        actual: &str,
        value_preview: Option<&str>,
    ) -> Diagnostic {
        let file = self.default_source_file();
        let span = self
            .jit_functions
            .get(function_index)
            .and_then(|entry| entry.return_span)
            .unwrap_or_else(|| Span::new(Position::new(1, 0), Position::new(1, 0)));
        runtime_type_error(expected, actual, value_preview, file, span)
    }

    pub fn new() -> Self {
        Self {
            arena: ValueArena::new(),
            globals: vec![NanBox::from_none(); 65536],
            constants: Vec::new(),
            gc_heap: GcHeap::new(),
            jit_functions: Vec::new(),
            named_functions: HashMap::new(),
            source_file: None,
            source_text: None,
            runtime_error: None,
            error: None,
            handler_stack: Vec::new(),
            shadow_roots: Vec::new(),
            shadow_frames: Vec::new(),
            identity_fn_index: usize::MAX,
            pending_thunk: None,
            unit_adts: HashMap::new(),
        }
    }

    /// Return a stable pointer to the unit ADT value for `name`, allocating it on first use.
    pub fn intern_unit_adt(&mut self, name: &str) -> *mut Value {
        if let Some(&ptr) = self.unit_adts.get(name) {
            return ptr;
        }
        let ptr = self.alloc(Value::AdtUnit(std::rc::Rc::new(name.to_string())));
        self.unit_adts.insert(name.to_string(), ptr);
        ptr
    }

    /// Allocate a Value in the arena, returning a stable pointer.
    pub fn alloc(&mut self, value: Value) -> *mut Value {
        self.arena.alloc(value)
    }

    pub fn boxed_to_tagged(&mut self, value: Value) -> JitTaggedValue {
        match value {
            Value::Integer(v) => JitTaggedValue::int(v),
            Value::Float(v) => JitTaggedValue::float_bits(v.to_bits() as i64),
            Value::Boolean(v) => JitTaggedValue::bool(v),
            other => JitTaggedValue::ptr(self.alloc(other)),
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn boxed_ptr_to_tagged(&mut self, value: *mut Value) -> JitTaggedValue {
        match unsafe { value.as_ref() } {
            Some(value) => self.boxed_to_tagged(value.clone()),
            None => JitTaggedValue::none(),
        }
    }

    pub fn tagged_to_boxed(&mut self, value: JitTaggedValue) -> *mut Value {
        match value.tag {
            JIT_TAG_INT => self.alloc(Value::Integer(value.payload)),
            JIT_TAG_FLOAT => self.alloc(Value::Float(f64::from_bits(value.payload as u64))),
            JIT_TAG_BOOL => self.alloc(Value::Boolean(value.payload != 0)),
            JIT_TAG_PTR => value.as_ptr(),
            _ => {
                self.set_internal_error(format!("unknown JIT tagged value tag: {}", value.tag));
                std::ptr::null_mut()
            }
        }
    }

    pub fn clone_from_tagged(&mut self, value: JitTaggedValue) -> Option<Value> {
        match value.tag {
            JIT_TAG_INT => Some(Value::Integer(value.payload)),
            JIT_TAG_FLOAT => Some(Value::Float(f64::from_bits(value.payload as u64))),
            JIT_TAG_BOOL => Some(Value::Boolean(value.payload != 0)),
            JIT_TAG_PTR => unsafe { value.as_ptr().as_ref().cloned() },
            _ => {
                self.set_internal_error(format!("unknown JIT tagged value tag: {}", value.tag));
                None
            }
        }
    }

    /// Read a global slot by index, returning a decoded `Value`.
    pub fn global_get(&self, idx: usize) -> Value {
        self.globals[idx].clone().to_value()
    }

    /// Write a `Value` into a global slot, encoding it as needed.
    pub fn global_set(&mut self, idx: usize, value: Value) {
        self.globals[idx] = NanBox::from_value(value);
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
            self.arena.iter().count()
                + self.shadow_roots.len()
                + self.globals.len()
                + self.constants.len()
                + self.handler_stack.len(),
        );
        roots.extend(self.arena.iter().cloned());
        for ptr in &self.shadow_roots {
            if let Some(value) = unsafe { ptr.as_ref() } {
                roots.push(value.clone());
            }
        }
        roots.extend(self.globals.iter().map(|s| s.clone().to_value()));
        roots.extend(self.constants.iter().map(|s| s.clone().to_value()));
        for frame in &self.handler_stack {
            for arm in &frame.arms {
                roots.push(arm.closure.clone());
            }
        }
        self.gc_heap.collect_roots(roots.iter());
    }
}

fn strip_leading_ansi_and_whitespace(mut message: &str) -> &str {
    loop {
        let trimmed = message.trim_start_matches(char::is_whitespace);
        if let Some(rest) = trimmed.strip_prefix("\u{1b}[")
            && let Some(end) = rest.find('m')
        {
            message = &rest[end + 1..];
            continue;
        }
        return trimmed;
    }
}

pub(crate) fn is_rendered_runtime_diagnostic(message: &str) -> bool {
    let message = strip_leading_ansi_and_whitespace(message);
    message.starts_with("• ") || message.starts_with("Error[") || message.starts_with("error[")
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
                        return Err(self.render_runtime_diagnostic(
                            &self.runtime_type_error_diagnostic(
                                &expected,
                                &actual,
                                Some(&preview),
                                None,
                            ),
                        ));
                    }
                }

                let mut arg_values: Vec<JitTaggedValue> = Vec::with_capacity(args.len());
                for v in args {
                    arg_values.push(self.boxed_to_tagged(v));
                }
                let mut capture_values: Vec<JitTaggedValue> =
                    Vec::with_capacity(closure.captures.len());
                for v in &closure.captures {
                    capture_values.push(self.boxed_to_tagged(v.clone()));
                }

                let result = unsafe {
                    match entry.call_abi {
                        JitCallAbi::Array => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                *const JitTaggedValue,
                                i64,
                                *const JitTaggedValue,
                                i64,
                            )
                                -> JitTaggedValue = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_values.as_ptr(),
                                arg_values.len() as i64,
                                capture_values.as_ptr(),
                                capture_values.len() as i64,
                            )
                        }
                        JitCallAbi::Reg1 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                i64,
                                i64,
                                *const JitTaggedValue,
                                i64,
                            )
                                -> JitTaggedValue = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_values[0].tag,
                                arg_values[0].payload,
                                capture_values.as_ptr(),
                                capture_values.len() as i64,
                            )
                        }
                        JitCallAbi::Reg2 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                i64,
                                i64,
                                i64,
                                i64,
                                *const JitTaggedValue,
                                i64,
                            )
                                -> JitTaggedValue = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_values[0].tag,
                                arg_values[0].payload,
                                arg_values[1].tag,
                                arg_values[1].payload,
                                capture_values.as_ptr(),
                                capture_values.len() as i64,
                            )
                        }
                        JitCallAbi::Reg3 => {
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                *const JitTaggedValue,
                                i64,
                            )
                                -> JitTaggedValue = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_values[0].tag,
                                arg_values[0].payload,
                                arg_values[1].tag,
                                arg_values[1].payload,
                                arg_values[2].tag,
                                arg_values[2].payload,
                                capture_values.as_ptr(),
                                capture_values.len() as i64,
                            )
                        }
                        JitCallAbi::Reg4 => {
                            // Pass individual (tag, payload) pairs instead of
                            // JitTaggedValue structs to match the Cranelift
                            // function signature exactly.  On aarch64 the C ABI
                            // may lay out spilled structs differently from
                            // individual i64 params.
                            let func: unsafe extern "C" fn(
                                *mut JitContext,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                i64,
                                *const JitTaggedValue,
                                i64,
                            )
                                -> JitTaggedValue = std::mem::transmute(entry.ptr);
                            func(
                                self as *mut JitContext,
                                arg_values[0].tag,
                                arg_values[0].payload,
                                arg_values[1].tag,
                                arg_values[1].payload,
                                arg_values[2].tag,
                                arg_values[2].payload,
                                arg_values[3].tag,
                                arg_values[3].payload,
                                capture_values.as_ptr(),
                                capture_values.len() as i64,
                            )
                        }
                    }
                };
                if result.tag == JIT_TAG_PTR && result.as_ptr().is_null() {
                    if let Some(diag) = self.take_runtime_error() {
                        return Err(self.render_runtime_diagnostic(&diag));
                    }
                    return Err(self
                        .take_internal_error()
                        .unwrap_or_else(|| "unknown JIT call error".to_string()));
                }
                if let Some(diag) = self.take_runtime_error() {
                    return Err(self.render_runtime_diagnostic(&diag));
                }
                if let Some(err) = self.take_internal_error() {
                    return Err(err);
                }
                let result = self
                    .clone_from_tagged(result)
                    .ok_or_else(|| "unknown JIT call error".to_string())?;
                if let Err((expected, actual)) =
                    self.check_contract_return(closure.function_index, &result)
                {
                    let preview = format_value(self, &result);
                    return Err(self.render_runtime_diagnostic(
                        &self.contract_return_error_diagnostic(
                            closure.function_index,
                            &expected,
                            &actual,
                            Some(&preview),
                        ),
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

    use super::{JitContext, is_rendered_runtime_diagnostic};
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
            constructor: Rc::new("Node".to_string()),
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
        assert_eq!(
            unsafe { &*root }.adt_constructor(&ctx.gc_heap),
            Some("Node")
        );

        ctx.pop_gc_roots();
        ctx.arena.reset();
        ctx.collect_gc();
        assert_eq!(ctx.gc_heap.live_count(), 0);
    }

    #[test]
    fn collect_gc_preserves_arena_rooted_gc_adt_without_shadow_roots() {
        let mut ctx = JitContext::new();
        let list = ctx.gc_heap.alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::None,
        });
        let adt = ctx.gc_heap.alloc(HeapObject::Adt {
            constructor: Rc::new("Node".to_string()),
            fields: AdtFields::from_vec(vec![Value::Gc(list)]),
        });
        let root = ctx.alloc(Value::GcAdt(adt));

        ctx.gc_heap.alloc(HeapObject::Cons {
            head: Value::Integer(99),
            tail: Value::None,
        });

        ctx.collect_gc();
        assert_eq!(ctx.gc_heap.live_count(), 2);
        assert_eq!(
            unsafe { &*root }.adt_constructor(&ctx.gc_heap),
            Some("Node")
        );
    }

    #[test]
    fn rendered_runtime_diagnostic_detection_accepts_plain_text_header() {
        let rendered = "• 1 error • examples/io/read_file_demo.flx\nerror[E1009]: read_file failed";
        assert!(is_rendered_runtime_diagnostic(rendered));
    }

    #[test]
    fn rendered_runtime_diagnostic_detection_ignores_leading_ansi_and_whitespace() {
        let rendered =
            "\n\u{1b}[1m• 1 error • examples/io/read_file_demo.flx\nerror[E1009]: read_file failed";
        assert!(is_rendered_runtime_diagnostic(rendered));
    }
}
