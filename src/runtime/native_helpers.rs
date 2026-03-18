#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! Runtime helper functions callable from JIT-compiled code.
//!
//! All functions use `extern "C"` ABI and operate on JIT tagged values.
//! They receive a `*mut JitContext` as their first argument for arena allocation
//! and error reporting.

use std::ptr;
use std::rc::Rc;
use std::slice::from_raw_parts;
use std::str::from_utf8_unchecked;

use crate::diagnostics::position::{Position, Span};
use crate::diagnostics::{Diagnostic, DiagnosticPhase, ErrorType};
use crate::runtime::native_context::{
    JitHandlerArm, JitHandlerFrame, JitTaggedValue, is_rendered_runtime_diagnostic,
};
use crate::primop::{PrimOp, execute_primop};
use crate::runtime::RuntimeContext;
use crate::runtime::{
    base::get_base_function_by_index,
    base::list_ops::format_value,
    gc::{
        hamt::{hamt_empty, hamt_insert, hamt_lookup},
        heap_object::HeapObject,
    },
    jit_closure::JitClosure,
    value::{AdtFields, Value},
};

use crate::runtime::native_context::{
    JIT_TAG_BOOL, JIT_TAG_FLOAT, JIT_TAG_INT, JIT_TAG_PTR, JIT_TAG_THUNK, JitCallAbi, JitContext,
    JitFunctionEntry, JitThunk,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Safely dereference a JitContext pointer. Returns None if null.
unsafe fn ctx_ref<'a>(ctx: *mut JitContext) -> &'a mut JitContext {
    unsafe { &mut *ctx }
}

unsafe fn invoke_jit_entry(
    ctx: *mut JitContext,
    entry: &JitFunctionEntry,
    args: &[JitTaggedValue],
    captures: &[JitTaggedValue],
) -> JitTaggedValue {
    match entry.call_abi {
        JitCallAbi::Array => {
            type F = unsafe extern "C" fn(
                *mut JitContext,
                *const JitTaggedValue,
                i64,
                *const JitTaggedValue,
                i64,
            ) -> JitTaggedValue;
            let f: F = unsafe { std::mem::transmute(entry.ptr) };
            unsafe {
                f(
                    ctx,
                    args.as_ptr(),
                    args.len() as i64,
                    captures.as_ptr(),
                    captures.len() as i64,
                )
            }
        }
        JitCallAbi::Reg1 => {
            type F = unsafe extern "C" fn(
                *mut JitContext,
                i64,
                i64,
                *const JitTaggedValue,
                i64,
            ) -> JitTaggedValue;
            let f: F = unsafe { std::mem::transmute(entry.ptr) };
            let a0 = args[0];
            unsafe {
                f(
                    ctx,
                    a0.tag,
                    a0.payload,
                    captures.as_ptr(),
                    captures.len() as i64,
                )
            }
        }
        JitCallAbi::Reg2 => {
            type F = unsafe extern "C" fn(
                *mut JitContext,
                i64,
                i64,
                i64,
                i64,
                *const JitTaggedValue,
                i64,
            ) -> JitTaggedValue;
            let f: F = unsafe { std::mem::transmute(entry.ptr) };
            let a0 = args[0];
            let a1 = args[1];
            unsafe {
                f(
                    ctx,
                    a0.tag,
                    a0.payload,
                    a1.tag,
                    a1.payload,
                    captures.as_ptr(),
                    captures.len() as i64,
                )
            }
        }
        JitCallAbi::Reg3 => {
            type F = unsafe extern "C" fn(
                *mut JitContext,
                i64,
                i64,
                i64,
                i64,
                i64,
                i64,
                *const JitTaggedValue,
                i64,
            ) -> JitTaggedValue;
            let f: F = unsafe { std::mem::transmute(entry.ptr) };
            let a0 = args[0];
            let a1 = args[1];
            let a2 = args[2];
            unsafe {
                f(
                    ctx,
                    a0.tag,
                    a0.payload,
                    a1.tag,
                    a1.payload,
                    a2.tag,
                    a2.payload,
                    captures.as_ptr(),
                    captures.len() as i64,
                )
            }
        }
        JitCallAbi::Reg4 => {
            type F = unsafe extern "C" fn(
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
            ) -> JitTaggedValue;
            let f: F = unsafe { std::mem::transmute(entry.ptr) };
            let a0 = args[0];
            let a1 = args[1];
            let a2 = args[2];
            let a3 = args[3];
            unsafe {
                f(
                    ctx,
                    a0.tag,
                    a0.payload,
                    a1.tag,
                    a1.payload,
                    a2.tag,
                    a2.payload,
                    a3.tag,
                    a3.payload,
                    captures.as_ptr(),
                    captures.len() as i64,
                )
            }
        }
    }
}

fn drain_jit_thunks(ctx: &mut JitContext, mut result: JitTaggedValue) -> JitTaggedValue {
    while result.tag == JIT_TAG_THUNK {
        let Some(thunk) = ctx.pending_thunk.take() else {
            ctx.set_internal_error("JIT_TAG_THUNK returned without pending_thunk".to_string());
            return JitTaggedValue::none();
        };
        let Some(entry) = ctx.jit_functions.get(thunk.fn_index).cloned() else {
            ctx.set_internal_error(format!("unknown JIT function index: {}", thunk.fn_index));
            return JitTaggedValue::none();
        };
        result = unsafe { invoke_jit_entry(ctx as *mut JitContext, &entry, &thunk.args, &[]) };
    }
    result
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

fn classify_runtime_error_code(message: &str) -> &'static str {
    if message.contains("wrong number of arguments") {
        "E1000"
    } else if message.contains("division by zero") {
        "E1008"
    } else if message.contains("not a function") || message.contains("not callable") {
        "E1001"
    } else if message.contains("expected") || message.contains("expects") {
        "E1004"
    } else {
        "E1009"
    }
}

fn parse_rendered_runtime_diagnostic(err: &str) -> Option<Diagnostic> {
    let mut lines = err.lines();
    let header = lines.next()?.trim();
    if !header.starts_with("• ") {
        return None;
    }

    let error_line = lines.find(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("Error[") || trimmed.starts_with("error[")
    })?;
    let error_line = error_line.trim();
    let code_end = error_line.find(']')?;
    let prefix_len = if error_line.starts_with("Error[") {
        "Error[".len()
    } else {
        "error[".len()
    };
    let code = error_line.get(prefix_len..code_end)?;
    let title = error_line.get(code_end + 1..)?.trim().strip_prefix(": ")?;

    let mut message_lines = Vec::new();
    let mut location_line = None;
    for line in lines {
        if location_line.is_none()
            && line.starts_with("  ")
            && line.trim().contains(':')
            && !line.contains('|')
        {
            location_line = Some(line.trim().to_string());
            break;
        }
        if !line.trim().is_empty() {
            message_lines.push(line.trim_end().to_string());
        }
    }
    let location_line = location_line?;
    let (file_and_line, column) = location_line.rsplit_once(':')?;
    let (file, line) = file_and_line.rsplit_once(':')?;
    let line = line.parse::<usize>().ok()?;
    let column = column.parse::<usize>().ok()?;

    Some(
        Diagnostic::make_error_dynamic(
            code,
            title,
            ErrorType::Runtime,
            message_lines.join("\n").trim(),
            None,
            file.to_string(),
            Span::new(
                Position::new(line, column.saturating_sub(1)),
                Position::new(line, column.saturating_sub(1)),
            ),
        )
        .with_phase(DiagnosticPhase::Runtime),
    )
}

fn runtime_diagnostic_from_message(
    ctx: &JitContext,
    message: &str,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
) -> Diagnostic {
    if is_rendered_runtime_diagnostic(message)
        && let Some(diag) = parse_rendered_runtime_diagnostic(message)
    {
        return diag;
    }

    let (message, hint) = split_hint(message);
    let (title, details) = split_first_line(message);
    if let Some(actual) = title.strip_prefix("not callable: ") {
        return ctx.runtime_error_diagnostic(
            "E1001",
            "Not A Function",
            &format!("Cannot call non-function value (got {}).", actual.trim()),
            start_line,
            start_column,
            end_line,
            end_column,
        );
    }

    let file = ctx
        .source_file
        .clone()
        .unwrap_or_else(|| "<jit>".to_string());
    let span = Span::new(
        Position::new(start_line, start_column.saturating_sub(1)),
        Position::new(end_line, end_column.saturating_sub(1)),
    );
    Diagnostic::make_error_dynamic(
        classify_runtime_error_code(title),
        title.trim(),
        ErrorType::Runtime,
        details.trim(),
        hint.map(|h| h.trim().to_string()),
        file,
        span,
    )
    .with_phase(DiagnosticPhase::Runtime)
}

fn set_runtime_error_from_message(
    ctx: &mut JitContext,
    message: &str,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
) {
    if is_rendered_runtime_diagnostic(message)
        && parse_rendered_runtime_diagnostic(message).is_none()
    {
        // Preserve already-rendered diagnostics verbatim when they cannot be
        // losslessly converted back into our structured form.
        ctx.set_internal_error(message.to_string());
        return;
    }

    ctx.set_runtime_error_diag(runtime_diagnostic_from_message(
        ctx,
        message,
        start_line,
        start_column,
        end_line,
        end_column,
    ));
}

fn clone_tagged_arg(
    ctx: &mut JitContext,
    value: JitTaggedValue,
    label: &str,
    index: usize,
) -> Option<Value> {
    match ctx.clone_from_tagged(value) {
        Some(value) => Some(value),
        None => {
            if ctx.error.is_none() {
                ctx.set_internal_error(format!(
                    "{label} received invalid tagged value at index {index}"
                ));
            }
            None
        }
    }
}

fn clone_values_from_tagged_ptrs(
    ctx: &mut JitContext,
    values_ptr: *const JitTaggedValue,
    len: usize,
    label: &str,
) -> Option<Vec<Value>> {
    let mut values = Vec::with_capacity(len);
    for i in 0..len {
        let tagged = unsafe { *values_ptr.add(i) };
        values.push(clone_tagged_arg(ctx, tagged, label, i)?);
    }
    Some(values)
}

fn maybe_collect_gc(ctx: &mut JitContext) {
    if ctx.gc_heap.should_collect() {
        ctx.collect_gc();
    }
}

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

/// Lightweight helper for inline div/mod: sets "division by zero" error on
/// the JIT context. The JIT emits a null return immediately after this call.
#[unsafe(no_mangle)]
pub extern "C" fn rt_division_by_zero(ctx: *mut JitContext) {
    unsafe { ctx_ref(ctx) }.set_internal_error("division by zero");
}

/// Re-render the current `ctx.error` as a structured diagnostic with span
/// information.  Called from Cranelift-compiled code after a runtime helper
/// sets `ctx.error` to a raw message.  This produces the same formatted
/// output as the VM's diagnostic pipeline (error code, source snippet, span
/// highlight) — minus the stack trace which the JIT does not track.
#[unsafe(no_mangle)]
pub extern "C" fn rt_render_error_with_span(
    ctx: *mut JitContext,
    start_line: i64,
    start_col: i64,
    end_line: i64,
    end_col: i64,
) {
    let ctx = unsafe { ctx_ref(ctx) };
    if let Some(raw) = ctx.take_internal_error() {
        let diag = runtime_diagnostic_from_message(
            ctx,
            &raw,
            start_line as usize,
            start_col as usize,
            end_line as usize,
            end_col as usize,
        );
        ctx.set_runtime_error_diag(diag);
    }
}

// ---------------------------------------------------------------------------
// Value constructors
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_integer(_ctx: *mut JitContext, value: i64) -> JitTaggedValue {
    JitTaggedValue::int(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_float(_ctx: *mut JitContext, bits: i64) -> JitTaggedValue {
    JitTaggedValue::float_bits(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_bool(_ctx: *mut JitContext, value: i64) -> JitTaggedValue {
    JitTaggedValue::bool(value != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_none(ctx: *mut JitContext) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    JitTaggedValue::ptr(ctx.alloc(Value::None))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_force_boxed(ctx: *mut JitContext, value: JitTaggedValue) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let boxed = ctx.tagged_to_boxed(value);
    if boxed.is_null() {
        JitTaggedValue::none()
    } else {
        JitTaggedValue::ptr(boxed)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_push_gc_roots(
    ctx: *mut JitContext,
    values_ptr: *const JitTaggedValue,
    len: i64,
) {
    let ctx = unsafe { ctx_ref(ctx) };
    let values = unsafe { from_raw_parts(values_ptr, len as usize) };
    let mut roots = Vec::new();
    for value in values {
        if value.tag == JIT_TAG_PTR {
            let ptr = value.as_ptr();
            if !ptr.is_null() {
                roots.push(ptr);
            }
        }
    }
    ctx.push_gc_roots(&roots);
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_pop_gc_roots(ctx: *mut JitContext) {
    unsafe { ctx_ref(ctx) }.pop_gc_roots();
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_empty_list(ctx: *mut JitContext) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::EmptyList)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_string(ctx: *mut JitContext, ptr: *const u8, len: i64) -> *mut Value {
    let s = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len as usize)) };
    let rc: Rc<String> = Rc::new(s.to_string());
    unsafe { ctx_ref(ctx) }.alloc(Value::String(rc))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_base_function(ctx: *mut JitContext, base_fn_index: i64) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::BaseFunction(base_fn_index as u8))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_jit_closure(
    ctx: *mut JitContext,
    function_index: i64,
    captures_ptr: *const JitTaggedValue,
    ncaptures: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(captures) = clone_values_from_tagged_ptrs(
        ctx,
        captures_ptr,
        ncaptures as usize,
        "jit closure construction",
    ) else {
        return ptr::null_mut();
    };
    let closure = JitClosure::new(function_index as usize, captures);
    ctx.alloc(Value::JitClosure(Rc::new(closure)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_cons(
    ctx: *mut JitContext,
    head: *mut Value,
    tail: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if head.is_null() || tail.is_null() {
        ctx.error = Some("cons received null value".to_string());
        return ptr::null_mut();
    }
    let handle = ctx.gc_heap.alloc(HeapObject::Cons {
        head: unsafe { (*head).clone() },
        tail: unsafe { (*tail).clone() },
    });
    ctx.alloc(Value::Gc(handle))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_cons(ctx: *mut JitContext, value: *mut Value) -> i64 {
    if value.is_null() {
        return 0;
    }
    let ctx = unsafe { ctx_ref(ctx) };
    let is_cons = matches!(
        unsafe { &*value },
        Value::Gc(h) if matches!(ctx.gc_heap.get(*h), HeapObject::Cons { .. })
    );
    if is_cons { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_cons_head(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if value.is_null() {
        ctx.error = Some("cons head on null".to_string());
        return ptr::null_mut();
    }
    match unsafe { &*value } {
        Value::Gc(h) => match ctx.gc_heap.get(*h) {
            HeapObject::Cons { head, .. } => ctx.alloc(head.clone()),
            _ => {
                ctx.error = Some("cons head expected non-empty list".to_string());
                ptr::null_mut()
            }
        },
        _ => {
            ctx.error = Some("cons head expected non-empty list".to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_cons_tail(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if value.is_null() {
        ctx.error = Some("cons tail on null".to_string());
        return ptr::null_mut();
    }
    match unsafe { &*value } {
        Value::Gc(h) => match ctx.gc_heap.get(*h) {
            HeapObject::Cons { tail, .. } => ctx.alloc(tail.clone()),
            _ => {
                ctx.error = Some("cons tail expected non-empty list".to_string());
                ptr::null_mut()
            }
        },
        _ => {
            ctx.error = Some("cons tail expected non-empty list".to_string());
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_add(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::int(*l + *r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::float_bits((l + r).to_bits() as i64),
        (Value::Integer(l), Value::Float(r)) => {
            JitTaggedValue::float_bits((*l as f64 + *r).to_bits() as i64)
        }
        (Value::Float(l), Value::Integer(r)) => {
            JitTaggedValue::float_bits((l + *r as f64).to_bits() as i64)
        }
        (Value::String(l), Value::String(r)) => {
            JitTaggedValue::ptr(ctx.alloc(Value::String(format!("{}{}", l, r).into())))
        }
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot add {} and {} values.",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_sub(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::int(*l - *r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::float_bits((l - r).to_bits() as i64),
        (Value::Integer(l), Value::Float(r)) => {
            JitTaggedValue::float_bits((*l as f64 - *r).to_bits() as i64)
        }
        (Value::Float(l), Value::Integer(r)) => {
            JitTaggedValue::float_bits((l - *r as f64).to_bits() as i64)
        }
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot subtract {} and {} values.",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_mul(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::int(*l * *r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::float_bits((l * r).to_bits() as i64),
        (Value::Integer(l), Value::Float(r)) => {
            JitTaggedValue::float_bits((*l as f64 * *r).to_bits() as i64)
        }
        (Value::Float(l), Value::Integer(r)) => {
            JitTaggedValue::float_bits((l * *r as f64).to_bits() as i64)
        }
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot multiply {} and {} values.",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_div(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(_), Value::Integer(0)) | (Value::Float(_), Value::Integer(0)) => {
            ctx.error = Some("division by zero".to_string());
            JitTaggedValue::none()
        }
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::int(*l / *r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::float_bits((l / r).to_bits() as i64),
        (Value::Integer(l), Value::Float(r)) => {
            JitTaggedValue::float_bits((*l as f64 / *r).to_bits() as i64)
        }
        (Value::Float(l), Value::Integer(r)) => {
            JitTaggedValue::float_bits((l / *r as f64).to_bits() as i64)
        }
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot divide {} and {} values.",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_mod(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(_), Value::Integer(0)) | (Value::Float(_), Value::Integer(0)) => {
            ctx.error = Some("division by zero".to_string());
            JitTaggedValue::none()
        }
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::int(*l % *r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::float_bits((l % r).to_bits() as i64),
        (Value::Integer(l), Value::Float(r)) => {
            JitTaggedValue::float_bits((*l as f64 % *r).to_bits() as i64)
        }
        (Value::Float(l), Value::Integer(r)) => {
            JitTaggedValue::float_bits((l % *r as f64).to_bits() as i64)
        }
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot modulo {} and {} values.",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

// ---------------------------------------------------------------------------
// Prefix operators
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_negate(ctx: *mut JitContext, a: JitTaggedValue) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    match a {
        Value::Integer(v) => JitTaggedValue::int(-v),
        Value::Float(v) => JitTaggedValue::float_bits((-v).to_bits() as i64),
        _ => {
            ctx.error = Some(format!(
                "Invalid Operation\nCannot negate {} value.",
                a.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_not(ctx: *mut JitContext, a: JitTaggedValue) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    // Match VM's OpBang: negate truthiness of any value.
    JitTaggedValue::bool(!a.is_truthy())
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_truthy(ctx: *mut JitContext, a: JitTaggedValue) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    match ctx.clone_from_tagged(a) {
        Some(a) => i64::from(a.is_truthy()),
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_bool_value(ctx: *mut JitContext, a: JitTaggedValue) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return 0;
    };
    match a {
        Value::Boolean(v) => i64::from(v),
        _ => {
            if a.is_truthy() {
                1
            } else {
                0
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Comparisons
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_equal(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    let result = values_equal(ctx, &a, &b);
    JitTaggedValue::bool(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_not_equal(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    let result = values_equal(ctx, &a, &b);
    JitTaggedValue::bool(!result)
}

fn values_equal(ctx: &JitContext, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => l == r,
        (Value::Float(l), Value::Float(r)) => l == r,
        (Value::Integer(l), Value::Float(r)) => *l as f64 == *r,
        (Value::Float(l), Value::Integer(r)) => *l == *r as f64,
        (Value::Boolean(l), Value::Boolean(r)) => l == r,
        (Value::String(l), Value::String(r)) => l == r,
        (Value::None, Value::None) => true,
        (Value::None, _) | (_, Value::None) => false,
        (Value::Some(l), Value::Some(r)) => l == r,
        (Value::Left(l), Value::Left(r)) => l == r,
        (Value::Right(l), Value::Right(r)) => l == r,
        (Value::Tuple(l), Value::Tuple(r)) => l == r,
        (Value::AdtUnit(l), Value::AdtUnit(r)) => l == r,
        (left, right) if left.type_name() == "Adt" && right.type_name() == "Adt" => {
            match (left.as_adt(&ctx.gc_heap), right.as_adt(&ctx.gc_heap)) {
                (Some(left_adt), Some(right_adt)) => {
                    if left_adt.constructor() != right_adt.constructor() {
                        return false;
                    }
                    let left_fields = left_adt.fields();
                    let right_fields = right_adt.fields();
                    if left_fields.len() != right_fields.len() {
                        return false;
                    }
                    for i in 0..left_fields.len() {
                        if !values_equal(ctx, &left_fields[i], &right_fields[i]) {
                            return false;
                        }
                    }
                    true
                }
                _ => false,
            }
        }
        (Value::Left(_), Value::Right(_)) | (Value::Right(_), Value::Left(_)) => false,
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_greater_than(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::bool(l > r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::bool(l > r),
        (Value::Integer(l), Value::Float(r)) => JitTaggedValue::bool((*l as f64) > *r),
        (Value::Float(l), Value::Integer(r)) => JitTaggedValue::bool(*l > *r as f64),
        (Value::String(l), Value::String(r)) => JitTaggedValue::bool(l > r),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_less_than_or_equal(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::bool(l <= r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::bool(l <= r),
        (Value::Integer(l), Value::Float(r)) => JitTaggedValue::bool((*l as f64) <= *r),
        (Value::Float(l), Value::Integer(r)) => JitTaggedValue::bool(*l <= *r as f64),
        (Value::String(l), Value::String(r)) => JitTaggedValue::bool(l <= r),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_greater_than_or_equal(
    ctx: *mut JitContext,
    a: JitTaggedValue,
    b: JitTaggedValue,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(a) = ctx.clone_from_tagged(a) else {
        return JitTaggedValue::none();
    };
    let Some(b) = ctx.clone_from_tagged(b) else {
        return JitTaggedValue::none();
    };
    match (&a, &b) {
        (Value::Integer(l), Value::Integer(r)) => JitTaggedValue::bool(l >= r),
        (Value::Float(l), Value::Float(r)) => JitTaggedValue::bool(l >= r),
        (Value::Integer(l), Value::Float(r)) => JitTaggedValue::bool((*l as f64) >= *r),
        (Value::Float(l), Value::Integer(r)) => JitTaggedValue::bool(*l >= *r as f64),
        (Value::String(l), Value::String(r)) => JitTaggedValue::bool(l >= r),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            JitTaggedValue::none()
        }
    }
}

// ---------------------------------------------------------------------------
// Base function calls
// ---------------------------------------------------------------------------

/// Register a mutual tail call thunk so the trampoline loop in `jit_execute`
/// can re-invoke the target without growing the native call stack.
///
/// The JIT function that emits a mutual tail call stores the callee index and
/// the tagged argument array here, then returns `JIT_TAG_THUNK` to the caller.
/// The `JitContext::pending_thunk` field is consumed by `invoke_jit_thunk` in
/// `src/jit/mod.rs`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_set_thunk(
    ctx: *mut JitContext,
    fn_index: i64,
    args_ptr: *const JitTaggedValue,
    nargs: i64,
) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    let args = unsafe { from_raw_parts(args_ptr, nargs as usize) }.to_vec();
    ctx.pending_thunk = Some(JitThunk {
        fn_index: fn_index as usize,
        args,
    });
    JitTaggedValue {
        tag: JIT_TAG_THUNK,
        payload: 0,
    }
}

/// Call a Base function by index, passing args as a tagged-value array
/// (16 bytes each: `i64 tag` + `i64 payload`).
///
/// Unlike [`rt_call_base_function`], this avoids arena allocation for
/// unboxed `Int` / `Float` / `Bool` arguments: they are materialised inline
/// as stack `Value`s and passed by borrowed reference, so no `*mut Value`
/// arena slot is needed per primitive argument.
#[unsafe(no_mangle)]
pub extern "C" fn rt_call_base_function_tagged(
    ctx: *mut JitContext,
    base_fn_index: i64,
    args_ptr: *const JitTaggedValue,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match get_base_function_by_index(base_fn_index as usize) {
        Some(_) => {}
        None => {
            ctx.set_internal_error(format!("unknown Base function index: {}", base_fn_index));
            return ptr::null_mut();
        }
    }

    let nargs = nargs as usize;
    let tagged_args = unsafe { from_raw_parts(args_ptr, nargs) };

    // Materialise each arg. For Int/Float/Bool we own the Value on the stack;
    // for PTR we borrow through the raw arena pointer. Pre-allocating
    // `owned_storage` to `nargs` ensures it never reallocates, keeping
    // the pointers we store in `refs` valid for the entire call.
    let mut owned_storage: Vec<Value> = Vec::with_capacity(nargs);
    let mut refs: Vec<*const Value> = Vec::with_capacity(nargs);
    for &tagged in tagged_args {
        match tagged.tag {
            JIT_TAG_INT => {
                owned_storage.push(Value::Integer(tagged.payload));
                refs.push(owned_storage.last().unwrap() as *const Value);
            }
            JIT_TAG_FLOAT => {
                owned_storage.push(Value::Float(f64::from_bits(tagged.payload as u64)));
                refs.push(owned_storage.last().unwrap() as *const Value);
            }
            JIT_TAG_BOOL => {
                owned_storage.push(Value::Boolean(tagged.payload != 0));
                refs.push(owned_storage.last().unwrap() as *const Value);
            }
            JIT_TAG_PTR => {
                if tagged.payload == 0 {
                    ctx.set_internal_error(format!(
                        "base function arg {} evaluated to null",
                        refs.len()
                    ));
                    return ptr::null_mut();
                }
                refs.push(tagged.payload as *const Value);
            }
            _ => {
                ctx.set_internal_error(format!("unknown tag {} in base function arg", tagged.tag));
                return ptr::null_mut();
            }
        }
    }

    // SAFETY: `owned_storage` is not moved/reallocated after we took interior
    // pointers (capacity == nargs, at most nargs elements pushed). Arena-backed
    // PTR pointers remain valid — no GC fires inside `invoke_base_function_borrowed`.
    let borrowed: Vec<&Value> = refs.into_iter().map(|p| unsafe { &*p }).collect();
    match ctx.invoke_base_function_borrowed(base_fn_index as usize, &borrowed) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            set_runtime_error_from_message(
                ctx,
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            );
            ptr::null_mut()
        }
    }
}

/// Call a Base function by index. Arguments are passed as an array of
/// `*mut Value` pointers.
#[unsafe(no_mangle)]
pub extern "C" fn rt_call_base_function(
    ctx: *mut JitContext,
    base_fn_index: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match get_base_function_by_index(base_fn_index as usize) {
        Some(_) => {}
        None => {
            ctx.error = Some(format!("unknown Base function index: {}", base_fn_index));
            return ptr::null_mut();
        }
    }

    // Collect borrowed arguments from the pointer array. Borrow-capable Base
    // functions can use them directly; owned-only ones fall back to cloning.
    let mut args: Vec<&Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("base function arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { &*arg_ptr });
    }

    match ctx.invoke_base_function_borrowed(base_fn_index as usize, &args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            set_runtime_error_from_message(
                ctx,
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            );
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_call_primop(
    ctx: *mut JitContext,
    primop_id: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };

    let op = match PrimOp::from_id(primop_id as u8) {
        Some(op) => op,
        None => {
            ctx.set_internal_error(format!("unknown primop id: {}", primop_id));
            return ptr::null_mut();
        }
    };

    let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("primop arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    match execute_primop(ctx, op, args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            set_runtime_error_from_message(
                ctx,
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            );
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_call_value(
    ctx: *mut JitContext,
    callee: *mut Value,
    args_ptr: *const *mut Value,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let callee_value = unsafe { (*callee).clone() };
    let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("call arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    if let Value::JitClosure(closure) = &callee_value {
        let Some(entry) = ctx.jit_functions.get(closure.function_index).cloned() else {
            ctx.set_internal_error(format!(
                "unknown JIT function index: {}",
                closure.function_index
            ));
            return ptr::null_mut();
        };

        if args.len() != entry.num_params {
            set_runtime_error_from_message(
                ctx,
                &format!(
                    "wrong number of arguments: want={}, got={}",
                    entry.num_params,
                    args.len()
                ),
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            );
            return ptr::null_mut();
        }

        for (index, arg) in args.iter().enumerate() {
            if let Err((expected, actual)) =
                ctx.check_contract_arg(closure.function_index, index, arg)
            {
                let preview = format_value(ctx, arg);
                ctx.set_runtime_error_diag(ctx.runtime_type_error_diagnostic_at(
                    &expected,
                    &actual,
                    Some(&preview),
                    start_line as usize,
                    start_column as usize,
                    end_line as usize,
                    end_column as usize,
                ));
                return ptr::null_mut();
            }
        }

        let mut arg_values: Vec<JitTaggedValue> = Vec::with_capacity(args.len());
        for v in args {
            arg_values.push(ctx.boxed_to_tagged(v));
        }
        let mut capture_values: Vec<JitTaggedValue> = Vec::with_capacity(closure.captures.len());
        for v in &closure.captures {
            capture_values.push(ctx.boxed_to_tagged(v.clone()));
        }

        let ctx_ptr = ctx as *mut JitContext;
        let raw_result = unsafe { invoke_jit_entry(ctx_ptr, &entry, &arg_values, &capture_values) };
        let result = drain_jit_thunks(ctx, raw_result);

        if result.tag == JIT_TAG_PTR && result.as_ptr().is_null() {
            return ptr::null_mut();
        }
        if let Some(diag) = ctx.take_runtime_error() {
            ctx.set_runtime_error_diag(diag);
            return ptr::null_mut();
        }
        if let Some(err) = ctx.take_internal_error() {
            ctx.set_internal_error(err);
            return ptr::null_mut();
        }
        let Some(result_value) = ctx.clone_from_tagged(result) else {
            ctx.set_internal_error("unknown JIT call error");
            return ptr::null_mut();
        };
        if let Err((expected, actual)) =
            ctx.check_contract_return(closure.function_index, &result_value)
        {
            let preview = format_value(ctx, &result_value);
            ctx.set_runtime_error_diag(ctx.contract_return_error_diagnostic(
                closure.function_index,
                &expected,
                &actual,
                Some(&preview),
            ));
            return ptr::null_mut();
        }
        return ctx.alloc(result_value);
    }

    match crate::runtime::RuntimeContext::invoke_value(ctx, callee_value, args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            set_runtime_error_from_message(
                ctx,
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            );
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_call_jit_function(
    ctx: *mut JitContext,
    function_index: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(entry) = ctx.jit_functions.get(function_index as usize).cloned() else {
        ctx.set_internal_error(format!("unknown JIT function index: {}", function_index));
        return ptr::null_mut();
    };

    let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("call arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    if args.len() != entry.num_params {
        set_runtime_error_from_message(
            ctx,
            &format!(
                "wrong number of arguments: want={}, got={}",
                entry.num_params,
                args.len()
            ),
            start_line as usize,
            start_column as usize,
            end_line as usize,
            end_column as usize,
        );
        return ptr::null_mut();
    }

    for (index, arg) in args.iter().enumerate() {
        if let Err((expected, actual)) = ctx.check_contract_arg(function_index as usize, index, arg)
        {
            let preview = format_value(ctx, arg);
            ctx.set_runtime_error_diag(ctx.runtime_type_error_diagnostic_at(
                &expected,
                &actual,
                Some(&preview),
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            ));
            return ptr::null_mut();
        }
    }

    let arg_values: Vec<JitTaggedValue> =
        args.into_iter().map(|v| ctx.boxed_to_tagged(v)).collect();
    let ctx_ptr = ctx as *mut JitContext;
    let raw_result = unsafe { invoke_jit_entry(ctx_ptr, &entry, &arg_values, &[]) };
    let result = drain_jit_thunks(ctx, raw_result);

    if result.tag == JIT_TAG_PTR && result.as_ptr().is_null() {
        return ptr::null_mut();
    }
    if let Some(diag) = ctx.take_runtime_error() {
        ctx.set_runtime_error_diag(diag);
        return ptr::null_mut();
    }
    if let Some(err) = ctx.take_internal_error() {
        ctx.set_internal_error(err);
        return ptr::null_mut();
    }

    let Some(result_value) = ctx.clone_from_tagged(result) else {
        ctx.set_internal_error("unknown JIT call error");
        return ptr::null_mut();
    };
    if let Err((expected, actual)) =
        ctx.check_contract_return(function_index as usize, &result_value)
    {
        let preview = format_value(ctx, &result_value);
        ctx.set_runtime_error_diag(ctx.runtime_type_error_diagnostic_at(
            &expected,
            &actual,
            Some(&preview),
            start_line as usize,
            start_column as usize,
            end_line as usize,
            end_column as usize,
        ));
        return ptr::null_mut();
    }
    ctx.alloc(result_value)
}

// ---------------------------------------------------------------------------
// Global variable access
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_get_global(ctx: *mut JitContext, index: i64) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let value = ctx.global_get(index as usize);
    ctx.alloc(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_set_global(ctx: *mut JitContext, index: i64, value: *mut Value) {
    let ctx = unsafe { ctx_ref(ctx) };
    let value = unsafe { (*value).clone() };
    ctx.global_set(index as usize, value);
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_set_arity_error(ctx: *mut JitContext, got: i64, want: i64) {
    let ctx = unsafe { ctx_ref(ctx) };
    ctx.error = Some(format!(
        "wrong number of arguments: want={}, got={}",
        want, got
    ));
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_call(
    ctx: *mut JitContext,
    function_index: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("call arg {} evaluated to null", i));
            return 0;
        }
        let arg = unsafe { &*arg_ptr };
        if let Err((expected, actual)) = ctx.check_contract_arg(function_index as usize, i, arg) {
            let preview = arg.to_string();
            ctx.set_runtime_error_diag(ctx.runtime_type_error_diagnostic_at(
                &expected,
                &actual,
                Some(&preview),
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            ));
            return 0;
        }
    }
    1
}

fn check_jit_contract_call_args(
    ctx: &mut JitContext,
    function_index: usize,
    args: &[*mut Value],
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
) -> i64 {
    for (i, arg_ptr) in args.iter().copied().enumerate() {
        if arg_ptr.is_null() {
            ctx.set_internal_error(format!("call arg {} evaluated to null", i));
            return 0;
        }
        let arg = unsafe { &*arg_ptr };
        if let Err((expected, actual)) = ctx.check_contract_arg(function_index, i, arg) {
            let preview = arg.to_string();
            ctx.set_runtime_error_diag(ctx.runtime_type_error_diagnostic_at(
                &expected,
                &actual,
                Some(&preview),
                start_line,
                start_column,
                end_line,
                end_column,
            ));
            return 0;
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_call1(
    ctx: *mut JitContext,
    function_index: i64,
    arg0: *mut Value,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    check_jit_contract_call_args(
        ctx,
        function_index as usize,
        &[arg0],
        start_line as usize,
        start_column as usize,
        end_line as usize,
        end_column as usize,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_call2(
    ctx: *mut JitContext,
    function_index: i64,
    arg0: *mut Value,
    arg1: *mut Value,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    check_jit_contract_call_args(
        ctx,
        function_index as usize,
        &[arg0, arg1],
        start_line as usize,
        start_column as usize,
        end_line as usize,
        end_column as usize,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_call3(
    ctx: *mut JitContext,
    function_index: i64,
    arg0: *mut Value,
    arg1: *mut Value,
    arg2: *mut Value,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    check_jit_contract_call_args(
        ctx,
        function_index as usize,
        &[arg0, arg1, arg2],
        start_line as usize,
        start_column as usize,
        end_line as usize,
        end_column as usize,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_call4(
    ctx: *mut JitContext,
    function_index: i64,
    arg0: *mut Value,
    arg1: *mut Value,
    arg2: *mut Value,
    arg3: *mut Value,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    check_jit_contract_call_args(
        ctx,
        function_index as usize,
        &[arg0, arg1, arg2, arg3],
        start_line as usize,
        start_column as usize,
        end_line as usize,
        end_column as usize,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_check_jit_contract_return(
    ctx: *mut JitContext,
    function_index: i64,
    value: *mut Value,
    _start_line: i64,
    _start_column: i64,
    _end_line: i64,
    _end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if value.is_null() {
        return ptr::null_mut();
    }
    let value_ref = unsafe { &*value };
    if let Err((expected, actual)) = ctx.check_contract_return(function_index as usize, value_ref) {
        let preview = value_ref.to_string();
        ctx.set_runtime_error_diag(ctx.contract_return_error_diagnostic(
            function_index as usize,
            &expected,
            &actual,
            Some(&preview),
        ));
        return ptr::null_mut();
    }
    value
}

// ---------------------------------------------------------------------------
// Value wrappers: Some / Left / Right
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_some(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let v = unsafe { (*value).clone() };
    unsafe { ctx_ref(ctx) }.alloc(Value::Some(Rc::new(v)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_left(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let v = unsafe { (*value).clone() };
    unsafe { ctx_ref(ctx) }.alloc(Value::Left(Rc::new(v)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_right(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let v = unsafe { (*value).clone() };
    unsafe { ctx_ref(ctx) }.alloc(Value::Right(Rc::new(v)))
}

// ---------------------------------------------------------------------------
// Pattern matching helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_some(_ctx: *mut JitContext, value: *mut Value) -> i64 {
    if matches!(unsafe { &*value }, Value::Some(_)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_left(_ctx: *mut JitContext, value: *mut Value) -> i64 {
    if matches!(unsafe { &*value }, Value::Left(_)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_right(_ctx: *mut JitContext, value: *mut Value) -> i64 {
    if matches!(unsafe { &*value }, Value::Right(_)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_none(_ctx: *mut JitContext, value: *mut Value) -> i64 {
    if matches!(unsafe { &*value }, Value::None) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_empty_list(ctx: *mut JitContext, value: *mut Value) -> i64 {
    let v = unsafe { &*value };
    match v {
        Value::EmptyList | Value::None => 1,
        // Also check for empty cons-based list (shouldn't normally happen,
        // but handle gracefully)
        Value::Gc(h) => {
            let ctx = unsafe { ctx_ref(ctx) };
            match ctx.gc_heap.get(*h) {
                HeapObject::Cons { .. } => 0,
                _ => 0,
            }
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_unwrap_some(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match unsafe { &*value } {
        Value::Some(inner) => ctx.alloc(inner.as_ref().clone()),
        _ => {
            ctx.error = Some("unwrap_some on non-Some value".to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_unwrap_left(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match unsafe { &*value } {
        Value::Left(inner) => ctx.alloc(inner.as_ref().clone()),
        _ => {
            ctx.error = Some("unwrap_left on non-Left value".to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_unwrap_right(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match unsafe { &*value } {
        Value::Right(inner) => ctx.alloc(inner.as_ref().clone()),
        _ => {
            ctx.error = Some("unwrap_right on non-Right value".to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_values_equal(_ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> i64 {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(_ctx) };
    if values_equal(ctx, a, b) { 1 } else { 0 }
}

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_array(
    ctx: *mut JitContext,
    elements_ptr: *const JitTaggedValue,
    len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if len == 0 {
        return ctx.alloc(Value::Array(Rc::new(vec![])));
    }
    let Some(elements) =
        clone_values_from_tagged_ptrs(ctx, elements_ptr, len as usize, "array construction")
    else {
        return ptr::null_mut();
    };
    ctx.alloc(Value::Array(Rc::new(elements)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_tuple(
    ctx: *mut JitContext,
    elements_ptr: *const JitTaggedValue,
    len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(elements) =
        clone_values_from_tagged_ptrs(ctx, elements_ptr, len as usize, "tuple construction")
    else {
        return ptr::null_mut();
    };
    ctx.alloc(Value::Tuple(Rc::new(elements)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_hash(
    ctx: *mut JitContext,
    pairs_ptr: *const JitTaggedValue,
    npairs: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let mut root = hamt_empty(&mut ctx.gc_heap);
    for i in 0..npairs as usize {
        let Some(key) = clone_tagged_arg(
            ctx,
            unsafe { *pairs_ptr.add(i * 2) },
            "hash construction",
            i * 2,
        ) else {
            return ptr::null_mut();
        };
        let Some(value) = clone_tagged_arg(
            ctx,
            unsafe { *pairs_ptr.add(i * 2 + 1) },
            "hash construction",
            i * 2 + 1,
        ) else {
            return ptr::null_mut();
        };
        let hash_key = match key.to_hash_key() {
            Some(k) => k,
            None => {
                ctx.error = Some(format!("unusable as hash key: {}", key.type_name()));
                return ptr::null_mut();
            }
        };
        root = hamt_insert(&mut ctx.gc_heap, root, hash_key, value);
    }
    ctx.alloc(Value::Gc(root))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_index(
    ctx: *mut JitContext,
    collection: *mut Value,
    key: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let left = unsafe { &*collection };
    let index = unsafe { &*key };
    match (left, index) {
        (Value::Array(elements), Value::Integer(idx)) => {
            if *idx < 0 || *idx as usize >= elements.len() {
                ctx.alloc(Value::None)
            } else {
                ctx.alloc(Value::Some(Rc::new(elements[*idx as usize].clone())))
            }
        }
        (Value::Tuple(elements), Value::Integer(idx)) => {
            if *idx < 0 || *idx as usize >= elements.len() {
                ctx.alloc(Value::None)
            } else {
                ctx.alloc(Value::Some(Rc::new(elements[*idx as usize].clone())))
            }
        }
        (Value::Gc(handle), _) => match index {
            Value::Integer(idx) => match ctx.gc_heap.get(*handle) {
                HeapObject::Cons { .. } => {
                    if *idx < 0 {
                        return ctx.alloc(Value::None);
                    }
                    let mut current = Value::Gc(*handle);
                    let mut remaining = *idx as usize;
                    loop {
                        match &current {
                            Value::Gc(h) => match ctx.gc_heap.get(*h) {
                                HeapObject::Cons { head, tail } => {
                                    if remaining == 0 {
                                        return ctx.alloc(Value::Some(Rc::new(head.clone())));
                                    }
                                    remaining -= 1;
                                    current = tail.clone();
                                }
                                _ => return ctx.alloc(Value::None),
                            },
                            _ => return ctx.alloc(Value::None),
                        }
                    }
                }
                _ => {
                    let hash_key = match index.to_hash_key() {
                        Some(k) => k,
                        None => {
                            ctx.error =
                                Some(format!("unusable as hash key: {}", index.type_name()));
                            return ptr::null_mut();
                        }
                    };
                    match hamt_lookup(&ctx.gc_heap, *handle, &hash_key) {
                        Some(value) => ctx.alloc(Value::Some(Rc::new(value))),
                        None => ctx.alloc(Value::None),
                    }
                }
            },
            _ => {
                let hash_key = match index.to_hash_key() {
                    Some(k) => k,
                    None => {
                        ctx.error = Some(format!("unusable as hash key: {}", index.type_name()));
                        return ptr::null_mut();
                    }
                };
                match hamt_lookup(&ctx.gc_heap, *handle, &hash_key) {
                    Some(value) => ctx.alloc(Value::Some(Rc::new(value))),
                    None => ctx.alloc(Value::None),
                }
            }
        },
        _ => {
            ctx.error = Some(format!(
                "index operator not supported: {}",
                left.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_tuple(_ctx: *mut JitContext, value: *mut Value) -> i64 {
    if matches!(unsafe { &*value }, Value::Tuple(_)) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_tuple_len_eq(ctx: *mut JitContext, value: *mut Value, len: i64) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    match unsafe { &*value } {
        Value::Tuple(elements) => (elements.len() as i64 == len) as i64,
        _ => {
            ctx.error = Some(format!(
                "expected Tuple, got {}",
                unsafe { &*value }.type_name()
            ));
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_tuple_get(ctx: *mut JitContext, value: *mut Value, index: i64) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    match unsafe { &*value } {
        Value::Tuple(elements) => {
            if index < 0 || index as usize >= elements.len() {
                ctx.error = Some(format!(
                    "tuple index {} out of bounds for tuple of length {}",
                    index,
                    elements.len()
                ));
                ptr::null_mut()
            } else {
                ctx.alloc(elements[index as usize].clone())
            }
        }
        _ => {
            ctx.error = Some(format!(
                "tuple field access expected Tuple, got {}",
                unsafe { &*value }.type_name()
            ));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// String operations
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_to_string(ctx: *mut JitContext, value: *mut Value) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let v = unsafe { &*value };
    let s = v.to_string_value();
    ctx.alloc(Value::String(s.into()))
}

/// Concatenate two `*mut Value` strings, returning `*mut Value`.
/// Used by interpolated-string codegen where both operands are already boxed
/// `Value::String` pointers (from `rt_make_string` / `rt_to_string`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_string_concat(
    ctx: *mut JitContext,
    a: *mut Value,
    b: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let a_str = match unsafe { &*a } {
        Value::String(s) => s.clone(),
        other => Rc::from(format_value(ctx, other)),
    };
    let b_str = match unsafe { &*b } {
        Value::String(s) => s.clone(),
        other => Rc::from(format_value(ctx, other)),
    };
    ctx.alloc(Value::String(format!("{}{}", a_str, b_str).into()))
}

// ---------------------------------------------------------------------------
// ADT helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    fields_ptr: *const JitTaggedValue,
    arity: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    // ABI contract: constructor bytes are emitted by the compiler/JIT and must be valid UTF-8.
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };

    // Fields arrive as raw `*mut Value` pointers; clone into owned runtime values.
    // The helper assumes each pointer is non-null and points to a live Value.
    let Some(fields) =
        clone_values_from_tagged_ptrs(ctx, fields_ptr, arity as usize, "adt construction")
    else {
        return ptr::null_mut();
    };

    // Allocate ADT object in the JIT context arena and return the boxed runtime pointer.
    // Nullary constructors use the lighter `AdtUnit` representation.
    if fields.is_empty() {
        ctx.alloc(Value::AdtUnit(constructor))
    } else {
        let handle = ctx.gc_heap.alloc(HeapObject::Adt {
            constructor,
            fields: AdtFields::from_vec(fields),
        });
        ctx.alloc(Value::GcAdt(handle))
    }
}

/// Unit (nullary) ADT constructor with interning — returns the same `*mut Value` for
/// each unique constructor name within a program execution, avoiding repeated arena
/// allocations for identical unit values such as `None`, `True`, `False`, etc.
#[unsafe(no_mangle)]
pub extern "C" fn rt_intern_unit_adt(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let name =
        unsafe { from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize)) };
    ctx.intern_unit_adt(name)
}

/// Specialized 1-field ADT constructor — avoids stack-slot + loop overhead.
#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt1(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    f0: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };
    let v0 = unsafe { (*f0).clone() };
    let handle = ctx.gc_heap.alloc(HeapObject::Adt {
        constructor,
        fields: AdtFields::from_vec(vec![v0]),
    });
    ctx.alloc(Value::GcAdt(handle))
}

/// Specialized 2-field ADT constructor — avoids stack-slot + loop overhead.
#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt2(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    f0: *mut Value,
    f1: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };
    let v0 = unsafe { (*f0).clone() };
    let v1 = unsafe { (*f1).clone() };
    let handle = ctx.gc_heap.alloc(HeapObject::Adt {
        constructor,
        fields: AdtFields::from_vec(vec![v0, v1]),
    });
    ctx.alloc(Value::GcAdt(handle))
}

/// Specialized 3-field ADT constructor — avoids stack-slot + loop overhead.
#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt3(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    f0: *mut Value,
    f1: *mut Value,
    f2: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };
    let v0 = unsafe { (*f0).clone() };
    let v1 = unsafe { (*f1).clone() };
    let v2 = unsafe { (*f2).clone() };
    let handle = ctx.gc_heap.alloc(HeapObject::Adt {
        constructor,
        fields: AdtFields::from_vec(vec![v0, v1, v2]),
    });
    ctx.alloc(Value::GcAdt(handle))
}

/// Specialized 4-field ADT constructor — avoids stack-slot + loop overhead.
#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt4(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    f0: *mut Value,
    f1: *mut Value,
    f2: *mut Value,
    f3: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };
    let v0 = unsafe { (*f0).clone() };
    let v1 = unsafe { (*f1).clone() };
    let v2 = unsafe { (*f2).clone() };
    let v3 = unsafe { (*f3).clone() };
    let handle = ctx.gc_heap.alloc(HeapObject::Adt {
        constructor,
        fields: AdtFields::from_vec(vec![v0, v1, v2, v3]),
    });
    ctx.alloc(Value::GcAdt(handle))
}

/// Specialized 5-field ADT constructor — avoids stack-slot + loop overhead.
/// Covers `Node(Color, Tree, Int, Bool, Tree)` in rbtree benchmarks.
#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt5(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    f0: *mut Value,
    f1: *mut Value,
    f2: *mut Value,
    f3: *mut Value,
    f4: *mut Value,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    let constructor: Rc<String> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::new(s.to_string())
    };
    let v0 = unsafe { (*f0).clone() };
    let v1 = unsafe { (*f1).clone() };
    let v2 = unsafe { (*f2).clone() };
    let v3 = unsafe { (*f3).clone() };
    let v4 = unsafe { (*f4).clone() };
    let handle = ctx.gc_heap.alloc(HeapObject::Adt {
        constructor,
        fields: AdtFields::from_vec(vec![v0, v1, v2, v3, v4]),
    });
    ctx.alloc(Value::GcAdt(handle))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_adt_constructor(
    ctx: *mut JitContext,
    value: *mut Value,
    constructor_ptr: *const u8,
    constructor_len: i64,
) -> i64 {
    let ctx = unsafe { ctx_ref(ctx) };
    // Null values never match any constructor tag.
    if value.is_null() {
        return 0;
    }

    // ABI contract: constructor bytes are compiler/JIT-emitted and valid UTF-8.
    let expected =
        unsafe { from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize)) };

    match unsafe { &*value } {
        value if value.adt_constructor(&ctx.gc_heap) == Some(expected) => 1,
        Value::AdtUnit(name) => {
            if name.as_ref() == expected {
                1
            } else {
                0
            }
        }
        // Non-ADT values cannot satisfy constructor patterns.
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_adt_field(
    ctx: *mut JitContext,
    value: *mut Value,
    field_idx: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };

    // Null pointer is a runtime error; return null and set context error.
    if value.is_null() {
        ctx.error = Some("adt field access on null".to_string());
        return ptr::null_mut();
    }

    match unsafe { &*value } {
        value @ (Value::Adt(_) | Value::GcAdt(_)) => {
            // Field index comes from JIT as i64 and is interpreted as usize.
            let idx = field_idx as usize;
            if let Some(field) = value.adt_clone_field(&ctx.gc_heap, idx) {
                ctx.alloc(field)
            } else {
                let len = value.adt_field_count(&ctx.gc_heap).unwrap_or(0);
                // Out-of-bounds ADT field access is reported through JitContext error state.
                ctx.error = Some(format!(
                    "adt field index {} out of bounds (len={})",
                    idx, len
                ));
                ptr::null_mut()
            }
        }
        Value::AdtUnit(name) => {
            ctx.error = Some(format!(
                "adt field index {} out of bounds (AdtUnit '{}' has 0 fields)",
                field_idx, name
            ));
            ptr::null_mut()
        }
        _ => {
            // Accessing fields on non-ADT values is a type error.
            ctx.error = Some(format!(
                "expected Adt, got {}",
                unsafe { &*value }.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_adt_field_or_none(
    ctx: *mut JitContext,
    value: *mut Value,
    field_idx: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if value.is_null() {
        return ctx.alloc(Value::None);
    }

    match unsafe { &*value } {
        value @ (Value::Adt(_) | Value::GcAdt(_)) => {
            let idx = field_idx as usize;
            if let Some(field) = value.adt_clone_field(&ctx.gc_heap, idx) {
                ctx.alloc(field)
            } else {
                ctx.alloc(Value::None)
            }
        }
        Value::AdtUnit(_) => ctx.alloc(Value::None),
        _ => ctx.alloc(Value::None),
    }
}

// ---------------------------------------------------------------------------
// Algebraic effects: handler push / pop / perform
// ---------------------------------------------------------------------------

/// Push an effect handler frame onto the JIT context's handler stack.
///
/// `ops_ptr` points to `narms` i64 values (symbol IDs for each op name).
/// `closures_ptr` points to `narms` `*mut Value` pointers (arm closure Values).
#[unsafe(no_mangle)]
pub extern "C" fn rt_push_handler(
    ctx: *mut JitContext,
    effect_id: i64,
    ops_ptr: *const i64,
    closures_ptr: *const *mut Value,
    narms: i64,
) {
    let ctx = unsafe { ctx_ref(ctx) };
    let narms = narms as usize;
    let mut arms = Vec::with_capacity(narms);
    for i in 0..narms {
        let op = unsafe { *ops_ptr.add(i) } as u32;
        let closure_val = unsafe { (*closures_ptr.add(i)).as_ref().unwrap().clone() };
        arms.push(JitHandlerArm {
            op,
            closure: closure_val,
        });
    }
    ctx.handler_stack.push(JitHandlerFrame {
        effect: effect_id as u32,
        arms,
    });
}

/// Pop the top handler frame from the JIT context's handler stack.
#[unsafe(no_mangle)]
pub extern "C" fn rt_pop_handler(ctx: *mut JitContext) {
    let ctx = unsafe { ctx_ref(ctx) };
    ctx.handler_stack.pop();
}

/// Perform an effect operation.
///
/// Searches the handler stack (from top) for a frame matching `effect_id`,
/// then finds the arm matching `op_id`. Calls the arm synchronously, passing
/// a shallow `resume` closure (identity: returns its argument) as the first
/// argument followed by the operation arguments.
///
/// Returns null (and sets `ctx.error`) if no matching handler is found.
#[unsafe(no_mangle)]
pub extern "C" fn rt_perform(
    ctx: *mut JitContext,
    effect_id: i64,
    op_id: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
    effect_name_ptr: *const u8,
    effect_name_len: i64,
    op_name_ptr: *const u8,
    op_name_len: i64,
    line: i64,
    column: i64,
) -> *mut Value {
    let effect_u32 = effect_id as u32;
    let op_u32 = op_id as u32;
    let nargs = nargs as usize;
    let effect_name = unsafe {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(
            effect_name_ptr,
            effect_name_len as usize,
        ))
    };
    let op_name = unsafe {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(
            op_name_ptr,
            op_name_len as usize,
        ))
    };
    let line = line as usize;
    let column = column as usize;

    // Find matching handler (search from top of stack)
    let arm_closure = {
        let ctx_mut = unsafe { ctx_ref(ctx) };
        let mut found: Option<Value> = None;
        for frame in ctx_mut.handler_stack.iter().rev() {
            if frame.effect == effect_u32 {
                for arm in &frame.arms {
                    if arm.op == op_u32 {
                        found = Some(arm.closure.clone());
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
                // Found effect but no matching op — error
                ctx_mut.set_runtime_error_code(
                    "E1009",
                    "UNHANDLED OPERATION",
                    &format!("unhandled operation: {}.{}", effect_name, op_name),
                    line,
                    column,
                    line,
                    column,
                );
                return ptr::null_mut();
            }
        }
        match found {
            Some(c) => c,
            None => {
                ctx_mut.set_runtime_error_code(
                    "E1009",
                    "UNHANDLED EFFECT",
                    &format!(
                        "unhandled effect: {} (no matching handle block)",
                        effect_name
                    ),
                    line,
                    column,
                    line,
                    column,
                );
                return ptr::null_mut();
            }
        }
    };

    // Collect operation arguments
    let mut call_args: Vec<Value> = Vec::with_capacity(1 + nargs);

    // Build the resume value: a JitClosure wrapping the identity function.
    // `identity_fn_index == usize::MAX` means not yet compiled (shouldn't happen).
    let identity_idx = unsafe { (*ctx).identity_fn_index };
    let resume_val = Value::JitClosure(Rc::new(JitClosure::new(identity_idx, vec![])));
    call_args.push(resume_val);

    for i in 0..nargs {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            unsafe { ctx_ref(ctx) }.error = Some(format!("perform arg {} is null", i));
            return ptr::null_mut();
        }
        call_args.push(unsafe { (*arg_ptr).clone() });
    }

    let ctx_mut = unsafe { ctx_ref(ctx) };
    match ctx_mut.invoke_value(arm_closure, call_args) {
        Ok(result) => {
            // The handler arm's resume(v) returns v. When v is ()
            // (empty tuple), normalize it to None (Unit) so the perform
            // call site gets the expected type.
            let normalized = match &result {
                Value::Tuple(fields) if fields.is_empty() => Value::None,
                _ => result,
            };
            ctx_mut.alloc(normalized)
        }
        Err(msg) => {
            ctx_mut.error = Some(msg);
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Value unboxing (LLVM backend)
// ---------------------------------------------------------------------------

/// Convert a `*mut Value` arena pointer back to a properly tagged `JitTaggedValue`.
///
/// This reads the actual `Value` discriminant from the pointer and produces
/// the correct tag (INT for integers, BOOL for booleans, etc.) instead of
/// blindly wrapping as `JIT_TAG_PTR`. Used by the LLVM backend after
/// `rt_call_value` and other helpers that return `*mut Value`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_unbox_to_tagged(ctx: *mut JitContext, value: *mut Value) -> JitTaggedValue {
    let ctx = unsafe { ctx_ref(ctx) };
    ctx.boxed_ptr_to_tagged(value)
}

// Lookup table for registering helpers with Cranelift JITModule
// ---------------------------------------------------------------------------

/// Returns all runtime helper function pointers and their names, for
/// registration with the Cranelift JIT module.
pub fn rt_symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("rt_make_integer", rt_make_integer as *const u8),
        ("rt_make_float", rt_make_float as *const u8),
        ("rt_make_bool", rt_make_bool as *const u8),
        ("rt_make_none", rt_make_none as *const u8),
        ("rt_division_by_zero", rt_division_by_zero as *const u8),
        (
            "rt_render_error_with_span",
            rt_render_error_with_span as *const u8,
        ),
        ("rt_force_boxed", rt_force_boxed as *const u8),
        ("rt_push_gc_roots", rt_push_gc_roots as *const u8),
        ("rt_pop_gc_roots", rt_pop_gc_roots as *const u8),
        ("rt_make_empty_list", rt_make_empty_list as *const u8),
        ("rt_make_string", rt_make_string as *const u8),
        ("rt_make_base_function", rt_make_base_function as *const u8),
        ("rt_make_jit_closure", rt_make_jit_closure as *const u8),
        ("rt_make_cons", rt_make_cons as *const u8),
        ("rt_is_cons", rt_is_cons as *const u8),
        ("rt_cons_head", rt_cons_head as *const u8),
        ("rt_cons_tail", rt_cons_tail as *const u8),
        ("rt_add", rt_add as *const u8),
        ("rt_sub", rt_sub as *const u8),
        ("rt_mul", rt_mul as *const u8),
        ("rt_div", rt_div as *const u8),
        ("rt_mod", rt_mod as *const u8),
        ("rt_negate", rt_negate as *const u8),
        ("rt_not", rt_not as *const u8),
        ("rt_is_truthy", rt_is_truthy as *const u8),
        ("rt_bool_value", rt_bool_value as *const u8),
        ("rt_equal", rt_equal as *const u8),
        ("rt_not_equal", rt_not_equal as *const u8),
        ("rt_greater_than", rt_greater_than as *const u8),
        ("rt_less_than_or_equal", rt_less_than_or_equal as *const u8),
        (
            "rt_greater_than_or_equal",
            rt_greater_than_or_equal as *const u8,
        ),
        ("rt_set_thunk", rt_set_thunk as *const u8),
        (
            "rt_call_base_function_tagged",
            rt_call_base_function_tagged as *const u8,
        ),
        ("rt_call_base_function", rt_call_base_function as *const u8),
        ("rt_call_primop", rt_call_primop as *const u8),
        ("rt_call_value", rt_call_value as *const u8),
        ("rt_call_jit_function", rt_call_jit_function as *const u8),
        ("rt_get_global", rt_get_global as *const u8),
        ("rt_set_global", rt_set_global as *const u8),
        ("rt_set_arity_error", rt_set_arity_error as *const u8),
        (
            "rt_check_jit_contract_call",
            rt_check_jit_contract_call as *const u8,
        ),
        (
            "rt_check_jit_contract_call1",
            rt_check_jit_contract_call1 as *const u8,
        ),
        (
            "rt_check_jit_contract_call2",
            rt_check_jit_contract_call2 as *const u8,
        ),
        (
            "rt_check_jit_contract_call3",
            rt_check_jit_contract_call3 as *const u8,
        ),
        (
            "rt_check_jit_contract_call4",
            rt_check_jit_contract_call4 as *const u8,
        ),
        (
            "rt_check_jit_contract_return",
            rt_check_jit_contract_return as *const u8,
        ),
        // Phase 4: wrappers
        ("rt_make_some", rt_make_some as *const u8),
        ("rt_make_left", rt_make_left as *const u8),
        ("rt_make_right", rt_make_right as *const u8),
        // Phase 4: pattern matching
        ("rt_is_some", rt_is_some as *const u8),
        ("rt_is_left", rt_is_left as *const u8),
        ("rt_is_right", rt_is_right as *const u8),
        ("rt_is_none", rt_is_none as *const u8),
        ("rt_is_empty_list", rt_is_empty_list as *const u8),
        ("rt_unwrap_some", rt_unwrap_some as *const u8),
        ("rt_unwrap_left", rt_unwrap_left as *const u8),
        ("rt_unwrap_right", rt_unwrap_right as *const u8),
        ("rt_values_equal", rt_values_equal as *const u8),
        // Phase 4: collections
        ("rt_make_array", rt_make_array as *const u8),
        ("rt_make_tuple", rt_make_tuple as *const u8),
        ("rt_make_hash", rt_make_hash as *const u8),
        ("rt_index", rt_index as *const u8),
        ("rt_is_tuple", rt_is_tuple as *const u8),
        ("rt_tuple_len_eq", rt_tuple_len_eq as *const u8),
        ("rt_tuple_get", rt_tuple_get as *const u8),
        // Phase 4: string ops
        ("rt_to_string", rt_to_string as *const u8),
        ("rt_string_concat", rt_string_concat as *const u8),
        // Phase 5: ADT helpers
        ("rt_intern_unit_adt", rt_intern_unit_adt as *const u8),
        ("rt_make_adt", rt_make_adt as *const u8),
        ("rt_make_adt1", rt_make_adt1 as *const u8),
        ("rt_make_adt2", rt_make_adt2 as *const u8),
        ("rt_make_adt3", rt_make_adt3 as *const u8),
        ("rt_make_adt4", rt_make_adt4 as *const u8),
        ("rt_make_adt5", rt_make_adt5 as *const u8),
        ("rt_is_adt_constructor", rt_is_adt_constructor as *const u8),
        ("rt_adt_field", rt_adt_field as *const u8),
        ("rt_adt_field_or_none", rt_adt_field_or_none as *const u8),
        // Algebraic effects
        ("rt_push_handler", rt_push_handler as *const u8),
        ("rt_pop_handler", rt_pop_handler as *const u8),
        ("rt_perform", rt_perform as *const u8),
        // Value unboxing (LLVM backend)
        ("rt_unbox_to_tagged", rt_unbox_to_tagged as *const u8),
    ]
}
