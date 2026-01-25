use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

/// Position in source code for error reporting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

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
