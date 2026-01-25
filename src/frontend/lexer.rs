use crate::frontend::token::Token;
use crate::frontend::token_type::{TokenType, lookup_ident};

/// The Flux lexer
#[derive(Debug, Clone)]
pub struct Lexer {
    input: Vec<char>,
    position: usize,
    read_position: usize,
    current_char: Option<char>,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(input: impl Into<String>) -> Self {
        let mut lexer = Self {
            input: input.into().chars().collect(),
            position: 0,
            read_position: 0,
            current_char: None,
            line: 1,
            column: 0,
        };
        lexer.read_char();
        lexer
    }

    /// Get the next token from the input
    pub fn next_token(&mut self) -> Token {
        self.skip_ignorable();

        let line = self.line;
        let col = self.column;

        let token = match self.current_char {
            // Two-character operators
            Some('=') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Eq, "==", line, col)
            }
            Some('!') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::NotEq, "!=", line, col)
            }

            // Single-character operators and delimiters
            Some('=') => Token::new(TokenType::Assign, "=", line, col),
            Some('!') => Token::new(TokenType::Bang, "!", line, col),
            Some('+') => Token::new(TokenType::Plus, "+", line, col),
            Some('-') => Token::new(TokenType::Minus, "-", line, col),
            Some('*') => Token::new(TokenType::Asterisk, "*", line, col),
            Some('/') => Token::new(TokenType::Slash, "/", line, col),
            Some('<') => Token::new(TokenType::Lt, "<", line, col),
            Some('>') => Token::new(TokenType::Gt, ">", line, col),
            Some('(') => Token::new(TokenType::LParen, "(", line, col),
            Some(')') => Token::new(TokenType::RParen, ")", line, col),
            Some('{') => Token::new(TokenType::LBrace, "{", line, col),
            Some('}') => Token::new(TokenType::RBrace, "}", line, col),
            Some(',') => Token::new(TokenType::Comma, ",", line, col),
            Some(';') => Token::new(TokenType::Semicolon, ";", line, col),

            // String literals
            Some('"') => {
                let string = self.read_string();
                return Token::new(TokenType::String, string, line, col);
            }

            // End of file
            None => Token::new(TokenType::Eof, "", line, col),

            // Identifiers and keywords
            Some(ch) if is_letter(ch) => {
                let ident = self.read_identifier();
                let token_type = lookup_ident(&ident);
                return Token::new(token_type, ident, line, col);
            }

            // Numbers
            Some(ch) if ch.is_ascii_digit() => {
                let num = self.read_number();
                return Token::new(TokenType::Int, num, line, col);
            }

            // Illegal character
            Some(ch) => Token::new(TokenType::Illegal, ch.to_string(), line, col),
        };

        self.read_char();
        token
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    fn read_char(&mut self) {
        self.current_char = if self.read_position >= self.input.len() {
            None
        } else {
            Some(self.input[self.read_position])
        };

        self.position = self.read_position;
        self.read_position += 1;

        match self.current_char {
            Some('\n') => {
                self.line += 1;
                self.column = 0;
            }
            Some(_) => {
                self.column += 1;
            }
            None => {}
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input.get(self.read_position).copied()
    }

    fn skip_ignorable(&mut self) {
        loop {
            // Whitespace
            while matches!(self.current_char, Some(' ' | '\t' | '\r' | '\n')) {
                self.read_char();
            }

            // Single-line comments: //
            if self.current_char == Some('/') && self.peek_char() == Some('/') {
                while self.current_char.is_some() && self.current_char != Some('\n') {
                    self.read_char();
                }
                continue; // there may be whitespace/comments again
            }

            break;
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while self
            .current_char
            .is_some_and(|c| is_letter(c) || c.is_ascii_digit())
        {
            self.read_char();
        }
        self.input[start..self.position].iter().collect()
    }

    fn read_number(&mut self) -> String {
        let start = self.position;
        while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
            self.read_char();
        }
        self.input[start..self.position].iter().collect()
    }

    fn read_string(&mut self) -> String {
        self.read_char(); // skip opening quote
        let start = self.position;

        while let Some(c) = self.current_char {
            if c == '"' {
                break;
            }
            self.read_char();
        }

        let result: String = self.input[start..self.position].iter().collect();

        // Consume the closing quote
        if self.current_char == Some('"') {
            self.read_char();
        }

        result
    }
}

fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_char_tokens() {
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
    fn test_two_char_tokens() {
        let input = "== !=";
        let mut lexer = Lexer::new(input);

        assert_eq!(lexer.next_token().token_type, TokenType::Eq);
        assert_eq!(lexer.next_token().token_type, TokenType::NotEq);
    }

    #[test]
    fn test_keywords() {
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
    fn test_identifiers() {
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
    fn test_strings() {
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
    fn test_comments() {
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
    fn test_position_tracking() {
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
    fn test_complete_program() {
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
}
