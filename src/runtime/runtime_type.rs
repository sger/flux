use std::{collections::HashSet, fmt};

use crate::{
    runtime::{RuntimeContext, value::Value},
    syntax::Identifier,
};

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeType {
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
    Function {
        params: Vec<RuntimeType>,
        ret: Box<RuntimeType>,
        effects: Vec<Identifier>,
    },
}

impl RuntimeType {
    pub fn type_name(&self) -> String {
        match self {
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
            RuntimeType::Function {
                params,
                ret,
                effects,
            } => {
                let params_str = params
                    .iter()
                    .map(RuntimeType::type_name)
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut out = format!("({params_str}) -> {}", ret.type_name());
                if !effects.is_empty() {
                    let effects_str = effects
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(" with {effects_str}"));
                }
                out
            }
        }
    }

    pub fn matches_value(&self, value: &Value, ctx: &dyn RuntimeContext) -> bool {
        match self {
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
                        Value::Cons(cell) => {
                            if !inner.matches_value(&cell.head, ctx) {
                                return false;
                            }
                            current = &cell.tail;
                        }
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
            RuntimeType::Map(_, _) => matches!(value, Value::HashMap(_)),
            RuntimeType::Tuple(expected) => match value {
                Value::Tuple(elements) if elements.len() == expected.len() => expected
                    .iter()
                    .zip(elements.iter())
                    .all(|(ty, value)| ty.matches_value(value, ctx)),
                _ => false,
            },
            RuntimeType::Function {
                params,
                ret,
                effects,
            } => {
                // If the value is callable but has no contract (e.g. an
                // untyped lambda), accept it.  HM inference already proved
                // type correctness at compile time; rejecting it here would
                // reject valid programs like `twice(\n -> n + 1, 5)`.
                if !value.is_callable() {
                    return false;
                }
                let Some(contract) = ctx.callable_contract(value) else {
                    return true;
                };
                if contract.params.len() != params.len() {
                    return false;
                }
                for (expected, actual) in params.iter().zip(contract.params.iter()) {
                    if let Some(actual) = actual
                        && !runtime_type_compatible(expected, actual)
                    {
                        return false;
                    }
                }
                if let Some(actual_ret) = contract.ret.as_ref()
                    && !runtime_type_compatible(ret, actual_ret)
                {
                    return false;
                }
                effects_subset(&contract.effects, effects)
            }
        }
    }
}

fn runtime_type_compatible(expected: &RuntimeType, actual: &RuntimeType) -> bool {
    match (expected, actual) {
        (RuntimeType::Int, RuntimeType::Int)
        | (RuntimeType::Float, RuntimeType::Float)
        | (RuntimeType::Bool, RuntimeType::Bool)
        | (RuntimeType::String, RuntimeType::String)
        | (RuntimeType::Unit, RuntimeType::Unit) => true,
        (RuntimeType::Option(e), RuntimeType::Option(a))
        | (RuntimeType::List(e), RuntimeType::List(a))
        | (RuntimeType::Array(e), RuntimeType::Array(a)) => runtime_type_compatible(e, a),
        (RuntimeType::Either(el, er), RuntimeType::Either(al, ar))
        | (RuntimeType::Map(el, er), RuntimeType::Map(al, ar)) => {
            runtime_type_compatible(el, al) && runtime_type_compatible(er, ar)
        }
        (RuntimeType::Tuple(elems_e), RuntimeType::Tuple(elems_a)) => {
            elems_e.len() == elems_a.len()
                && elems_e
                    .iter()
                    .zip(elems_a.iter())
                    .all(|(e, a)| runtime_type_compatible(e, a))
        }
        (
            RuntimeType::Function {
                params: e_params,
                ret: e_ret,
                effects: e_effects,
            },
            RuntimeType::Function {
                params: a_params,
                ret: a_ret,
                effects: a_effects,
            },
        ) => {
            e_params.len() == a_params.len()
                && e_params
                    .iter()
                    .zip(a_params.iter())
                    .all(|(e, a)| runtime_type_compatible(e, a))
                && runtime_type_compatible(e_ret, a_ret)
                && effects_subset(a_effects, e_effects)
        }
        _ => false,
    }
}

fn effects_subset(actual: &[Identifier], expected: &[Identifier]) -> bool {
    let expected_set: HashSet<Identifier> = expected.iter().copied().collect();
    actual.iter().all(|effect| expected_set.contains(effect))
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
        RuntimeContext, closure::Closure, compiled_function::CompiledFunction, cons_cell::ConsCell,
        function_contract::FunctionContract, value::Value,
    };
    use std::rc::Rc;

    struct TestCtx;

    impl TestCtx {
        fn new() -> Self {
            Self
        }
    }

    impl RuntimeContext for TestCtx {
        fn invoke_value(&mut self, _callee: Value, _args: Vec<Value>) -> Result<Value, String> {
            Err("not used in runtime_type tests".to_string())
        }

        fn invoke_base_function_borrowed(
            &mut self,
            _base_fn_index: usize,
            _args: &[&Value],
        ) -> Result<Value, String> {
            Err("not used in runtime_type tests".to_string())
        }

        fn callable_contract<'a>(&'a self, callee: &'a Value) -> Option<&'a FunctionContract> {
            match callee {
                Value::Closure(closure) => closure.function.contract.as_ref(),
                _ => None,
            }
        }
    }

    #[test]
    fn list_runtime_type_matches_cons_lists_recursively() {
        let ctx = TestCtx::new();
        let good = ConsCell::cons(
            Value::Integer(1),
            ConsCell::cons(Value::Integer(2), Value::EmptyList),
        );

        let ty = RuntimeType::List(Box::new(RuntimeType::Int));
        assert!(ty.matches_value(&good, &ctx));
    }

    #[test]
    fn list_runtime_type_rejects_mixed_element_types() {
        let ctx = TestCtx::new();
        let bad = ConsCell::cons(
            Value::Integer(1),
            ConsCell::cons(Value::String("x".to_string().into()), Value::EmptyList),
        );

        let ty = RuntimeType::List(Box::new(RuntimeType::Int));
        assert!(!ty.matches_value(&bad, &ctx));
    }

    #[test]
    fn either_runtime_type_matches_left_and_right_payloads() {
        let ctx = TestCtx::new();
        let ty = RuntimeType::Either(Box::new(RuntimeType::String), Box::new(RuntimeType::Int));

        assert!(ty.matches_value(
            &Value::Left(Value::String("ok".to_string().into()).into()),
            &ctx
        ));
        assert!(ty.matches_value(&Value::Right(Value::Integer(7).into()), &ctx));
        assert!(!ty.matches_value(&Value::Left(Value::Integer(7).into()), &ctx));
    }

    #[test]
    fn runtime_type_compatibility_requires_matching_shapes() {
        let ctx = TestCtx::new();
        assert!(!RuntimeType::Int.matches_value(&Value::String("x".to_string().into()), &ctx));
        assert!(!super::runtime_type_compatible(
            &RuntimeType::Int,
            &RuntimeType::String
        ));
        assert!(super::runtime_type_compatible(
            &RuntimeType::Int,
            &RuntimeType::Int
        ));
    }

    #[test]
    fn function_runtime_type_accepts_closure_with_subset_effects() {
        let ctx = TestCtx::new();
        let contract = FunctionContract {
            params: vec![Some(RuntimeType::Int)],
            ret: Some(RuntimeType::Bool),
            effects: vec![],
        };
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(Some(contract));
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        };
        assert!(expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn function_runtime_type_accepts_closure_missing_contract() {
        // Closures without contracts are accepted — HM inference already
        // proved type correctness at compile time.
        let ctx = TestCtx::new();
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(None);
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        };
        assert!(expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn function_runtime_type_rejects_effect_superset() {
        let mut interner = crate::syntax::interner::Interner::new();
        let io = interner.intern("IO");
        let time = interner.intern("Time");
        let ctx = TestCtx::new();
        let contract = FunctionContract {
            params: vec![Some(RuntimeType::Int)],
            ret: Some(RuntimeType::Bool),
            effects: vec![io, time],
        };
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(Some(contract));
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![io],
        };
        assert!(!expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn function_runtime_type_rejects_param_mismatch() {
        let ctx = TestCtx::new();
        let contract = FunctionContract {
            params: vec![Some(RuntimeType::String)],
            ret: Some(RuntimeType::Bool),
            effects: vec![],
        };
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(Some(contract));
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        };
        assert!(!expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn function_runtime_type_rejects_return_mismatch() {
        let ctx = TestCtx::new();
        let contract = FunctionContract {
            params: vec![Some(RuntimeType::Int)],
            ret: Some(RuntimeType::Int),
            effects: vec![],
        };
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(Some(contract));
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        };
        assert!(!expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn function_runtime_type_accepts_effect_subset() {
        let mut interner = crate::syntax::interner::Interner::new();
        let io = interner.intern("IO");
        let time = interner.intern("Time");
        let ctx = TestCtx::new();
        let contract = FunctionContract {
            params: vec![Some(RuntimeType::Int)],
            ret: Some(RuntimeType::Bool),
            effects: vec![io],
        };
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(Some(contract));
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![io, time],
        };
        assert!(expected.matches_value(&closure, &ctx));
    }
}
