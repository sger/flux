use crate::frontend::{
    Identifier,
    block::Block,
    diagnostics::{
        Diagnostic, EXPECTED_EXPRESSION, UNTERMINATED_BLOCK_COMMENT, UNTERMINATED_STRING,
        compiler_errors::{missing_comma, unexpected_token},
    },
    expression::Expression,
    position::{Position, Span},
    precedence::Precedence,
    token_type::TokenType,
};

use super::Parser;

const LIST_ERROR_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy)]
pub(super) enum SyncMode {
    Expr,
    Stmt,
    Block,
}

impl Parser {
    // Token navigation
    pub(super) fn next_token(&mut self) {
        let mut next = self.lexer.next_token();
        // Skip doc comments - they're lexed but not parsed
        while next.token_type == TokenType::DocComment {
            next = self.lexer.next_token();
        }
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

    pub(super) fn consume_if_peek(&mut self, token_type: TokenType) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            false
        }
    }

    pub(super) fn list_error_limit(&self) -> usize {
        LIST_ERROR_LIMIT
    }

    pub(super) fn list_diag_count_since(&self, diag_start: usize) -> usize {
        self.errors.len().saturating_sub(diag_start)
    }

    pub(super) fn check_list_error_limit(
        &mut self,
        diag_start: usize,
        end: TokenType,
        partial_reason: &str,
    ) -> bool {
        if self.list_diag_count_since(diag_start) < LIST_ERROR_LIMIT {
            return false;
        }

        let span = if self.peek_token.token_type != TokenType::Eof {
            self.peek_token.span()
        } else {
            self.current_token.span()
        };

        self.errors.push(unexpected_token(
            span,
            format!(
                "Too many errors in this {}; stopping after {} errors.",
                partial_reason, LIST_ERROR_LIMIT
            ),
        ));
        self.sync_to_list_end(end);
        true
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

    pub(super) fn synchronize(&mut self, mode: SyncMode) {
        while !self.is_current_token(TokenType::Eof) {
            let token_type = self.current_token.token_type;
            let at_boundary = match mode {
                SyncMode::Expr => matches!(
                    token_type,
                    TokenType::Comma
                        | TokenType::Semicolon
                        | TokenType::RParen
                        | TokenType::RBracket
                        | TokenType::RBrace
                        | TokenType::Arrow
                        | TokenType::Eof
                ),
                SyncMode::Stmt => {
                    matches!(
                        token_type,
                        TokenType::Semicolon | TokenType::RBrace | TokenType::Eof
                    )
                }
                SyncMode::Block => matches!(token_type, TokenType::RBrace | TokenType::Eof),
            };

            if at_boundary {
                break;
            }

            self.next_token();
        }
    }

    // Complex parsing helpers
    pub(super) fn parse_qualified_name(&mut self) -> Option<Identifier> {
        let first_sym = self
            .current_token
            .symbol
            .expect("ident token should have symbol");
        if !self.is_peek_token(TokenType::Dot) {
            return Some(first_sym);
        }

        // Build dotted name, then intern the whole thing
        let mut name = self.current_token.literal.to_string();
        while self.is_peek_token(TokenType::Dot) {
            self.next_token(); // consume '.'
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            name.push('.');
            name.push_str(&self.current_token.literal);
        }
        Some(self.lexer.interner_mut().intern(&name))
    }

    pub(super) fn parse_function_parameters(&mut self) -> Option<Vec<Identifier>> {
        debug_assert!(self.is_current_token(TokenType::LParen));
        let mut identifiers = Vec::new();
        let diag_start = self.errors.len();

        // Empty list: ()
        if self.consume_if_peek(TokenType::RParen) {
            return Some(identifiers);
        }

        loop {
            // Move to parameter candidate token
            self.next_token();

            // Allow trailing comma: fun f(a, ) { ... }
            if self.is_current_token(TokenType::RParen) {
                return Some(identifiers);
            }

            // Parse identifier or recover (recovery stops on Comma/RParen/Eof)
            if let Some(param) = self.parse_parameter_identifier_or_recover() {
                identifiers.push(param);
                self.next_token(); // move to delimiter after a valid parameter
            }
            if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list") {
                return Some(identifiers);
            }
            // else: current_token is already at delimiter due to recovery

            match self.current_token.token_type {
                TokenType::Comma => {
                    // Allow trailing comma in parameter lists.
                    if self.consume_if_peek(TokenType::RParen) {
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
                    if self.check_list_error_limit(diag_start, TokenType::RParen, "parameter list")
                    {
                        return Some(identifiers);
                    }

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
        let mut last_missing_comma_at = None;
        let diag_start = self.errors.len();

        if self.consume_if_peek(end) {
            return Some(list);
        }

        loop {
            self.next_token();

            // Allow trailing comma in list contexts: f(a, ), [a, ], ...
            if self.is_current_token(end) {
                return Some(list);
            }

            if self.is_current_token(TokenType::Comma) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    "Expected expression after `,`, got `,`.",
                ));
                if self.check_list_error_limit(diag_start, end, "list") {
                    return Some(list);
                }
                continue;
            }

            if self.is_current_token(TokenType::Eof) {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!("Expected `{}` before end of file.", end),
                ));
                if self.check_list_error_limit(diag_start, end, "list") {
                    return Some(list);
                }
                return None;
            }

            list.push(self.parse_expression(Precedence::Lowest)?);

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma

                if self.consume_if_peek(end) {
                    return Some(list);
                }

                continue;
            }

            if self.consume_if_peek(end) {
                return Some(list);
            }

            // Adjacent expression-starting token inside a delimited list strongly
            // indicates a missing comma: f(a b), [a b], etc.
            if self.token_starts_expression(self.peek_token.token_type)
                && !self.token_can_continue_expression(self.peek_token.token_type)
            {
                let (context, example) = match end {
                    TokenType::RParen => ("arguments", "`f(a, b)`"),
                    TokenType::RBracket => ("items", "`[a, b]`"),
                    _ => ("items", "`a, b`"),
                };
                let missing_comma_at = self.peek_token.position;
                if last_missing_comma_at != Some(missing_comma_at) {
                    self.errors
                        .push(missing_comma(self.peek_token.span(), context, example));
                    last_missing_comma_at = Some(missing_comma_at);
                    if self.check_list_error_limit(diag_start, end, "list") {
                        return Some(list);
                    }
                }
                // Pretend a comma existed and continue parsing the next list item.
                continue;
            }

            self.errors.push(unexpected_token(
                self.peek_token.span(),
                format!(
                    "Expected `,` or `{}` after list item, got {}.",
                    end, self.peek_token.token_type
                ),
            ));
            if self.check_list_error_limit(diag_start, end, "list") {
                return Some(list);
            }

            self.recover_expression_list_to_delimiter(end);

            if self.is_peek_token(TokenType::Comma) {
                self.next_token();

                if self.consume_if_peek(end) {
                    return Some(list);
                }

                continue;
            }

            if self.consume_if_peek(end) {
                return Some(list);
            }

            let message = if self.peek_token.token_type == TokenType::Eof {
                format!("Expected `{}` before end of file.", end)
            } else {
                format!("Expected `,` or `{}` in expression list.", end)
            };
            self.errors
                .push(unexpected_token(self.peek_token.span(), message));
            if self.check_list_error_limit(diag_start, end, "list") {
                return Some(list);
            }
            return None;
        }
    }

    fn token_can_continue_expression(&self, token_type: TokenType) -> bool {
        matches!(
            token_type,
            TokenType::LParen
                | TokenType::LBracket
                | TokenType::Dot
                | TokenType::Pipe
                | TokenType::Or
                | TokenType::And
                | TokenType::Eq
                | TokenType::NotEq
                | TokenType::Lt
                | TokenType::Gt
                | TokenType::Lte
                | TokenType::Gte
                | TokenType::Plus
                | TokenType::Minus
                | TokenType::Asterisk
                | TokenType::Slash
                | TokenType::Percent
        )
    }

    fn recover_expression_list_to_delimiter(&mut self, end: TokenType) {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        while self.peek_token.token_type != TokenType::Eof {
            let at_top_level = paren_depth == 0 && bracket_depth == 0 && brace_depth == 0;
            let token_type = self.peek_token.token_type;

            if at_top_level && (token_type == TokenType::Comma || token_type == end) {
                break;
            }

            match token_type {
                TokenType::LParen => paren_depth += 1,
                TokenType::LBracket => bracket_depth += 1,
                TokenType::LBrace => brace_depth += 1,
                TokenType::RParen => paren_depth = paren_depth.saturating_sub(1),
                TokenType::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenType::RBrace => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }

            self.next_token();
        }
    }

    pub(super) fn sync_to_list_end(&mut self, end: TokenType) {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        while self.peek_token.token_type != TokenType::Eof {
            let at_top_level = paren_depth == 0 && bracket_depth == 0 && brace_depth == 0;
            let token_type = self.peek_token.token_type;

            if at_top_level && token_type == end {
                self.next_token(); // consume end delimiter
                return;
            }

            match token_type {
                TokenType::LParen => paren_depth += 1,
                TokenType::LBracket => bracket_depth += 1,
                TokenType::LBrace => brace_depth += 1,
                TokenType::RParen => paren_depth = paren_depth.saturating_sub(1),
                TokenType::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenType::RBrace => brace_depth = brace_depth.saturating_sub(1),
                _ => {}
            }

            self.next_token();
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

    pub(super) fn unterminated_block_comment_error(&mut self) {
        let token_span = self.current_token.span();
        let error_spec = &UNTERMINATED_BLOCK_COMMENT;
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
        self.synchronize(SyncMode::Stmt);
    }

    pub(super) fn peek_error(&mut self, expected: TokenType) {
        self.errors.push(unexpected_token(
            self.peek_token.span(),
            format!("Expected {}, got {}.", expected, self.peek_token.token_type),
        ));
    }

    pub(super) fn validate_parameter_identifier(&mut self) -> Option<Identifier> {
        if self.current_token.token_type == TokenType::Ident {
            Some(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            )
        } else {
            self.errors.push(unexpected_token(
                self.current_token.span(),
                format!(
                    "Expected identifier as parameter, got {}.",
                    self.current_token.token_type
                ),
            ));
            None
        }
    }

    fn parse_parameter_identifier_or_recover(&mut self) -> Option<Identifier> {
        if let Some(identifier) = self.validate_parameter_identifier() {
            return Some(identifier);
        }

        while !matches!(
            self.current_token.token_type,
            TokenType::Comma | TokenType::RParen | TokenType::Eof
        ) {
            self.next_token();
        }

        None
    }
}
