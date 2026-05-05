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
//! Future cyclic data features must use cycle-aware memory management.
use crate::runtime::value::Value;

pub mod r#async;
pub mod closure;
pub mod compiled_function;
pub mod cons_cell;
pub mod continuation;
pub mod evidence;
pub mod frame;
pub mod function_contract;
pub mod hamt;
pub mod handler_arm;
pub mod handler_descriptor;
pub mod handler_frame;
pub mod hash_key;
pub mod leak_detector;
pub mod perform_descriptor;
pub mod runtime_type;
pub mod value;
pub mod yield_state;

pub mod nanbox;

pub trait RuntimeContext {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String>;
    fn invoke_base_function_borrowed(
        &mut self,
        base_fn_index: usize,
        args: &[&Value],
    ) -> Result<Value, String>;
    fn invoke_unary_value(&mut self, callee: &Value, arg: Value) -> Result<Value, String> {
        self.invoke_value(callee.clone(), vec![arg])
    }
    fn invoke_binary_value(
        &mut self,
        callee: &Value,
        left: Value,
        right: Value,
    ) -> Result<Value, String> {
        self.invoke_value(callee.clone(), vec![left, right])
    }
    fn callable_contract<'a>(
        &'a self,
        _callee: &'a Value,
    ) -> Option<&'a function_contract::FunctionContract> {
        None
    }

    // ── Fiber-suspend hooks (proposal 0174 Phase 1b-vi-b₂.1) ────────────
    // Default impls panic; only the VM implementor needs to provide real
    // bodies. The native (LLVM) backend will get its own implementation in
    // Phase 1b-vi-d.

    /// Current frame index (for setting the FiberRunAsync boundary).
    fn current_frame_index(&self) -> usize {
        unimplemented!("current_frame_index not implemented for this RuntimeContext")
    }

    /// Current stack pointer (for setting the FiberRunAsync boundary).
    fn current_sp(&self) -> usize {
        unimplemented!("current_sp not implemented for this RuntimeContext")
    }

    /// Capture a delimited continuation from the current frame back to
    /// `(entry_frame_index, entry_sp)`. Returns a `Value::Continuation`.
    /// Mirrors the OpPerform capture mechanism with a non-handler boundary.
    fn capture_to_fiber_boundary(
        &mut self,
        _entry_frame_index: usize,
        _entry_sp: usize,
    ) -> Result<Value, String> {
        unimplemented!("capture_to_fiber_boundary not implemented for this RuntimeContext")
    }

    /// Resume a captured continuation outside any `OpPerform` context. Drives
    /// the inner interpreter loop until the captured frames return; the final
    /// value of the resumed body is returned.
    fn resume_from_dispatch(
        &mut self,
        _cont: Value,
        _resume_val: Value,
    ) -> Result<Value, String> {
        unimplemented!("resume_from_dispatch not implemented for this RuntimeContext")
    }
}
