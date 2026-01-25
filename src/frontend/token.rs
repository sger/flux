use std::fmt;

use super::{Position, TokenType};

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

pub fn lookup_ident(ident: &str) -> TokenType {
    match ident {
        "fun" => TokenType::Fun,
        "let" => TokenType::Let,
        "if" => TokenType::If,
        "else" => TokenType::Else,
        "return" => TokenType::Return,
        "true" => TokenType::True,
        "false" => TokenType::False,
        _ => TokenType::Ident,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_keywords() {
        assert_eq!(lookup_ident("fun"), TokenType::Fun);
        assert_eq!(lookup_ident("let"), TokenType::Let);
        assert_eq!(lookup_ident("if"), TokenType::If);
        assert_eq!(lookup_ident("else"), TokenType::Else);
        assert_eq!(lookup_ident("return"), TokenType::Return);
        assert_eq!(lookup_ident("true"), TokenType::True);
        assert_eq!(lookup_ident("false"), TokenType::False);
    }

    #[test]
    fn test_lookup_ident() {
        assert_eq!(lookup_ident("foo"), TokenType::Ident);
        assert_eq!(lookup_ident("test"), TokenType::Ident);
        assert_eq!(lookup_ident("test_func"), TokenType::Ident);
    }
}
