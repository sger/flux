use std::fmt;

use super::position::{Position, Span};
use super::token_type::TokenType;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub token_type: TokenType,
    pub literal: String,
    pub position: Position,
    pub end_position: Position,
}

impl Token {
    pub fn new(
        token_type: TokenType,
        literal: impl Into<String>,
        line: usize,
        column: usize,
    ) -> Self {
        let literal = literal.into();
        let start = Position::new(line, column);
        let len = literal.chars().count();
        let end = Position::new(line, column.saturating_add(len));
        Self {
            token_type,
            literal,
            position: start,
            end_position: end,
        }
    }

    pub fn new_with_end(
        token_type: TokenType,
        literal: impl Into<String>,
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
            self.token_type, self.literal, self.position
        )
    }
}
