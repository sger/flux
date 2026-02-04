use crate::frontend::{
    block::Block,
    diagnostics::{Diagnostic, EXPECTED_EXPRESSION, ErrorType},
    expression::{Expression, MatchArm, Pattern, StringPart},
    lexer::Lexer,
    position::Span,
    precedence::{Precedence, token_precedence},
    program::Program,
    statement::Statement,
    token::Token,
    token_type::TokenType,
};

pub struct Parser {
    lexer: Lexer,
    current_token: Token,
    peek_token: Token,
    peek2_token: Token,
    pub errors: Vec<Diagnostic>,
    suppress_unterminated_string_error: bool,
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

    fn next_token(&mut self) {
        self.current_token = self.peek_token.clone();
        self.peek_token = self.peek2_token.clone();
        self.peek2_token = self.lexer.next_token();
    }

    fn span_from(&self, start: crate::frontend::position::Position) -> Span {
        Span::new(start, self.current_token.position)
    }

    fn synchronize_after_error(&mut self) {
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

    fn parse_statement(&mut self) -> Option<Statement> {
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

    fn parse_expression_statement(&mut self) -> Option<Statement> {
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

    fn parse_function_statement(&mut self) -> Option<Statement> {
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

    fn parse_return_statement(&mut self) -> Option<Statement> {
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

    fn parse_let_statement(&mut self) -> Option<Statement> {
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

    fn parse_assignment_statement(&mut self) -> Option<Statement> {
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

    fn parse_module_statement(&mut self) -> Option<Statement> {
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

    fn parse_import_statement(&mut self) -> Option<Statement> {
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

    fn parse_qualified_name(&mut self) -> Option<String> {
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

    fn parse_expression(&mut self, precedence: Precedence) -> Option<Expression> {
        let mut left = self.parse_prefix()?;

        while !self.is_peek_token(TokenType::Semicolon) && precedence < self.peek_precedence() {
            self.next_token();
            left = self.parse_infix(left)?;
        }

        Some(left)
    }

    fn parse_prefix(&mut self) -> Option<Expression> {
        match &self.current_token.token_type {
            TokenType::Ident => self.parse_identifier(),
            TokenType::Int => self.parse_integer(),
            TokenType::Float => self.parse_float(),
            TokenType::String => self.parse_string(),
            TokenType::UnterminatedString => {
                if self.suppress_unterminated_string_error {
                    self.suppress_unterminated_string_error = false;
                    None
                } else {
                    self.unterminated_string_error();
                    None
                }
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

    fn no_prefix_parse_error(&mut self) {
        let error_spec = &EXPECTED_EXPRESSION;
        let diag = Diagnostic::make_error(
            error_spec,
            &[&self.current_token.token_type.to_string()],
            String::new(), // No file context in parser
            Span::new(self.current_token.position, self.current_token.position),
        );
        self.errors.push(diag);
    }

    fn unterminated_string_error(&mut self) {
        // Get the string literal content
        let literal = &self.current_token.literal;

        // The literal includes everything from opening quote to end of line
        // Find where to end the highlighting (before any "//" comment)
        let mut highlight_len = literal.len();
        if let Some(comment_pos) = literal.find("//") {
            // Trim whitespace before the comment
            let before_comment = &literal[..comment_pos];
            highlight_len = before_comment.trim_end().len();
        }

        // Ensure we highlight at least something (minimum 1 char)
        if highlight_len == 0 {
            highlight_len = 1;
        }

        // Start position is where the token begins
        let start_pos = self.current_token.position;

        // End position is start + length of content to highlight
        let end_pos = crate::frontend::position::Position::new(
            start_pos.line,
            start_pos.column + highlight_len,
        );

        let error_spec = &EXPECTED_EXPRESSION;
        let diag = Diagnostic::make_error(
            error_spec,
            &["unterminated string literal"],
            String::new(), // No file context in parser
            Span::new(start_pos, end_pos),
        );
        self.errors.push(diag);
        self.synchronize_after_error();
    }

    fn parse_infix(&mut self, left: Expression) -> Option<Expression> {
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

    fn parse_infix_expression(&mut self, left: Expression) -> Option<Expression> {
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
    fn parse_pipe_expression(&mut self, left: Expression) -> Option<Expression> {
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

    fn parse_call_expression(&mut self, function: Expression) -> Option<Expression> {
        let start = function.span().start;
        let arguments = self.parse_expression_list(TokenType::RParen)?;
        Some(Expression::Call {
            function: Box::new(function),
            arguments,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_index_expression(&mut self, left: Expression) -> Option<Expression> {
        let start = left.span().start;
        self.next_token();
        let index = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::RBracket) {
            return None;
        }

        Some(Expression::Index {
            left: Box::new(left),
            index: Box::new(index),
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_member_access(&mut self, object: Expression) -> Option<Expression> {
        let start = object.span().start;
        if !self.expect_peek(TokenType::Ident) {
            return None;
        }

        let member = self.current_token.literal.clone();

        Some(Expression::MemberAccess {
            object: Box::new(object),
            member,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_identifier(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let mut name = self.current_token.literal.clone();
        // Only collect dotted segments for module paths (PascalCase names)
        // Don't collect ALL_CAPS constants like PI, TAU, MAX
        if is_pascal_case_ident(&self.current_token) {
            while self.is_peek_token(TokenType::Dot) && is_pascal_case_ident(&self.peek2_token) {
                self.next_token(); // consume '.'
                if !self.expect_peek(TokenType::Ident) {
                    return None;
                }
                name.push('.');
                name.push_str(&self.current_token.literal);
            }
        }
        Some(Expression::Identifier {
            name,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_integer(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        match self.current_token.literal.parse::<i64>() {
            Ok(value) => Some(Expression::Integer {
                value,
                span: Span::new(start, self.current_token.position),
            }),
            Err(_) => {
                self.errors.push(
                    Diagnostic::error("INVALID INTEGER")
                        .with_code("E103")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message(format!(
                            "Could not parse `{}` as an integer.",
                            self.current_token.literal
                        )),
                );
                None
            }
        }
    }

    fn parse_float(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        match self.current_token.literal.parse::<f64>() {
            Ok(value) => Some(Expression::Float {
                value,
                span: Span::new(start, self.current_token.position),
            }),
            Err(_) => {
                self.errors.push(
                    Diagnostic::error("INVALID FLOAT")
                        .with_code("E104")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.current_token.span())
                        .with_message(format!(
                            "Could not parse `{}` as a float.",
                            self.current_token.literal
                        )),
                );
                None
            }
        }
    }

    fn parse_string(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let first_part = self.current_token.literal.clone();

        // Simple string - no interpolation
        Some(Expression::String {
            value: first_part,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_interpolation_start(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let first_part = self.current_token.literal.clone();

        // InterpolationStart token signals the lexer found #{
        // Now parse as interpolated string
        self.parse_interpolated_string(start, first_part)
    }

    /// Check if peek_token could be the start of an interpolation expression
    fn is_interpolation_expression_start(&self) -> bool {
        matches!(
            self.peek_token.token_type,
            TokenType::Ident
                | TokenType::Int
                | TokenType::Float
                | TokenType::True
                | TokenType::False
                | TokenType::None
                | TokenType::Some
                | TokenType::Bang
                | TokenType::Minus
                | TokenType::LParen
                | TokenType::LBracket
                | TokenType::LBrace
                | TokenType::If
                | TokenType::Fun
                | TokenType::Match
                | TokenType::String
        )
    }

    /// Parse an interpolated string like "Hello #{name}!"
    fn parse_interpolated_string(
        &mut self,
        start: crate::frontend::position::Position,
        first_literal: String,
    ) -> Option<Expression> {
        let mut parts = Vec::new();

        // Add the first literal part if non-empty
        if !first_literal.is_empty() {
            parts.push(StringPart::Literal(first_literal));
        }

        loop {
            // Parse the interpolation expression
            self.next_token();
            let expr = self.parse_expression(Precedence::Lowest)?;
            parts.push(StringPart::Interpolation(Box::new(expr)));

            // Expect closing brace of interpolation
            if !self.expect_peek(TokenType::RBrace) {
                return None;
            }

            // After RBrace, check what's next
            // It should be either InterpolationStart (more content) or StringEnd (end of string)
            if self.is_peek_token(TokenType::InterpolationStart) {
                // More string content with another interpolation
                self.next_token();
                let literal = self.current_token.literal.clone();
                if !literal.is_empty() {
                    parts.push(StringPart::Literal(literal));
                }
                // Continue to parse next interpolation (loop will handle it)
            } else if self.is_peek_token(TokenType::StringEnd) {
                // End of interpolated string
                self.next_token();
                let final_literal = self.current_token.literal.clone();
                if !final_literal.is_empty() {
                    parts.push(StringPart::Literal(final_literal));
                }
                break;
            } else {
                // Unexpected token - report error
                self.errors.push(
                    Diagnostic::error("UNTERMINATED INTERPOLATION")
                        .with_code("E107")
                        .with_error_type(ErrorType::Compiler)
                        .with_span(self.peek_token.span())
                        .with_message("Expected string continuation or end after interpolation."),
                );
                self.suppress_unterminated_string_error = true;
                self.synchronize_after_error();
                return None;
            }
        }

        Some(Expression::InterpolatedString {
            parts,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_boolean(&self) -> Option<Expression> {
        let start = self.current_token.position;
        Some(Expression::Boolean {
            value: self.current_token.token_type == TokenType::True,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_none(&self) -> Option<Expression> {
        let start = self.current_token.position;
        Some(Expression::None {
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_some(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_left(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_right(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_match_expression(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.current_token.position;
        match &self.current_token.token_type {
            TokenType::Ident if self.current_token.literal == "_" => Some(Pattern::Wildcard {
                span: Span::new(start, self.current_token.position),
            }),
            TokenType::Ident => Some(Pattern::Identifier {
                name: self.current_token.literal.clone(),
                span: Span::new(start, self.current_token.position),
            }),
            TokenType::None => Some(Pattern::None {
                span: Span::new(start, self.current_token.position),
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
                    span: Span::new(start, self.current_token.position),
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
                    span: Span::new(start, self.current_token.position),
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
                    span: Span::new(start, self.current_token.position),
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

    fn parse_prefix_expression(&mut self) -> Option<Expression> {
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

    fn parse_grouped_expression(&mut self) -> Option<Expression> {
        self.next_token();
        let expression = self.parse_expression(Precedence::Lowest)?;
        if !self.expect_peek(TokenType::RParen) {
            return None;
        }
        Some(expression)
    }

    fn parse_array(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let elements = self.parse_expression_list(TokenType::RBracket)?;
        Some(Expression::Array {
            elements,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_hash(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_if_expression(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_function_literal(&mut self) -> Option<Expression> {
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
            span: Span::new(start, self.current_token.position),
        })
    }

    /// Parse a lambda expression: \x -> expr, \(x, y) -> expr, \() -> expr
    fn parse_lambda(&mut self) -> Option<Expression> {
        let start = self.current_token.position;

        // Move past the backslash
        self.next_token();

        // Parse parameters
        let parameters = if self.is_current_token(TokenType::LParen) {
            // Parenthesized parameters: \() -> or \(x) -> or \(x, y) ->
            let params = self.parse_function_parameters()?;
            self.next_token(); // move past )
            params
        } else if self.is_current_token(TokenType::Ident) {
            // Single unparenthesized parameter: \x ->
            let param = self.current_token.literal.clone();
            self.next_token();
            vec![param]
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
        if !self.is_current_token(TokenType::Arrow) {
            self.errors.push(
                Diagnostic::error("EXPECTED ARROW")
                    .with_code("E107")
                    .with_error_type(ErrorType::Compiler)
                    .with_span(self.current_token.span())
                    .with_message(format!(
                        "Expected `->` after lambda parameters, found `{}`.",
                        self.current_token.token_type
                    ))
                    .with_note(
                        "Lambda functions are anonymous functions defined with backslash syntax",
                    )
                    .with_help("Use `\\parameter -> expression` for the lambda syntax")
                    .with_example("let double = \\x -> x * 2;\nlet add = \\(a, b) -> a + b;"),
            );
            return None;
        }
        self.next_token(); // consume ->

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
                    span: Span::new(expr_start, self.current_token.position),
                }],
                span: expr_span,
            }
        };

        Some(Expression::Function {
            parameters,
            body,
            span: Span::new(start, self.current_token.position),
        })
    }

    fn parse_function_parameters(&mut self) -> Option<Vec<String>> {
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

    fn parse_block(&mut self) -> Block {
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

    fn parse_expression_list(&mut self, end: TokenType) -> Option<Vec<Expression>> {
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
            let eof_pos = crate::frontend::position::Position::new(line, usize::MAX - 1);
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

    fn is_current_token(&self, token_type: TokenType) -> bool {
        self.current_token.token_type == token_type
    }

    fn is_peek_token(&self, token_type: TokenType) -> bool {
        self.peek_token.token_type == token_type
    }

    fn expect_peek(&mut self, token_type: TokenType) -> bool {
        if self.is_peek_token(token_type) {
            self.next_token();
            true
        } else {
            self.peek_error(token_type);
            false
        }
    }

    fn current_precedence(&self) -> Precedence {
        token_precedence(&self.current_token.token_type)
    }

    fn peek_precedence(&self) -> Precedence {
        token_precedence(&self.peek_token.token_type)
    }

    fn peek_error(&mut self, expected: TokenType) {
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
