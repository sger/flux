use flux::frontend::lexer::Lexer;
use flux::frontend::token_type::TokenType;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_char_tokens() {
        let input = "=+-!*/<>,;(){}";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Assign,
            TokenType::Plus,
            TokenType::Minus,
            TokenType::Bang,
            TokenType::Asterisk,
            TokenType::Slash,
            TokenType::Lt,
            TokenType::Gt,
            TokenType::Comma,
            TokenType::Semicolon,
            TokenType::LParen,
            TokenType::RParen,
            TokenType::LBrace,
            TokenType::RBrace,
            TokenType::Eof,
        ];

        for expected_type in expected {
            let tok = lexer.next_token();
            assert_eq!(
                tok.token_type, expected_type,
                "Expected {:?}",
                expected_type
            );
        }
    }

    #[test]
    fn two_char_tokens() {
        let input = "== !=";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Eq);
        assert_eq!(lexer.next_token().token_type, TokenType::NotEq);
    }

    #[test]
    fn keywords() {
        let input = "let fun if else return true false";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Let,
            TokenType::Fun,
            TokenType::If,
            TokenType::Else,
            TokenType::Return,
            TokenType::True,
            TokenType::False,
        ];

        for expected_type in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
        }
    }

    #[test]
    fn identifiers() {
        let input = "foo bar_baz _private camelCase foo123";
        let mut lexer = Lexer::new(input);

        let expected = vec!["foo", "bar_baz", "_private", "camelCase", "foo123"];

        for expected_literal in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, TokenType::Ident);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn strings() {
        let input = r#""" "hello" "hello world""#;
        let mut lexer = Lexer::new(input);

        let expected = vec!["", "hello", "hello world"];

        for expected_literal in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, TokenType::String);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn string_interpolation_tokens_simple() {
        let input = r#""Hello #{name}""#;
        let mut lexer = Lexer::new(input);

        let expected = vec![
            (TokenType::InterpolationStart, "Hello "),
            (TokenType::Ident, "name"),
            (TokenType::RBrace, "}"),
            (TokenType::StringEnd, ""),
            (TokenType::Eof, ""),
        ];

        for (expected_type, expected_literal) in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn string_interpolation_escape_literal() {
        let input = r#""Hello \#{name}""#;
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::String);
        assert_eq!(tok.literal, "Hello #{name}");

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::Eof);
    }

    #[test]
    fn comments() {
        let input = r#"
// This is a comment
let x = 5; // inline comment
"#;
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Let,
            TokenType::Ident,
            TokenType::Assign,
            TokenType::Int,
            TokenType::Semicolon,
        ];

        for expected_type in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
        }
    }

    #[test]
    fn position_tracking() {
        let input = "let x = 5;\nreturn x;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token(); // let
        assert_eq!(tok.position.line, 1);
        assert_eq!(tok.position.column, 1);

        // Skip to second line
        lexer.next_token(); // x
        lexer.next_token(); // =
        lexer.next_token(); // 5
        lexer.next_token(); // ;

        let tok = lexer.next_token(); // return
        assert_eq!(tok.position.line, 2);
        assert_eq!(tok.position.column, 1);
    }

    #[test]
    fn complete_program() {
        let input = r#"
fun fib(n) {
    if n < 2 { return n; };
    fib(n - 1) + fib(n - 2);
}
"#;
        let mut lexer = Lexer::new(input);

        loop {
            let tok = lexer.next_token();
            assert_ne!(tok.token_type, TokenType::Illegal, "Unexpected: {:?}", tok);

            if tok.token_type == TokenType::Eof {
                break;
            }
        }
    }

    #[test]
    fn array_and_hash_tokens() {
        let input = "[1, 2]; {\"a\": 1}";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::LBracket,
            TokenType::Int,
            TokenType::Comma,
            TokenType::Int,
            TokenType::RBracket,
            TokenType::Semicolon,
            TokenType::LBrace,
            TokenType::String,
            TokenType::Colon,
            TokenType::Int,
            TokenType::RBrace,
        ];

        for expected_type in expected {
            assert_eq!(lexer.next_token().token_type, expected_type);
        }
    }

    #[test]
    fn lambda_tokens() {
        let input = r"\x -> x * 2";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Backslash,
            TokenType::Ident,
            TokenType::Arrow,
            TokenType::Ident,
            TokenType::Asterisk,
            TokenType::Int,
        ];

        for expected_type in expected {
            assert_eq!(lexer.next_token().token_type, expected_type);
        }
    }

    #[test]
    fn lambda_with_parens_tokens() {
        let input = r"\(x, y) -> x + y";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Backslash,
            TokenType::LParen,
            TokenType::Ident,
            TokenType::Comma,
            TokenType::Ident,
            TokenType::RParen,
            TokenType::Arrow,
            TokenType::Ident,
            TokenType::Plus,
            TokenType::Ident,
        ];

        for expected_type in expected {
            assert_eq!(lexer.next_token().token_type, expected_type);
        }
    }
}
