use crate::frontend::{
    block::Block,
    diagnostics::{Diagnostic, EXPECTED_EXPRESSION, ErrorType, UNTERMINATED_STRING},
    expression::Expression,
    position::{Position, Span},
    precedence::{Precedence, token_precedence},
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    // Token navigation
    pub(super) fn next_token(&mut self) {
        self.current_token = self.peek_token.clone();
        self.peek_token = self.peek2_token.clone();
        self.peek2_token = self.lexer.next_token();
    }

    pub(super) fn is_current_token(&self, token_type: TokenType) -> bool {
        self.current_token.token_type == token_type
    }

    pub(super) fn is_peek_token(&self, token_type: TokenType) -> bool {
        self.peek_token.token_type == token_type
    }

    pub(super) fn expect_peek(&mut self, token_type: TokenType) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.peek_error(token_type);
            false
        }
    }

    // Span/position utilities
    pub(super) fn span_from(&self, start: Position) -> Span {
        Span::new(start, self.current_token.position)
    }

    // Precedence helpers
    pub(super) fn current_precedence(&self) -> Precedence {
        token_precedence(&self.current_token.token_type)
    }

    pub(super) fn peek_precedence(&self) -> Precedence {
        token_precedence(&self.peek_token.token_type)
    }

    // Complex parsing helpers
    pub(super) fn parse_qualified_name(&mut self) -> Option<String> {
        let mut name = self.current_token.literal.clone();
        while self.is_peek_token(TokenType::Dot) {
            self.next_token(); // consume '.'
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            name.push('.');
            name.push_str(&self.current_token.literal);
        }
        Some(name)
    }

    pub(super) fn parse_function_parameters(&mut self) -> Option<Vec<String>> {
        let mut identifiers = Vec::new();

        if self.is_peek_token(TokenType::RParen) {
            self.next_token();
            return Some(identifiers);
        }

        self.next_token();
        identifiers.push(self.current_token.literal.clone());

        while self.is_peek_token(TokenType::Comma) {
            self.next_token();
            self.next_token();
            identifiers.push(self.current_token.literal.clone());
        }

        if !self.expect_peek(TokenType::RParen) {
            return None;
        }

        Some(identifiers)
    }

    pub(super) fn parse_block(&mut self) -> Block {
        let start = self.current_token.position;
        let mut statements = Vec::new();
        self.next_token();

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            if let Some(statement) = self.parse_statement() {
                statements.push(statement);
            }
            self.next_token();
        }

        Block {
            statements,
            span: Span::new(start, self.current_token.position),
        }
    }

    pub(super) fn parse_expression_list(&mut self, end: TokenType) -> Option<Vec<Expression>> {
        let mut list = Vec::new();

        if self.is_peek_token(end) {
            self.next_token();
            return Some(list);
        }

        self.next_token();
        list.push(self.parse_expression(Precedence::Lowest)?);

        while self.is_peek_token(TokenType::Comma) {
            self.next_token();
            self.next_token();
            list.push(self.parse_expression(Precedence::Lowest)?);
        }

        if !self.is_peek_token(end) {
            let line = self.current_token.position.line;
            let eof_pos = Position::new(line, usize::MAX - 1);
            self.errors.push(
                Diagnostic::error("UNEXPECTED TOKEN")
                    .with_code("E105")
                    .with_error_type(ErrorType::Compiler)
                    .with_span(Span::new(eof_pos, eof_pos))
                    .with_message(format!("Missing {} before end of line.", end)),
            );
            return None;
        }
        self.next_token();

        Some(list)
    }

    // Error handling
    pub(super) fn no_prefix_parse_error(&mut self) {
        let error_spec = &EXPECTED_EXPRESSION;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&self.current_token.token_type.to_string()],
            String::new(), // No file context in parser
            Span::new(self.current_token.position, self.current_token.position),
        );
        self.errors.push(diag);
    }

    pub(super) fn unterminated_string_error(&mut self) {
        // Unterminated strings are a lexical error; use the lexer-provided end position
        // where the closing quote should have appeared.
        let token_span = self.current_token.span();
        let error_pos = token_span.end;

        let error_spec = &UNTERMINATED_STRING;
        let diag = Diagnostic::make_error(
            error_spec,
            &[],           // No message formatting args needed
            String::new(), // No file context in parser
            Span::new(error_pos, error_pos),
        );
        self.errors.push(diag);
        self.synchronize_after_error();
    }

    pub(super) fn synchronize_after_error(&mut self) {
        // Advance to a reasonable boundary to avoid cascading errors.
        self.next_token();
        while !matches!(
            self.current_token.token_type,
            TokenType::Semicolon | TokenType::RBrace | TokenType::Eof
        ) {
            self.next_token();
        }
        if self.current_token.token_type == TokenType::RBrace {
            self.next_token();
        }
    }

    pub(super) fn peek_error(&mut self, expected: TokenType) {
        self.errors.push(
            Diagnostic::error("UNEXPECTED TOKEN")
                .with_code("E105")
                .with_error_type(ErrorType::Compiler)
                .with_span(self.peek_token.span())
                .with_message(format!(
                    "Expected {}, got {}.",
                    expected, self.peek_token.token_type
                )),
        );
    }
}
