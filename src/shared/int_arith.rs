//! What Flux's `Int` arithmetic means, in one place for the VM and the
//! constant folders.
//!
//! `Int` is a 64-bit two's-complement integer. `+`, `-`, `*`, negation and
//! `abs` wrap. `/` and `%` truncate toward zero, and `MIN / -1` is `MIN` and
//! `MIN % -1` is `0`, as Go and Rust's `wrapping_div` give. A zero divisor is
//! the caller's to report: these functions are only defined for a non-zero
//! one.

pub fn add(a: i64, b: i64) -> i64 {
    a.wrapping_add(b)
}

pub fn sub(a: i64, b: i64) -> i64 {
    a.wrapping_sub(b)
}

pub fn mul(a: i64, b: i64) -> i64 {
    a.wrapping_mul(b)
}

pub fn neg(a: i64) -> i64 {
    a.wrapping_neg()
}

pub fn abs(a: i64) -> i64 {
    a.wrapping_abs()
}

/// `b` must not be zero.
pub fn div(a: i64, b: i64) -> i64 {
    a.wrapping_div(b)
}

/// `b` must not be zero.
pub fn rem(a: i64, b: i64) -> i64 {
    a.wrapping_rem(b)
}
