use std::{collections::HashMap, fmt, rc::Rc};

use crate::runtime::{
    builtin_function::BuiltinFunction, closure::Closure, compiled_function::CompiledFunction,
    hash_key::HashKey,
};

/// Runtime value used by the VM stack, globals, constants, and closures.
///
/// ## Memory Management Model
///
/// Values use `Rc` (reference counting) for heap-allocated types (String, Array, Hash, etc.)
/// while keeping primitives (Integer, Float, Boolean, None) unboxed for efficiency.
///
/// ### No-Cycle Invariant
///
/// This design **requires maintaining acyclic value graphs**. Runtime values must form
/// directed acyclic graphs (DAGs), never cycles.
///
/// **Invariant guarantees:**
/// - Closures may capture values, but captured values cannot reference the capturing closure
/// - No language feature exposes mutable reference cells that could create back-edges
/// - Values are semantically immutable after creation
///
/// **Why this matters:**
/// - `Rc` cannot handle reference cycles (would cause memory leaks)
/// - The language design enforces this through immutability and lack of mutable cells
/// - Future features requiring cycles must migrate to cycle-aware GC (Proposal 017)
///
/// **Validation:**
/// - Tests verify deeply nested captures complete without leaks
/// - Leak detector tracks allocation/deallocation of Rc-wrapped types
///
/// ### Design Rationale
///
/// Using `Rc<str>` instead of `Rc<String>` avoids double indirection.
/// Using `Rc<Vec<Value>>` and `Rc<HashMap<...>>` makes cloning O(1) instead of O(n).
///
/// See [Proposal 019](../../docs/proposals/019_zero_copy_value_passing.md) for details.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 64-bit signed integer.
    Integer(i64),
    /// 64-bit floating point number.
    Float(f64),
    /// Boolean value.
    Boolean(bool),
    /// UTF-8 string value.
    String(Rc<str>),
    /// Absence of value.
    None,
    /// Optional value wrapper.
    Some(Rc<Value>),
    /// Either-left wrapper.
    Left(Rc<Value>),
    /// Either-right wrapper.
    Right(Rc<Value>),
    /// Internal return-signal wrapper used by function returns.
    ReturnValue(Rc<Value>),
    /// Compiled function object.
    Function(Rc<CompiledFunction>),
    /// Runtime closure object.
    Closure(Rc<Closure>),
    /// Builtin function handle.
    Builtin(BuiltinFunction),
    /// Ordered collection of values.
    Array(Rc<Vec<Value>>),
    /// Hash map keyed by hashable values.
    Hash(Rc<HashMap<HashKey, Value>>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Integer(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Boolean(v) => write!(f, "{}", v),
            Value::String(v) => write!(f, "\"{}\"", v),
            Value::None => write!(f, "None"),
            Value::Some(v) => write!(f, "Some({})", v),
            Value::Left(v) => write!(f, "Left({})", v),
            Value::Right(v) => write!(f, "Right({})", v),
            Value::ReturnValue(v) => write!(f, "{}", v),
            Value::Function(_) => write!(f, "<function>"),
            Value::Closure(_) => write!(f, "<closure>"),
            Value::Builtin(_) => write!(f, "<builtin>"),
            Value::Array(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Value::Hash(pairs) => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
        }
    }
}

impl Value {
    /// Returns the canonical runtime type label used in diagnostics and builtins.
    ///
    /// These labels are user-visible and are expected to remain stable.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Integer(_) => "Int",
            Value::Float(_) => "Float",
            Value::Boolean(_) => "Bool",
            Value::String(_) => "String",
            Value::None => "None",
            Value::Some(_) => "Some",
            Value::Left(_) => "Left",
            Value::Right(_) => "Right",
            Value::ReturnValue(_) => "ReturnValue",
            Value::Function(_) => "Function",
            Value::Closure(_) => "Closure",
            Value::Builtin(_) => "Builtin",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
        }
    }

    /// Returns whether this value is truthy according to Flux semantics.
    ///
    /// Only `Boolean(false)` and `None` are falsy; all other values are truthy.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Boolean(false) | Value::None)
    }

    /// Converts this value into a hash-map key if the value is hashable.
    ///
    /// Hashable variants are:
    /// - `Integer`
    /// - `Boolean`
    /// - `String`
    ///
    /// Returns `None` for all other variants.
    pub fn to_hash_key(&self) -> Option<HashKey> {
        match self {
            Value::Integer(v) => Some(HashKey::Integer(*v)),
            Value::Boolean(v) => Some(HashKey::Boolean(*v)),
            Value::String(v) => Some(HashKey::String(v.to_string())),
            _ => None,
        }
    }

    /// Converts a value to interpolation-friendly string text.
    ///
    /// Unlike [`std::fmt::Display`], strings are returned without quotes.
    /// This helper is used by interpolation and string conversion builtins.
    pub fn to_string_value(&self) -> String {
        match self {
            Value::Integer(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Boolean(v) => v.to_string(),
            Value::String(v) => v.to_string(),
            Value::None => "None".to_string(),
            Value::Some(v) => format!("Some({})", v.to_string_value()),
            Value::Left(_) => "Left({})".to_string(),
            Value::Right(_) => "Right({})".to_string(),
            Value::ReturnValue(v) => v.to_string_value(),
            Value::Function(_) => "<function>".to_string(),
            Value::Closure(_) => "<closure>".to_string(),
            Value::Builtin(_) => "<builtin>".to_string(),
            Value::Array(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Hash(pairs) => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                format!("{{{}}}", items.join(", "))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_display() {
        assert_eq!(Value::Integer(42).to_string(), "42");
        assert_eq!(Value::Float(3.5).to_string(), "3.5");
        assert_eq!(Value::Boolean(true).to_string(), "true");
        assert_eq!(
            Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)])).to_string(),
            "[1, 2]"
        );
    }

    #[test]
    fn test_is_truthy() {
        assert!(Value::Integer(0).is_truthy());
        assert!(Value::Float(0.0).is_truthy());
        assert!(Value::Boolean(true).is_truthy());
        assert!(!Value::Boolean(false).is_truthy());
        assert!(!Value::None.is_truthy());
    }

    #[test]
    fn test_hash_key() {
        assert_eq!(Value::Integer(1).to_hash_key(), Some(HashKey::Integer(1)));
        assert_eq!(
            Value::Boolean(false).to_hash_key(),
            Some(HashKey::Boolean(false))
        );
        assert_eq!(
            Value::String("a".into()).to_hash_key(),
            Some(HashKey::String("a".to_string()))
        );
        assert_eq!(Value::Array(Rc::new(vec![])).to_hash_key(), None);
    }

    #[test]
    fn test_type_name() {
        assert_eq!(Value::Integer(1).type_name(), "Int");
        assert_eq!(Value::Float(1.0).type_name(), "Float");
        assert_eq!(Value::Boolean(true).type_name(), "Bool");
        assert_eq!(Value::String("x".into()).type_name(), "String");
        assert_eq!(Value::None.type_name(), "None");
        assert_eq!(Value::Some(Rc::new(Value::Integer(1))).type_name(), "Some");
        assert_eq!(Value::Left(Rc::new(Value::Integer(1))).type_name(), "Left");
        assert_eq!(
            Value::Right(Rc::new(Value::Integer(1))).type_name(),
            "Right"
        );
        assert_eq!(
            Value::ReturnValue(Rc::new(Value::Integer(1))).type_name(),
            "ReturnValue"
        );
        assert_eq!(Value::Array(Rc::new(vec![])).type_name(), "Array");
        assert_eq!(Value::Hash(Rc::new(HashMap::new())).type_name(), "Hash");
    }

    #[test]
    fn test_to_string_value() {
        assert_eq!(Value::String("hello".into()).to_string_value(), "hello");
        assert_eq!(
            Value::Some(Rc::new(Value::String("x".into()))).to_string_value(),
            "Some(x)"
        );
        assert_eq!(
            Value::ReturnValue(Rc::new(Value::Integer(7))).to_string_value(),
            "7"
        );
        assert_eq!(
            Value::Array(Rc::new(vec![Value::String("a".into()), Value::Integer(2)]))
                .to_string_value(),
            "[\"a\", 2]"
        );
    }

    #[test]
    fn test_clone_shares_rc_for_string() {
        let value = Value::String("hello".into());
        let cloned = value.clone();

        match (value, cloned) {
            (Value::String(left), Value::String(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected string values"),
        }
    }

    #[test]
    fn test_clone_shares_rc_for_array_and_hash() {
        let array = Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)]));
        let array_clone = array.clone();
        match (array, array_clone) {
            (Value::Array(left), Value::Array(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected array values"),
        }

        let mut map = HashMap::new();
        map.insert(HashKey::String("k".to_string()), Value::Integer(42));
        let hash = Value::Hash(Rc::new(map));
        let hash_clone = hash.clone();
        match (hash, hash_clone) {
            (Value::Hash(left), Value::Hash(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected hash values"),
        }
    }

    #[test]
    fn test_clone_shares_rc_for_wrappers() {
        let some = Value::Some(Rc::new(Value::Integer(7)));
        let some_clone = some.clone();
        match (some, some_clone) {
            (Value::Some(left), Value::Some(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected some values"),
        }

        let ret = Value::ReturnValue(Rc::new(Value::String("ok".into())));
        let ret_clone = ret.clone();
        match (ret, ret_clone) {
            (Value::ReturnValue(left), Value::ReturnValue(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected return values"),
        }
    }
}
