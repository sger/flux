use crate::{
    diagnostics::{DiagnosticBuilder, unexpected_token, unknown_keyword},
    syntax::{
        data_variant::DataVariant, effect_ops::EffectOp, precedence::Precedence,
        statement::Statement, token_type::TokenType,
    },
};

use super::{Parser, helpers::SyncMode};

impl Parser {
    fn looks_like_text_block_juxtaposition(&self) -> bool {
        if self.current_token.token_type != TokenType::Ident
            || self.peek_token.token_type != TokenType::Ident
        {
            return false;
        }

        // Strong signal: at least three bare words on one line (`Roses are red`).
        self.peek2_token.token_type == TokenType::Ident
            && self.peek2_token.position.line == self.peek_token.position.line
    }

    fn build_juxtaposed_identifier_error(
        &self,
        ident_name: &str,
        span: crate::diagnostics::position::Span,
    ) -> crate::diagnostics::Diagnostic {
        let mut diag = unexpected_token(
            span,
            format!("Unexpected identifier `{ident_name}` after expression."),
        )
        .with_hint_text("If you intended a function call, use parentheses: `f(x)`.")
        .with_hint_text("If you intended separate expressions, add an operator or a separator.");

        if self.looks_like_text_block_juxtaposition() {
            diag = diag.with_hint_text(
                "If this is text, wrap lines in quotes or use multiline strings with triple quotes: `\"\"\"...\"\"\"`.",
            );
        }

        diag
    }

    pub(super) fn parse_statement(&mut self) -> Option<Statement> {
        let statement = match self.current_token.token_type {
            TokenType::Module => self.parse_module_statement(),
            TokenType::Import => self.parse_import_statement(),
            TokenType::Let => self.parse_let_statement(),
            TokenType::Return => self.parse_return_statement(),
            TokenType::Type => self.parse_type_adt_statement(),
            TokenType::Data => self.parse_data_statement(),
            TokenType::Effect => self.parse_effect_statement(),
            TokenType::Fn if self.is_peek_token(TokenType::Ident) => {
                self.parse_function_statement(false)
            }
            TokenType::Public if self.is_peek_token(TokenType::Fn) => {
                self.next_token(); // fn
                self.parse_function_statement(true)
            }
            TokenType::Ident if self.current_token.literal == "fn" => {
                // Defensive path: `fn` should lex as TokenType::Fn.
                None
            }
            TokenType::Ident
                if self.current_token.literal != "fn"
                    && (self.current_token.literal.starts_with("fn")
                        || self.current_token.literal.starts_with("fun"))
                    && self.is_peek_token(TokenType::Ident) =>
            {
                self.errors.push(
                    unknown_keyword(self.current_token.span(), &self.current_token.literal, None)
                        .with_message(format!(
                            "Unknown keyword `{}`. Flux uses `fn` for function declarations.",
                            self.current_token.literal
                        ))
                        .with_hint_text("Did you mean `fn`?"),
                );
                None
            }

            // Check if we have `identifier = expression` (reassignment without 'let')
            TokenType::Ident if self.is_peek_token(TokenType::Assign) => {
                self.parse_assignment_statement()
            }
            _ => self.parse_expression_statement(),
        };

        if statement.is_none() {
            self.synchronize(SyncMode::Stmt);
        }

        statement
    }

    pub(super) fn parse_expression_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        let expression = match self.parse_expression(Precedence::Lowest) {
            Some(expression) => expression,
            None => {
                self.synchronize(SyncMode::Expr);
                return None;
            }
        };

        // Detect juxtaposed identifiers on the same line: `foo bar` with no operator.
        // In Flux, function calls require parentheses, so two bare identifiers on the
        // same line without a separator is always a parse error. Emitting the error here
        // (at the expression start) rather than at the downstream failure token prevents
        // cascade errors — e.g. "Expected expression, found ," several tokens later.
        if self.peek_token.token_type == TokenType::Ident
            && self.peek_token.position.line == self.current_token.end_position.line
        {
            let ident_name = self.peek_token.literal.to_string();
            let error_span = self.span_from(start);
            self.errors
                .push(self.build_juxtaposed_identifier_error(&ident_name, error_span));
            // Skip the current line AND any subsequent lines that also consist of
            // bare identifier sequences (e.g. lines of prose inside a `""` that should
            // have been `"""`). This emits a single error for the whole group instead
            // of one per line.
            let mut skip_line = self.current_token.end_position.line;
            loop {
                // Advance past everything remaining on `skip_line`.
                while !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.position.line == skip_line
                {
                    self.next_token();
                }
                // If the next line also opens with two consecutive identifiers on the
                // same line, it is part of the same erroneous block — skip it silently.
                if !self.is_peek_token(TokenType::Eof)
                    && self.peek_token.token_type == TokenType::Ident
                    && self.peek2_token.token_type == TokenType::Ident
                    && self.peek2_token.position.line == self.peek_token.position.line
                {
                    skip_line = self.peek_token.position.line;
                    self.next_token(); // step onto the next line
                } else {
                    break;
                }
            }
            // Return Some so parse_statement does not call synchronize() and consume
            // valid code on subsequent lines.
            return Some(Statement::Expression {
                expression,
                has_semicolon: false,
                span: self.span_from(start),
            });
        }

        let has_semicolon = if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
            true
        } else {
            false
        };

        Some(Statement::Expression {
            expression,
            has_semicolon,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_function_statement(&mut self, is_public: bool) -> Option<Statement> {
        let start = if is_public {
            self.current_token.position
        } else {
            self.current_token.position
        };

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // Optional generic type parameters <T, U, ...>
        let mut type_params = Vec::new();
        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume '<'

            loop {
                if !self.expect_peek(TokenType::Ident) {
                    return None;
                }
                type_params.push(
                    self.current_token
                        .symbol
                        .expect("ident token should have symbol"),
                );
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume ','
                } else {
                    break;
                }
            }
            if !self.expect_peek(TokenType::Gt) {
                return None;
            }
        }

        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        let (parameters, parameter_types) = self.parse_typed_function_parameters()?;

        let return_type = if self.is_peek_token(TokenType::Arrow) {
            self.next_token(); // ->
            self.next_token(); // start of return type
            self.parse_type_expr()
        } else {
            None
        };

        let effects = self.parse_effect_list()?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let body = self.parse_block();

        Some(Statement::Function {
            is_public,
            name,
            type_params,
            parameters,
            parameter_types,
            return_type,
            effects,
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
            match self.parse_expression(Precedence::Lowest) {
                Some(expression) => Some(expression),
                None => {
                    self.synchronize(SyncMode::Stmt);
                    return None;
                }
            }
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

        if self.is_peek_token(TokenType::LParen) {
            self.next_token(); // consume '(' so parse_pattern sees tuple pattern start
            let pattern = self.parse_pattern()?;

            if !self.expect_peek(TokenType::Assign) {
                return None;
            }

            self.next_token();
            let value = match self.parse_expression(Precedence::Lowest) {
                Some(expression) => expression,
                None => {
                    self.synchronize(SyncMode::Stmt);
                    return None;
                }
            };

            if self.is_peek_token(TokenType::Semicolon) {
                self.next_token();
            }

            return Some(Statement::LetDestructure {
                pattern,
                value,
                span: self.span_from(start),
            });
        }

        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        let type_annotation = if self.is_peek_token(TokenType::Colon) {
            self.next_token(); // :
            self.next_token(); // type start
            self.parse_type_expr()
        } else {
            None
        };

        if !self.expect_peek(TokenType::Assign) {
            return None;
        }

        self.next_token();

        let value = match self.parse_expression(Precedence::Lowest) {
            Some(expression) => expression,
            None => {
                self.synchronize(SyncMode::Stmt);
                return None;
            }
        };

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Let {
            name,
            type_annotation,
            value,
            span: self.span_from(start),
        })
    }

    pub(super) fn parse_assignment_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        if !self.expect_peek(TokenType::Assign) {
            return None;
        }

        self.next_token();

        let value = match self.parse_expression(Precedence::Lowest) {
            Some(expression) => expression,
            None => {
                self.synchronize(SyncMode::Stmt);
                return None;
            }
        };

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
        let mut except = Vec::new();

        if self.is_peek_token(TokenType::As) {
            self.next_token(); // consume 'as'
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            alias = Some(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            );
        }

        if self.peek_token.token_type == TokenType::Ident && self.peek_token.literal == "except" {
            self.next_token(); // consume `except`
            except = self.parse_import_except_list()?;
        }

        // No semicolon required for import statements

        Some(Statement::Import {
            name,
            alias,
            except,
            span: self.span_from(start),
        })
    }

    fn parse_import_except_list(&mut self) -> Option<Vec<crate::syntax::Identifier>> {
        if !self.expect_peek(TokenType::LBracket) {
            return None;
        }

        let mut names = Vec::new();
        if self.is_peek_token(TokenType::RBracket) {
            self.next_token();
            return Some(names);
        }

        loop {
            if !self.expect_peek(TokenType::Ident) {
                return None;
            }
            names.push(
                self.current_token
                    .symbol
                    .expect("ident token should have symbol"),
            );

            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma
                continue;
            }

            if self.is_peek_token(TokenType::RBracket) {
                self.next_token(); // consume closing bracket
                break;
            }

            self.errors.push(unexpected_token(
                self.peek_token.span(),
                format!(
                    "Expected `,` or `]` in import except list, got {}.",
                    self.peek_token.token_type
                ),
            ));
            return None;
        }

        Some(names)
    }

    /// Parses a `data` declaration with optional type parameters and constructor
    /// variants, for example `data Option<T> { Some(T), None }`.
    pub(super) fn parse_data_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        // current: 'data' — advance to type name
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // Optional generic type parameters <T, U, ...> (parsed but stored only as names for now)
        let mut type_params = Vec::new();

        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume '<'
            loop {
                if !self.expect_peek(TokenType::Ident) {
                    return None;
                }
                type_params.push(
                    self.current_token
                        .symbol
                        .expect("ident token should have symbol"),
                );
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume ','
                } else {
                    break;
                }
            }
            if !self.expect_peek(TokenType::Gt) {
                return None;
            }
        }

        // Expect opening brace
        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }
        self.next_token(); // move past '{'

        let mut variants = Vec::new();

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            let var_start = self.current_token.position;

            if self.current_token.token_type != TokenType::Ident {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "Expected constructor name in `data` declaration, got {}",
                        self.current_token.token_type
                    ),
                ));
                return None;
            }

            let var_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            // Optional field types: VariantName(Type1, Type2, ...)
            let mut fields = Vec::new();

            if self.is_peek_token(TokenType::LParen) {
                self.next_token(); // advance to '('
                // Empty parens: Variant() — skip past ')'
                if !self.consume_if_peek(TokenType::RParen) {
                    self.next_token(); // move to start of first type

                    loop {
                        if self.is_current_token(TokenType::RParen)
                            || self.is_current_token(TokenType::Eof)
                        {
                            break;
                        }
                        let Some(ty) = self.parse_type_expr() else {
                            return None;
                        };
                        fields.push(ty);
                        // parse_type_expr leaves current at last token of type; advance past it
                        self.next_token();
                        match self.current_token.token_type {
                            TokenType::Comma => {
                                self.next_token(); // move to start of next type
                            }
                            TokenType::RParen | TokenType::Eof => break,
                            _ => {
                                self.errors.push(unexpected_token(
                                    self.current_token.span(),
                                    format!(
                                        "Expected `,` or `)` in constructor fields, got {}.",
                                        self.current_token.token_type
                                    ),
                                ));
                                return None;
                            }
                        }
                    }
                }

                // current_token is '(' after consume_if_peek or ')' after the loop;
                // in both cases we need current to be ')' to leave this block correctly.
            }

            variants.push(DataVariant {
                name: var_name,
                fields,
                span: self.span_from(var_start),
            });

            // Optional trailing comma between variants
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume ','
            }
            self.next_token(); // advance to next variant or '}'
        }

        Some(Statement::Data {
            name,
            type_params,
            variants,
            span: self.span_from(start),
        })
    }

    /// Parses ADT sugar:
    /// `type Result<T, E> = Ok(T) | Err(E)`
    ///
    /// This desugars directly into `Statement::Data`.
    pub(super) fn parse_type_adt_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        // current: 'type' — advance to type name
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // Optional generic type parameters <T, U, ...>
        let mut type_params = Vec::new();
        if self.is_peek_token(TokenType::Lt) {
            self.next_token(); // consume '<'
            loop {
                if !self.expect_peek(TokenType::Ident) {
                    return None;
                }
                type_params.push(
                    self.current_token
                        .symbol
                        .expect("ident token should have symbol"),
                );
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token(); // consume ','
                } else {
                    break;
                }
            }
            if !self.expect_peek(TokenType::Gt) {
                return None;
            }
        }

        // Expect '='
        if !self.expect_peek(TokenType::Assign) {
            self.errors.push(unexpected_token(
                self.peek_token.span(),
                "Expected `=` after `type` declaration name.".to_string(),
            ));
            return None;
        }
        self.next_token(); // move past '=' to first constructor token

        let mut variants = Vec::new();
        loop {
            let var_start = self.current_token.position;
            if self.current_token.token_type != TokenType::Ident {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "Expected constructor name in `type` declaration, got {}",
                        self.current_token.token_type
                    ),
                ));
                return None;
            }
            let var_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            let mut fields = Vec::new();
            if self.is_peek_token(TokenType::LParen) {
                self.next_token(); // move to '('
                // Empty Variant()
                if !self.consume_if_peek(TokenType::RParen) {
                    self.next_token(); // first field type
                    loop {
                        if self.is_current_token(TokenType::RParen)
                            || self.is_current_token(TokenType::Eof)
                        {
                            break;
                        }
                        let Some(ty) = self.parse_type_expr() else {
                            return None;
                        };
                        fields.push(ty);
                        self.next_token(); // past field type
                        match self.current_token.token_type {
                            TokenType::Comma => self.next_token(),
                            TokenType::RParen | TokenType::Eof => break,
                            _ => {
                                self.errors.push(unexpected_token(
                                    self.current_token.span(),
                                    format!(
                                        "Expected `,` or `)` in constructor fields, got {}.",
                                        self.current_token.token_type
                                    ),
                                ));
                                return None;
                            }
                        }
                    }
                }
            }

            variants.push(DataVariant {
                name: var_name,
                fields,
                span: self.span_from(var_start),
            });

            // Continue on '|', stop at statement end.
            if self.is_peek_token(TokenType::Bar) {
                self.next_token(); // consume '|'
                self.next_token(); // next constructor start
                continue;
            }
            if self.is_peek_token(TokenType::Semicolon) {
                self.next_token(); // consume ';'
            }
            break;
        }

        Some(Statement::Data {
            name,
            type_params,
            variants,
            span: self.span_from(start),
        })
    }

    /// Parses `effect Name { op: TypeExpr, ... }`.
    /// current_token is `effect` on entry.
    pub(super) fn parse_effect_statement(&mut self) -> Option<Statement> {
        let start = self.current_token.position;

        // Effect name
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }
        let name = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        // `{`
        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }
        self.next_token(); // move past `{`

        let mut ops: Vec<EffectOp> = Vec::new();

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            // Optional `fn` keyword prefix (e.g. `fn print: String -> ()`)
            if self.is_current_token(TokenType::Fn) {
                self.next_token(); // skip `fn`
            }

            if self.current_token.token_type != TokenType::Ident {
                self.errors.push(unexpected_token(
                    self.current_token.span(),
                    format!(
                        "Expected operation name in `effect` declaration, got {}.",
                        self.current_token.token_type
                    ),
                ));
                return None;
            }
            let op_start = self.current_token.position;
            let op_name = self
                .current_token
                .symbol
                .expect("ident token should have symbol");

            // `:` before the type expression
            if !self.expect_peek(TokenType::Colon) {
                return None;
            }
            self.next_token(); // move to start of TypeExpr

            let type_expr = self.parse_type_expr()?;
            let op_end = self.current_token.span().end;

            ops.push(EffectOp {
                name: op_name,
                type_expr,
                span: crate::diagnostics::position::Span::new(op_start, op_end),
            });

            // Optional comma or newline separator
            if self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume ','
            }
            self.next_token(); // advance to next op or `}`
        }

        Some(Statement::EffectDecl {
            name,
            ops,
            span: self.span_from(start),
        })
    }
}
