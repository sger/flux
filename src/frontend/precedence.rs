use crate::frontend::token_type::TokenType;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Equals,      // ==, !=
    LessGreater, // <, >, <=, >=  (TODO: Implement <= and >=)
    Sum,         // +, -
    Product,     // *, /, %  (TODO: Implement %)
    Prefix,      // -x, !x
    Call,        // fn(x)
    Index,       // array[index]
}

pub fn token_precedence(token_type: &TokenType) -> Precedence {
    match token_type {
        TokenType::Eq | TokenType::NotEq => Precedence::Equals,
        TokenType::Lt | TokenType::Gt => Precedence::LessGreater,
        TokenType::Lte | TokenType::Gte => Precedence::LessGreater,
        TokenType::Plus | TokenType::Minus => Precedence::Sum,
        TokenType::Asterisk | TokenType::Slash => Precedence::Product,
        TokenType::Percent => Precedence::Product,
        TokenType::LParen => Precedence::Call,
        TokenType::LBracket | TokenType::Dot => Precedence::Index,
        _ => Precedence::Lowest,
    }
}
