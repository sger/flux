use crate::frontend::{
    diagnostics::Diagnostic, lexer::Lexer, position::Span, program::Program, token::Token,
    token_type::TokenType,
};

mod expression;
mod helpers;
mod literal;
mod statement;

pub struct Parser {
    pub(super) lexer: Lexer,
    pub(super) current_token: Token,
    pub(super) peek_token: Token,
    pub(super) peek2_token: Token,
    pub errors: Vec<Diagnostic>,
    pub(super) suppress_unterminated_string_error: bool,
}

impl Parser {
    pub fn new(lexer: Lexer) -> Self {
        let mut parser = Parser {
            lexer,
            current_token: Token::new(TokenType::Eof, "", 0, 0),
            peek_token: Token::new(TokenType::Eof, "", 0, 0),
            peek2_token: Token::new(TokenType::Eof, "", 0, 0),
            errors: Vec::new(),
            suppress_unterminated_string_error: false,
        };
        parser.next_token();
        parser.next_token();
        parser.next_token();
        parser
    }

    pub fn parse_program(&mut self) -> Program {
        let start = self.current_token.position;
        let mut program = Program::new();

        while self.current_token.token_type != TokenType::Eof {
            if self.current_token.token_type == TokenType::RBrace {
                self.next_token();
                continue;
            }
            if let Some(statement) = self.parse_statement() {
                program.statements.push(statement);
            }
            self.next_token();
        }

        program.span = Span::new(start, self.current_token.position);
        program
    }
}

fn is_uppercase_ident(token: &Token) -> bool {
    if token.token_type != TokenType::Ident {
        return false;
    }
    token
        .literal
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

/// Check if token is PascalCase (starts uppercase, contains lowercase)
/// This distinguishes module names like "Math", "Constants" from
/// ALL_CAPS constants like "PI", "TAU", "MAX"
fn is_pascal_case_ident(token: &Token) -> bool {
    if token.token_type != TokenType::Ident {
        return false;
    }
    let literal = &token.literal;
    let mut chars = literal.chars();
    // First char must be uppercase
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    // Must contain at least one lowercase letter (to distinguish from ALL_CAPS)
    literal.chars().any(|ch| ch.is_ascii_lowercase())
}

#[cfg(test)]
mod parser_test;
