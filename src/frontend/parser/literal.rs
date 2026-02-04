use crate::frontend::{
    diagnostics::{Diagnostic, ErrorType},
    expression::{Expression, StringPart},
    position::{Position, Span},
    precedence::Precedence,
    token_type::TokenType,
};

use super::Parser;

impl Parser {
    pub(super) fn parse_identifier(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let mut name = self.current_token.literal.clone();
        // Only collect dotted segments for module paths (PascalCase names)
        // Don't collect ALL_CAPS constants like PI, TAU, MAX
        if super::is_pascal_case_ident(&self.current_token) {
            while self.is_peek_token(TokenType::Dot) && super::is_pascal_case_ident(&self.peek2_token) {
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

    pub(super) fn parse_integer(&mut self) -> Option<Expression> {
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

    pub(super) fn parse_float(&mut self) -> Option<Expression> {
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

    pub(super) fn parse_string(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let first_part = self.current_token.literal.clone();

        // Simple string - no interpolation
        Some(Expression::String {
            value: first_part,
            span: Span::new(start, self.current_token.position),
        })
    }

    pub(super) fn parse_interpolation_start(&mut self) -> Option<Expression> {
        let start = self.current_token.position;
        let first_part = self.current_token.literal.clone();

        // InterpolationStart token signals the lexer found #{
        // Now parse as interpolated string
        self.parse_interpolated_string(start, first_part)
    }

    /// Check if peek_token could be the start of an interpolation expression
    pub(super) fn is_interpolation_expression_start(&self) -> bool {
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
    pub(super) fn parse_interpolated_string(
        &mut self,
        start: Position,
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

    pub(super) fn parse_boolean(&self) -> Option<Expression> {
        let start = self.current_token.position;
        Some(Expression::Boolean {
            value: self.current_token.token_type == TokenType::True,
            span: Span::new(start, self.current_token.position),
        })
    }
}
