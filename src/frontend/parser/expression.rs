use crate::frontend::{
    block::Block,
    diagnostics::{
        DiagnosticBuilder,
        compiler_errors::{
            invalid_pattern, lambda_syntax_error, missing_comma, pipe_target_error,
            unexpected_token,
        },
    },
    expression::{Expression, MatchArm, Pattern},
    position::{Position, Span},
    precedence::{Fixity, Precedence, infix_op, postfix_op, prefix_op, rhs_precedence_for_infix},
    statement::Statement,
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    fn parse_parenthesized<T>(
        &mut self,
        mut parse_inner: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<T> {
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }
        self.next_token();
        let inner = parse_inner(self)?;
        if !self.expect_peek(TokenType::RParen) {
            return None;
        }
        Some(inner)
    }

    fn build_match_expression(
        &self,
        start: Position,
        scrutinee: Expression,
        arms: Vec<MatchArm>,
    ) -> Expression {
        Expression::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: Span::new(start, self.current_token.end_position),
        }
    }

    fn emit_match_semicolon_separator_diagnostic(&mut self, diag_start: usize) -> bool {
        self.errors.push(unexpected_token(
            self.peek_token.span(),
            "Match arms must be separated by `,`, not `;`.",
        ));
        self.check_list_error_limit(diag_start, TokenType::RBrace, "match arm list")
    }

    fn emit_match_eof_diagnostic(&mut self, diag_start: usize) -> bool {
        self.errors.push(unexpected_token(
            self.peek_token.span(),
            "Expected `}` to close match expression before end of file.",
        ));
        self.check_list_error_limit(diag_start, TokenType::RBrace, "match arm list")
    }

    // Core expression parsing
    pub(super) fn parse_expression(&mut self, precedence: Precedence) -> Option<Expression> {
        let mut left = self.parse_prefix()?;

        while !self.is_expression_terminator(self.peek_token.token_type) {
            let peek_precedence = if let Some(peek_info) = postfix_op(&self.peek_token.token_type) {
                peek_info.precedence
            } else if let Some(peek_info) = infix_op(&self.peek_token.token_type) {
                peek_info.precedence
            } else {
                break;
            };

            if precedence >= peek_precedence {
                break;
            }

            self.next_token();
            left = self.parse_infix(left)?;
        }

        Some(left)
    }

    pub(super) fn parse_prefix(&mut self) -> Option<Expression> {
        match &self.current_token.token_type {
            TokenType::Ident => self.parse_identifier(),
            TokenType::Int => self.parse_integer(),
            TokenType::Float => self.parse_float(),
            TokenType::String => self.parse_string(),
            TokenType::UnterminatedString => {
                let should_suppress = self
                    .suppress_unterminated_string_error_at
                    .take()
                    .is_some_and(|pos| pos == self.current_token.position);
                if !should_suppress {
                    self.unterminated_string_error();
                }
                None
            }
            TokenType::UnterminatedBlockComment => {
                self.unterminated_block_comment_error();
                None
            }
            TokenType::InterpolationStart => self.parse_interpolation_start(),
            TokenType::True | TokenType::False => self.parse_boolean(),
            TokenType::None => self.parse_none(),
            TokenType::Some => self.parse_some(),
            TokenType::Left => self.parse_left(),
            TokenType::Right => self.parse_right(),
            TokenType::Match => self.parse_match_expression(),
            TokenType::LParen => self.parse_grouped_expression(),
            TokenType::LBracket => self.parse_array(),
            TokenType::LBrace => self.parse_hash(),
            TokenType::If => self.parse_if_expression(),
            TokenType::Fun => self.parse_function_literal(),
            TokenType::Backslash => self.parse_lambda(),
            token if prefix_op(token).is_some() => self.parse_prefix_expression(),
            _ => {
                self.no_prefix_parse_error();
                None
            }
        }
    }

    pub(super) fn parse_infix(&mut self, left: Expression) -> Option<Expression> {
        match self.current_token.token_type {
            // These are parsed as postfix/special forms rather than generic infix nodes.
            TokenType::LParen => self.parse_call_expression(left),
            TokenType::LBracket => self.parse_index_expression(left),
            TokenType::Dot => self.parse_member_access(left),
            TokenType::Pipe => self.parse_pipe_expression(left),
            _ if infix_op(&self.current_token.token_type).is_some() => {
                self.parse_infix_expression(left)
            }
            _ => Some(left),
        }
    }

    // Infix expressions
    pub(super) fn parse_infix_expression(&mut self, left: Expression) -> Option<Expression> {
        let token_type = self.current_token.token_type;
        let op_info = match infix_op(&token_type) {
            Some(info) => info,
            None => {
                debug_assert!(
                    false,
                    "generic infix parse attempted without registry metadata for {:?}",
                    token_type
                );
                return None;
            }
        };
        debug_assert!(
            op_info.fixity == Fixity::Infix,
            "generic infix parse expected fixity Infix for {:?}, got {:?}",
            token_type,
            op_info.fixity
        );

        let operator = self.current_token.literal.to_string();
        let right_precedence = match rhs_precedence_for_infix(&token_type) {
            Some(precedence) => precedence,
            None => {
                debug_assert!(
                    false,
                    "missing rhs precedence for generic infix operator {:?}",
                    token_type
                );
                return None;
            }
        };
        let start = left.span().start;
        self.next_token();
        let right = self.parse_expression(right_precedence)?;
        let end = right.span().end;
        Some(Expression::Infix {
            left: Box::new(left),
            operator,
            right: Box::new(right),
            span: Span::new(start, end),
        })
    }

    // Pipe operator: a |> f(b, c) transforms to f(a, b, c)
    pub(super) fn parse_pipe_expression(&mut self, left: Expression) -> Option<Expression> {
        let start = left.span().start;
        let right_precedence = match rhs_precedence_for_infix(&self.current_token.token_type) {
            Some(precedence) => precedence,
            None => {
                debug_assert!(false, "missing rhs precedence metadata for pipe operator");
                Precedence::Pipe
            }
        };
        self.next_token(); // consume |>

        // Parse the right side - could be identifier or call
        let right = self.parse_expression(right_precedence)?;

        // Transform based on what we got
        match right {
            // a |> f => f(a)
            Expression::Identifier { name, span } => Some(Expression::Call {
                function: Box::new(Expression::Identifier { name, span }),
                arguments: vec![left],
                span: Span::new(start, span.end),
            }),
            // a |> Module.func => Module.func(a)
            Expression::MemberAccess {
                object,
                member,
                span,
            } => Some(Expression::Call {
                function: Box::new(Expression::MemberAccess {
                    object,
                    member,
                    span,
                }),
                arguments: vec![left],
                span: Span::new(start, span.end),
            }),
            // a |> f(b, c) => f(a, b, c)
            Expression::Call {
                function,
                mut arguments,
                span,
            } => {
                arguments.insert(0, left);
                Some(Expression::Call {
                    function,
                    arguments,
                    span: Span::new(start, span.end),
                })
            }
            _ => {
                self.errors
                    .push(pipe_target_error(self.current_token.span()));
                None
            }
        }
    }

    pub(super) fn parse_call_expression(&mut self, function: Expression) -> Option<Expression> {
        let start = function.span().start;
        let arguments = self.parse_expression_list(TokenType::RParen)?;
        Some(Expression::Call {
            function: Box::new(function),
            arguments,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_index_expression(&mut self, left: Expression) -> Option<Expression> {
        let start = left.span().start;
        self.next_token();
        let index = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::RBracket) {
            return None;
        }

        Some(Expression::Index {
            left: Box::new(left),
            index: Box::new(index),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_member_access(&mut self, object: Expression) -> Option<Expression> {
        let start = object.span().start;
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let member = self
            .current_token
            .symbol
            .expect("ident token should have symbol");

        Some(Expression::MemberAccess {
            object: Box::new(object),
            member,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    // Prefix expressions
    pub(super) fn parse_prefix_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let operator = self.current_token.literal.to_string();
        let token_type = self.current_token.token_type;
        let precedence = match prefix_op(&token_type) {
            Some(info) => info.precedence,
            None => {
                debug_assert!(
                    false,
                    "prefix parse attempted without registry metadata for {:?}",
                    token_type
                );
                return None;
            }
        };

        self.next_token();
        let right = self.parse_expression(precedence)?;
        let end = right.span().end;
        Some(Expression::Prefix {
            operator,
            right: Box::new(right),
            span: Span::new(start, end),
        })
    }

    pub(super) fn parse_grouped_expression(&mut self) -> Option<Expression> {
        self.next_token();
        let expression = self.parse_expression(Precedence::Lowest)?;

        // Tuple-like input "(a b)" is a common missing-comma error. Flux currently
        // treats parenthesized forms as grouped expressions; recover to ')' to avoid
        // cascading diagnostics and keep parsing subsequent statements.
        if self.token_starts_expression(self.peek_token.token_type) {
            self.errors
                .push(missing_comma(self.peek_token.span(), "items", "`(a, b)`"));

            // Recover to the matching ')' of this group. If this group is malformed
            // and likely belongs to a larger statement (for example `if (cond { ...`),
            // stop at top-level statement boundaries to avoid consuming following code.
            let mut nested_parens = 0usize;
            while self.peek_token.token_type != TokenType::Eof {
                if nested_parens == 0
                    && matches!(
                        self.peek_token.token_type,
                        TokenType::Semicolon | TokenType::RBrace | TokenType::LBrace
                    )
                {
                    break;
                }
                match self.peek_token.token_type {
                    TokenType::LParen => {
                        nested_parens += 1;
                        self.next_token();
                    }
                    TokenType::RParen => {
                        if nested_parens == 0 {
                            break;
                        }
                        nested_parens -= 1;
                        self.next_token();
                    }
                    _ => self.next_token(),
                }
            }
        }

        if !self.expect_peek(TokenType::RParen) {
            return None;
        }
        Some(expression)
    }

    // Collections
    pub(super) fn parse_array(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let elements = self.parse_expression_list(TokenType::RBracket)?;
        Some(Expression::Array {
            elements,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_hash(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let mut pairs = Vec::new();

        while !self.is_peek_token(TokenType::RBrace) {
            self.next_token();
            let key = self.parse_expression(Precedence::Lowest)?;

            if !self.expect_peek(TokenType::Colon) {
                return None;
            }

            self.next_token();

            let value = self.parse_expression(Precedence::Lowest)?;

            pairs.push((key, value));

            if !self.is_peek_token(TokenType::RBrace) && !self.expect_peek(TokenType::Comma) {
                return None;
            }
        }

        if !self.expect_peek(TokenType::RBrace) {
            return None;
        }

        Some(Expression::Hash {
            pairs,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    // Complex expressions
    pub(super) fn parse_if_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        self.next_token();
        let condition = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let consequence = self.parse_block();

        let alternative = if self.is_peek_token(TokenType::Else) {
            self.next_token();

            if !self.expect_peek(TokenType::LBrace) {
                return None;
            };
            Some(self.parse_block())
        } else {
            None
        };

        Some(Expression::If {
            condition: Box::new(condition),
            consequence,
            alternative,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_match_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        self.next_token();
        let scrutinee = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let mut arms = Vec::new();
        let diag_start = self.errors.len();

        while !self.is_peek_token(TokenType::RBrace) {
            self.next_token();
            let pattern = self.parse_pattern()?;
            let mut guard = None;

            if self.is_peek_token(TokenType::If) {
                self.next_token(); // consume `if`
                self.next_token(); // move to guard expression start
                guard = Some(self.parse_expression(Precedence::Lowest)?);
            }

            if !self.expect_peek(TokenType::Arrow) {
                return None;
            }

            self.next_token();
            let body = self.parse_expression(Precedence::Lowest)?;

            let span = Span::new(pattern.span().start, body.span().end);
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span,
            });

            match self.peek_token.token_type {
                TokenType::Comma => {
                    self.next_token();
                }
                TokenType::RBrace => {}
                TokenType::Semicolon => {
                    if self.emit_match_semicolon_separator_diagnostic(diag_start) {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }
                    // Recover by treating `;` as a comma separator.
                    self.next_token();
                }
                TokenType::Eof => {
                    if self.emit_match_eof_diagnostic(diag_start) {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }
                    return Some(self.build_match_expression(start, scrutinee, arms));
                }
                _ => {
                    self.errors.push(unexpected_token(
                        self.peek_token.span(),
                        format!(
                            "Expected `,` or `}}` after match arm, got {}.",
                            self.peek_token.token_type
                        ),
                    ));
                    if self.check_list_error_limit(diag_start, TokenType::RBrace, "match arm list")
                    {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }

                    while !matches!(
                        self.peek_token.token_type,
                        TokenType::Comma
                            | TokenType::Semicolon
                            | TokenType::RBrace
                            | TokenType::Eof
                    ) {
                        self.next_token();
                    }

                    match self.peek_token.token_type {
                        TokenType::Comma => {
                            self.next_token();
                        }
                        TokenType::Semicolon => {
                            if self.emit_match_semicolon_separator_diagnostic(diag_start) {
                                return Some(self.build_match_expression(start, scrutinee, arms));
                            }
                            self.next_token();
                        }
                        TokenType::RBrace => {}
                        TokenType::Eof => {
                            if self.emit_match_eof_diagnostic(diag_start) {
                                return Some(self.build_match_expression(start, scrutinee, arms));
                            }
                            return Some(self.build_match_expression(start, scrutinee, arms));
                        }
                        _ => {}
                    }
                }
            }
        }

        if !self.expect_peek(TokenType::RBrace) {
            return None;
        }

        Some(self.build_match_expression(start, scrutinee, arms))
    }

    pub(super) fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current_token.position;
        match &self.current_token.token_type {
            TokenType::Ident if self.current_token.literal == "_" => Some(Pattern::Wildcard {
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::Ident => Some(Pattern::Identifier {
                name: self
                    .current_token
                    .symbol
                    .expect("ident token should have symbol"),
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::None => Some(Pattern::None {
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::Some => {
                let inner_pattern = self.parse_parenthesized(|parser| parser.parse_pattern())?;
                Some(Pattern::Some {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Left => {
                let inner_pattern = self.parse_parenthesized(|parser| parser.parse_pattern())?;
                Some(Pattern::Left {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Right => {
                let inner_pattern = self.parse_parenthesized(|parser| parser.parse_pattern())?;
                Some(Pattern::Right {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Int
            | TokenType::Float
            | TokenType::String
            | TokenType::True
            | TokenType::False => {
                let expr = self.parse_prefix()?;
                let span = expr.span();
                Some(Pattern::Literal {
                    expression: expr,
                    span,
                })
            }
            _ => {
                self.errors.push(invalid_pattern(
                    self.current_token.span(),
                    &self.current_token.token_type.to_string(),
                ));
                None
            }
        }
    }

    pub(super) fn parse_function_literal(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        let parameters = self.parse_function_parameters()?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let body = self.parse_block();
        Some(Expression::Function {
            parameters,
            body,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parse a lambda expression: \x -> expr, \(x, y) -> expr, \() -> expr
    pub(super) fn parse_lambda(&mut self) -> Option<Expression> {
        debug_assert!(self.is_current_token(TokenType::Backslash));
        let start = self.current_token.position;

        // Consume `\` and position on the first parameter token or `(`.
        self.next_token();

        // Parse parameters
        let parameters = if self.is_current_token(TokenType::LParen) {
            // Parenthesized parameters: \() -> or \(x) -> or \(x, y) ->
            self.parse_function_parameters()?
        } else if self.is_current_token(TokenType::Arrow) {
            self.errors.push(lambda_syntax_error(
                self.current_token.span(),
                "Expected parameter or `(` after `\\`.",
            ));
            return None;
        } else {
            // Single unparenthesized parameter: \x ->
            // Validate using the same identifier checks used by parenthesized
            // parameter lists to keep diagnostics consistent.
            let mut params = Vec::new();
            if let Some(param) = self.validate_parameter_identifier() {
                params.push(param);
            }
            params
        };

        // Expect ->
        if !self.is_peek_token(TokenType::Arrow) {
            self.errors.push(
                lambda_syntax_error(
                    self.peek_token.span(),
                    format!(
                        "Expected `->` after lambda parameters, got `{}`.",
                        self.peek_token.token_type
                    ),
                )
                .with_example("let add = \\(a, b) -> a + b;"),
            );
            return None;
        }
        self.next_token(); // consume `->`
        self.next_token(); // move to lambda body

        // Parse body
        let body = if self.is_current_token(TokenType::LBrace) {
            // Block body: \x -> { statements }
            self.parse_block()
        } else {
            // Expression body: \x -> expr
            let expr_start = self.current_token.position;
            let expr = self.parse_expression(Precedence::Lowest)?;
            let expr_span = expr.span();
            Block {
                statements: vec![Statement::Expression {
                    expression: expr,
                    span: Span::new(expr_start, self.current_token.end_position),
                }],
                span: expr_span,
            }
        };

        Some(Expression::Function {
            parameters,
            body,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    // Option/Either expressions
    pub(super) fn parse_none(&self) -> Option<Expression> {
        let start = self.current_token.position;
        Some(Expression::None {
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_some(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let value =
            self.parse_parenthesized(|parser| parser.parse_expression(Precedence::Lowest))?;
        Some(Expression::Some {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_left(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let value =
            self.parse_parenthesized(|parser| parser.parse_expression(Precedence::Lowest))?;

        Some(Expression::Left {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_right(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let value =
            self.parse_parenthesized(|parser| parser.parse_expression(Precedence::Lowest))?;

        Some(Expression::Right {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }
}
