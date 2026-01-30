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
    /// Are we in the middle of an interpolated string (after processing #{)?
    in_string: bool,
    /// Brace nesting depth within an interpolation expression
    interpolation_depth: usize,
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
            in_string: false,
            interpolation_depth: 0,
        };
        lexer.read_char();
        lexer
    }

    /// Get the next token from the input
    pub fn next_token(&mut self) -> Token {
        // If we're in the middle of an interpolated string, continue reading it
        if self.in_string && self.interpolation_depth == 0 {
            return self.continue_string();
        }

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
            Some('<') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Lte, "<=", line, col)
            }
            Some('>') if self.peek_char() == Some('=') => {
                self.read_char();
                Token::new(TokenType::Gte, ">=", line, col)
            }
            Some('-') if self.peek_char() == Some('>') => {
                self.read_char();
                Token::new(TokenType::Arrow, "->", line, col)
            }
            // Logical operators
            Some('&') if self.peek_char() == Some('&') => {
                self.read_char();
                Token::new(TokenType::And, "&&", line, col)
            }
            Some('|') if self.peek_char() == Some('|') => {
                self.read_char();
                Token::new(TokenType::Or, "||", line, col)
            }
            // Single-character operators and delimiters
            Some('=') => Token::new(TokenType::Assign, "=", line, col),
            Some('!') => Token::new(TokenType::Bang, "!", line, col),
            Some('+') => Token::new(TokenType::Plus, "+", line, col),
            Some('-') => Token::new(TokenType::Minus, "-", line, col),
            Some('*') => Token::new(TokenType::Asterisk, "*", line, col),
            Some('/') => Token::new(TokenType::Slash, "/", line, col),
            Some('%') => Token::new(TokenType::Percent, "%", line, col),
            Some('<') => Token::new(TokenType::Lt, "<", line, col),
            Some('>') => Token::new(TokenType::Gt, ">", line, col),
            Some('(') => Token::new(TokenType::LParen, "(", line, col),
            Some(')') => Token::new(TokenType::RParen, ")", line, col),
            Some('{') => {
                if self.in_string {
                    self.interpolation_depth += 1;
                }
                Token::new(TokenType::LBrace, "{", line, col)
            }
            Some('}') => {
                if self.in_string && self.interpolation_depth > 0 {
                    self.interpolation_depth -= 1;
                }
                Token::new(TokenType::RBrace, "}", line, col)
            }
            Some(',') => Token::new(TokenType::Comma, ",", line, col),
            Some(';') => Token::new(TokenType::Semicolon, ";", line, col),
            Some('[') => Token::new(TokenType::LBracket, "[", line, col),
            Some(']') => Token::new(TokenType::RBracket, "]", line, col),
            Some(':') => Token::new(TokenType::Colon, ":", line, col),
            Some('.') => Token::new(TokenType::Dot, ".", line, col),

            // String literals
            Some('"') => {
                return self.read_string_start();
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
                let (num, is_float) = self.read_number();
                let token_type = if is_float {
                    TokenType::Float
                } else {
                    TokenType::Int
                };
                return Token::new(token_type, num, line, col);
            }

            // Illegal character
            Some(ch) => Token::new(TokenType::Illegal, ch.to_string(), line, col),
        };

        self.read_char();
        token
    }

    pub fn tokenize(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();

        loop {
            let token = self.next_token();
            let is_eof = token.token_type == TokenType::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        tokens
    }

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

    fn read_number(&mut self) -> (String, bool) {
        let start = self.position;
        while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
            self.read_char();
        }
        let mut is_float = false;
        if self.current_char == Some('.') && self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.read_char(); // consume '.'
            while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
                self.read_char();
            }
        }
        let literal: String = self.input[start..self.position].iter().collect();
        (literal, is_float)
    }

    /// Read the start of a string (called when we see opening ")
    fn read_string_start(&mut self) -> Token {
        let line = self.line;
        let col = self.column;
        self.read_char(); // skip opening quote

        let (content, _ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // String has interpolation - mark that we're in a string
            // depth = 1 because we already consumed the opening { of #{
            self.in_string = true;
            self.interpolation_depth = 1;
            Token::new(TokenType::String, content, line, col)
        } else {
            // Simple string with no interpolation (or unterminated)
            Token::new(TokenType::String, content, line, col)
        }
    }

    /// Continue reading a string after an interpolation expression
    fn continue_string(&mut self) -> Token {
        let line = self.line;
        let col = self.column;

        let (content, _ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // More interpolations to come - reset depth since we consumed #{
            self.interpolation_depth = 1;
            Token::new(TokenType::String, content, line, col)
        } else {
            // End of interpolated string
            self.in_string = false;
            Token::new(TokenType::StringEnd, content, line, col)
        }
    }

    /// Read string content until we hit closing quote or interpolation start
    /// Returns (content, ended_with_quote, has_interpolation)
    fn read_string_content(&mut self) -> (String, bool, bool) {
        let mut result = String::new();

        while let Some(c) = self.current_char {
            match c {
                '"' => {
                    // End of string
                    self.read_char(); // consume closing quote
                    return (result, true, false);
                }
                '#' if self.peek_char() == Some('{') => {
                    // Start of interpolation
                    self.read_char(); // consume '#'
                    self.read_char(); // consume '{'
                    return (result, false, true);
                }
                '\\' => {
                    // Escape sequence
                    self.read_char(); // consume backslash
                    if let Some(escaped) = self.read_escape_sequence() {
                        result.push(escaped);
                    }
                }
                _ => {
                    result.push(c);
                    self.read_char();
                }
            }
        }

        // Hit EOF without closing quote
        (result, false, false)
    }

    /// Process an escape sequence after seeing backslash
    fn read_escape_sequence(&mut self) -> Option<char> {
        let result = match self.current_char {
            Some('n') => Some('\n'),
            Some('t') => Some('\t'),
            Some('r') => Some('\r'),
            Some('\\') => Some('\\'),
            Some('"') => Some('"'),
            Some('#') => Some('#'), // \# for literal #
            Some(c) => {
                // Unknown escape - just return the character as-is
                Some(c)
            }
            None => None,
        };
        self.read_char();
        result
    }

    /// Check if we're currently inside an interpolation expression
    pub fn is_in_interpolation(&self) -> bool {
        self.in_string && self.interpolation_depth == 0
    }
}

fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}
