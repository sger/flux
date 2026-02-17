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

/// Set an error on the context and return null.
unsafe fn err(ctx: *mut JitContext, msg: String) -> *mut Value {
    unsafe { ctx_ref(ctx) }.error = Some(msg);
    ptr::null_mut()
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
pub extern "C" fn rt_make_string(ctx: *mut JitContext, ptr: *const u8, len: i64) -> *mut Value {
    let s = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len as usize)) };
    let rc: Rc<str> = Rc::from(s);
    unsafe { ctx_ref(ctx) }.alloc(Value::String(rc))
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
        (Value::Integer(_, ), Value::Integer(0)) | (Value::Float(_), Value::Integer(0)) => {
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
    let args: Vec<Value> = (0..nargs as usize)
        .map(|i| unsafe { (*args_ptr.add(i)).as_ref().unwrap().clone() })
        .collect();

    match (builtin.func)(ctx, args) {
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
        ("rt_make_string", rt_make_string as *const u8),
        ("rt_add", rt_add as *const u8),
        ("rt_sub", rt_sub as *const u8),
        ("rt_mul", rt_mul as *const u8),
        ("rt_div", rt_div as *const u8),
        ("rt_mod", rt_mod as *const u8),
        ("rt_negate", rt_negate as *const u8),
        ("rt_not", rt_not as *const u8),
        ("rt_equal", rt_equal as *const u8),
        ("rt_not_equal", rt_not_equal as *const u8),
        ("rt_greater_than", rt_greater_than as *const u8),
        ("rt_less_than_or_equal", rt_less_than_or_equal as *const u8),
        (
            "rt_greater_than_or_equal",
            rt_greater_than_or_equal as *const u8,
        ),
        ("rt_call_builtin", rt_call_builtin as *const u8),
        ("rt_get_global", rt_get_global as *const u8),
        ("rt_set_global", rt_set_global as *const u8),
    ]
}
