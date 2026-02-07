//! The Flux lexer - tokenizes source code into tokens

// Module declarations
mod comments;
mod escape;
mod helpers;
mod identifiers;
mod numbers;
mod reader;
mod state;
mod strings;

use std::sync::Arc;

// Re-export state for visibility
use reader::CharReader;
use state::LexerState;

use crate::frontend::position::Position;
use crate::frontend::token::Token;
use crate::frontend::token_type::{TokenType, lookup_ident};

use helpers::is_letter_byte;
use numbers::NumberKind;

/// Warning emitted during lexing
#[derive(Debug, Clone)]
pub struct LexerWarning {
    pub message: String,
    pub position: Position,
}

/// The Flux lexer
#[derive(Debug, Clone)]
pub struct Lexer {
    reader: CharReader,
    state: LexerState,
    warnings: Vec<LexerWarning>,
    /// Track unterminated block comment error (position where /* started)
    unterminated_block_comment_pos: Option<Position>,
}

impl Lexer {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            reader: CharReader::new(input.into()),
            state: LexerState::Normal,
            warnings: Vec::new(),
            unterminated_block_comment_pos: None,
        }
    }

    /// Get warnings collected during lexing
    pub fn warnings(&self) -> &[LexerWarning] {
        &self.warnings
    }

    /// Get the next token from the input
    pub fn next_token(&mut self) -> Token {
        // Cache interpolation flags once per token; hot path avoids repeated state lookups.
        let in_interp_ctx = self.in_interpolated_string_context();
        let in_interp_expr = in_interp_ctx && self.current_interpolation_depth() > 0;

        // If we're in the middle of an interpolated string, continue reading it.
        if in_interp_ctx && !in_interp_expr {
            return self.continue_string();
        }

        self.skip_ignorable();

        // Check if we encountered an unterminated block comment
        if let Some(error_pos) = self.unterminated_block_comment_pos.take() {
            return Token::new_static_with_end(
                TokenType::UnterminatedBlockComment,
                "",
                error_pos.line,
                error_pos.column,
                self.cursor_position(),
            );
        }

        let cursor = self.cursor_position();
        let line = cursor.line;
        let col = cursor.column;

        // Snapshot lookahead once. This keeps comment/operator dispatch branch-light.
        let b0 = self.current_byte();
        let b1 = self.peek_byte();
        let b2 = self.peek2_byte();
        let start_idx = self.current_index();

        // End of file
        let Some(b0) = b0 else {
            // Future improvement: if this is non-empty at EOF, emit a dedicated
            // unterminated interpolation/string diagnostic from the lexer.
            self.clear_interpolation_state();
            return Token::new_static(TokenType::Eof, "", line, col);
        };

        // String literals are delegated; reader state advances internally.
        if b0 == b'"' {
            return self.read_string_start();
        }

        // Doc comments (/// or /**) are tokens; non-doc comments are skipped in
        // skip_ignorable(), so slash falls back to operator token.
        if b0 == b'/' && b1 == Some(b'/') && b2 == Some(b'/') {
            return self.read_doc_line_comment();
        }
        if b0 == b'/' && b1 == Some(b'*') && b2 == Some(b'*') {
            return self.read_doc_block_comment();
        }

        // Two-byte operator dispatch table.
        if let Some(token) = self.two_byte_token(b0, b1, line, col) {
            return token;
        }

        // One-byte operator/delimiter dispatch table.
        if let Some(token) = self.one_byte_token(b0, line, col, in_interp_expr) {
            return token;
        }

        // Identifiers/keywords: ASCII fast path with zero-copy keyword lookup.
        if is_letter_byte(b0) {
            let (start, end) = self.read_identifier_span();
            let ident = self.slice_str(start, end);
            let token_type = lookup_ident(ident);
            return Token::new_span_with_end(
                token_type,
                self.source_arc(),
                start,
                end,
                line,
                col,
                self.cursor_position(),
            );
        }

        // Numbers: lex into a span and defer parsing.
        if b0.is_ascii_digit() {
            let ((start, end), kind) = self.read_number_span();
            let token_type = match kind {
                NumberKind::Int => TokenType::Int,
                NumberKind::Float => TokenType::Float,
            };

            return Token::new_span_with_end(
                token_type,
                self.source_arc(),
                start,
                end,
                line,
                col,
                self.cursor_position(),
            );
        }

        // Illegal character: emit span-backed literal, no per-token allocation.
        self.read_char();
        Token::new_span_with_end(
            TokenType::Illegal,
            self.source_arc(),
            start_idx,
            self.current_index(),
            line,
            col,
            self.cursor_position(),
        )
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
        self.reader.advance();
    }

    fn current_byte(&self) -> Option<u8> {
        self.reader.current_byte()
    }

    fn peek_byte(&self) -> Option<u8> {
        self.reader.peek_byte()
    }

    fn peek2_byte(&self) -> Option<u8> {
        self.reader.peek2_byte()
    }

    fn current_index(&self) -> usize {
        self.reader.index()
    }

    fn cursor_position(&self) -> Position {
        self.reader.position()
    }

    fn slice_str(&self, start: usize, end: usize) -> &str {
        self.reader.slice_str(start, end)
    }

    fn source_arc(&self) -> Arc<str> {
        self.reader.source_arc()
    }

    fn skip_ignorable(&mut self) {
        loop {
            // Tight ASCII whitespace run in one pass.
            self.reader.skip_ascii_whitespace();

            // Snapshot lookahead once per outer iteration.
            let b0 = self.current_byte();
            let b1 = self.peek_byte();
            let b2 = self.peek2_byte();

            match (b0, b1, b2) {
                // Doc comments are tokens; do not skip here.
                (Some(b'/'), Some(b'/'), Some(b'/')) => break,
                (Some(b'/'), Some(b'*'), Some(b'*')) => break,

                // Non-doc line comment.
                (Some(b'/'), Some(b'/'), _) => {
                    self.read_char(); // '/'
                    self.read_char(); // '/'
                    self.reader.skip_line_comment_body();
                    continue;
                }

                // Non-doc block comment.
                (Some(b'/'), Some(b'*'), _) => {
                    let comment_start = self.cursor_position();
                    if !self.skip_block_comment() {
                        self.unterminated_block_comment_pos = Some(comment_start);
                        break;
                    }
                    continue;
                }

                _ => break,
            }
        }
    }

    fn two_byte_token(&mut self, b0: u8, b1: Option<u8>, line: usize, col: usize) -> Option<Token> {
        let (token_type, literal) = match (b0, b1) {
            (b'=', Some(b'=')) => (TokenType::Eq, "=="),
            (b'!', Some(b'=')) => (TokenType::NotEq, "!="),
            (b'<', Some(b'=')) => (TokenType::Lte, "<="),
            (b'>', Some(b'=')) => (TokenType::Gte, ">="),
            (b'-', Some(b'>')) => (TokenType::Arrow, "->"),
            (b'&', Some(b'&')) => (TokenType::And, "&&"),
            (b'|', Some(b'|')) => (TokenType::Or, "||"),
            (b'|', Some(b'>')) => (TokenType::Pipe, "|>"),
            _ => return None,
        };

        self.reader.advance_ascii_bytes(2);
        Some(Token::new_static_with_end(
            token_type,
            literal,
            line,
            col,
            self.cursor_position(),
        ))
    }

    fn one_byte_token(
        &mut self,
        b0: u8,
        line: usize,
        col: usize,
        in_interp_expr: bool,
    ) -> Option<Token> {
        let (token_type, literal) = match b0 {
            b'=' => (TokenType::Assign, "="),
            b'!' => (TokenType::Bang, "!"),
            b'+' => (TokenType::Plus, "+"),
            b'-' => (TokenType::Minus, "-"),
            b'*' => (TokenType::Asterisk, "*"),
            b'/' => (TokenType::Slash, "/"),
            b'%' => (TokenType::Percent, "%"),
            b'<' => (TokenType::Lt, "<"),
            b'>' => (TokenType::Gt, ">"),
            b'(' => (TokenType::LParen, "("),
            b')' => (TokenType::RParen, ")"),
            b'{' => {
                if in_interp_expr {
                    self.increment_current_interpolation_depth();
                }
                (TokenType::LBrace, "{")
            }
            b'}' => {
                if in_interp_expr {
                    self.decrement_current_interpolation_depth();
                }
                (TokenType::RBrace, "}")
            }
            b',' => (TokenType::Comma, ","),
            b';' => (TokenType::Semicolon, ";"),
            b'[' => (TokenType::LBracket, "["),
            b']' => (TokenType::RBracket, "]"),
            b':' => (TokenType::Colon, ":"),
            b'.' => (TokenType::Dot, "."),
            b'\\' => (TokenType::Backslash, "\\"),
            _ => return None,
        };

        self.reader.advance_ascii_bytes(1);
        Some(Token::new_static_with_end(
            token_type,
            literal,
            line,
            col,
            self.cursor_position(),
        ))
    }
}
