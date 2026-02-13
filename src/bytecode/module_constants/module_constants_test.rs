use std::collections::HashMap;

use crate::{
    bytecode::module_constants::{
        ConstEvalError, analyze_module_constants, eval_const_expr, topological_sort_constants,
    },
    diagnostics::position::{Position, Span},
    runtime::value::Value,
    syntax::{block::Block, expression::Expression, interner::Interner, statement::Statement},
};

fn pos(line: usize, column: usize) -> Position {
    Position::new(line, column)
}

fn eval(expr: &Expression) -> Result<Value, ConstEvalError> {
    let interner = Interner::new();
    eval_const_expr(expr, &HashMap::new(), &interner)
}

#[test]
fn analyze_orders_dependencies() {
    let mut interner = Interner::new();
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let body = Block {
        statements: vec![
            Statement::Let {
                name: sym_b,
                value: Expression::Identifier {
                    name: sym_a,
                    span: Span::new(pos(1, 1), pos(1, 2)),
                },
                span: Span::new(pos(1, 0), pos(1, 2)),
            },
            Statement::Let {
                name: sym_a,
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
    assert_eq!(analysis.eval_order, vec![sym_a, sym_b]);
    assert!(analysis.expressions.contains_key(&sym_a));
    assert!(analysis.expressions.contains_key(&sym_b));
}

#[test]
fn analyze_detects_cycle() {
    let mut interner = Interner::new();
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let body = Block {
        statements: vec![
            Statement::Let {
                name: sym_a,
                value: Expression::Identifier {
                    name: sym_b,
                    span: Span::new(pos(1, 1), pos(1, 2)),
                },
                span: Span::new(pos(1, 0), pos(1, 2)),
            },
            Statement::Let {
                name: sym_b,
                value: Expression::Identifier {
                    name: sym_a,
                    span: Span::new(pos(2, 1), pos(2, 2)),
                },
                span: Span::new(pos(2, 0), pos(2, 2)),
            },
        ],
        span: Span::default(),
    };

    let err = analyze_module_constants(&body).unwrap_err();
    assert!(err.contains(&sym_a));
    assert!(err.contains(&sym_b));
}

#[test]
fn topo_sort_simple() {
    let mut interner = Interner::new();
    let sym_a = interner.intern("A");
    let sym_b = interner.intern("B");
    let sym_c = interner.intern("C");

    let mut deps = HashMap::new();
    deps.insert(sym_a, vec![]);
    deps.insert(sym_b, vec![sym_a]);
    deps.insert(sym_c, vec![sym_b]);

    let result = topological_sort_constants(&deps).unwrap();
    assert_eq!(result, vec![sym_a, sym_b, sym_c]);
}

#[test]
fn topo_sort_independent() {
    let mut interner = Interner::new();
    let sym_a = interner.intern("A");
    let sym_b = interner.intern("B");

    let mut deps = HashMap::new();
    deps.insert(sym_a, vec![]);
    deps.insert(sym_b, vec![]);

    let result = topological_sort_constants(&deps).unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains(&sym_a));
    assert!(result.contains(&sym_b));
}

#[test]
fn topo_sort_cycle() {
    let mut interner = Interner::new();
    let sym_a = interner.intern("A");
    let sym_b = interner.intern("B");

    let mut deps = HashMap::new();
    deps.insert(sym_a, vec![sym_b]);
    deps.insert(sym_b, vec![sym_a]);

    let result = topological_sort_constants(&deps);
    assert!(result.is_err());
    let cycle = result.unwrap_err();
    assert!(cycle.contains(&sym_a));
    assert!(cycle.contains(&sym_b));
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
    assert_eq!(result, Value::Boolean(true));
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
fn const_undefined_identifier_uses_resolved_name() {
    let mut interner = Interner::new();
    let missing = interner.intern("missing_const");
    let expr = Expression::Identifier {
        name: missing,
        span: Default::default(),
    };

    let err = eval_const_expr(&expr, &HashMap::new(), &interner).unwrap_err();
    assert_eq!(err.code, "E041");
    assert!(
        err.message
            .contains("'missing_const' is not a module constant.")
    );
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
    assert_eq!(result, Value::Integer(51));
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
        Value::Array(arr) => {
            assert_eq!(arr.len(), 100);
            assert_eq!(arr[0], Value::Integer(0));
            assert_eq!(arr[99], Value::Integer(99));
        }
        _ => panic!("Expected array"),
    }
}

#[test]
fn const_hash_not_supported() {
    let pairs: Vec<(Expression, Expression)> = (0..3)
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

    // Hash literals are not supported in module constants (require runtime GC heap)
    let result = eval(&expr);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message.contains("not supported"));
}

#[test]
fn const_error_with_hint() {
    let mut interner = Interner::new();
    let sym_foo = interner.intern("foo");

    let expr = Expression::Call {
        function: Box::new(Expression::Identifier {
            name: sym_foo,
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
