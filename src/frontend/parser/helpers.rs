use crate::frontend::{
    block::Block,
    diagnostics::{
        Diagnostic, EXPECTED_EXPRESSION, UNTERMINATED_STRING,
        compiler_errors::{missing_comma, unexpected_token},
    },
    expression::Expression,
    position::{Position, Span},
    precedence::{Precedence, token_precedence},
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    // Token navigation
    pub(super) fn next_token(&mut self) {
        let next = self.lexer.next_token();
        self.current_token = std::mem::replace(
            &mut self.peek_token,
            std::mem::replace(&mut self.peek2_token, next),
        );
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
        Span::new(start, self.current_token.end_position)
    }

    pub(super) fn is_expression_terminator(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Semicolon
                | TokenType::Comma
                | TokenType::RParen
                | TokenType::RBracket
                | TokenType::RBrace
                | TokenType::Colon
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    pub(super) fn token_starts_expression(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::Ident
                | TokenType::Int
                | TokenType::Float
                | TokenType::String
                | TokenType::UnterminatedString
                | TokenType::InterpolationStart
                | TokenType::True
                | TokenType::False
                | TokenType::None
                | TokenType::Some
                | TokenType::Left
                | TokenType::Right
                | TokenType::Bang
                | TokenType::Minus
                | TokenType::LParen
                | TokenType::LBracket
                | TokenType::LBrace
                | TokenType::If
                | TokenType::Fun
                | TokenType::Match
                | TokenType::Backslash
        )
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
        debug_assert!(self.is_current_token(TokenType::LParen));
        let mut identifiers = Vec::new();

        // Empty list: ()
        if self.is_peek_token(TokenType::RParen) {
            self.next_token(); // consume ')'
            return Some(identifiers);
        }

        loop {
            // Move to parameter candidate token
            self.next_token();

            // Parse identifier or recover (recovery stops on Comma/RParen/Eof)
            if let Some(param) = self.parse_parameter_identifier_or_recover() {
                identifiers.push(param);
                self.next_token(); // move to delimiter after a valid parameter
            }
            // else: current_token is already at delimiter due to recovery

            match self.current_token.token_type {
                TokenType::Comma => {
                    // Reject trailing comma: f(a,)
                    if self.is_peek_token(TokenType::RParen) {
                        self.next_token(); // consume ')', so error points at ')'
                        self.errors.push(unexpected_token(
                            self.current_token.span(),
                            "Expected identifier as parameter, got `)`.",
                        ));
                        return Some(identifiers);
                    }

                    // Continue with next parameter (comma is current delimiter; next loop consumes next token)
                    continue;
                }

                TokenType::RParen | TokenType::Eof => return Some(identifiers),

                _ => {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        format!(
                            "Expected `,` or `)` after parameter, got {}.",
                            self.current_token.token_type
                        ),
                    ));

                    // Recover to a safe delimiter
                    while !matches!(
                        self.current_token.token_type,
                        TokenType::Comma | TokenType::RParen | TokenType::Eof
                    ) {
                        self.next_token();
                    }

                    // If we recovered to comma, attempt to continue parsing more params.
                    if self.current_token.token_type == TokenType::Comma {
                        continue;
                    }

                    return Some(identifiers);
                }
            }
        }
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
            span: Span::new(start, self.current_token.end_position),
        }
    }

    pub(super) fn parse_expression_list(&mut self, end: TokenType) -> Option<Vec<Expression>> {
        let mut list = Vec::new();

        if self.is_peek_token(end) {
            self.next_token();
            return Some(list);
        }

        loop {
            self.next_token();

            if self.is_current_token(end) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "Expected expression after `,`, got {}.",
                        self.current_token.token_type
                    ),
                ));
                return None;
            }

            if self.is_current_token(TokenType::Comma) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    "Expected expression after `,`, got `,`.",
                ));
                continue;
            }

            if self.is_current_token(TokenType::Eof) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!("Expected `{}` before end of file.", end),
                ));
                return None;
            }

            list.push(self.parse_expression(Precedence::Lowest)?);

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma
                continue;
            }

            if self.is_peek_token(end) {
                self.next_token();
                return Some(list);
            }

            // Adjacent expression-starting token inside a delimited list strongly
            // indicates a missing comma: f(a b), [a b], etc.
            if self.token_starts_expression(self.peek_token.token_type) {
                let (context, example) = match end {
                    TokenType::RParen => ("arguments", "`f(a, b)`"),
                    TokenType::RBracket => ("items", "`[a, b]`"),
                    _ => ("items", "`a, b`"),
                };
                self.errors
                    .push(missing_comma(self.peek_token.span(), context, example));
                // Pretend a comma existed and continue parsing the next list item.
                continue;
            }

            let message = if self.peek_token.token_type == TokenType::Eof {
                format!("Expected `{}` before end of file.", end)
            } else {
                format!(
                    "Expected `{}` after list item, got {}.",
                    end, self.peek_token.token_type
                )
            };
            self.errors
                .push(unexpected_token(self.peek_token.span(), message));
            return None;
        }
    }

    // Error handling
    pub(super) fn no_prefix_parse_error(&mut self) {
        let error_spec = &EXPECTED_EXPRESSION;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&self.current_token.token_type.to_string()],
            String::new(), // No file context in parser
            self.current_token.span(),
        );
        self.errors.push(diag);
    }

    pub(super) fn unterminated_string_error(&mut self) {
        // Unterminated strings are a lexical error; use the lexer-provided end position
        // where the closing quote should have appeared.
        let token_span = self.current_token.span();

        let error_spec = &UNTERMINATED_STRING;
        let diag = Diagnostic::make_error(
            error_spec,
            &[],           // No message formatting args needed
            String::new(), // No file context in parser
            token_span,
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
        self.errors.push(unexpected_token(
            self.peek_token.span(),
            format!("Expected {}, got {}.", expected, self.peek_token.token_type),
        ));
    }

    fn parse_parameter_identifier_or_recover(&mut self) -> Option<String> {
        if self.current_token.token_type == TokenType::Ident {
            return Some(self.current_token.literal.clone());
        }

        self.errors.push(unexpected_token(
            self.current_token.span(),
            format!(
                "Expected identifier as parameter, got {}.",
                self.current_token.token_type
            ),
        ));

        while !matches!(
            self.current_token.token_type,
            TokenType::Comma | TokenType::RParen | TokenType::Eof
        ) {
            self.next_token();
        }

        None
    }
}
