use crate::frontend::{
    diagnostics::{Diagnostic, ErrorType},
    precedence::Precedence,
    statement::Statement,
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    pub(super) fn parse_statement(&mut self) -> Option<Statement> {
        match self.current_token.token_type {
            TokenType::Module => self.parse_module_statement(),
            TokenType::Import => self.parse_import_statement(),
            TokenType::Let => self.parse_let_statement(),
            TokenType::Return => self.parse_return_statement(),
            TokenType::Fun if self.is_peek_token(TokenType::Ident) => {
                self.parse_function_statement()
            }
            TokenType::Ident if self.current_token.literal == "fn" => {
                self.errors.push(
                    Diagnostic::error("UNKNOWN KEYWORD")
                        .with_code("E101")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message("Flux uses `fun` for function declarations.")
                        .with_suggestion_message(
                            self.current_token.span(),
                            "fun",
                            "Replace 'fn' with 'fun'",
                        ),
                );
                self.synchronize_after_error();
                None
            }
            TokenType::Ident
                if self.current_token.literal != "fun"
                    && self.current_token.literal.starts_with("fun")
                    && self.is_peek_token(TokenType::Ident) =>
            {
                self.errors.push(
                    Diagnostic::error("UNKNOWN KEYWORD")
                        .with_code("E101")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message(format!(
                            "Unknown keyword `{}`. Flux uses `fun` for function declarations.",
                            self.current_token.literal
                        ))
                        .with_hint_text("Did you mean `fun`?"),
                );
                self.synchronize_after_error();
                None
            }

            // Check if we have `identifier = expression` (reassignment without 'let')
            TokenType::Ident if self.is_peek_token(TokenType::Assign) => {
                self.parse_assignment_statement()
            }
            _ => self.parse_expression_statement(),
        }
    }

    pub(super) fn parse_expression_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        let expression = self.parse_expression(Precedence::Lowest)?;

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Expression {
            expression,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_function_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self.current_token.literal.clone();

        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        let parameters = self.parse_function_parameters()?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let body = self.parse_block();

        Some(Statement::Function {
            name,
            parameters,
            body,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_return_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        self.next_token();

        let value = if self.is_current_token(TokenType::Semicolon) {
            None
        } else {
            Some(self.parse_expression(Precedence::Lowest)?)
        };

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Return {
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_let_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self.current_token.literal.clone();

        if !self.expect_peek(TokenType::Assign) {
            return None;
        }

        self.next_token();

        let value = self.parse_expression(Precedence::Lowest)?;

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Let {
            name,
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_assignment_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        let name = self.current_token.literal.clone();

        if !self.expect_peek(TokenType::Assign) {
            return None;
        }

        self.next_token();

        let value = self.parse_expression(Precedence::Lowest)?;

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Assign {
            name,
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_module_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self.parse_qualified_name()?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let body = self.parse_block();

        Some(Statement::Module {
            name,
            body,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_import_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self.parse_qualified_name()?;
        let mut alias = None;

        if self.is_peek_token(TokenType::As) {
            self.next_token(); // consume 'as'
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            alias = Some(self.current_token.literal.clone());
        }

        // No semicolon required for import statements

        Some(Statement::Import {
            name,
            alias,
            span: self.span_from(start),
        })
    }
}
