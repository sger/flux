use std::fmt;

use super::position::Position;
use super::token_type::TokenType;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub token_type: TokenType,
    pub literal: String,
    pub position: Position,
}

impl Token {
    pub fn new(
        token_type: TokenType,
        literal: impl Into<String>,
        line: usize,
        column: usize,
    ) -> Self {
        Self {
            token_type,
            literal: literal.into(),
            position: Position::new(line, column),
        }
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
