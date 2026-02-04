use std::collections::HashMap;

use crate::{
    bytecode::module_constants::{
        analyze_module_constants, eval_const_expr, topological_sort_constants, ConstEvalError,
    },
    frontend::{
        block::Block,
        expression::Expression,
        position::{Position, Span},
        statement::Statement,
    },
    runtime::object::Object,
};

fn pos(line: usize, column: usize) -> Position {
    Position::new(line, column)
}

fn eval(expr: &Expression) -> Result<Object, ConstEvalError> {
    eval_const_expr(expr, &HashMap::new())
}

#[test]
fn analyze_orders_dependencies() {
    let body = Block {
        statements: vec![
            Statement::Let {
                name: "b".to_string(),
                value: Expression::Identifier {
                    name: "a".to_string(),
                    span: Span::new(pos(1, 1), pos(1, 2)),
                },
                span: Span::new(pos(1, 0), pos(1, 2)),
            },
            Statement::Let {
                name: "a".to_string(),
                value: Expression::Integer {
                    value: 1,
                    span: Span::new(pos(2, 1), pos(2, 2)),
                },
                span: Span::new(pos(2, 0), pos(2, 2)),
            },
        ],
        span: Span::default(),
    };

    let analysis = analyze_module_constants(&body).unwrap();
    assert_eq!(analysis.eval_order, vec!["a".to_string(), "b".to_string()]);
    assert!(analysis.expressions.contains_key("a"));
    assert!(analysis.expressions.contains_key("b"));
}

#[test]
fn analyze_detects_cycle() {
    let body = Block {
        statements: vec![
            Statement::Let {
                name: "a".to_string(),
                value: Expression::Identifier {
                    name: "b".to_string(),
                    span: Span::new(pos(1, 1), pos(1, 2)),
                },
                span: Span::new(pos(1, 0), pos(1, 2)),
            },
            Statement::Let {
                name: "b".to_string(),
                value: Expression::Identifier {
                    name: "a".to_string(),
                    span: Span::new(pos(2, 1), pos(2, 2)),
                },
                span: Span::new(pos(2, 0), pos(2, 2)),
            },
        ],
        span: Span::default(),
    };

    let err = analyze_module_constants(&body).unwrap_err();
    assert!(err.contains(&"a".to_string()));
    assert!(err.contains(&"b".to_string()));
}

#[test]
fn topo_sort_simple() {
    let mut deps = HashMap::new();
    deps.insert("A".to_string(), vec![]);
    deps.insert("B".to_string(), vec!["A".to_string()]);
    deps.insert("C".to_string(), vec!["B".to_string()]);

    let result = topological_sort_constants(&deps).unwrap();
    assert_eq!(result, vec!["A", "B", "C"]);
}

#[test]
fn topo_sort_independent() {
    let mut deps = HashMap::new();
    deps.insert("A".to_string(), vec![]);
    deps.insert("B".to_string(), vec![]);

    let result = topological_sort_constants(&deps).unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"A".to_string()));
    assert!(result.contains(&"B".to_string()));
}

#[test]
fn topo_sort_cycle() {
    let mut deps = HashMap::new();
    deps.insert("A".to_string(), vec!["B".to_string()]);
    deps.insert("B".to_string(), vec!["A".to_string()]);

    let result = topological_sort_constants(&deps);
    assert!(result.is_err());
    let cycle = result.unwrap_err();
    assert!(cycle.contains(&"A".to_string()));
    assert!(cycle.contains(&"B".to_string()));
}

#[test]
fn const_divide_by_zero() {
    let expr = Expression::Infix {
        left: Box::new(Expression::Integer {
            value: 1,
            span: Default::default(),
        }),
        operator: "/".to_string(),
        right: Box::new(Expression::Integer {
            value: 0,
            span: Default::default(),
        }),
        span: Default::default(),
    };
    let err = eval(&expr).unwrap_err();
    assert_eq!(err.code, "E059");
    assert!(err.message.contains("Division by zero"));
}

#[test]
fn const_mod_by_zero() {
    let expr = Expression::Infix {
        left: Box::new(Expression::Integer {
            value: 1,
            span: Default::default(),
        }),
        operator: "%".to_string(),
        right: Box::new(Expression::Integer {
            value: 0,
            span: Default::default(),
        }),
        span: Default::default(),
    };
    let err = eval(&expr).unwrap_err();
    assert_eq!(err.code, "E059");
    assert!(err.message.contains("Modulo by zero"));
}

#[test]
fn const_string_ordering() {
    let expr = Expression::Infix {
        left: Box::new(Expression::String {
            value: "b".to_string(),
            span: Default::default(),
        }),
        operator: ">".to_string(),
        right: Box::new(Expression::String {
            value: "a".to_string(),
            span: Default::default(),
        }),
        span: Default::default(),
    };
    let result = eval(&expr).unwrap();
    assert_eq!(result, Object::Boolean(true));
}

#[test]
fn const_float_divide_by_zero() {
    let expr = Expression::Infix {
        left: Box::new(Expression::Float {
            value: 1.0,
            span: Default::default(),
        }),
        operator: "/".to_string(),
        right: Box::new(Expression::Float {
            value: 0.0,
            span: Default::default(),
        }),
        span: Default::default(),
    };
    let err = eval(&expr).unwrap_err();
    assert_eq!(err.code, "E059");
    assert!(err.message.contains("Division by zero"));
}

#[test]
fn const_deep_recursion() {
    let mut expr = Expression::Integer {
        value: 1,
        span: Default::default(),
    };

    for _ in 0..50 {
        expr = Expression::Infix {
            left: Box::new(expr),
            operator: "+".to_string(),
            right: Box::new(Expression::Integer {
                value: 1,
                span: Default::default(),
            }),
            span: Default::default(),
        };
    }

    let result = eval(&expr).unwrap();
    assert_eq!(result, Object::Integer(51));
}

#[test]
fn const_large_array() {
    let elements: Vec<Expression> = (0..100)
        .map(|i| Expression::Integer {
            value: i,
            span: Default::default(),
        })
        .collect();

    let expr = Expression::Array {
        elements,
        span: Default::default(),
    };

    let result = eval(&expr).unwrap();
    match result {
        Object::Array(arr) => {
            assert_eq!(arr.len(), 100);
            assert_eq!(arr[0], Object::Integer(0));
            assert_eq!(arr[99], Object::Integer(99));
        }
        _ => panic!("Expected array"),
    }
}

#[test]
fn const_large_hash() {
    let pairs: Vec<(Expression, Expression)> = (0..50)
        .map(|i| {
            (
                Expression::Integer {
                    value: i,
                    span: Default::default(),
                },
                Expression::Integer {
                    value: i * 2,
                    span: Default::default(),
                },
            )
        })
        .collect();

    let expr = Expression::Hash {
        pairs,
        span: Default::default(),
    };

    let result = eval(&expr).unwrap();
    match result {
        Object::Hash(map) => {
            assert_eq!(map.len(), 50);
            assert_eq!(
                map.get(&crate::runtime::hash_key::HashKey::Integer(0)),
                Some(&Object::Integer(0))
            );
            assert_eq!(
                map.get(&crate::runtime::hash_key::HashKey::Integer(49)),
                Some(&Object::Integer(98))
            );
        }
        _ => panic!("Expected hash"),
    }
}

#[test]
fn const_error_with_hint() {
    let expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: "foo".to_string(),
            span: Default::default(),
        }),
        arguments: vec![],
        span: Default::default(),
    };

    let err = eval(&expr).unwrap_err();
    assert_eq!(err.code, "E042");
    assert!(err.hint.is_some());
    assert!(
        err.hint
            .unwrap()
            .contains("Module constants must be evaluable at compile time")
    );
}
