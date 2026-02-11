//! Runtime core types and VM execution.
//!
//! # No-Cycle Invariant
//! Flux runtime values are represented as immutable graphs and are expected to
//! remain acyclic. Heap-backed `Value` variants use `Rc` for cheap sharing, so
//! introducing cycles would leak memory under reference counting.
//!
//! The invariant is:
//! - Runtime values form immutable DAGs, not cyclic graphs.
//! - Language/runtime features must not create back-edges into already-reachable
//!   values in the `Rc`-managed value graph.
//! - Closures may capture values, but captured values must not reference the
//!   capturing closure.
//!
//! Any future cyclic data feature must use cycle-aware memory management.
use crate::runtime::value::Value;

pub mod builtin_function;
pub mod builtins;
pub mod closure;
pub mod compiled_function;
pub mod frame;
pub mod hash_key;
pub mod leak_detector;
pub mod value;
pub mod vm;

pub type BuiltinFn = fn(Vec<Value>) -> Result<Value, String>;
