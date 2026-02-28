use crate::{
    diagnostics::{
        DiagnosticBuilder, invalid_pattern, lambda_syntax_error, match_fat_arrow,
        match_pipe_separator, missing_array_close_bracket, missing_comprehension_close_bracket,
        missing_do_block_brace, missing_else_body_brace, missing_hash_close_brace,
        missing_if_body_brace, missing_lambda_arrow, missing_match_arrow, pipe_target_error,
        position::{Position, Span},
        unclosed_delimiter, unexpected_token, unknown_keyword_alias,
    },
    syntax::{
        block::Block,
        expression::{Expression, HandleArm, MatchArm, Pattern},
        precedence::{
            Fixity, Precedence, infix_op, parse_loop_precedence, prefix_op,
            rhs_precedence_for_infix,
        },
        statement::Statement,
        token_type::TokenType,
    },
};

use super::{Parser, helpers::ParameterListContext};

impl Parser {
    fn parse_parenthesized<T>(
        &mut self,
        context: &str,
        form_hint: &str,
        mut parse_inner: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<T> {
        if !self.expect_peek_context(
            TokenType::LParen,
            format!("Expected `(` after {context}."),
            format!("Use the form `{form_hint}`."),
        ) {
            return None;
        }
        self.next_token();
        let inner = parse_inner(self)?;
        if !self.expect_peek_context(
            TokenType::RParen,
            format!("Expected `)` to close {context}."),
            format!("Use the form `{form_hint}`."),
        ) {
            return None;
        }
        Some(inner)
    }

    /// Parses comma-separated patterns between the current `(` and the closing `close`.
    /// `current_token` must be `(` on entry; leaves `current_token` at `close` on exit.
    fn parse_comma_separated_patterns(&mut self, close: TokenType) -> Option<Vec<Pattern>> {
        debug_assert!(self.is_current_token(TokenType::LParen));
        let mut patterns = Vec::new();
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();

        // Empty: Constructor()
        if self.consume_if_peek(close) {
            return Some(patterns);
        }

        self.next_token(); // move to first pattern start

        loop {
            if self.is_current_token(close) || self.is_current_token(TokenType::Eof) {
                break;
            }

            let pattern = self.parse_pattern()?;
            patterns.push(pattern);
            self.next_token(); // advance past last token of pattern

            match self.current_token.token_type {
                TokenType::Comma => {
                    self.next_token(); // move to start of next pattern
                }
                ref t if *t == close || *t == TokenType::Eof => break,
                _ => {
                    if !self.push_followup_unless_structural_root(
                        construct_checkpoint,
                        unexpected_token(
                            self.current_token.span(),
                            format!(
                                "Expected `,` or `)` in constructor pattern, got {}",
                                self.current_token.token_type
                            ),
                        ),
                    ) {
                        return Some(patterns);
                    }
                    return None;
                }
            }
        }
        Some(patterns)
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

    fn emit_match_pipe_separator_diagnostic(&mut self, diag_start: usize) -> bool {
        self.errors
            .push(match_pipe_separator(self.peek_token.span()));
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
            let Some(peek_precedence) = parse_loop_precedence(&self.peek_token.token_type) else {
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
            TokenType::Perform => self.parse_perform_expression(),
            TokenType::LParen => self.parse_grouped_expression(),
            TokenType::LBracket => self.parse_list_literal(),
            TokenType::Hash => self.parse_array_literal_prefixed(),
            TokenType::LBrace => self.parse_hash(),
            TokenType::If => self.parse_if_expression(),
            TokenType::Do => self.parse_do_block_expression(),
            TokenType::Fn => self.parse_function_literal(),
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
            TokenType::Handle => self.parse_handle_expression(left),
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
        let open_pos = self.current_token.position;
        let arguments = self.parse_expression_list(TokenType::RParen, open_pos)?;
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

        if !self.expect_peek_context(
            TokenType::RBracket,
            "Expected `]` to close index expression.".to_string(),
            "Index expressions use `expr[index]`.".to_string(),
        ) {
            let _ = self.recover_to_matching_delimiter(TokenType::RBracket, &[TokenType::Comma]);
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
        // A newline right after `.` strongly indicates a dangling member access
        // (e.g. `point.\nnext_stmt`) rather than a continued expression.
        if self.peek_token.position.line > self.current_token.end_position.line {
            self.errors.push(unexpected_token(
                self.current_token.span(),
                format!(
                    "Expected identifier or tuple field index after `.`, got {}.",
                    self.peek_token.token_type
                ),
            ));
            // Recover to a statement boundary (or the nearest call-arg close) to
            // avoid cascading diagnostics from a dangling member access.
            let _ = self.recover_to_matching_delimiter(TokenType::RParen, &[TokenType::Ident]);
            // Recover by dropping the dangling dot and keeping the object.
            return Some(object);
        }

        if self.is_peek_token(TokenType::Int) {
            self.next_token();
            let index = match self.current_token.literal.parse::<usize>() {
                Ok(index) => index,
                Err(_) => {
                    self.errors.push(unexpected_token(
                        self.current_token.span(),
                        format!(
                            "Invalid tuple field index `{}`; expected non-negative integer.",
                            self.current_token.literal
                        ),
                    ));
                    return None;
                }
            };

            return Some(Expression::TupleFieldAccess {
                object: Box::new(object),
                index,
                span: Span::new(start, self.current_token.end_position),
            });
        }

        if self.is_peek_token(TokenType::RParen) {
            self.errors.push(unexpected_token(
                self.peek_token.span(),
                "Expected identifier or tuple field index after `.`, got `)`.".to_string(),
            ));
            // Recover as if the member access was omitted so parsing can continue.
            return Some(object);
        }

        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected identifier after `.` in member access.".to_string(),
            "Member access uses `value.member`.".to_string(),
        ) {
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
        let start = self.current_token.position;
        if self.is_peek_token(TokenType::RParen) {
            self.next_token();
            return Some(Expression::TupleLiteral {
                elements: vec![],
                span: Span::new(start, self.current_token.end_position),
            });
        }

        self.next_token();
        let first = self.parse_expression(Precedence::Lowest)?;

        if self.is_peek_token(TokenType::Comma) {
            let mut elements = vec![first];
            self.next_token(); // consume comma after first tuple element

            if self.is_peek_token(TokenType::RParen) {
                self.next_token();
                return Some(Expression::TupleLiteral {
                    elements,
                    span: Span::new(start, self.current_token.end_position),
                });
            }

            loop {
                self.next_token();
                elements.push(self.parse_expression(Precedence::Lowest)?);
                if self.is_peek_token(TokenType::Comma) {
                    self.next_token();
                } else {
                    break;
                }
            }

            if self.is_peek_token(TokenType::RParen) {
                self.next_token();
            } else {
                if self.peek_token.position.line > self.current_token.end_position.line {
                    let open_span = Span::new(start, start);
                    self.errors.push(unclosed_delimiter(
                        open_span,
                        "(",
                        ")",
                        Some(self.peek_token.span()),
                    ));
                } else {
                    self.peek_error(TokenType::RParen);
                }
                // Recover missing `)` so following code can continue parsing with
                // minimal cascades.
                if !(self.recover_to_matching_delimiter(
                    TokenType::RParen,
                    &[TokenType::Comma, TokenType::LBrace],
                ) || self.is_peek_token(TokenType::LBrace)
                    || (self.peek_token.position.line > self.current_token.end_position.line
                        && self.token_starts_statement(self.peek_token.token_type)))
                {
                    return None;
                }
            }

            return Some(Expression::TupleLiteral {
                elements,
                span: Span::new(start, self.current_token.end_position),
            });
        }

        if self.is_peek_token(TokenType::RParen) {
            self.next_token();
        } else {
            // Use unclosed_delimiter when the unexpected token is on a later
            // line (suggesting a truly forgotten `)`) and fall back to the
            // generic peek_error for same-line issues (e.g. missing comma).
            if self.peek_token.position.line > self.current_token.end_position.line {
                let open_span = Span::new(start, start);
                self.errors.push(unclosed_delimiter(
                    open_span,
                    "(",
                    ")",
                    Some(self.peek_token.span()),
                ));
            } else {
                self.peek_error(TokenType::RParen);
            }
            // Same recovery as tuple literals: report missing `)` and try to
            // resynchronize locally before giving up.
            if !(self.recover_to_matching_delimiter(
                TokenType::RParen,
                &[TokenType::Comma, TokenType::LBrace],
            ) || self.is_peek_token(TokenType::LBrace)
                || (self.peek_token.position.line > self.current_token.end_position.line
                    && self.token_starts_statement(self.peek_token.token_type)))
            {
                return None;
            }
        }
        Some(first)
    }

    // Collections
    pub(super) fn parse_list_literal(&mut self) -> Option<Expression> {
        let start = self.current_token.position;

        // Empty array shorthand: [||]
        // Lexer tokenizes "||" as TokenType::Or.
        if self.consume_if_peek(TokenType::Or) {
            if !self.expect_peek_context(
                TokenType::RBracket,
                "Expected `]` to close empty array literal.".to_string(),
                "Empty arrays use `[||]`.".to_string(),
            ) {
                return None;
            }
            return Some(Expression::ArrayLiteral {
                elements: vec![],
                span: Span::new(start, self.current_token.end_position),
            });
        }

        // Array literal: [| ... |]
        if self.consume_if_peek(TokenType::Bar) {
            // Empty array: [||]
            if self.consume_if_peek(TokenType::RBracket) {
                return Some(Expression::ArrayLiteral {
                    elements: vec![],
                    span: Span::new(start, self.current_token.end_position),
                });
            }

            self.next_token();
            let first = self.parse_expression(Precedence::Lowest)?;
            let elements = self.parse_expression_list_with_first(first, TokenType::Bar, start)?;
            if self.is_peek_token(TokenType::RBracket) {
                self.next_token();
            } else {
                self.errors
                    .push(missing_array_close_bracket(self.peek_token.span()));
                return None;
            }
            return Some(Expression::ArrayLiteral {
                elements,
                span: Span::new(start, self.current_token.end_position),
            });
        }

        // Empty list: []
        if self.consume_if_peek(TokenType::RBracket) {
            return Some(Expression::EmptyList {
                span: Span::new(start, self.current_token.end_position),
            });
        }

        // Parse the first element
        self.next_token();
        let first = self.parse_expression(Precedence::Lowest)?;

        // Check for cons syntax [head | tail] or list comprehension [expr | x <- xs, ...]
        if self.is_peek_token(TokenType::Bar) {
            self.next_token(); // consume `|`, now current = Bar

            // Disambiguate: if peek is Ident and peek2 is LeftArrow, it's a comprehension
            if self.is_peek_token(TokenType::Ident)
                && self.peek2_token.token_type == TokenType::LeftArrow
            {
                return self.parse_list_comprehension(first, start);
            }

            // Malformed comprehension shape: `[expr | <- source]`
            // should report a contextual parser diagnostic rather than
            // falling through to generic expected-expression errors.
            if self.is_peek_token(TokenType::LeftArrow) {
                self.errors.push(
                    unexpected_token(
                        self.peek_token.span(),
                        "Expected generator identifier before `<-` in list comprehension.",
                    )
                    .with_hint_text("List comprehensions use `[expr | name <- source, ...]`."),
                );
                return None;
            }

            // Otherwise: cons cell [head | tail]
            self.next_token();
            let tail = self.parse_expression(Precedence::Lowest)?;

            if !self.is_peek_token(TokenType::RBracket) {
                // Heuristic: if the next token is on a different line or
                // starts a statement, this is likely a missing `]` — point
                // the error at the opening `[` (Rust-style).
                if self.peek_token.position.line > self.current_token.end_position.line
                    || self.token_starts_statement(self.peek_token.token_type)
                    || self.peek_token.token_type == TokenType::Eof
                {
                    self.errors.push(unclosed_delimiter(
                        Span::new(start, start),
                        "[",
                        "]",
                        Some(self.peek_token.span()),
                    ));
                } else {
                    self.peek_error(TokenType::RBracket);
                }
                return None;
            } else {
                self.next_token();
            }
            return Some(Expression::Cons {
                head: Box::new(first),
                tail: Box::new(tail),
                span: Span::new(start, self.current_token.end_position),
            });
        }

        // Otherwise, parse remaining elements as list literal.
        // `first` is already parsed; delegate to the "with_first" variant
        // which provides the same missing-comma recovery as parse_expression_list.
        let elements = self.parse_expression_list_with_first(first, TokenType::RBracket, start)?;
        Some(Expression::ListLiteral {
            elements,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parse a list comprehension after `[body_expr |` has been consumed.
    /// Current token is `|`, peek is the first generator variable.
    ///
    /// Syntax: [expr | var <- source, guard, var2 <- source2, ...]
    /// Desugars to nested map/filter/flat_map calls.
    fn parse_list_comprehension(
        &mut self,
        body: Expression,
        start: Position,
    ) -> Option<Expression> {
        // Collect clauses: generators (var <- source) and guards (expr)
        enum Clause {
            Generator {
                var: crate::syntax::Identifier,
                source: Expression,
            },
            Guard(Expression),
        }

        let mut clauses = Vec::new();

        // Parse first generator (required) — peek is Ident, peek2 is LeftArrow
        loop {
            // Expect identifier
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected generator identifier in list comprehension.".to_string(),
                "List comprehensions use `[expr | name <- source, ...]`.".to_string(),
            ) {
                return None;
            }
            let var = self
                .lexer
                .interner_mut()
                .intern(&self.current_token.literal);

            // Expect <-
            if !self.expect_peek_context(
                TokenType::LeftArrow,
                "Expected `<-` after list-comprehension generator variable.".to_string(),
                "List comprehensions use `[expr | name <- source, ...]`.".to_string(),
            ) {
                return None;
            }

            // Parse source expression
            self.next_token();
            let source = self.parse_expression(Precedence::Lowest)?;
            clauses.push(Clause::Generator { var, source });

            // Check for more clauses separated by commas
            while self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume comma

                // Is the next clause a generator (Ident <- ...) or a guard?
                if self.is_peek_token(TokenType::Ident)
                    && self.peek2_token.token_type == TokenType::LeftArrow
                {
                    // Next generator — break inner loop, continue outer loop
                    break;
                }

                // Guard expression
                self.next_token();
                let guard = self.parse_expression(Precedence::Lowest)?;
                clauses.push(Clause::Guard(guard));
            }

            // If we broke out because of a new generator, continue the outer loop
            if self.is_peek_token(TokenType::Ident)
                && self.peek2_token.token_type == TokenType::LeftArrow
            {
                continue;
            }

            break;
        }

        // Expect closing ]
        if self.is_peek_token(TokenType::RBracket) {
            self.next_token();
        } else {
            self.errors
                .push(missing_comprehension_close_bracket(self.peek_token.span()));
            return None;
        }

        let span = Span::new(start, self.current_token.end_position);

        // Desugar clauses into nested map/filter/flat_map calls.
        // Process left-to-right: each generator groups with its trailing guards.
        // The algorithm builds from the inside out using recursion over clause groups.

        // Group clauses: each group is (generator, trailing_guards)
        struct GeneratorGroup {
            var: crate::syntax::Identifier,
            source: Expression,
            guards: Vec<Expression>,
        }

        let mut groups: Vec<GeneratorGroup> = Vec::new();
        for clause in clauses {
            match clause {
                Clause::Generator { var, source } => {
                    groups.push(GeneratorGroup {
                        var,
                        source,
                        guards: Vec::new(),
                    });
                }
                Clause::Guard(expr) => {
                    if let Some(group) = groups.last_mut() {
                        group.guards.push(expr);
                    }
                }
            }
        }

        // Build the desugared expression from right to left
        let mut result = body;
        for (i, group) in groups.iter().enumerate().rev() {
            // Apply guards to the source: filter(filter(source, \v -> g1), \v -> g2)
            let mut source = group.source.clone();
            for guard in &group.guards {
                let lambda = self.make_lambda(group.var, guard.clone(), span);
                source = self.make_call("filter", vec![source, lambda], span);
            }

            // If this is the last (innermost) generator, use map; otherwise flat_map
            let lambda = self.make_lambda(group.var, result, span);
            result = if i == groups.len() - 1 {
                self.make_call("map", vec![source, lambda], span)
            } else {
                self.make_call("flat_map", vec![source, lambda], span)
            };
        }

        Some(result)
    }

    /// Build an `Expression::Identifier` from a string, interning it.
    fn make_ident(&mut self, name: &str, span: Span) -> Expression {
        let sym = self.lexer.interner_mut().intern(name);
        Expression::Identifier { name: sym, span }
    }

    /// Build a single-parameter lambda: `\param -> body`
    fn make_lambda(
        &self,
        param: crate::syntax::Identifier,
        body: Expression,
        span: Span,
    ) -> Expression {
        let body_span = body.span();
        Expression::Function {
            parameters: vec![param],
            parameter_types: vec![None],
            return_type: None,
            effects: vec![],
            body: Block {
                statements: vec![Statement::Expression {
                    expression: body,
                    has_semicolon: false,
                    span: body_span,
                }],
                span: body_span,
            },
            span,
        }
    }

    /// Build a function call: `name(args...)`
    fn make_call(&mut self, name: &str, arguments: Vec<Expression>, span: Span) -> Expression {
        Expression::Call {
            function: Box::new(self.make_ident(name, span)),
            arguments,
            span,
        }
    }

    pub(super) fn parse_array_literal_prefixed(&mut self) -> Option<Expression> {
        // Legacy syntax kept for compatibility: #[a, b, c]
        let start = self.current_token.position;
        if !self.expect_peek_context(
            TokenType::LBracket,
            "Expected `[` after `#` to start legacy array literal.".to_string(),
            "Legacy array literals use `#[a, b, c]`.".to_string(),
        ) {
            return None;
        }

        if self.consume_if_peek(TokenType::RBracket) {
            return Some(Expression::ArrayLiteral {
                elements: vec![],
                span: Span::new(start, self.current_token.end_position),
            });
        }

        self.next_token();
        let first = self.parse_expression(Precedence::Lowest)?;
        let elements = self.parse_expression_list_with_first(first, TokenType::RBracket, start)?;
        Some(Expression::ArrayLiteral {
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

            if !self.expect_peek_context(
                TokenType::Colon,
                "Expected `:` between hash key and value.".to_string(),
                "Hash literals use `{key: value, ...}`.".to_string(),
            ) {
                return None;
            }

            self.next_token();

            let value = self.parse_expression(Precedence::Lowest)?;

            pairs.push((key, value));

            if self.is_peek_token(TokenType::RBrace) {
                // continue to closing-brace consume below
            } else if self.is_peek_token(TokenType::Comma) {
                self.next_token();
            } else {
                if self.peek_token.token_type == TokenType::Eof
                    || self.peek_token.position.line > self.current_token.end_position.line
                    || self.token_starts_statement(self.peek_token.token_type)
                {
                    self.errors
                        .push(missing_hash_close_brace(self.peek_token.span()));
                } else {
                    self.peek_error(TokenType::Comma);
                }
                return None;
            }
        }

        if self.is_peek_token(TokenType::RBrace) {
            self.next_token();
        } else {
            self.errors
                .push(missing_hash_close_brace(self.peek_token.span()));
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

        if !self.is_peek_token(TokenType::LBrace) {
            self.errors
                .push(missing_if_body_brace(self.peek_token.span()));
            return None;
        }
        self.next_token();

        let consequence = self.parse_block();

        if self.peek_token.token_type == TokenType::Ident
            && matches!(self.peek_token.literal.as_ref(), "elif" | "elsif")
        {
            self.errors.push(
                unknown_keyword_alias(
                    self.peek_token.span(),
                    &self.peek_token.literal,
                    "else if",
                    "chained conditionals",
                )
                .with_hint_text("Replace `elif`/`elsif` with `else if`."),
            );
        }

        let alternative = if self.is_peek_token(TokenType::Else) {
            self.next_token();

            if self.is_peek_token(TokenType::If) {
                // `else if`: consume `if`, recurse, wrap in a synthetic block
                self.next_token();
                let span_start = self.current_token.position;
                let nested_if = self.parse_if_expression()?;
                let span = Span::new(span_start, self.current_token.end_position);
                Some(Block {
                    statements: vec![Statement::Expression {
                        expression: nested_if,
                        has_semicolon: false,
                        span,
                    }],
                    span,
                })
            } else {
                if !self.is_peek_token(TokenType::LBrace) {
                    self.errors
                        .push(missing_else_body_brace(self.peek_token.span()));
                    return None;
                }
                self.next_token();
                Some(self.parse_block())
            }
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

    pub(super) fn parse_do_block_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        if !self.is_peek_token(TokenType::LBrace) {
            self.errors
                .push(missing_do_block_brace(self.peek_token.span()));
            return None;
        }
        self.next_token();

        let block = self.parse_block();
        Some(Expression::DoBlock {
            block,
            span: Span::new(start, self.current_token.end_position),
        })
    }

    /// Parses `perform Effect.op(arg1, arg2)`.
    /// `current_token` is `Perform` on entry.
    pub(super) fn parse_perform_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;

        // Effect name (Ident)
        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected effect name after `perform`.".to_string(),
            "Perform expressions use `perform Effect.op(args...)`.".to_string(),
        ) {
            return None;
        }
        let effect = self
            .lexer
            .interner_mut()
            .intern(&self.current_token.literal);

        // `.`
        if !self.expect_peek_context(
            TokenType::Dot,
            "Expected `.` between effect and operation in `perform`.".to_string(),
            "Perform expressions use `perform Effect.op(args...)`.".to_string(),
        ) {
            return None;
        }

        // Operation name (Ident)
        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected operation name after `perform Effect.`.".to_string(),
            "Perform expressions use `perform Effect.op(args...)`.".to_string(),
        ) {
            return None;
        }
        let operation = self
            .lexer
            .interner_mut()
            .intern(&self.current_token.literal);

        // `(`
        if !self.expect_peek_context(
            TokenType::LParen,
            "Expected `(` after performed operation name.".to_string(),
            "Perform expressions use `perform Effect.op(args...)`.".to_string(),
        ) {
            return None;
        }

        let open_pos = self.current_token.position;
        let args = self.parse_expression_list(TokenType::RParen, open_pos)?;
        let end = self.current_token.end_position;

        Some(Expression::Perform {
            effect,
            operation,
            args,
            span: Span::new(start, end),
        })
    }

    /// Parses `expr handle Effect { op(resume, arg1, ...) -> body, ... }`.
    /// `current_token` is `Handle`; `left` is the expression being handled.
    pub(super) fn parse_handle_expression(&mut self, left: Expression) -> Option<Expression> {
        let start = left.span().start;

        // Expect the effect name (Ident)
        if !self.expect_peek_context(
            TokenType::Ident,
            "Expected effect name after `handle`.".to_string(),
            "Handle expressions use `expr handle Effect { op(resume, ...) -> body }`.".to_string(),
        ) {
            return None;
        }
        let effect = self
            .lexer
            .interner_mut()
            .intern(&self.current_token.literal);

        // Expect `{`
        if !self.expect_peek_context(
            TokenType::LBrace,
            "Expected `{` to begin `handle` arms.".to_string(),
            "Handle expressions use `expr handle Effect { ... }`.".to_string(),
        ) {
            return None;
        }

        let mut arms: Vec<HandleArm> = Vec::new();

        while !self.is_peek_token(TokenType::RBrace) && !self.is_peek_token(TokenType::Eof) {
            // op name
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected operation name in `handle` arm.".to_string(),
                "Handle arms use `op(resume, ...) -> body`.".to_string(),
            ) {
                return None;
            }
            let arm_start = self.current_token.position;
            let op_name = self
                .lexer
                .interner_mut()
                .intern(&self.current_token.literal);

            // `(`
            if !self.expect_peek_context(
                TokenType::LParen,
                "Expected `(` after handle operation name.".to_string(),
                "Handle arms use `op(resume, ...) -> body`.".to_string(),
            ) {
                return None;
            }

            // First param is the resume continuation
            if !self.expect_peek_context(
                TokenType::Ident,
                "Expected resume parameter in handle arm.".to_string(),
                "Handle arms use `op(resume, arg1, ...) -> body`.".to_string(),
            ) {
                return None;
            }
            let resume_param = self
                .lexer
                .interner_mut()
                .intern(&self.current_token.literal);

            // Remaining params
            let mut params = Vec::new();
            while self.is_peek_token(TokenType::Comma) {
                self.next_token(); // consume `,`
                if !self.expect_peek_context(
                    TokenType::Ident,
                    "Expected parameter name after `,` in handle arm.".to_string(),
                    "Handle arms use `op(resume, arg1, ...) -> body`.".to_string(),
                ) {
                    return None;
                }
                let p = self
                    .lexer
                    .interner_mut()
                    .intern(&self.current_token.literal);
                params.push(p);
            }

            // `)`
            if !self.expect_peek_context(
                TokenType::RParen,
                "Expected `)` after handle-arm parameter list.".to_string(),
                "Handle arms use `op(resume, arg1, ...) -> body`.".to_string(),
            ) {
                return None;
            }

            // `->`
            if !self.expect_peek_context(
                TokenType::Arrow,
                "Expected `->` in handle arm.".to_string(),
                "Handle arms use `op(resume, arg1, ...) -> body`.".to_string(),
            ) {
                return None;
            }

            // body
            self.next_token();
            let body = self.parse_expression(Precedence::Lowest)?;
            let arm_end = body.span().end;

            arms.push(HandleArm {
                operation_name: op_name,
                resume_param,
                params,
                body,
                span: Span::new(arm_start, arm_end),
            });

            // Optional trailing comma
            if self.is_peek_token(TokenType::Comma) {
                self.next_token();
            }
        }

        if !self.expect_peek_context(
            TokenType::RBrace,
            "Expected `}` to close `handle` expression.".to_string(),
            "Handle expressions use `expr handle Effect { ... }`.".to_string(),
        ) {
            return None;
        }

        let end = self.current_token.span().end;
        Some(Expression::Handle {
            expr: Box::new(left),
            effect,
            arms,
            span: Span::new(start, end),
        })
    }

    pub(super) fn parse_match_expression(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        self.next_token();
        let scrutinee = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek_context(
            TokenType::LBrace,
            "Expected `{` to begin match arms.".to_string(),
            "Match expressions use `match value { pattern -> body, ... }`.".to_string(),
        ) {
            return None;
        }

        let mut arms = Vec::new();
        let diag_start = self.errors.len();
        let construct_checkpoint = self.start_construct_diagnostics_checkpoint();

        while !self.is_peek_token(TokenType::RBrace) {
            self.next_token();
            let pattern = self.parse_pattern()?;
            let mut guard = None;

            if self.is_peek_token(TokenType::If) {
                self.next_token(); // consume `if`
                self.next_token(); // move to guard expression start
                guard = Some(self.parse_expression(Precedence::Lowest)?);
            }

            if self.is_peek_token(TokenType::Assign) && self.peek2_token.token_type == TokenType::Gt
            {
                self.errors.push(match_fat_arrow(self.peek_token.span()));
                self.next_token(); // consume '='
                self.next_token(); // consume '>'
            } else if self.is_peek_token(TokenType::Arrow) {
                self.next_token();
            } else {
                self.errors.push(missing_match_arrow(
                    self.peek_token.span(),
                    &self.peek_token.token_type.to_string(),
                ));
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
                TokenType::Bar => {
                    if self.emit_match_pipe_separator_diagnostic(diag_start) {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }
                    // Recover by treating `|` as a comma separator.
                    self.next_token();
                }
                TokenType::Eof => {
                    if self.emit_match_eof_diagnostic(diag_start) {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }
                    return Some(self.build_match_expression(start, scrutinee, arms));
                }
                _ => {
                    if !self.push_followup_unless_structural_root(
                        construct_checkpoint,
                        unexpected_token(
                            self.peek_token.span(),
                            format!(
                                "Expected `,` or `}}` after match arm, got {}.",
                                self.peek_token.token_type
                            ),
                        ),
                    ) {
                        return Some(self.build_match_expression(start, scrutinee, arms));
                    }
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
                        TokenType::Bar => {
                            if self.emit_match_pipe_separator_diagnostic(diag_start) {
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

        if !self.expect_peek_context(
            TokenType::RBrace,
            "Expected `}` to close match expression.".to_string(),
            "Match expressions use `match value { pattern -> body, ... }`.".to_string(),
        ) {
            return None;
        }

        Some(self.build_match_expression(start, scrutinee, arms))
    }

    /// Parses a single match pattern, including ADT constructors such as
    /// `Red`, `Circle(r)`, and nested constructor fields.
    pub(super) fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current_token.position;
        match &self.current_token.token_type {
            TokenType::Ident if self.current_token.literal == "_" => Some(Pattern::Wildcard {
                span: Span::new(start, self.current_token.end_position),
            }),
            // Uppercase-initial identifier → ADT constructor pattern: `Red`, `Circle(r)`, `Node(l, v, r)`
            TokenType::Ident
                if self
                    .current_token
                    .literal
                    .starts_with(|c: char| c.is_uppercase()) =>
            {
                let name = self
                    .current_token
                    .symbol
                    .expect("ident token should have symbol");

                // If followed by '(' → parse field sub-patterns
                if self.is_peek_token(TokenType::LParen) {
                    self.next_token(); // advance to '('
                    let fields = self
                        .parse_comma_separated_patterns(TokenType::RParen)
                        .unwrap_or_default();
                    Some(Pattern::Constructor {
                        name,
                        fields,
                        span: Span::new(start, self.current_token.end_position),
                    })
                } else {
                    // Zero-argument constructor: `Red`, `None_` etc.
                    Some(Pattern::Constructor {
                        name,
                        fields: vec![],
                        span: Span::new(start, self.current_token.end_position),
                    })
                }
            }
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
                let inner_pattern = self.parse_parenthesized(
                    "`Some` pattern payload",
                    "Some(pattern)",
                    |parser| parser.parse_pattern(),
                )?;
                Some(Pattern::Some {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Left => {
                let inner_pattern = self.parse_parenthesized(
                    "`Left` pattern payload",
                    "Left(pattern)",
                    |parser| parser.parse_pattern(),
                )?;
                Some(Pattern::Left {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::Right => {
                let inner_pattern = self.parse_parenthesized(
                    "`Right` pattern payload",
                    "Right(pattern)",
                    |parser| parser.parse_pattern(),
                )?;
                Some(Pattern::Right {
                    pattern: Box::new(inner_pattern),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::LBracket => {
                // Empty list pattern: []
                if self.is_peek_token(TokenType::RBracket) {
                    self.next_token(); // consume ]
                    return Some(Pattern::EmptyList {
                        span: Span::new(start, self.current_token.end_position),
                    });
                }
                // Cons pattern: [head | tail]
                self.next_token(); // advance to head pattern
                let head = self.parse_pattern()?;
                if !self.expect_peek_context(
                    TokenType::Bar,
                    "Expected `|` in cons pattern.".to_string(),
                    "Cons patterns use `[head | tail]`.".to_string(),
                ) {
                    return None;
                }
                self.next_token(); // advance to tail pattern
                let tail = self.parse_pattern()?;
                if !self.expect_peek_context(
                    TokenType::RBracket,
                    "Expected `]` to close list pattern.".to_string(),
                    "List patterns use `[]` or `[head | tail]`.".to_string(),
                ) {
                    return None;
                }
                Some(Pattern::Cons {
                    head: Box::new(head),
                    tail: Box::new(tail),
                    span: Span::new(start, self.current_token.end_position),
                })
            }
            TokenType::LParen => {
                if self.is_peek_token(TokenType::RParen) {
                    self.next_token();
                    return Some(Pattern::Tuple {
                        elements: vec![],
                        span: Span::new(start, self.current_token.end_position),
                    });
                }

                self.next_token();
                let first = self.parse_pattern()?;
                if !self.expect_peek_context(
                    TokenType::Comma,
                    "Expected `,` in tuple pattern.".to_string(),
                    "Tuple patterns use `(a, b, ...)`.".to_string(),
                ) {
                    self.errors.push(unexpected_token(
                        self.peek_token.span(),
                        "Tuple patterns require a comma, for example `(x, y)`.".to_string(),
                    ));
                    return None;
                }

                let mut elements = vec![first];
                if self.is_peek_token(TokenType::RParen) {
                    self.next_token();
                    return Some(Pattern::Tuple {
                        elements,
                        span: Span::new(start, self.current_token.end_position),
                    });
                }

                loop {
                    self.next_token();
                    elements.push(self.parse_pattern()?);
                    if self.is_peek_token(TokenType::Comma) {
                        self.next_token();
                    } else {
                        break;
                    }
                }

                if !self.expect_peek_context(
                    TokenType::RParen,
                    "Expected `)` to close tuple pattern.".to_string(),
                    "Tuple patterns use `(a, b, ...)`.".to_string(),
                ) {
                    return None;
                }

                Some(Pattern::Tuple {
                    elements,
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
        if !self.expect_peek_context(
            TokenType::LParen,
            "Expected `(` after `fn` in function literal.".to_string(),
            "Function literals use `fn(params) { ... }`.".to_string(),
        ) {
            return None;
        }

        let parameters = self.parse_function_parameters()?;
        let parameter_types = vec![None; parameters.len()];

        if !self.expect_peek_context(
            TokenType::LBrace,
            "Expected `{` to begin function literal body.".to_string(),
            "Function literals use `fn(params) { ... }`.".to_string(),
        ) {
            return None;
        }

        let body = self.parse_block();
        Some(Expression::Function {
            parameters,
            parameter_types,
            return_type: None,
            effects: vec![],
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
        let (parameters, parameter_types) = if self.is_current_token(TokenType::LParen) {
            // Parenthesized parameters: \() -> or \(x) -> or \(x, y) ->
            self.parse_typed_function_parameters(ParameterListContext::Lambda)?
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
            let mut types = Vec::new();
            if let Some(param) = self.validate_parameter_identifier() {
                params.push(param);
                let param_name = self.lexer.interner().resolve(param).to_string();
                let type_annotation = self.parse_type_annotation_opt_with_missing_colon(
                    &[
                        TokenType::Arrow,
                        TokenType::LBrace,
                        TokenType::Semicolon,
                        TokenType::Eof,
                    ],
                    "lambda parameter",
                    Some(param_name.as_str()),
                );
                types.push(type_annotation);
            }
            (params, types)
        };

        // Expect ->
        if self.is_current_token(TokenType::Arrow) {
            // already at arrow via annotation recovery
        } else if self.is_peek_token(TokenType::Arrow) {
            self.next_token(); // consume `->`
        } else {
            self.errors.push(missing_lambda_arrow(
                self.peek_token.span(),
                &self.peek_token.token_type.to_string(),
            ));
            return None;
        }
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
                    has_semicolon: false,
                    span: Span::new(expr_start, self.current_token.end_position),
                }],
                span: expr_span,
            }
        };

        Some(Expression::Function {
            parameters,
            parameter_types,
            return_type: None,
            effects: vec![],
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
        let value = self.parse_parenthesized("`Some` payload", "Some(value)", |parser| {
            parser.parse_expression(Precedence::Lowest)
        })?;
        Some(Expression::Some {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_left(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let value = self.parse_parenthesized("`Left` payload", "Left(value)", |parser| {
            parser.parse_expression(Precedence::Lowest)
        })?;

        Some(Expression::Left {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }

    pub(super) fn parse_right(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let value = self.parse_parenthesized("`Right` payload", "Right(value)", |parser| {
            parser.parse_expression(Precedence::Lowest)
        })?;

        Some(Expression::Right {
            value: Box::new(value),
            span: Span::new(start, self.current_token.end_position),
        })
    }
}
