use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenType {
    // Special
    Illegal,
    Eof,

    // Identifiers and literals
    Ident,
    Int,
    String,

    // Arithmetic Operators
    Plus,
    Minus,
    Asterisk,
    Slash,

    // Comparison Operators
    Lt,
    Gt,
    Eq,
    NotEq,

    // Logical operators
    Bang,

    // Assignment
    Assign,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,

    // Keywords
    Fun,
    Let,
    If,
    Else,
    Return,
    True,
    False,
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            // Special
            TokenType::Illegal => "ILLEGAL",
            TokenType::Eof => "EOF",

            // Identifiers and literals
            TokenType::Ident => "IDENT",
            TokenType::Int => "INT",
            TokenType::String => "STRING",

            // Arithmetic Operators
            TokenType::Plus => "+",
            TokenType::Minus => "-",
            TokenType::Asterisk => "*",
            TokenType::Slash => "/",

            // Comparison Operators
            TokenType::Lt => "<",
            TokenType::Gt => ">",
            TokenType::Eq => "==",
            TokenType::NotEq => "!=",

            // Logical operators
            TokenType::Bang => "!",

            // Assignment
            TokenType::Assign => "=",

            // Delimiters
            TokenType::LParen => "(",
            TokenType::RParen => ")",
            TokenType::LBrace => "{",
            TokenType::RBrace => "}",
            TokenType::Comma => ",",
            TokenType::Semicolon => ";",

            // Keywords
            TokenType::Fun => "FUN",
            TokenType::Let => "LET",
            TokenType::If => "IF",
            TokenType::Else => "ELSE",
            TokenType::Return => "RETURN",
            TokenType::True => "TRUE",
            TokenType::False => "FALSE",
        };
        write!(f, "{}", s)
    }
}
