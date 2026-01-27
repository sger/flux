use std::{collections::HashMap, fmt, rc::Rc};

use crate::runtime::{
    builtin_function::BuiltinFunction, closure::Closure, compiled_function::CompiledFunction,
    hash_key::HashKey,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Integer(i64),
    Boolean(bool),
    String(String),
    Null,
    ReturnValue(Box<Object>),
    Function(Rc<CompiledFunction>),
    Closure(Rc<Closure>),
    Builtin(BuiltinFunction),
    Array(Vec<Object>),
    Hash(HashMap<HashKey, Object>),
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Object::Integer(v) => write!(f, "{}", v),
            Object::Boolean(v) => write!(f, "{}", v),
            Object::String(v) => write!(f, "\"{}\"", v),
            Object::Null => write!(f, "null"),
            Object::ReturnValue(v) => write!(f, "{}", v),
            Object::Function(_) => write!(f, "<function>"),
            Object::Closure(_) => write!(f, "<closure>"),
            Object::Builtin(_) => write!(f, "<builtin>"),
            Object::Array(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Object::Hash(pairs) => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
        }
    }
}

impl Object {
    pub fn type_name(&self) -> &'static str {
        match self {
            Object::Integer(_) => "Int",
            Object::Boolean(_) => "Bool",
            Object::String(_) => "String",
            Object::Null => "Null",
            Object::ReturnValue(_) => "ReturnValue",
            Object::Function(_) => "Function",
            Object::Closure(_) => "Closure",
            Object::Builtin(_) => "Builtin",
            Object::Array(_) => "Array",
            Object::Hash(_) => "Hash",
        }
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, Object::Boolean(false) | Object::Null)
    }

    pub fn to_hash_key(&self) -> Option<HashKey> {
        match self {
            Object::Integer(v) => Some(HashKey::Integer(*v)),
            Object::Boolean(v) => Some(HashKey::Boolean(*v)),
            Object::String(v) => Some(HashKey::String(v.clone())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_display() {
        assert_eq!(Object::Integer(42).to_string(), "42");
        assert_eq!(Object::Boolean(true).to_string(), "true");
        assert_eq!(Object::Null.to_string(), "null");
        assert_eq!(
            Object::Array(vec![Object::Integer(1), Object::Integer(2)]).to_string(),
            "[1, 2]"
        );
    }

    #[test]
    fn test_is_truthy() {
        assert!(Object::Integer(0).is_truthy());
        assert!(Object::Boolean(true).is_truthy());
        assert!(!Object::Boolean(false).is_truthy());
        assert!(!Object::Null.is_truthy());
    }

    #[test]
    fn test_hash_key() {
        assert_eq!(Object::Integer(1).to_hash_key(), Some(HashKey::Integer(1)));
        assert_eq!(
            Object::String("a".to_string()).to_hash_key(),
            Some(HashKey::String("a".to_string()))
        );
        assert_eq!(Object::Array(vec![]).to_hash_key(), None);
    }
}
