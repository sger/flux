use std::fmt;
use std::sync::Arc;

use crate::frontend::lexeme::Lexeme;

use super::position::{Position, Span};
use super::token_type::TokenType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token_type: TokenType,
    pub literal: Lexeme,
    pub position: Position,
    pub end_position: Position,
}

impl Token {
    pub fn new(
        token_type: TokenType,
        literal: impl Into<Lexeme>,
        line: usize,
        column: usize,
    ) -> Self {
        let literal = literal.into();
        let start = Position::new(line, column);
        let end = Position::new(line, column.saturating_add(literal.len_chars()));
        Self {
            token_type,
            literal,
            position: start,
            end_position: end,
        }
    }

    pub fn new_static(
        token_type: TokenType,
        literal: &'static str,
        line: usize,
        column: usize,
    ) -> Self {
        Self::new(token_type, Lexeme::Static(literal), line, column)
    }

    pub fn new_span(
        token_type: TokenType,
        source: Arc<str>,
        start: usize,
        end: usize,
        line: usize,
        column: usize,
    ) -> Self {
        Self::new(
            token_type,
            Lexeme::from_span(source, start, end),
            line,
            column,
        )
    }

    pub fn new_span_with_end(
        token_type: TokenType,
        source: Arc<str>,
        start: usize,
        end: usize,
        line: usize,
        column: usize,
        end_position: Position,
    ) -> Self {
        Self {
            token_type,
            literal: Lexeme::from_span(source, start, end),
            position: Position::new(line, column),
            end_position,
        }
    }

    pub fn new_static_with_end(
        token_type: TokenType,
        literal: &'static str,
        line: usize,
        column: usize,
        end_position: Position,
    ) -> Self {
        Self::new_with_end(
            token_type,
            Lexeme::Static(literal),
            line,
            column,
            end_position,
        )
    }

    pub fn new_with_end(
        token_type: TokenType,
        literal: impl Into<Lexeme>,
        line: usize,
        column: usize,
        end_position: Position,
    ) -> Self {
        Self {
            token_type,
            literal: literal.into(),
            position: Position::new(line, column),
            end_position,
        }
    }

    pub fn span(&self) -> Span {
        Span::new(self.position, self.end_position)
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Token({}, {:?}, {})",
            self.token_type,
            self.literal.as_str(),
            self.position
        )
    }
}
