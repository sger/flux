use std::fmt;

use crate::runtime::{
    RuntimeContext,
    gc::{HeapObject, hamt::is_hamt},
    value::Value,
};

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeType {
    Any,
    Int,
    Float,
    Bool,
    String,
    Unit,
    Option(Box<RuntimeType>),
    List(Box<RuntimeType>),
    Either(Box<RuntimeType>, Box<RuntimeType>),
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
            RuntimeType::List(inner) => format!("List<{}>", inner.type_name()),
            RuntimeType::Either(left, right) => {
                format!("Either<{}, {}>", left.type_name(), right.type_name())
            }
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
            RuntimeType::List(inner) => {
                let mut current = value;
                loop {
                    match current {
                        Value::None | Value::EmptyList => return true,
                        Value::Gc(handle) => match ctx.gc_heap().get(*handle) {
                            HeapObject::Cons { head, tail } => {
                                if !inner.matches_value(head, ctx) {
                                    return false;
                                }
                                current = tail;
                            }
                            _ => return false,
                        },
                        _ => return false,
                    }
                }
            }
            RuntimeType::Either(left, right) => match value {
                Value::Left(v) => left.matches_value(v, ctx),
                Value::Right(v) => right.matches_value(v, ctx),
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

#[cfg(test)]
mod tests {
    use super::RuntimeType;
    use crate::runtime::{
        RuntimeContext,
        gc::{GcHeap, HeapObject},
        value::Value,
    };

    struct TestCtx {
        heap: GcHeap,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                heap: GcHeap::new(),
            }
        }
    }

    impl RuntimeContext for TestCtx {
        fn invoke_value(&mut self, _callee: Value, _args: Vec<Value>) -> Result<Value, String> {
            Err("not used in runtime_type tests".to_string())
        }

        fn gc_heap(&self) -> &GcHeap {
            &self.heap
        }

        fn gc_heap_mut(&mut self) -> &mut GcHeap {
            &mut self.heap
        }
    }

    #[test]
    fn list_runtime_type_matches_cons_lists_recursively() {
        let mut ctx = TestCtx::new();
        let h2 = ctx.gc_heap_mut().alloc(HeapObject::Cons {
            head: Value::Integer(2),
            tail: Value::EmptyList,
        });
        let h1 = ctx.gc_heap_mut().alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::Gc(h2),
        });
        let good = Value::Gc(h1);

        let ty = RuntimeType::List(Box::new(RuntimeType::Int));
        assert!(ty.matches_value(&good, &ctx));
    }

    #[test]
    fn list_runtime_type_rejects_mixed_element_types() {
        let mut ctx = TestCtx::new();
        let h2 = ctx.gc_heap_mut().alloc(HeapObject::Cons {
            head: Value::String("x".into()),
            tail: Value::EmptyList,
        });
        let h1 = ctx.gc_heap_mut().alloc(HeapObject::Cons {
            head: Value::Integer(1),
            tail: Value::Gc(h2),
        });
        let bad = Value::Gc(h1);

        let ty = RuntimeType::List(Box::new(RuntimeType::Int));
        assert!(!ty.matches_value(&bad, &ctx));
    }

    #[test]
    fn either_runtime_type_matches_left_and_right_payloads() {
        let ctx = TestCtx::new();
        let ty = RuntimeType::Either(Box::new(RuntimeType::String), Box::new(RuntimeType::Int));

        assert!(ty.matches_value(&Value::Left(Value::String("ok".into()).into()), &ctx));
        assert!(ty.matches_value(&Value::Right(Value::Integer(7).into()), &ctx));
        assert!(!ty.matches_value(&Value::Left(Value::Integer(7).into()), &ctx));
    }
}
