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

pub mod r#async;
pub mod nanbox;

pub trait RuntimeContext {
    fn invoke_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, String>;
    fn task_spawn(&mut self, _action: Value) -> Result<Value, String> {
        Err("Task.spawn is not supported by this runtime context".to_string())
    }
    fn task_blocking_join(&mut self, _task: Value) -> Result<Value, String> {
        Err("Task.blocking_join is not supported by this runtime context".to_string())
    }
    fn task_cancel(&mut self, _task: Value) -> Result<Value, String> {
        Err("Task.cancel is not supported by this runtime context".to_string())
    }
    fn async_sleep(&mut self, _ms: Value) -> Result<Value, String> {
        Err("Async.sleep is not supported by this runtime context".to_string())
    }
    fn async_yield_now(&mut self) -> Result<Value, String> {
        Err("Async.yield_now is not supported by this runtime context".to_string())
    }
    fn async_both(&mut self, _left: Value, _right: Value) -> Result<Value, String> {
        Err("Async.both is not supported by this runtime context".to_string())
    }
    fn async_race(&mut self, _left: Value, _right: Value) -> Result<Value, String> {
        Err("Async.race is not supported by this runtime context".to_string())
    }
    fn async_timeout(&mut self, _ms: Value, _action: Value) -> Result<Value, String> {
        Err("Async.timeout is not supported by this runtime context".to_string())
    }
    fn async_timeout_result(&mut self, _ms: Value, _action: Value) -> Result<Value, String> {
        Err("Async.timeout_result is not supported by this runtime context".to_string())
    }
    fn async_scope(&mut self, _body: Value) -> Result<Value, String> {
        Err("Async.scope is not supported by this runtime context".to_string())
    }
    fn async_fork(&mut self, _scope: Value, _action: Value) -> Result<Value, String> {
        Err("Async.fork is not supported by this runtime context".to_string())
    }
    fn async_try(&mut self, _body: Value) -> Result<Value, String> {
        Err("Async.try_ is not supported by this runtime context".to_string())
    }
    fn async_finally(&mut self, _body: Value, _cleanup: Value) -> Result<Value, String> {
        Err("Async.finally is not supported by this runtime context".to_string())
    }
    fn async_bracket(
        &mut self,
        _acquire: Value,
        _release: Value,
        _body: Value,
    ) -> Result<Value, String> {
        Err("Async.bracket is not supported by this runtime context".to_string())
    }
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
}
