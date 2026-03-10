#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! Runtime helper functions callable from JIT-compiled code.
//!
//! All functions use `extern "C"` ABI and operate on `*mut Value` pointers.
//! They receive a `*mut JitContext` as their first argument for arena allocation
//! and error reporting.
//!
//! Convention: return `std::ptr::null_mut()` on error with message stored in
//! `ctx.error`.

use std::ptr;
use std::rc::Rc;
use std::slice::from_raw_parts;
use std::str::from_utf8_unchecked;

use crate::jit::context::{JitHandlerArm, JitHandlerFrame};
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

use super::context::JitContext;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Safely dereference a JitContext pointer. Returns None if null.
unsafe fn ctx_ref<'a>(ctx: *mut JitContext) -> &'a mut JitContext {
    unsafe { &mut *ctx }
}

fn clone_value_arg(
    ctx: &mut JitContext,
    value: *mut Value,
    label: &str,
    index: usize,
) -> Option<Value> {
    match unsafe { value.as_ref() } {
        Some(value) => Some(value.clone()),
        None => {
            ctx.error = Some(format!("{label} received null value pointer at index {index}"));
            None
        }
    }
}

fn clone_values_from_ptrs(
    ctx: &mut JitContext,
    values_ptr: *const *mut Value,
    len: usize,
    label: &str,
) -> Option<Vec<Value>> {
    let mut values = Vec::with_capacity(len);
    for i in 0..len {
        let value_ptr = unsafe { *values_ptr.add(i) };
        values.push(clone_value_arg(ctx, value_ptr, label, i)?);
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
    unsafe { ctx_ref(ctx) }.error = Some("division by zero".to_string());
}

// ---------------------------------------------------------------------------
// Value constructors
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_integer(ctx: *mut JitContext, value: i64) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::Integer(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_float(ctx: *mut JitContext, bits: i64) -> *mut Value {
    let value = f64::from_bits(bits as u64);
    unsafe { ctx_ref(ctx) }.alloc(Value::Float(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_bool(ctx: *mut JitContext, value: i64) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::Boolean(value != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_none(ctx: *mut JitContext) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::None)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_push_gc_roots(
    ctx: *mut JitContext,
    values_ptr: *const *mut Value,
    len: i64,
) {
    let ctx = unsafe { ctx_ref(ctx) };
    let roots = unsafe { from_raw_parts(values_ptr, len as usize) };
    ctx.push_gc_roots(roots);
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
    let rc: Rc<str> = Rc::from(s);
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
    captures_ptr: *const *mut Value,
    ncaptures: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(captures) =
        clone_values_from_ptrs(ctx, captures_ptr, ncaptures as usize, "jit closure construction")
    else {
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
pub extern "C" fn rt_add(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Integer(l + r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Float(l + r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Float(*l as f64 + r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Float(l + *r as f64)),
        (Value::String(l), Value::String(r)) => {
            ctx.alloc(Value::String(format!("{}{}", l, r).into()))
        }
        _ => {
            ctx.error = Some(format!(
                "cannot add {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_sub(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Integer(l - r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Float(l - r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Float(*l as f64 - r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Float(l - *r as f64)),
        _ => {
            ctx.error = Some(format!(
                "cannot subtract {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_mul(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Integer(l * r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Float(l * r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Float(*l as f64 * r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Float(l * *r as f64)),
        _ => {
            ctx.error = Some(format!(
                "cannot multiply {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_div(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(_), Value::Integer(0)) | (Value::Float(_), Value::Integer(0)) => {
            ctx.error = Some("division by zero".to_string());
            ptr::null_mut()
        }
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Integer(l / r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Float(l / r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Float(*l as f64 / r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Float(l / *r as f64)),
        _ => {
            ctx.error = Some(format!(
                "cannot divide {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_mod(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(_), Value::Integer(0)) | (Value::Float(_), Value::Integer(0)) => {
            ctx.error = Some("division by zero".to_string());
            ptr::null_mut()
        }
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Integer(l % r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Float(l % r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Float(*l as f64 % r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Float(l % *r as f64)),
        _ => {
            ctx.error = Some(format!(
                "cannot modulo {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Prefix operators
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_negate(ctx: *mut JitContext, a: *mut Value) -> *mut Value {
    let a = unsafe { &*a };
    let ctx = unsafe { ctx_ref(ctx) };
    match a {
        Value::Integer(v) => ctx.alloc(Value::Integer(-v)),
        Value::Float(v) => ctx.alloc(Value::Float(-v)),
        _ => {
            ctx.error = Some(format!("cannot negate {}", a.type_name()));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_not(ctx: *mut JitContext, a: *mut Value) -> *mut Value {
    let a = unsafe { &*a };
    let ctx = unsafe { ctx_ref(ctx) };
    match a {
        Value::Boolean(v) => ctx.alloc(Value::Boolean(!v)),
        _ => {
            ctx.error = Some(format!("cannot apply ! to {}", a.type_name()));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_is_truthy(_ctx: *mut JitContext, a: *mut Value) -> i64 {
    let a = unsafe { &*a };
    if a.is_truthy() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_bool_value(_ctx: *mut JitContext, a: *mut Value) -> i64 {
    let a = unsafe { &*a };
    match a {
        Value::Boolean(v) => i64::from(*v),
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
pub extern "C" fn rt_equal(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    let result = values_equal(ctx, a, b);
    ctx.alloc(Value::Boolean(result))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_not_equal(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    let result = values_equal(ctx, a, b);
    ctx.alloc(Value::Boolean(!result))
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
    a: *mut Value,
    b: *mut Value,
) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(l > r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Boolean(l > r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Boolean(*l as f64 > *r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(*l > *r as f64)),
        (Value::String(l), Value::String(r)) => ctx.alloc(Value::Boolean(l > r)),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_less_than_or_equal(
    ctx: *mut JitContext,
    a: *mut Value,
    b: *mut Value,
) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(l <= r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Boolean(l <= r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Boolean(*l as f64 <= *r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(*l <= *r as f64)),
        (Value::String(l), Value::String(r)) => ctx.alloc(Value::Boolean(l <= r)),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_greater_than_or_equal(
    ctx: *mut JitContext,
    a: *mut Value,
    b: *mut Value,
) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    match (a, b) {
        (Value::Integer(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(l >= r)),
        (Value::Float(l), Value::Float(r)) => ctx.alloc(Value::Boolean(l >= r)),
        (Value::Integer(l), Value::Float(r)) => ctx.alloc(Value::Boolean(*l as f64 >= *r)),
        (Value::Float(l), Value::Integer(r)) => ctx.alloc(Value::Boolean(*l >= *r as f64)),
        (Value::String(l), Value::String(r)) => ctx.alloc(Value::Boolean(l >= r)),
        _ => {
            ctx.error = Some(format!(
                "cannot compare {} and {}",
                a.type_name(),
                b.type_name()
            ));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Base function calls
// ---------------------------------------------------------------------------

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
            ctx.error = Some(format!("base function arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { &*arg_ptr });
    }

    match ctx.invoke_base_function_borrowed(base_fn_index as usize, &args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            ctx.error = Some(ctx.render_runtime_error_from_string(
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            ));
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
            ctx.error = Some(format!("unknown primop id: {}", primop_id));
            return ptr::null_mut();
        }
    };

    let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.error = Some(format!("primop arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    match execute_primop(ctx, op, args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            ctx.error = Some(ctx.render_runtime_error_message(
                "E1004",
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            ));
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
            ctx.error = Some(format!("call arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    match crate::runtime::RuntimeContext::invoke_value(ctx, callee_value, args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            ctx.error = Some(ctx.render_runtime_error_from_string(
                &msg,
                start_line as usize,
                start_column as usize,
                end_line as usize,
                end_column as usize,
            ));
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Global variable access
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_get_global(ctx: *mut JitContext, index: i64) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let value = ctx.globals[index as usize].clone();
    ctx.alloc(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_set_global(ctx: *mut JitContext, index: i64, value: *mut Value) {
    let ctx = unsafe { ctx_ref(ctx) };
    let value = unsafe { (*value).clone() };
    ctx.globals[index as usize] = value;
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
            ctx.error = Some(format!("call arg {} evaluated to null", i));
            return 0;
        }
        let arg = unsafe { &*arg_ptr };
        if let Err((expected, actual)) = ctx.check_contract_arg(function_index as usize, i, arg) {
            let preview = arg.to_string();
            ctx.error = Some(ctx.render_runtime_type_error_at(
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
            ctx.error = Some(format!("call arg {} evaluated to null", i));
            return 0;
        }
        let arg = unsafe { &*arg_ptr };
        if let Err((expected, actual)) = ctx.check_contract_arg(function_index, i, arg) {
            let preview = arg.to_string();
            ctx.error = Some(ctx.render_runtime_type_error_at(
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
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if value.is_null() {
        return ptr::null_mut();
    }
    let value_ref = unsafe { &*value };
    if let Err((expected, actual)) = ctx.check_contract_return(function_index as usize, value_ref) {
        let preview = value_ref.to_string();
        ctx.error = Some(ctx.render_runtime_type_error_at(
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
    elements_ptr: *const *mut Value,
    len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    if len == 0 {
        return ctx.alloc(Value::Array(Rc::new(vec![])));
    }
    let Some(elements) = clone_values_from_ptrs(ctx, elements_ptr, len as usize, "array construction")
    else {
        return ptr::null_mut();
    };
    ctx.alloc(Value::Array(Rc::new(elements)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_tuple(
    ctx: *mut JitContext,
    elements_ptr: *const *mut Value,
    len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let Some(elements) = clone_values_from_ptrs(ctx, elements_ptr, len as usize, "tuple construction")
    else {
        return ptr::null_mut();
    };
    ctx.alloc(Value::Tuple(Rc::new(elements)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_hash(
    ctx: *mut JitContext,
    pairs_ptr: *const *mut Value,
    npairs: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let mut root = hamt_empty(&mut ctx.gc_heap);
    for i in 0..npairs as usize {
        let Some(key) = clone_value_arg(ctx, unsafe { *pairs_ptr.add(i * 2) }, "hash construction", i * 2) else {
            return ptr::null_mut();
        };
        let Some(value) = clone_value_arg(
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
    let s = format_value(ctx, v);
    ctx.alloc(Value::String(s.into()))
}

// ---------------------------------------------------------------------------
// ADT helpers
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_adt(
    ctx: *mut JitContext,
    constructor_ptr: *const u8,
    constructor_len: i64,
    fields_ptr: *const *mut Value,
    arity: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    maybe_collect_gc(ctx);
    // ABI contract: constructor bytes are emitted by the compiler/JIT and must be valid UTF-8.
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
    };

    // Fields arrive as raw `*mut Value` pointers; clone into owned runtime values.
    // The helper assumes each pointer is non-null and points to a live Value.
    let Some(fields) = clone_values_from_ptrs(ctx, fields_ptr, arity as usize, "adt construction")
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
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
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
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
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
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
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
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
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
    let constructor: Rc<str> = {
        let s = unsafe {
            from_utf8_unchecked(from_raw_parts(constructor_ptr, constructor_len as usize))
        };
        Rc::from(s)
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
                ctx_mut.error = Some(ctx_mut.render_runtime_error(
                    "E1009",
                    "UNHANDLED OPERATION",
                    &format!("unhandled operation: {}.{}", effect_name, op_name),
                    line,
                    column,
                    line,
                    column,
                ));
                return ptr::null_mut();
            }
        }
        match found {
            Some(c) => c,
            None => {
                ctx_mut.error = Some(ctx_mut.render_runtime_error(
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
                ));
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
        Ok(result) => ctx_mut.alloc(result),
        Err(msg) => {
            ctx_mut.error = Some(msg);
            ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
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
        ("rt_call_base_function", rt_call_base_function as *const u8),
        ("rt_call_primop", rt_call_primop as *const u8),
        ("rt_call_value", rt_call_value as *const u8),
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
        // Phase 5: ADT helpers
        ("rt_make_adt", rt_make_adt as *const u8),
        ("rt_make_adt1", rt_make_adt1 as *const u8),
        ("rt_make_adt2", rt_make_adt2 as *const u8),
        ("rt_make_adt3", rt_make_adt3 as *const u8),
        ("rt_make_adt4", rt_make_adt4 as *const u8),
        ("rt_make_adt5", rt_make_adt5 as *const u8),
        ("rt_is_adt_constructor", rt_is_adt_constructor as *const u8),
        ("rt_adt_field", rt_adt_field as *const u8),
        // Algebraic effects
        ("rt_push_handler", rt_push_handler as *const u8),
        ("rt_pop_handler", rt_pop_handler as *const u8),
        ("rt_perform", rt_perform as *const u8),
    ]
}
