//! Copied value representation for cross-worker transfer boundaries.
//!
//! VM `Value` uses `Rc` internally and is intentionally not `Send`. Phase 1a
//! therefore crosses worker boundaries by copying approved values into this
//! owned representation, or by moving opaque runtime handles. The home worker
//! converts copied payloads back into ordinary VM values when delivering the
//! result.

use std::rc::Rc;

use crate::runtime::{
    closure::Closure,
    compiled_function::CompiledFunction,
    cons_cell::ConsCell,
    hamt::{HamtEntry, HamtNode},
    handler_descriptor::HandlerDescriptor,
    perform_descriptor::PerformDescriptor,
    value::{AdtFields, AdtValue, Value},
};

#[derive(Debug, Clone, PartialEq)]
pub enum SendValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    EmptyList,
    Some(Box<SendValue>),
    Left(Box<SendValue>),
    Right(Box<SendValue>),
    Array(Vec<SendValue>),
    Tuple(Vec<SendValue>),
    Adt {
        constructor: String,
        fields: Vec<SendValue>,
    },
    AdtUnit(String),
    List(Vec<SendValue>),
    Map(Vec<(SendValueKey, SendValue)>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SendValueKey {
    Int(i64),
    Bool(bool),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendValueError {
    UnsupportedType(&'static str),
    UnsupportedMapKey,
    NotAClosure,
}

#[derive(Debug, Clone)]
pub struct SendClosure {
    function: CompiledFunction,
    free: Vec<SendValue>,
    constants: Vec<Option<SendConstant>>,
    globals: Vec<Option<SendConstant>>,
}

#[derive(Debug, Clone)]
enum SendConstant {
    Value(SendValue),
    Function(CompiledFunction),
    Closure {
        function: CompiledFunction,
        free: Vec<SendValue>,
    },
    HandlerDescriptor(HandlerDescriptor),
    PerformDescriptor(PerformDescriptor),
}

impl SendValue {
    pub fn try_from_value(value: &Value) -> Result<Self, SendValueError> {
        match value {
            Value::Integer(value) => Ok(Self::Int(*value)),
            Value::Float(value) => Ok(Self::Float(*value)),
            Value::Boolean(value) => Ok(Self::Bool(*value)),
            Value::String(value) => Ok(Self::String((**value).clone())),
            Value::Bytes(value) => Ok(Self::Bytes((**value).clone())),
            Value::None => Ok(Self::None),
            Value::EmptyList => Ok(Self::EmptyList),
            Value::Some(value) => Ok(Self::Some(Box::new(Self::try_from_value(value)?))),
            Value::Left(value) => Ok(Self::Left(Box::new(Self::try_from_value(value)?))),
            Value::Right(value) => Ok(Self::Right(Box::new(Self::try_from_value(value)?))),
            Value::Array(values) => values
                .iter()
                .map(Self::try_from_value)
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Array),
            Value::Tuple(values) => values
                .iter()
                .map(Self::try_from_value)
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Tuple),
            Value::Adt(value) => Ok(Self::Adt {
                constructor: value.constructor.as_ref().clone(),
                fields: value
                    .fields
                    .iter()
                    .map(Self::try_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Value::AdtUnit(name) => Ok(Self::AdtUnit(name.as_ref().clone())),
            Value::Cons(_) => collect_list(value)
                .ok_or(SendValueError::UnsupportedType("List"))
                .and_then(|values| {
                    values
                        .iter()
                        .map(Self::try_from_value)
                        .collect::<Result<Vec<_>, _>>()
                })
                .map(Self::List),
            Value::HashMap(node) => collect_hamt(node)
                .into_iter()
                .map(|(key, value)| {
                    Ok((
                        SendValueKey::try_from_value(&key)?,
                        SendValue::try_from_value(&value)?,
                    ))
                })
                .collect::<Result<Vec<_>, SendValueError>>()
                .map(Self::Map),
            Value::Uninit
            | Value::ReturnValue(_)
            | Value::Function(_)
            | Value::Closure(_)
            | Value::Continuation(_)
            | Value::HandlerDescriptor(_)
            | Value::PerformDescriptor(_) => {
                Err(SendValueError::UnsupportedType(value.type_name()))
            }
        }
    }

    pub fn into_value(self) -> Value {
        match self {
            Self::Int(value) => Value::Integer(value),
            Self::Float(value) => Value::Float(value),
            Self::Bool(value) => Value::Boolean(value),
            Self::String(value) => Value::String(Rc::new(value)),
            Self::Bytes(value) => Value::Bytes(Rc::new(value)),
            Self::None => Value::None,
            Self::EmptyList => Value::EmptyList,
            Self::Some(value) => Value::Some(Rc::new(value.into_value())),
            Self::Left(value) => Value::Left(Rc::new(value.into_value())),
            Self::Right(value) => Value::Right(Rc::new(value.into_value())),
            Self::Array(values) => Value::Array(Rc::new(
                values.into_iter().map(SendValue::into_value).collect(),
            )),
            Self::Tuple(values) => Value::Tuple(Rc::new(
                values.into_iter().map(SendValue::into_value).collect(),
            )),
            Self::Adt {
                constructor,
                fields,
            } => Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new(constructor),
                fields: AdtFields::from_vec(
                    fields.into_iter().map(SendValue::into_value).collect(),
                ),
            })),
            Self::AdtUnit(name) => Value::AdtUnit(Rc::new(name)),
            Self::List(values) => values
                .into_iter()
                .rev()
                .fold(Value::EmptyList, |tail, head| {
                    ConsCell::cons(head.into_value(), tail)
                }),
            Self::Map(values) => {
                let mut map = crate::runtime::hamt::hamt_empty();
                for (key, value) in values {
                    map = crate::runtime::hamt::hamt_insert(
                        &map,
                        key.into_hash_key(),
                        value.into_value(),
                    );
                }
                Value::HashMap(map)
            }
        }
    }
}

impl SendValueKey {
    fn try_from_value(value: &Value) -> Result<Self, SendValueError> {
        match value {
            Value::Integer(value) => Ok(Self::Int(*value)),
            Value::Boolean(value) => Ok(Self::Bool(*value)),
            Value::String(value) => Ok(Self::String(value.as_ref().clone())),
            _ => Err(SendValueError::UnsupportedMapKey),
        }
    }

    fn into_hash_key(self) -> crate::runtime::hash_key::HashKey {
        match self {
            Self::Int(value) => crate::runtime::hash_key::HashKey::Integer(value),
            Self::Bool(value) => crate::runtime::hash_key::HashKey::Boolean(value),
            Self::String(value) => crate::runtime::hash_key::HashKey::String(value),
        }
    }
}

impl SendClosure {
    pub fn try_from_value_with_constants(
        value: &Value,
        constants: impl IntoIterator<Item = Value>,
    ) -> Result<Self, SendValueError> {
        Self::try_from_value_with_context(value, constants, Vec::new())
    }

    pub fn try_from_value_with_context(
        value: &Value,
        constants: impl IntoIterator<Item = Value>,
        globals: impl IntoIterator<Item = Value>,
    ) -> Result<Self, SendValueError> {
        let Value::Closure(closure) = value else {
            return Err(SendValueError::NotAClosure);
        };
        Self::try_from_closure_with_context(closure, constants, globals)
    }

    pub fn try_from_closure_with_constants(
        closure: &Closure,
        constants: impl IntoIterator<Item = Value>,
    ) -> Result<Self, SendValueError> {
        Self::try_from_closure_with_context(closure, constants, Vec::new())
    }

    pub fn try_from_closure_with_context(
        closure: &Closure,
        constants: impl IntoIterator<Item = Value>,
        globals: impl IntoIterator<Item = Value>,
    ) -> Result<Self, SendValueError> {
        let free = closure
            .free
            .iter()
            .map(SendValue::try_from_value)
            .collect::<Result<Vec<_>, _>>()?;
        let constants = constants
            .into_iter()
            .map(|constant| SendConstant::try_from_value(&constant).ok())
            .collect();
        let globals = globals
            .into_iter()
            .map(|global| {
                if matches!(global, Value::None | Value::Uninit) {
                    None
                } else {
                    SendConstant::try_from_value(&global).ok()
                }
            })
            .collect();
        Ok(Self {
            function: closure.function.as_ref().clone(),
            free,
            constants,
            globals,
        })
    }

    pub fn into_closure_value(self) -> Value {
        Value::Closure(Rc::new(Closure::new(
            Rc::new(self.function),
            self.free
                .into_iter()
                .map(SendValue::into_value)
                .collect::<Vec<_>>(),
        )))
    }

    pub fn constants_into_values(&self) -> Vec<Value> {
        self.constants
            .iter()
            .map(|constant| {
                constant
                    .clone()
                    .map(SendConstant::into_value)
                    .unwrap_or(Value::Uninit)
            })
            .collect()
    }

    pub fn globals_into_values(&self) -> Vec<Option<Value>> {
        self.globals
            .iter()
            .map(|global| global.clone().map(SendConstant::into_value))
            .collect()
    }
}

impl SendConstant {
    fn try_from_value(value: &Value) -> Result<Self, SendValueError> {
        match value {
            Value::Function(function) => Ok(Self::Function(function.as_ref().clone())),
            Value::Closure(closure) => {
                let free = closure
                    .free
                    .iter()
                    .map(SendValue::try_from_value)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::Closure {
                    function: closure.function.as_ref().clone(),
                    free,
                })
            }
            Value::HandlerDescriptor(descriptor) => {
                Ok(Self::HandlerDescriptor(descriptor.as_ref().clone()))
            }
            Value::PerformDescriptor(descriptor) => {
                Ok(Self::PerformDescriptor(descriptor.as_ref().clone()))
            }
            other => SendValue::try_from_value(other).map(Self::Value),
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::Value(value) => value.into_value(),
            Self::Function(function) => Value::Function(Rc::new(function)),
            Self::Closure { function, free } => Value::Closure(Rc::new(Closure::new(
                Rc::new(function),
                free.into_iter().map(SendValue::into_value).collect(),
            ))),
            Self::HandlerDescriptor(descriptor) => Value::HandlerDescriptor(Rc::new(descriptor)),
            Self::PerformDescriptor(descriptor) => Value::PerformDescriptor(Rc::new(descriptor)),
        }
    }
}

fn collect_list(value: &Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    let mut current = value;
    loop {
        match current {
            Value::EmptyList | Value::None => return Some(out),
            Value::Cons(cell) => {
                out.push(cell.head.clone());
                current = &cell.tail;
            }
            _ => return None,
        }
    }
}

fn collect_hamt(node: &Rc<HamtNode>) -> Vec<(Value, Value)> {
    let mut out = Vec::new();
    collect_hamt_node(node, &mut out);
    out
}

fn collect_hamt_node(node: &HamtNode, out: &mut Vec<(Value, Value)>) {
    for entry in &node.children {
        match entry {
            HamtEntry::Leaf(key, value) => {
                out.push((key_to_value(key), value.clone()));
            }
            HamtEntry::Node(child) => collect_hamt_node(child, out),
            HamtEntry::Collision(collision) => {
                for (key, value) in &collision.entries {
                    out.push((key_to_value(key), value.clone()));
                }
            }
        }
    }
}

fn key_to_value(key: &crate::runtime::hash_key::HashKey) -> Value {
    match key {
        crate::runtime::hash_key::HashKey::Integer(value) => Value::Integer(*value),
        crate::runtime::hash_key::HashKey::Boolean(value) => Value::Boolean(*value),
        crate::runtime::hash_key::HashKey::String(value) => Value::String(Rc::new(value.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_value_roundtrips_compound_sendable_values() {
        let value = Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Pair".to_string()),
            fields: AdtFields::Two(
                Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)])),
                Value::Some(Rc::new(Value::String(Rc::new("ok".to_string())))),
            ),
        }));

        let copied = SendValue::try_from_value(&value).expect("value is sendable");
        assert_eq!(copied.into_value(), value);
    }

    #[test]
    fn copied_value_breaks_rc_sharing_across_worker_boundary() {
        let source_string = Rc::new("shared".to_string());
        let source_array = Rc::new(vec![Value::String(Rc::clone(&source_string))]);
        let source_some = Rc::new(Value::Array(Rc::clone(&source_array)));
        let source_adt = Rc::new(AdtValue {
            constructor: Rc::new("Box".to_string()),
            fields: AdtFields::Two(
                Value::Some(Rc::clone(&source_some)),
                Value::String(Rc::clone(&source_string)),
            ),
        });
        let value = Value::Adt(Rc::clone(&source_adt));

        let copied = SendValue::try_from_value(&value)
            .expect("sendable value copies into worker-safe representation")
            .into_value();

        let Value::Adt(copied_adt) = copied else {
            panic!("expected copied ADT");
        };
        assert!(!Rc::ptr_eq(&source_adt, &copied_adt));
        let Some(Value::Some(copied_some)) = copied_adt.fields.get(0) else {
            panic!("expected copied Some field");
        };
        assert!(!Rc::ptr_eq(&source_some, copied_some));
        let Value::Array(copied_array) = copied_some.as_ref() else {
            panic!("expected copied Array payload");
        };
        assert!(!Rc::ptr_eq(&source_array, copied_array));
        let Some(Value::String(copied_nested_string)) = copied_array.first() else {
            panic!("expected copied nested String");
        };
        assert!(!Rc::ptr_eq(&source_string, copied_nested_string));
        let Some(Value::String(copied_string)) = copied_adt.fields.get(1) else {
            panic!("expected copied String field");
        };
        assert!(!Rc::ptr_eq(&source_string, copied_string));
    }

    #[test]
    fn copied_value_rejects_closures() {
        let function = Rc::new(crate::runtime::compiled_function::CompiledFunction::new(
            Vec::new(),
            0,
            0,
            None,
        ));
        let closure = Value::Closure(Rc::new(crate::runtime::closure::Closure::new(
            function,
            Vec::new(),
        )));

        assert_eq!(
            SendValue::try_from_value(&closure),
            Err(SendValueError::UnsupportedType("Closure"))
        );
    }

    #[test]
    fn copied_value_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SendValue>();
        assert_send::<SendClosure>();
    }

    #[test]
    fn send_closure_copies_function_free_values_and_constants() {
        let function = Rc::new(crate::runtime::compiled_function::CompiledFunction::new(
            vec![crate::bytecode::op_code::OpCode::OpReturnLocal as u8, 0],
            1,
            1,
            None,
        ));
        let function_constant = Value::Function(function.clone());
        let closure = Value::Closure(Rc::new(crate::runtime::closure::Closure::new(
            function,
            vec![Value::String(Rc::new("captured".to_string()))],
        )));

        let send_closure = SendClosure::try_from_value_with_constants(
            &closure,
            vec![Value::Integer(42), function_constant.clone()],
        )
        .expect("closure copies");

        let constants = send_closure.constants_into_values();
        assert_eq!(constants[0], Value::Integer(42));
        assert_eq!(constants[1], function_constant);

        let copied = send_closure.into_closure_value();
        let Value::Closure(copied) = copied else {
            panic!("expected closure");
        };
        assert_eq!(
            copied.free,
            vec![Value::String(Rc::new("captured".to_string()))]
        );
    }

    #[test]
    fn send_closure_preserves_internal_descriptor_constants() {
        let function = Rc::new(crate::runtime::compiled_function::CompiledFunction::new(
            vec![crate::bytecode::op_code::OpCode::OpReturn as u8],
            0,
            0,
            None,
        ));
        let closure = Value::Closure(Rc::new(crate::runtime::closure::Closure::new(
            function,
            Vec::new(),
        )));
        let descriptor = Value::PerformDescriptor(Rc::new(PerformDescriptor {
            effect: crate::syntax::symbol::Symbol::new(1),
            op: crate::syntax::symbol::Symbol::new(2),
            effect_name: "Suspend".into(),
            op_name: "sleep".into(),
        }));

        let send_closure =
            SendClosure::try_from_value_with_constants(&closure, vec![descriptor.clone()])
                .expect("descriptor constants are copied");

        assert_eq!(send_closure.constants_into_values(), vec![descriptor]);
    }

    #[test]
    fn send_closure_preserves_function_globals_for_worker_vm() {
        let function = Rc::new(crate::runtime::compiled_function::CompiledFunction::new(
            vec![crate::bytecode::op_code::OpCode::OpReturn as u8],
            0,
            0,
            None,
        ));
        let action = Value::Closure(Rc::new(crate::runtime::closure::Closure::new(
            function.clone(),
            Vec::new(),
        )));
        let global = Value::Closure(Rc::new(crate::runtime::closure::Closure::new(
            function,
            Vec::new(),
        )));

        let send_closure = SendClosure::try_from_value_with_context(
            &action,
            Vec::new(),
            vec![Value::None, global.clone()],
        )
        .expect("globals copy");

        let globals = send_closure.globals_into_values();
        assert_eq!(globals[0], None);
        assert_eq!(globals[1], Some(global));
    }
}
