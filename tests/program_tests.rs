#[cfg(test)]
mod tests {
    use flux::diagnostics::position::Span;
    use flux::syntax::{
        expression::Expression, interner::Interner, program::Program, statement::Statement,
    };

    #[test]
    fn display_let() {
        let mut interner = Interner::new();
        let x = interner.intern("x");
        let program = Program {
            statements: vec![Statement::Let {
                name: x,
                value: Expression::Integer {
                    value: 5,
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            span: Span::default(),
        };

        assert_eq!(program.display_with(&interner), "let x = 5;");
    }

    #[test]
    fn display_infix() {
        let interner = Interner::new();
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
        assert_eq!(expr.display_with(&interner), "(1 + 2)");
    }

    #[test]
    fn display_array() {
        let interner = Interner::new();
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
        assert_eq!(expr.display_with(&interner), "[1, 2, 3]");
    }

    #[test]
    fn display_hash() {
        let interner = Interner::new();
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
        assert_eq!(expr.display_with(&interner), "{\"a\": 1}");
    }
}
