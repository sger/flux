use crate::runtime::object::Object;

pub mod builtin_function;
pub mod builtins;
pub mod closure;
pub mod compiled_function;
pub mod frame;
pub mod hash_key;
pub mod leak_detector;
pub mod object;
pub mod value;
pub mod vm;

pub type BuiltinFn = fn(Vec<Object>) -> Result<Object, String>;
