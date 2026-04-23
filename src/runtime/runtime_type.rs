use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    runtime::{RuntimeContext, value::Value},
    syntax::{Identifier, symbol::Symbol},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdtConstructorContract {
    pub name: Identifier,
    pub display_name: String,
    #[serde(default)]
    pub fields: Vec<RuntimeType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    Adt {
        module_name: Option<Identifier>,
        module_name_text: Option<String>,
        name: Identifier,
        display_name: String,
        #[serde(default)]
        type_args: Vec<RuntimeType>,
        constructors: Vec<AdtConstructorContract>,
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
            RuntimeType::Adt {
                module_name_text,
                display_name,
                type_args,
                ..
            } => {
                let base = match module_name_text {
                    Some(module) => format!("{module}.{display_name}"),
                    None => display_name.clone(),
                };
                if type_args.is_empty() {
                    base
                } else {
                    let args = type_args
                        .iter()
                        .map(RuntimeType::type_name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{base}<{}>", args)
                }
            }
        }
    }

    pub fn collect_symbols(&self, out: &mut HashSet<Symbol>) {
        match self {
            RuntimeType::Option(inner) | RuntimeType::List(inner) | RuntimeType::Array(inner) => {
                inner.collect_symbols(out)
            }
            RuntimeType::Either(left, right) | RuntimeType::Map(left, right) => {
                left.collect_symbols(out);
                right.collect_symbols(out);
            }
            RuntimeType::Tuple(elements) => {
                for element in elements {
                    element.collect_symbols(out);
                }
            }
            RuntimeType::Function {
                params,
                ret,
                effects,
            } => {
                for param in params {
                    param.collect_symbols(out);
                }
                ret.collect_symbols(out);
                out.extend(effects.iter().copied());
            }
            RuntimeType::Adt {
                module_name,
                name,
                type_args,
                constructors,
                ..
            } => {
                if let Some(module_name) = module_name {
                    out.insert(*module_name);
                }
                out.insert(*name);
                for arg in type_args {
                    arg.collect_symbols(out);
                }
                for ctor in constructors {
                    out.insert(ctor.name);
                    for field in &ctor.fields {
                        field.collect_symbols(out);
                    }
                }
            }
            RuntimeType::Int
            | RuntimeType::Float
            | RuntimeType::Bool
            | RuntimeType::String
            | RuntimeType::Unit => {}
        }
    }

    pub fn remap_symbols(&self, remap: &std::collections::HashMap<Symbol, Symbol>) -> Self {
        match self {
            RuntimeType::Option(inner) => RuntimeType::Option(Box::new(inner.remap_symbols(remap))),
            RuntimeType::List(inner) => RuntimeType::List(Box::new(inner.remap_symbols(remap))),
            RuntimeType::Either(left, right) => RuntimeType::Either(
                Box::new(left.remap_symbols(remap)),
                Box::new(right.remap_symbols(remap)),
            ),
            RuntimeType::Array(inner) => RuntimeType::Array(Box::new(inner.remap_symbols(remap))),
            RuntimeType::Map(key, value) => RuntimeType::Map(
                Box::new(key.remap_symbols(remap)),
                Box::new(value.remap_symbols(remap)),
            ),
            RuntimeType::Tuple(elements) => RuntimeType::Tuple(
                elements
                    .iter()
                    .map(|element| element.remap_symbols(remap))
                    .collect(),
            ),
            RuntimeType::Function {
                params,
                ret,
                effects,
            } => RuntimeType::Function {
                params: params
                    .iter()
                    .map(|param| param.remap_symbols(remap))
                    .collect(),
                ret: Box::new(ret.remap_symbols(remap)),
                effects: effects
                    .iter()
                    .map(|effect| remap.get(effect).copied().unwrap_or(*effect))
                    .collect(),
            },
            RuntimeType::Adt {
                module_name,
                module_name_text,
                name,
                display_name,
                type_args,
                constructors,
            } => RuntimeType::Adt {
                module_name: module_name
                    .map(|module| remap.get(&module).copied().unwrap_or(module)),
                module_name_text: module_name_text.clone(),
                name: remap.get(name).copied().unwrap_or(*name),
                display_name: display_name.clone(),
                type_args: type_args
                    .iter()
                    .map(|arg| arg.remap_symbols(remap))
                    .collect(),
                constructors: constructors
                    .iter()
                    .map(|ctor| AdtConstructorContract {
                        name: remap.get(&ctor.name).copied().unwrap_or(ctor.name),
                        display_name: ctor.display_name.clone(),
                        fields: ctor
                            .fields
                            .iter()
                            .map(|field| field.remap_symbols(remap))
                            .collect(),
                    })
                    .collect(),
            },
            primitive => primitive.clone(),
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
                if !value.is_callable() {
                    return false;
                }
                let Some(contract) = ctx.callable_contract(value) else {
                    return false;
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
            RuntimeType::Adt { constructors, .. } => {
                match value {
                    Value::Adt(adt) => constructors.iter().any(|ctor| {
                        ctor.display_name.as_str() == adt.constructor.as_ref()
                            && ctor.fields.len() == adt.fields.len()
                            && ctor.fields.iter().zip(adt.fields.iter()).all(
                                |(field_ty, field_value)| field_ty.matches_value(field_value, ctx),
                            )
                    }),
                    Value::AdtUnit(constructor) => constructors.iter().any(|ctor| {
                        ctor.fields.is_empty() && ctor.display_name.as_str() == constructor.as_ref()
                    }),
                    _ => false,
                }
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
        (
            RuntimeType::Adt {
                module_name: e_module,
                name: e_name,
                type_args: e_args,
                constructors: e_ctors,
                ..
            },
            RuntimeType::Adt {
                module_name: a_module,
                name: a_name,
                type_args: a_args,
                constructors: a_ctors,
                ..
            },
        ) => {
            e_module == a_module
                && e_name == a_name
                && e_args.len() == a_args.len()
                && e_ctors.len() == a_ctors.len()
                && e_args
                    .iter()
                    .zip(a_args.iter())
                    .all(|(e, a)| runtime_type_compatible(e, a))
                && e_ctors.iter().zip(a_ctors.iter()).all(|(e, a)| {
                    e.name == a.name
                        && e.fields.len() == a.fields.len()
                        && e.fields
                            .iter()
                            .zip(a.fields.iter())
                            .all(|(e_field, a_field)| runtime_type_compatible(e_field, a_field))
                })
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
    use super::{AdtConstructorContract, RuntimeType};
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
    fn function_runtime_type_rejects_closure_missing_contract() {
        let ctx = TestCtx::new();
        let compiled = CompiledFunction::new(vec![], 0, 1, None).with_contract(None);
        let closure = Value::Closure(Rc::new(Closure::new(Rc::new(compiled), vec![])));
        let expected = RuntimeType::Function {
            params: vec![RuntimeType::Int],
            ret: Box::new(RuntimeType::Bool),
            effects: vec![],
        };
        assert!(!expected.matches_value(&closure, &ctx));
    }

    #[test]
    fn adt_runtime_type_checks_constructor_and_fields() {
        let mut interner = crate::syntax::interner::Interner::new();
        let maybe = interner.intern("MaybeInt");
        let just = interner.intern("Just");
        let none = interner.intern("None");
        let expected = RuntimeType::Adt {
            module_name: None,
            module_name_text: None,
            name: maybe,
            display_name: "MaybeInt".to_string(),
            type_args: vec![],
            constructors: vec![
                AdtConstructorContract {
                    name: just,
                    display_name: "Just".to_string(),
                    fields: vec![RuntimeType::Int],
                },
                AdtConstructorContract {
                    name: none,
                    display_name: "None".to_string(),
                    fields: vec![],
                },
            ],
        };
        let ctx = TestCtx::new();

        let good = Value::Adt(Rc::new(crate::runtime::value::AdtValue {
            constructor: Rc::new("Just".to_string()),
            fields: crate::runtime::value::AdtFields::One(Value::Integer(7)),
        }));
        let bad = Value::Adt(Rc::new(crate::runtime::value::AdtValue {
            constructor: Rc::new("Just".to_string()),
            fields: crate::runtime::value::AdtFields::One(Value::String(Rc::new("x".to_string()))),
        }));

        assert!(expected.matches_value(&good, &ctx));
        assert!(!expected.matches_value(&bad, &ctx));
    }

    #[test]
    fn function_runtime_type_rejects_effect_superset() {
        let mut interner = crate::syntax::interner::Interner::new();
        let io = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let time = crate::syntax::builtin_effects::time_effect_symbol(&mut interner);
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
        let io = crate::syntax::builtin_effects::io_effect_symbol(&mut interner);
        let time = crate::syntax::builtin_effects::time_effect_symbol(&mut interner);
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
