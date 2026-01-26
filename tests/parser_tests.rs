#[cfg(test)]
mod tests {
    use flux::frontend::{lexer::Lexer, parser::Parser, program::Program, statement::Statement};

    use super::*;

    fn parse(input: &str) -> Program {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "Parser errors: {:?}",
            parser.errors
        );
        program
    }

    #[test]
    fn let_statements() {
        let program = parse("let x = 5; let y = 10;");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(name, "x"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn return_statements() {
        let program = parse("return 5; return;");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[1] {
            Statement::Return { value: None } => {}
            _ => panic!("expected Return with no value"),
        }
    }

    #[test]
    fn operator_precedence() {
        let tests = vec![
            ("a + b * c;", "(a + (b * c))"),
            ("(a + b) * c;", "((a + b) * c)"),
            ("-a * b;", "((-a) * b)"),
            ("!-a;", "(!(-a))"),
            ("a + b + c;", "((a + b) + c)"),
        ];

        for (input, expected) in tests {
            let program = parse(input);
            assert_eq!(program.to_string(), expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn if_expression() {
        let program = parse("if x < y { x; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn if_else_expression() {
        let program = parse("if x < y { x; } else { y; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn function_literal() {
        let program = parse("fun(x, y) { x + y; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_function_statement() {
        let program = parse("fun add(x, y) { x + y; }");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Function {
                name, parameters, ..
            } => {
                assert_eq!(name, "add");
                assert_eq!(parameters, &vec!["x".to_string(), "y".to_string()]);
            }
            _ => panic!("expected Function statement"),
        }
    }

    #[test]
    fn test_call_expression() {
        let program = parse("add(1, 2 * 3);");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_string_literal() {
        let program = parse(r#""hello world";"#);
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_boolean_literals() {
        let program = parse("true; false;");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_array_literal() {
        let program = parse("[1, 2 * 2, 3 + 3];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "[1, (2 * 2), (3 + 3)]");
    }

    #[test]
    fn test_index_expression() {
        let program = parse("arr[1 + 1];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "(arr[(1 + 1)])");
    }

    #[test]
    fn test_hash_literal() {
        let program = parse(r#"{"one": 1, "two": 2};"#);
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_empty_hash() {
        let program = parse("{};");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "{}");
    }
}
