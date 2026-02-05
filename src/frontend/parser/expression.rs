use crate::frontend::{
    block::Block,
    diagnostics::{Diagnostic, ErrorType},
    expression::{Expression, MatchArm, Pattern},
    position::Span,
    precedence::Precedence,
    statement::Statement,
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    // Core expression parsing
    pub(super) fn parse_expression(&mut self, precedence: Precedence) -> Option<Expression> {
        let mut left = self.parse_prefix()?;

        while !self.is_expression_terminator(self.peek_token.token_type)
            && precedence < self.peek_precedence()
        {
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
            TokenType::InterpolationStart => self.parse_interpolation_start(),
            TokenType::True | TokenType::False => self.parse_boolean(),
            TokenType::None => self.parse_none(),
            TokenType::Some => self.parse_some(),
            TokenType::Left => self.parse_left(),
            TokenType::Right => self.parse_right(),
            TokenType::Match => self.parse_match_expression(),
            TokenType::Bang | TokenType::Minus => self.parse_prefix_expression(),
            TokenType::LParen => self.parse_grouped_expression(),
            TokenType::LBracket => self.parse_array(),
            TokenType::LBrace => self.parse_hash(),
            TokenType::If => self.parse_if_expression(),
            TokenType::Fun => self.parse_function_literal(),
            TokenType::Backslash => self.parse_lambda(),
            _ => {
                self.no_prefix_parse_error();
                None
            }
        }
    }

    pub(super) fn parse_infix(&mut self, left: Expression) -> Option<Expression> {
        match self.current_token.token_type {
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Asterisk
            | TokenType::Slash
            | TokenType::Percent
            | TokenType::Lt
            | TokenType::Gt
            | TokenType::Lte
            | TokenType::Gte
            | TokenType::Eq
            | TokenType::NotEq
            | TokenType::And
            | TokenType::Or => self.parse_infix_expression(left),
            TokenType::Pipe => self.parse_pipe_expression(left),
            TokenType::LParen => self.parse_call_expression(left),
            TokenType::LBracket => self.parse_index_expression(left),
            TokenType::Dot => self.parse_member_access(left),
            _ => Some(left),
        }
    }

    // Infix expressions
    pub(super) fn parse_infix_expression(&mut self, left: Expression) -> Option<Expression> {
        let operator = self.current_token.literal.clone();
        let precedence = self.current_precedence();
        let start = left.span().start;
        self.next_token();
        let right = self.parse_expression(precedence)?;
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
        let precedence = self.current_precedence();
        self.next_token(); // consume |>

        // Parse the right side - could be identifier or call
        let right = self.parse_expression(precedence)?;

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
                self.errors.push(
                    Diagnostic::error("INVALID PIPE TARGET")
                        .with_code("E103")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message("Pipe operator expects a function or function call.")
                        .with_hint_text("Use `value |> func` or `value |> func(arg)`"),
                );
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

        let member = self.current_token.literal.clone();

        Some(Expression::MemberAccess {
            object: Box::new(object),
            member,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    // Prefix expressions
    pub(super) fn parse_prefix_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let operator = self.current_token.literal.clone();
        self.next_token();
        let right = self.parse_expression(Precedence::Prefix)?;
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

        while !self.is_peek_token(TokenType::RBrace) {
            self.next_token();
            let pattern = self.parse_pattern()?;

            if !self.expect_peek(TokenType::Arrow) {
                return None;
            }

            self.next_token();
            let body = self.parse_expression(Precedence::Lowest)?;

            let span = Span::new(pattern.span().start, body.span().end);
            arms.push(MatchArm {
                pattern,
                body,
                span,
            });

            if self.is_peek_token(TokenType::Semicolon) {
                self.next_token();
            }

            if self.is_peek_token(TokenType::Comma) {
                self.next_token();
            }
        }

        if !self.expect_peek(TokenType::RBrace) {
            return None;
        }

        Some(Expression::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current_token.position;
        match &self.current_token.token_type {
            TokenType::Ident if self.current_token.literal == "_" => Some(Pattern::Wildcard {
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::Ident => Some(Pattern::Identifier {
                name: self.current_token.literal.clone(),
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::None => Some(Pattern::None {
                span: Span::new(start, self.current_token.end_position),
            }),
            TokenType::Some => {
                if !self.expect_peek(TokenType::LParen) {
                    return None;
                }
                self.next_token();
                let inner_pattern = self.parse_pattern()?;
                if !self.expect_peek(TokenType::RParen) {
                    return None;
                }
                Some(Pattern::Some {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Left => {
                if !self.expect_peek(TokenType::LParen) {
                    return None;
                }
                self.next_token();
                let inner_pattern = self.parse_pattern()?;
                if !self.expect_peek(TokenType::RParen) {
                    return None;
                }
                Some(Pattern::Left {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Right => {
                if !self.expect_peek(TokenType::LParen) {
                    return None;
                }
                self.next_token();
                let inner_pattern = self.parse_pattern()?;
                if !self.expect_peek(TokenType::RParen) {
                    return None;
                }
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
                self.errors.push(
                    Diagnostic::error("INVALID PATTERN")
                        .with_code("E106")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message(format!(
                            "Expected a pattern, found `{}`.",
                            self.current_token.token_type
                        )),
                );
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
        } else if self.is_current_token(TokenType::Ident) {
            // Single unparenthesized parameter: \x ->
            vec![self.current_token.literal.clone()]
        } else {
            self.errors.push(
                Diagnostic::error("INVALID LAMBDA")
                    .with_code("E106")
                    .with_error_type(ErrorType::Compiler)
                    .with_span(self.current_token.span())
                    .with_message("Expected parameter or `(` after `\\`.")
                    .with_hint_text("Use `\\x -> expr` or `\\(x, y) -> expr`."),
            );
            return None;
        };

        // Expect ->
        if !self.is_peek_token(TokenType::Arrow) {
            self.errors.push(
                Diagnostic::error("EXPECTED ARROW")
                    .with_code("E107")
                    .with_error_type(ErrorType::Compiler)
                    .with_span(self.peek_token.span())
                    .with_message(format!(
                        "Expected `->` after lambda parameters, got `{}`.",
                        self.peek_token.token_type
                    ))
                    .with_hint_text("Use `\\x -> expr` or `\\(x, y) -> expr`.")
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
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }
        self.next_token();
        let value = self.parse_expression(Precedence::Lowest)?;
        if !self.expect_peek(TokenType::RParen) {
            return None;
        }
        Some(Expression::Some {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_left(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        self.next_token();

        let value = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::RParen) {
            return None;
        }

        Some(Expression::Left {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_right(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        self.next_token();

        let value = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::RParen) {
            return None;
        }

        Some(Expression::Right {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }
}
