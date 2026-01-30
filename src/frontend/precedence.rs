use crate::frontend::token_type::TokenType;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    LogicalOr,   // || lower precedence than &&
    LogicalAnd,  // && higher precedence than ||
    Equals,      // ==, !=
    LessGreater, // <, >, <=, >=
    Sum,         // +, -
    Product,     // *, /, %
    Prefix,      // -x, !x
    Call,        // fn(x)
    Index,       // array[index]
}

pub fn token_precedence(token_type: &TokenType) -> Precedence {
    match token_type {
        TokenType::Or => Precedence::LogicalOr,
        TokenType::And => Precedence::LogicalAnd,
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
