//! String literal parsing and interpolation handling

use crate::frontend::position::Position;
use crate::frontend::token::Token;
use crate::frontend::token_type::TokenType;

use super::Lexer;

impl Lexer {
    /// Helper to create a string-family token using the lexer's cursor end position
    pub(super) fn string_token_with_cursor_end(
        &self,
        token_type: TokenType,
        content: String,
        line: usize,
        col: usize,
    ) -> Token {
        // String-family tokens use source cursor end (raw span), not cooked literal length.
        Token::new_with_end(
            token_type,
            content,
            line,
            col,
            Position::new(self.line, self.column),
        )
    }

    /// Read the start of a string (called when we see opening ")
    pub(super) fn read_string_start(&mut self) -> Token {
        let line = self.line;
        let col = self.column;
        self.read_char(); // skip opening quote

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // String has interpolation - mark that we're in a string
            // Invariant: depth = 1 because we consumed the '{' of '#{' internally.
            self.enter_interpolated_string();
            // Return InterpolationStart instead of String to signal interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, col)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, col)
        } else {
            // Simple string with no interpolation
            self.string_token_with_cursor_end(TokenType::String, content, line, col)
        }
    }

    /// Continue reading a string after an interpolation expression
    pub(super) fn continue_string(&mut self) -> Token {
        debug_assert!(self.in_interpolated_string_context());
        debug_assert!(!self.is_in_interpolation());

        let line = self.line;
        let col = self.column;

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // More interpolations to come - reset depth since we consumed the '{' of '#{'
            // Invariant: reset to 1 because '#{' consumed the '{' already.
            self.reset_current_interpolation_depth();
            // Return InterpolationStart to signal another interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, col)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, col)
        } else {
            // End of interpolated string
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::StringEnd, content, line, col)
        }
    }

    /// Read string content until we hit closing quote or interpolation start
    /// Returns (content, ended_with_quote, has_interpolation)
    fn read_string_content(&mut self) -> (String, bool, bool) {
        let mut result = String::new();

        while let Some(c) = self.current_char {
            match c {
                '\n' | '\r' => {
                    // Strings cannot span lines
                    return (result, false, false);
                }
                '"' => {
                    // End of string
                    self.read_char(); // consume closing quote
                    return (result, true, false);
                }
                '#' if self.peek_char() == Some('{') => {
                    // Start of interpolation
                    self.read_char(); // consume '#'
                    self.read_char(); // consume '{'
                    return (result, false, true);
                }
                '\\' => {
                    // Escape sequence
                    self.read_char(); // consume backslash
                    match self.read_escape_sequence() {
                        Some(escaped) => result.push(escaped),
                        None => {
                            // EOF right after backslash inside a string.
                            // Keep the raw backslash in the token literal and terminate.
                            result.push('\\');
                            return (result, false, false);
                        }
                    }
                }
                _ => {
                    result.push(c);
                    self.read_char();
                }
            }
        }

        // Hit EOF without closing quote
        (result, false, false)
    }
}
