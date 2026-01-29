#[cfg(test)]
mod tests {
    use flux::frontend::{
        expression::Expression, position::Span, program::Program, statement::Statement,
    };

    #[test]
    fn display_let() {
        let program = Program {
            statements: vec![Statement::Let {
                name: "x".to_string(),
                value: Expression::Integer {
                    value: 5,
                    span: Span::default(),
                },
                span: Span::default(),
            }],
        };

        assert_eq!(program.to_string(), "let x = 5;");
    }

    #[test]
    fn display_infix() {
        let expr = Expression::Infix {
            left: Box::new(Expression::Integer {
                value: 1,
                span: Span::default(),
            }),
            operator: "+".to_string(),
            right: Box::new(Expression::Integer {
                value: 2,
                span: Span::default(),
            }),
            span: Span::default(),
        };
        assert_eq!(expr.to_string(), "(1 + 2)");
    }

    #[test]
    fn display_array() {
        let expr = Expression::Array {
            elements: vec![
                Expression::Integer {
                    value: 1,
                    span: Span::default(),
                },
                Expression::Integer {
                    value: 2,
                    span: Span::default(),
                },
                Expression::Integer {
                    value: 3,
                    span: Span::default(),
                },
            ],
            span: Span::default(),
        };
        assert_eq!(expr.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn display_hash() {
        let expr = Expression::Hash {
            pairs: vec![(
                Expression::String {
                    value: "a".to_string(),
                    span: Span::default(),
                },
                Expression::Integer {
                    value: 1,
                    span: Span::default(),
                },
            )],
            span: Span::default(),
        };
        assert_eq!(expr.to_string(), "{\"a\": 1}");
    }
}
