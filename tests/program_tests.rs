#[cfg(test)]
mod tests {
    use flux::frontend::{expression::Expression, program::Program, statement::Statement};

    use super::*;

    #[test]
    fn display_let() {
        let program = Program {
            statements: vec![Statement::Let {
                name: "x".to_string(),
                value: Expression::Integer(5),
            }],
        };

        assert_eq!(program.to_string(), "let x = 5;");
    }

    #[test]
    fn display_infix() {
        let expr = Expression::Infix {
            left: Box::new(Expression::Integer(1)),
            operator: "+".to_string(),
            right: Box::new(Expression::Integer(2)),
        };
        assert_eq!(expr.to_string(), "(1 + 2)");
    }

    #[test]
    fn display_array() {
        let expr = Expression::Array {
            elements: vec![
                Expression::Integer(1),
                Expression::Integer(2),
                Expression::Integer(3),
            ],
        };
        assert_eq!(expr.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn display_hash() {
        let expr = Expression::Hash {
            pairs: vec![(Expression::String("a".to_string()), Expression::Integer(1))],
        };
        assert_eq!(expr.to_string(), "{\"a\": 1}");
    }
}
