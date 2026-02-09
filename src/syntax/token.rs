use std::fmt;
use std::rc::Rc;

use crate::syntax::interner::Interner;
use crate::syntax::lexeme::Lexeme;
use crate::syntax::symbol::Symbol;

use super::position::{Position, Span};
use super::token_type::TokenType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token_type: TokenType,
    pub literal: Lexeme,
    pub symbol: Option<Symbol>,
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
            symbol: None,
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
        source: Rc<str>,
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
        source: Rc<str>,
        start: usize,
        end: usize,
        line: usize,
        column: usize,
        end_position: Position,
    ) -> Self {
        Self {
            token_type,
            literal: Lexeme::from_span(source, start, end),
            symbol: None,
            position: Position::new(line, column),
            end_position,
        }
    }

    pub fn new_ident_span_with_end(
        source: Rc<str>,
        symbol: Symbol,
        start: usize,
        end: usize,
        line: usize,
        column: usize,
        end_position: Position,
    ) -> Self {
        Self {
            token_type: TokenType::Ident,
            literal: Lexeme::from_span(source, start, end),
            symbol: Some(symbol),
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
            symbol: None,
            position: Position::new(line, column),
            end_position,
        }
    }

    /// Returns the text content of a token.
    ///
    /// For identifiers with symbols, resolves through the interner.
    /// For other tokens, returns the literal text.
    pub fn token_text<'a>(&'a self, interner: &'a Interner) -> &'a str {
        if let Some(symbol) = self.symbol {
            return interner.resolve(symbol);
        }

        self.literal.as_str()
    }

    /// Returns the text content of a number token.
    pub fn number_text(&self) -> &str {
        self.literal.as_str()
    }

    /// Returns the text content of a string token (without quotes).
    pub fn string_text(&self) -> &str {
        self.literal.as_str()
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
