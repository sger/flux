use crate::frontend::{
    block::Block,
    diagnostic::Diagnostic,
    expression::Expression,
    lexer::Lexer,
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
    pub errors: Vec<Diagnostic>,
}

impl Parser {
    pub fn new(lexer: Lexer) -> Self {
        let mut parser = Parser {
            lexer,
            current_token: Token::new(TokenType::Eof, "", 0, 0),
            peek_token: Token::new(TokenType::Eof, "", 0, 0),
            errors: Vec::new(),
        };
        parser.next_token();
        parser.next_token();
        parser
    }

    pub fn parse_program(&mut self) -> Program {
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

        program
    }

    fn next_token(&mut self) {
        self.current_token = self.peek_token.clone();
        self.peek_token = self.lexer.next_token();
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
            TokenType::Let => self.parse_let_statement(),
            TokenType::Return => self.parse_return_statement(),
            TokenType::Fun if self.is_peek_token(TokenType::Ident) => {
                self.parse_function_statement()
            }
            TokenType::Ident if self.current_token.literal == "fn" => {
                self.errors.push(
                    Diagnostic::error("unknown keyword `fn`")
                        .with_position(self.current_token.position)
                        .with_message("Flux uses `fun` for function declarations")
                        .with_hint("Replace it with `fun`."),
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
                    Diagnostic::error(format!("unknown keyword `{}`", self.current_token.literal))
                        .with_position(self.current_token.position)
                        .with_message("Flux uses `fun` for function declarations")
                        .with_hint("Did you mean `fun`?"),
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
        let position = self.current_token.position;
        let expression = self.parse_expression(Precedence::Lowest)?;

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Expression {
            expression,
            position,
        })
    }

    fn parse_function_statement(&mut self) -> Option<Statement> {
        let position = self.current_token.position;

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
            position,
        })
    }

    fn parse_return_statement(&mut self) -> Option<Statement> {
        let position = self.current_token.position;
        self.next_token();

        let value = if self.is_current_token(TokenType::Semicolon) {
            None
        } else {
            Some(self.parse_expression(Precedence::Lowest)?)
        };

        if self.is_peek_token(TokenType::Semicolon) {
            self.next_token();
        }

        Some(Statement::Return { value, position })
    }

    fn parse_let_statement(&mut self) -> Option<Statement> {
        let position = self.current_token.position;

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
            position,
        })
    }

    fn parse_assignment_statement(&mut self) -> Option<Statement> {
        let position = self.current_token.position;
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
            position,
        })
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
            TokenType::True | TokenType::False => self.parse_boolean(),
            TokenType::Null => self.parse_null(),
            TokenType::Bang | TokenType::Minus => self.parse_prefix_expression(),
            TokenType::LParen => self.parse_grouped_expression(),
            TokenType::LBracket => self.parse_array(),
            TokenType::LBrace => self.parse_hash(),
            TokenType::If => self.parse_if_expression(),
            TokenType::Fun => self.parse_function_literal(),
            _ => {
                self.no_prefix_parse_error();
                None
            }
        }
    }

    fn no_prefix_parse_error(&mut self) {
        self.errors.push(
            Diagnostic::error(format!(
                "no prefix parse for {}",
                self.current_token.token_type
            ))
            .with_position(self.current_token.position)
            .with_message("expected an expression here"),
        );
    }

    fn parse_infix(&mut self, left: Expression) -> Option<Expression> {
        match self.current_token.token_type {
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Asterisk
            | TokenType::Slash
            | TokenType::Lt
            | TokenType::Gt
            | TokenType::Eq
            | TokenType::NotEq => self.parse_infix_expression(left),
            TokenType::LParen => self.parse_call_expression(left),
            TokenType::LBracket => self.parse_index_expression(left),
            _ => Some(left),
        }
    }

    fn parse_infix_expression(&mut self, left: Expression) -> Option<Expression> {
        let operator = self.current_token.literal.clone();
        let precedence = self.current_precedence();
        self.next_token();
        let right = self.parse_expression(precedence)?;
        Some(Expression::Infix {
            left: Box::new(left),
            operator,
            right: Box::new(right),
        })
    }

    fn parse_call_expression(&mut self, function: Expression) -> Option<Expression> {
        let arguments = self.parse_expression_list(TokenType::RParen)?;
        Some(Expression::Call {
            function: Box::new(function),
            arguments,
        })
    }

    fn parse_index_expression(&mut self, left: Expression) -> Option<Expression> {
        self.next_token();
        let index = self.parse_expression(Precedence::Lowest)?;

        if !self.expect_peek(TokenType::RBracket) {
            return None;
        }

        Some(Expression::Index {
            left: Box::new(left),
            index: Box::new(index),
        })
    }

    fn parse_identifier(&mut self) -> Option<Expression> {
        Some(Expression::Identifier(self.current_token.literal.clone()))
    }

    fn parse_integer(&mut self) -> Option<Expression> {
        match self.current_token.literal.parse::<i64>() {
            Ok(value) => Some(Expression::Integer(value)),
            Err(_) => {
                self.errors.push(
                    Diagnostic::error(format!(
                        "could not parse {} as integer",
                        self.current_token.literal
                    ))
                    .with_position(self.current_token.position),
                );
                None
            }
        }
    }

    fn parse_float(&mut self) -> Option<Expression> {
        match self.current_token.literal.parse::<f64>() {
            Ok(value) => Some(Expression::Float(value)),
            Err(_) => {
                self.errors.push(
                    Diagnostic::error(format!(
                        "could not parse {} as float",
                        self.current_token.literal
                    ))
                    .with_position(self.current_token.position),
                );
                None
            }
        }
    }

    fn parse_string(&self) -> Option<Expression> {
        Some(Expression::String(self.current_token.literal.clone()))
    }

    fn parse_boolean(&self) -> Option<Expression> {
        Some(Expression::Boolean(
            self.current_token.token_type == TokenType::True,
        ))
    }

    fn parse_null(&self) -> Option<Expression> {
        Some(Expression::Null)
    }

    fn parse_prefix_expression(&mut self) -> Option<Expression> {
        let operator = self.current_token.literal.clone();
        self.next_token();
        let right = self.parse_expression(Precedence::Prefix)?;
        Some(Expression::Prefix {
            operator,
            right: Box::new(right),
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
        let elements = self.parse_expression_list(TokenType::RBracket)?;
        Some(Expression::Array { elements })
    }

    fn parse_hash(&mut self) -> Option<Expression> {
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

        Some(Expression::Hash { pairs })
    }

    fn parse_if_expression(&mut self) -> Option<Expression> {
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
        })
    }

    fn parse_function_literal(&mut self) -> Option<Expression> {
        if !self.expect_peek(TokenType::LParen) {
            return None;
        }

        let parameters = self.parse_function_parameters()?;

        if !self.expect_peek(TokenType::LBrace) {
            return None;
        }

        let body = self.parse_block();
        Some(Expression::Function { parameters, body })
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
        let mut statements = Vec::new();
        self.next_token();

        while !self.is_current_token(TokenType::RBrace) && !self.is_current_token(TokenType::Eof) {
            if let Some(statement) = self.parse_statement() {
                statements.push(statement);
            }
            self.next_token();
        }

        Block { statements }
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

        if !self.expect_peek(end) {
            return None;
        }

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
            Diagnostic::error(format!(
                "expected {}, got {}",
                expected, self.peek_token.token_type
            ))
            .with_position(self.peek_token.position)
            .with_message("unexpected token"),
        );
    }
}
