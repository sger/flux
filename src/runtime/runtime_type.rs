use std::fmt;

use crate::runtime::{RuntimeContext, gc::hamt::is_hamt, value::Value};

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeType {
    Any,
    Int,
    Float,
    Bool,
    String,
    Unit,
    Option(Box<RuntimeType>),
    Array(Box<RuntimeType>),
    Map(Box<RuntimeType>, Box<RuntimeType>),
    Tuple(Vec<RuntimeType>),
}

impl RuntimeType {
    pub fn type_name(&self) -> String {
        match self {
            RuntimeType::Any => "Any".to_string(),
            RuntimeType::Int => "Int".to_string(),
            RuntimeType::Float => "Float".to_string(),
            RuntimeType::Bool => "Bool".to_string(),
            RuntimeType::String => "String".to_string(),
            RuntimeType::Unit => "Unit".to_string(),
            RuntimeType::Option(inner) => format!("Option<{}>", inner.type_name()),
            RuntimeType::Array(inner) => format!("Array<{}>", inner.type_name()),
            RuntimeType::Map(k, v) => format!("Map<{}, {}>", k.type_name(), v.type_name()),
            RuntimeType::Tuple(elements) => {
                let parts: Vec<String> = elements.iter().map(RuntimeType::type_name).collect();
                format!("({})", parts.join(", "))
            }
        }
    }

    pub fn matches_value(&self, value: &Value, ctx: &dyn RuntimeContext) -> bool {
        match self {
            RuntimeType::Any => true,
            RuntimeType::Int => matches!(value, Value::Integer(_)),
            RuntimeType::Float => matches!(value, Value::Float(_)),
            RuntimeType::Bool => matches!(value, Value::Boolean(_)),
            RuntimeType::String => matches!(value, Value::String(_)),
            RuntimeType::Unit => matches!(value, Value::None),
            RuntimeType::Option(inner) => match value {
                Value::None => true,
                Value::Some(v) => inner.matches_value(v, ctx),
                _ => false,
            },
            RuntimeType::Array(inner) => match value {
                Value::Array(elements) => elements.iter().all(|v| inner.matches_value(v, ctx)),
                _ => false,
            },
            RuntimeType::Map(_, _) => match value {
                Value::Gc(h) => is_hamt(ctx.gc_heap(), *h),
                _ => false,
            },
            RuntimeType::Tuple(expected) => match value {
                Value::Tuple(elements) if elements.len() == expected.len() => expected
                    .iter()
                    .zip(elements.iter())
                    .all(|(ty, value)| ty.matches_value(value, ctx)),
                _ => false,
            },
        }
    }
}

impl fmt::Display for RuntimeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_name())
    }
}
