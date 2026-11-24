//! Makes every conversion between a raw Int and a uniform word explicit,
//! before borrow inference and Dup/Drop insertion, so that Aether sees every
//! Int box (Lean's ExplicitBoxing, run before its RC pass).
//!
//! A binder with `FluxRep::IntRep` holds a raw Int. Every other position
//! takes the uniform word: call arguments and results, fields, captures,
//! and the arguments of every primop but Int arithmetic. Until the native
//! backend makes Ints raw, `BoxInt` and the unboxes are copies on both
//! backends.

pub(crate) mod check;
pub(crate) mod reps;
