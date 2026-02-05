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

    #[test]
    fn unknown_escape_sequences_pass_through() {
        // Document lexer behavior: unknown escapes are accepted and pass through
        // The linter will warn about these (W011), but the lexer is permissive
        let tests = vec![
            (r#""\x""#, "x"),
            (r#""\q""#, "q"),
            (r#""\s""#, "s"),
            (r#""\a\b\c""#, "abc"),
        ];

        for (input, expected) in tests {
            let mut lexer = Lexer::new(input);
            let tok = lexer.next_token();
            assert_eq!(
                tok.token_type,
                TokenType::String,
                "Should tokenize as String for input: {}",
                input
            );
            assert_eq!(
                tok.literal, expected,
                "Unknown escapes should pass through for input: {}",
                input
            );
        }
    }

    #[test]
    fn valid_escape_sequences_work_correctly() {
        // Verify that all valid escape sequences are handled correctly
        let tests = vec![
            (r#""\n""#, "\n"),
            (r#""\t""#, "\t"),
            (r#""\r""#, "\r"),
            (r#""\\""#, "\\"),
            (r#""\"""#, "\""),
            (r#""\#""#, "#"),
            (r#""\n\t\r""#, "\n\t\r"),
        ];

        for (input, expected) in tests {
            let mut lexer = Lexer::new(input);
            let tok = lexer.next_token();
            assert_eq!(tok.token_type, TokenType::String);
            assert_eq!(
                tok.literal, expected,
                "Escape sequence should be processed correctly for input: {}",
                input
            );
        }
    }

    #[test]
    fn lexer_warns_on_unknown_escape_sequences() {
        let mut lexer = Lexer::new(r#""\x\q\s""#);
        let tok = lexer.next_token();

        assert_eq!(tok.token_type, TokenType::String);
        assert_eq!(tok.literal, "xqs"); // Escapes pass through

        let warnings = lexer.warnings();
        assert_eq!(
            warnings.len(),
            3,
            "Should emit warning for each unknown escape"
        );
        assert!(warnings[0].message.contains("Unknown escape sequence"));
        assert!(warnings[0].message.contains("\\x"));
    }

    #[test]
    fn lexer_no_warnings_for_valid_escapes() {
        let mut lexer = Lexer::new(r#""\n\t\r\\\"\#""#);
        lexer.next_token();

        let warnings = lexer.warnings();
        assert_eq!(warnings.len(), 0, "Valid escapes should not produce warnings");
    }

    #[test]
    fn lexer_warns_in_interpolated_strings() {
        let mut lexer = Lexer::new(r#""hello \x world""#);
        let tok = lexer.next_token();

        assert_eq!(tok.token_type, TokenType::String);
        assert_eq!(tok.literal, "hello x world");

        let warnings = lexer.warnings();
        assert!(warnings.len() > 0, "Should warn about unknown escape");
        assert!(warnings[0].message.contains("\\x"));
    }

    // ════════════════════════════════════════════════════════════════════════
    //  Comment Tests
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn empty_block_comment_is_skipped() {
        let input = "let x = /* */ 5;";
        let mut lexer = Lexer::new(input);

        let expected = vec![
            TokenType::Let,
            TokenType::Ident,
            TokenType::Assign,
            TokenType::Int,
            TokenType::Semicolon,
            TokenType::Eof,
        ];

        for expected_type in expected {
            assert_eq!(lexer.next_token().token_type, expected_type);
        }
    }

    #[test]
    fn four_slashes_doc_comment_includes_fourth() {
        let input = "//// hi\nlet x = 1;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, "/ hi");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn skip_single_line_comments() {
        let input = "let x = 5; // this is a comment\nlet y = 10;";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::Assign);
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Semicolon);
        // Comment is skipped
        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn skip_block_comments() {
        let input = "let x = /* comment */ 5;";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::Assign);
        // Block comment is skipped
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Semicolon);
    }

    #[test]
    fn skip_multiline_block_comments() {
        let input = "let x = /* this is\n   a multiline\n   comment */ 5;";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::Assign);
        // Multiline block comment is skipped
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
    }

    #[test]
    fn skip_nested_block_comments() {
        let input = "let x = /* outer /* inner */ still outer */ 5;";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::Assign);
        // Nested block comment is skipped entirely
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Semicolon);
    }

    #[test]
    fn deeply_nested_block_comments() {
        let input = "/* level 1 /* level 2 /* level 3 */ back to 2 */ back to 1 */ let";
        let mut lexer = Lexer::new(input);

        // All nested comments should be skipped
        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn line_doc_comment() {
        let input = "/// This is a doc comment\nlet x = 5;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, "This is a doc comment");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn line_doc_comment_without_space() {
        let input = "///No space after slashes\nlet x = 5;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, "No space after slashes");
    }

    #[test]
    fn multiple_line_doc_comments() {
        let input = "/// First line\n/// Second line\nlet x = 5;";
        let mut lexer = Lexer::new(input);

        let tok1 = lexer.next_token();
        assert_eq!(tok1.token_type, TokenType::DocComment);
        assert_eq!(tok1.literal, "First line");

        let tok2 = lexer.next_token();
        assert_eq!(tok2.token_type, TokenType::DocComment);
        assert_eq!(tok2.literal, "Second line");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn block_doc_comment() {
        let input = "/** This is a block doc comment */ let x = 5;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, " This is a block doc comment ");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn multiline_block_doc_comment() {
        let input = "/**\n * This is a multiline\n * doc comment\n */\nlet x = 5;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, "\n * This is a multiline\n * doc comment\n ");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn nested_block_doc_comment() {
        let input = "/** outer /* nested */ still outer */ let x = 5;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert!(tok.literal.contains("outer"));
        assert!(tok.literal.contains("nested"));
        assert!(
            !tok.literal.contains("/*") && !tok.literal.contains("*/"),
            "doc comment content omits nested delimiters"
        );

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn empty_doc_block_comment_emits_doc_token() {
        let input = "/**/let x = 1;";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::DocComment);
        assert_eq!(tok.literal, "");

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn unterminated_block_comment_error() {
        let input = "let x = 5; /* unterminated comment";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
        assert_eq!(lexer.next_token().token_type, TokenType::Ident);
        assert_eq!(lexer.next_token().token_type, TokenType::Assign);
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Semicolon);

        // Should emit error for unterminated comment
        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedBlockComment);
    }

    #[test]
    fn unterminated_block_doc_comment_error() {
        let input = "/** unterminated doc comment";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedBlockComment);
        assert_eq!(tok.literal, "");
    }

    #[test]
    fn unterminated_nested_block_comment() {
        let input = "/* outer /* inner */ missing close";
        let mut lexer = Lexer::new(input);

        let tok = lexer.next_token();
        assert_eq!(tok.token_type, TokenType::UnterminatedBlockComment);
    }

    #[test]
    fn comment_types_dont_interfere() {
        let input = "// line comment\n/* block */ /** doc block */ /// doc line\nlet x = 5;";
        let mut lexer = Lexer::new(input);

        // Line comment is skipped
        // Block comment is skipped
        // Doc block comment is emitted
        let tok1 = lexer.next_token();
        assert_eq!(tok1.token_type, TokenType::DocComment);

        // Doc line comment is emitted
        let tok2 = lexer.next_token();
        assert_eq!(tok2.token_type, TokenType::DocComment);

        assert_eq!(lexer.next_token().token_type, TokenType::Let);
    }

    #[test]
    fn slash_operator_still_works() {
        let input = "10 / 2";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Slash);
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
    }

    #[test]
    fn asterisk_operator_still_works() {
        let input = "10 * 2";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Int);
        assert_eq!(lexer.next_token().token_type, TokenType::Asterisk);
        assert_eq!(lexer.next_token().token_type, TokenType::Int);
    }
}
