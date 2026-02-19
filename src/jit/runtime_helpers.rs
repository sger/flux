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

use crate::runtime::{
    builtins::get_builtin_by_index,
    gc::{
        hamt::{hamt_empty, hamt_insert, hamt_lookup},
        heap_object::HeapObject,
    },
    jit_closure::JitClosure,
    value::Value,
};

use super::context::JitContext;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Safely dereference a JitContext pointer. Returns None if null.
unsafe fn ctx_ref<'a>(ctx: *mut JitContext) -> &'a mut JitContext {
    unsafe { &mut *ctx }
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
pub extern "C" fn rt_make_builtin(ctx: *mut JitContext, builtin_index: i64) -> *mut Value {
    unsafe { ctx_ref(ctx) }.alloc(Value::Builtin(builtin_index as u8))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_jit_closure(
    ctx: *mut JitContext,
    function_index: i64,
    captures_ptr: *const *mut Value,
    ncaptures: i64,
) -> *mut Value {
    let captures: Vec<Value> = (0..ncaptures as usize)
        .map(|i| unsafe { (*captures_ptr.add(i)).as_ref().unwrap().clone() })
        .collect();
    let closure = JitClosure::new(function_index as usize, captures);
    unsafe { ctx_ref(ctx) }.alloc(Value::JitClosure(Rc::new(closure)))
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

// ---------------------------------------------------------------------------
// Comparisons
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rt_equal(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    let result = values_equal(a, b);
    ctx.alloc(Value::Boolean(result))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_not_equal(ctx: *mut JitContext, a: *mut Value, b: *mut Value) -> *mut Value {
    let (a, b) = unsafe { (&*a, &*b) };
    let ctx = unsafe { ctx_ref(ctx) };
    let result = values_equal(a, b);
    ctx.alloc(Value::Boolean(!result))
}

fn values_equal(a: &Value, b: &Value) -> bool {
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
// Builtin calls
// ---------------------------------------------------------------------------

/// Call a builtin function by index. Arguments are passed as an array of
/// `*mut Value` pointers.
#[unsafe(no_mangle)]
pub extern "C" fn rt_call_builtin(
    ctx: *mut JitContext,
    builtin_index: i64,
    args_ptr: *const *mut Value,
    nargs: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let builtin = match get_builtin_by_index(builtin_index as usize) {
        Some(b) => b,
        None => {
            ctx.error = Some(format!("unknown builtin index: {}", builtin_index));
            return ptr::null_mut();
        }
    };

    // Collect arguments from pointer array
    let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
    for i in 0..nargs as usize {
        let arg_ptr = unsafe { *args_ptr.add(i) };
        if arg_ptr.is_null() {
            ctx.error = Some(format!("builtin arg {} evaluated to null", i));
            return ptr::null_mut();
        }
        args.push(unsafe { (*arg_ptr).clone() });
    }

    match (builtin.func)(ctx, args) {
        Ok(result) => ctx.alloc(result),
        Err(msg) => {
            ctx.error = Some(msg);
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
            ctx.error = Some(msg);
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
    if values_equal(a, b) { 1 } else { 0 }
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
    let mut elements = Vec::with_capacity(len as usize);
    for i in 0..len as usize {
        let elem = unsafe { (*elements_ptr.add(i)).as_ref().unwrap().clone() };
        elements.push(elem);
    }
    ctx.alloc(Value::Array(Rc::new(elements)))
}

#[unsafe(no_mangle)]
pub extern "C" fn rt_make_tuple(
    ctx: *mut JitContext,
    elements_ptr: *const *mut Value,
    len: i64,
) -> *mut Value {
    let ctx = unsafe { ctx_ref(ctx) };
    let mut elements = Vec::with_capacity(len as usize);
    for i in 0..len as usize {
        let elem = unsafe { (*elements_ptr.add(i)).as_ref().unwrap().clone() };
        elements.push(elem);
    }
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
        let key = unsafe { (*pairs_ptr.add(i * 2)).as_ref().unwrap().clone() };
        let value = unsafe { (*pairs_ptr.add(i * 2 + 1)).as_ref().unwrap().clone() };
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
    let v = unsafe { &*value };
    let s = v.to_string_value();
    unsafe { ctx_ref(ctx) }.alloc(Value::String(s.into()))
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
        ("rt_make_empty_list", rt_make_empty_list as *const u8),
        ("rt_make_string", rt_make_string as *const u8),
        ("rt_make_builtin", rt_make_builtin as *const u8),
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
        ("rt_equal", rt_equal as *const u8),
        ("rt_not_equal", rt_not_equal as *const u8),
        ("rt_greater_than", rt_greater_than as *const u8),
        ("rt_less_than_or_equal", rt_less_than_or_equal as *const u8),
        (
            "rt_greater_than_or_equal",
            rt_greater_than_or_equal as *const u8,
        ),
        ("rt_call_builtin", rt_call_builtin as *const u8),
        ("rt_call_value", rt_call_value as *const u8),
        ("rt_get_global", rt_get_global as *const u8),
        ("rt_set_global", rt_set_global as *const u8),
        ("rt_set_arity_error", rt_set_arity_error as *const u8),
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
    ]
}
