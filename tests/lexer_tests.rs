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
    fn string_span_uses_source_width_with_escapes() {
        let input = r#""a\"b""#;
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::String);
        assert_eq!(tok.literal, "a\"b");
        assert_eq!(tok.position.column, 0);
        assert_eq!(tok.span().end.column, input.chars().count());
    }

    #[test]
    fn unterminated_string_uses_lexer_end_position() {
        let input = "\"abc";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.position.line, 1);
        assert_eq!(tok.position.column, 0);
        assert_eq!(tok.span().end.line, 1);
        assert_eq!(tok.span().end.column, input.chars().count());
    }

    #[test]
    fn unterminated_string_with_comment_like_text_keeps_full_length() {
        let input = "\"http://example.com";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.literal, "http://example.com");
        assert_eq!(tok.span().end.column, input.chars().count());
    }

    #[test]
    fn unterminated_string_with_comment_marker_on_same_line_uses_content_end() {
        let input = "\"a // comment";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.literal, "a // comment");
        // Expected closing quote position: opening quote + content length.
        assert_eq!(tok.span().end.column, 1 + "a // comment".chars().count());
    }

    #[test]
    fn unterminated_string_with_escape_uses_raw_cursor_end() {
        let input = "\"a\\n";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.literal, "a\n");
        assert_eq!(tok.position.column, 0);
        assert_eq!(tok.span().end.column, input.chars().count());
    }

    #[test]
    fn unterminated_string_eof_after_backslash_keeps_backslash_and_end() {
        let input = "\"a\\";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.literal, "a\\");
        assert_eq!(tok.position.column, 0);
        assert_eq!(tok.span().end.column, input.chars().count());
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
    fn interpolation_basic_with_trailing_literal() {
        let input = "\"a #{ 1 } b\"";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            (TokenType::InterpolationStart, "a "),
            (TokenType::Int, "1"),
            (TokenType::RBrace, "}"),
            (TokenType::StringEnd, " b"),
            (TokenType::Eof, ""),
        ];

        for (expected_type, expected_literal) in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn interpolation_nested_braces_in_expression() {
        let input = "\"a #{ {1} } b\"";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            (TokenType::InterpolationStart, "a "),
            (TokenType::LBrace, "{"),
            (TokenType::Int, "1"),
            (TokenType::RBrace, "}"),
            (TokenType::RBrace, "}"),
            (TokenType::StringEnd, " b"),
            (TokenType::Eof, ""),
        ];

        for (expected_type, expected_literal) in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn interpolation_brace_inside_inner_string_does_not_close_outer_expression() {
        let input = "\"a #{ \"}\" } b\"";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            (TokenType::InterpolationStart, "a "),
            (TokenType::String, "}"),
            (TokenType::RBrace, "}"),
            (TokenType::StringEnd, " b"),
            (TokenType::Eof, ""),
        ];

        for (expected_type, expected_literal) in expected {
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, expected_type);
            assert_eq!(tok.literal, expected_literal);
        }
    }

    #[test]
    fn unterminated_continuation_segment_uses_segment_end_position() {
        let input = "\"a #{1} bc";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::InterpolationStart);
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::RBrace);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedString);
        assert_eq!(tok.literal, " bc");
        // Continuation segment has no opening quote at token start.
        assert_eq!(tok.position.column, 7);
        assert_eq!(tok.span().end.column, 10);
    }

    #[test]
    fn interpolation_token_spans_track_source_positions() {
        let input = r#""Hello #{name}""#;
        let mut lexer = Lexer::new(input);

        let start = lexer.next_token();
        assert_eq!(start.token_type, TokenType::InterpolationStart);
        assert_eq!(start.position.column, 0);
        assert_eq!(start.span().end.column, "\"Hello #{".chars().count());

        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::RBrace);

        let end = lexer.next_token();
        assert_eq!(end.token_type, TokenType::StringEnd);
        assert_eq!(end.span().end.column, input.chars().count());
    }

    #[test]
    fn interpolation_state_helper_reflects_expression_phase() {
        let input = "\"#{x}\"";
        let mut lexer = Lexer::new(input);

        assert!(!lexer.is_in_interpolation());
        assert_eq!(lexer.next_token().token_type, TokenType::InterpolationStart);
        assert!(lexer.is_in_interpolation());

        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert!(lexer.is_in_interpolation());

        assert_eq!(lexer.next_token().token_type, TokenType::RBrace);
        assert!(!lexer.is_in_interpolation());

        assert_eq!(lexer.next_token().token_type, TokenType::StringEnd);
        assert!(!lexer.is_in_interpolation());
    }

    #[test]
    fn nested_interpolated_strings_keep_outer_state() {
        let input = "\"#{ \"#{x}\" }\"";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::InterpolationStart,
            TokenType::InterpolationStart,
            TokenType::Ident,
            TokenType::RBrace,
            TokenType::StringEnd,
            TokenType::RBrace,
            TokenType::StringEnd,
            TokenType::Eof,
        ];

        for expected_type in expected {
            assert_eq!(lexer.next_token().token_type, expected_type);
        }
    }

    #[test]
    fn eof_inside_interpolation_clears_interpolation_state() {
        let input = "\"#{x";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::InterpolationStart);
        assert!(lexer.is_in_interpolation());
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert!(lexer.is_in_interpolation());

        assert_eq!(lexer.next_token().token_type, TokenType::Eof);
        assert!(!lexer.is_in_interpolation());
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
        assert_eq!(tok.position.column, 0); // Columns are 0-indexed

        // Skip to second line
        lexer.next_token(); // x
        lexer.next_token(); // =
        lexer.next_token(); // 5
        lexer.next_token(); // ;

        let tok = lexer.next_token(); // return
        assert_eq!(tok.position.line, 2);
        assert_eq!(tok.position.column, 0); // Columns are 0-indexed
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
