//! The Flux lexer - tokenizes source code into tokens

// Module declarations
mod comments;
mod escape;
mod helpers;
mod identifiers;
mod numbers;
mod state;
mod strings;

// Re-export state for visibility
use state::LexerState;

use crate::frontend::position::Position;
use crate::frontend::token::Token;
use crate::frontend::token_type::{TokenType, lookup_ident};

use helpers::is_letter;

/// Warning emitted during lexing
#[derive(Debug, Clone)]
pub struct LexerWarning {
    pub message: String,
    pub position: Position,
}

/// The Flux lexer
#[derive(Debug, Clone)]
pub struct Lexer {
    input: Vec<char>,
    position: usize,
    read_position: usize,
    current_char: Option<char>,
    line: usize,
    column: usize,
    state: LexerState,
    warnings: Vec<LexerWarning>,
    /// Track unterminated block comment error (position where /* started)
    unterminated_block_comment_pos: Option<Position>,
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
            state: LexerState::Normal,
            warnings: Vec::new(),
            unterminated_block_comment_pos: None,
        };
        lexer.read_char();
        lexer
    }

    /// Get warnings collected during lexing
    pub fn warnings(&self) -> &[LexerWarning] {
        &self.warnings
    }

    /// Get the next token from the input
    pub fn next_token(&mut self) -> Token {
        // If we're in the middle of an interpolated string, continue reading it
        if self.in_interpolated_string_context() && !self.is_in_interpolation() {
            return self.continue_string();
        }

        self.skip_ignorable();

        // Check if we encountered an unterminated block comment
        if let Some(error_pos) = self.unterminated_block_comment_pos.take() {
            return Token::new_with_end(
                TokenType::UnterminatedBlockComment,
                "",
                error_pos.line,
                error_pos.column,
                Position::new(self.line, self.column),
            );
        }

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
            // Pipe operator
            Some('|') if self.peek_char() == Some('>') => {
                self.read_char();
                Token::new(TokenType::Pipe, "|>", line, col)
            }
            // Single-character operators and delimiters
            Some('=') => Token::new(TokenType::Assign, "=", line, col),
            Some('!') => Token::new(TokenType::Bang, "!", line, col),
            Some('+') => Token::new(TokenType::Plus, "+", line, col),
            Some('-') => Token::new(TokenType::Minus, "-", line, col),
            Some('*') => Token::new(TokenType::Asterisk, "*", line, col),
            Some('/') => {
                // Doc comments (/// or /**) are tokens; non-doc comments are skipped in
                // skip_ignorable(), so the fallback here is always Slash.
                if self.peek_char() == Some('/') && self.peek_n(2) == Some('/') {
                    return self.read_doc_line_comment();
                }
                if self.peek_char() == Some('*') && self.peek_n(2) == Some('*') {
                    return self.read_doc_block_comment();
                }
                Token::new(TokenType::Slash, "/", line, col)
            }
            Some('%') => Token::new(TokenType::Percent, "%", line, col),
            Some('<') => Token::new(TokenType::Lt, "<", line, col),
            Some('>') => Token::new(TokenType::Gt, ">", line, col),
            Some('(') => Token::new(TokenType::LParen, "(", line, col),
            Some(')') => Token::new(TokenType::RParen, ")", line, col),
            Some('{') => {
                if self.is_in_interpolation() {
                    self.increment_current_interpolation_depth();
                }
                Token::new(TokenType::LBrace, "{", line, col)
            }
            Some('}') => {
                if self.is_in_interpolation() {
                    self.decrement_current_interpolation_depth();
                }
                Token::new(TokenType::RBrace, "}", line, col)
            }
            Some(',') => Token::new(TokenType::Comma, ",", line, col),
            Some(';') => Token::new(TokenType::Semicolon, ";", line, col),
            Some('[') => Token::new(TokenType::LBracket, "[", line, col),
            Some(']') => Token::new(TokenType::RBracket, "]", line, col),
            Some(':') => Token::new(TokenType::Colon, ":", line, col),
            Some('.') => Token::new(TokenType::Dot, ".", line, col),
            Some('\\') => Token::new(TokenType::Backslash, "\\", line, col),

            // String literals
            Some('"') => {
                return self.read_string_start();
            }

            // End of file
            None => {
                // Future improvement: if this is non-empty at EOF, emit a dedicated
                // unterminated interpolation/string diagnostic from the lexer.
                self.clear_interpolation_state();
                Token::new(TokenType::Eof, "", line, col)
            }

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
        // Update column BEFORE moving to the next character
        // This ensures column represents the position of current_char, not the next char
        if self.current_char == Some('\n') {
            self.line += 1;
            self.column = 0;
        } else if self.current_char.is_some() {
            self.column += 1;
        }

        self.current_char = if self.read_position >= self.input.len() {
            None
        } else {
            Some(self.input[self.read_position])
        };

        self.position = self.read_position;
        self.read_position += 1;
    }

    fn peek_char(&self) -> Option<char> {
        self.input.get(self.read_position).copied()
    }

    /// Look ahead n chars without advancing.
    /// n=1 is equivalent to peek_char() (next char), n=2 is the char after that.
    /// Returns None when peeking past EOF.
    fn peek_n(&self, n: usize) -> Option<char> {
        debug_assert!(n > 0, "peek_n expects n >= 1");
        self.input.get(self.read_position + (n - 1)).copied()
    }

    fn skip_ignorable(&mut self) {
        loop {
            // Whitespace
            while matches!(self.current_char, Some(' ' | '\t' | '\r' | '\n')) {
                self.read_char();
            }

            // Single-line comments: // (but not ///)
            if self.current_char == Some('/') && self.peek_char() == Some('/') {
                // Check if it's a doc comment ///
                if self.peek_n(2) != Some('/') {
                    // Regular // comment - skip it
                    while self.current_char.is_some() && self.current_char != Some('\n') {
                        self.read_char();
                    }
                    continue; // there may be whitespace/comments again
                }
                // It's a doc comment /// - don't skip, let next_token handle it
                break;
            }

            // Block comments: /* (but not /**)
            if self.current_char == Some('/') && self.peek_char() == Some('*') {
                // Check if it's a doc comment /**
                if self.peek_n(2) != Some('*') {
                    // Regular /* comment - skip it
                    let comment_start = Position::new(self.line, self.column);
                    if !self.skip_block_comment() {
                        // Unterminated block comment - we've hit EOF
                        // Store the error position
                        self.unterminated_block_comment_pos = Some(comment_start);
                        break;
                    }
                    continue; // there may be whitespace/comments again
                }
                // It's a doc comment /** - don't skip, let next_token handle it
                break;
            }

            break;
        }
    }
}
