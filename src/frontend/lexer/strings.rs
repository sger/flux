//! String literal parsing and interpolation handling

use crate::frontend::lexeme::Lexeme;
use crate::frontend::token::Token;
use crate::frontend::token_type::TokenType;

use super::Lexer;

enum StringContent {
    Span { start: usize, end: usize },
    Owned(String),
}

impl Lexer {
    /// Helper to create a string-family token using the lexer's cursor end position
    fn string_token_with_cursor_end(
        &self,
        token_type: TokenType,
        content: StringContent,
        line: usize,
        column: usize,
    ) -> Token {
        // String-family tokens use source cursor end (raw span), not cooked literal length.
        match content {
            StringContent::Span { start, end } => Token::new_span_with_end(
                token_type,
                self.source_arc(),
                start,
                end,
                line,
                column,
                self.cursor_position(),
            ),
            StringContent::Owned(content) => Token::new_with_end(
                token_type,
                Lexeme::Owned(content),
                line,
                column,
                self.cursor_position(),
            ),
        }
    }

    /// Read the start of a string (called when we see opening ")
    pub(super) fn read_string_start(&mut self) -> Token {
        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;
        self.read_char(); // skip opening quote

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // String has interpolation - mark that we're in a string
            // Invariant: depth = 1 because we consumed the '{' of '#{' internally.
            self.enter_interpolated_string();
            // Return InterpolationStart instead of String to signal interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, column)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, column)
        } else {
            // Simple string with no interpolation
            self.string_token_with_cursor_end(TokenType::String, content, line, column)
        }
    }

    /// Continue reading a string after an interpolation expression
    pub(super) fn continue_string(&mut self) -> Token {
        debug_assert!(self.in_interpolated_string_context());
        debug_assert!(!self.is_in_interpolation());

        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;

        let (content, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // More interpolations to come - reset depth since we consumed the '{' of '#{'
            // Invariant: reset to 1 because '#{' consumed the '{' already.
            self.reset_current_interpolation_depth();
            // Return InterpolationStart to signal another interpolation
            self.string_token_with_cursor_end(TokenType::InterpolationStart, content, line, column)
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::UnterminatedString, content, line, column)
        } else {
            // End of interpolated string
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(TokenType::StringEnd, content, line, column)
        }
    }

    /// Read string content until we hit closing quote or interpolation start
    /// Returns (content, ended_with_quote, has_interpolation)
    fn read_string_content(&mut self) -> (StringContent, bool, bool) {
        let span_start = self.current_index();

        while let Some(c) = self.current_byte() {
            match c {
                b'\n' | b'\r' => {
                    // Strings cannot span lines
                    return (
                        StringContent::Span {
                            start: span_start,
                            end: self.current_index(),
                        },
                        false,
                        false,
                    );
                }
                b'"' => {
                    // End of string
                    let end = self.current_index();
                    self.read_char(); // consume closing quote
                    return (
                        StringContent::Span {
                            start: span_start,
                            end,
                        },
                        true,
                        false,
                    );
                }
                b'#' if self.peek_byte() == Some(b'{') => {
                    // Start of interpolation
                    let end = self.current_index();
                    self.read_char(); // consume '#'
                    self.read_char(); // consume '{'
                    return (
                        StringContent::Span {
                            start: span_start,
                            end,
                        },
                        false,
                        true,
                    );
                }
                b'\\' => {
                    // Escape sequence
                    let mut owned = String::with_capacity(self.current_index() - span_start + 16);
                    owned.push_str(self.slice_str(span_start, self.current_index()));
                    self.read_char(); // consume backslash '\\'

                    match self.read_escape_sequence() {
                        Some(escaped) => owned.push(escaped),
                        None => {
                            // EOF right after backslash inside a string.
                            // Keep the raw backslash in the token literal and terminate.
                            owned.push('\\');
                            return (StringContent::Owned(owned), false, false);
                        }
                    }

                    return self.read_string_content_slow(owned);
                }
                _ => self.read_char(),
            }
        }

        // Hit EOF without closing quote
        (
            StringContent::Span {
                start: span_start,
                end: self.current_index(),
            },
            false,
            false,
        )
    }

    fn read_string_content_slow(&mut self, mut owned: String) -> (StringContent, bool, bool) {
        loop {
            let run_start = self.current_index();
            self.reader.advance_until_special_in_string();
            let run_end = self.current_index();

            if run_end > run_start {
                owned.push_str(self.slice_str(run_start, run_end));
            }

            match self.current_byte() {
                Some(b'\n' | b'\r') => return (StringContent::Owned(owned), false, false),
                Some(b'"') => {
                    self.read_char();
                    return (StringContent::Owned(owned), true, false);
                }
                Some(b'#') if self.peek_byte() == Some(b'{') => {
                    self.read_char();
                    self.read_char();
                    return (StringContent::Owned(owned), false, true);
                }
                Some(b'\\') => {
                    self.read_char();
                    match self.read_escape_sequence() {
                        Some(ch) => owned.push(ch),
                        None => {
                            owned.push('\\');
                            return (StringContent::Owned(owned), false, false);
                        }
                    }
                }
                Some(_) => {
                    self.read_char();
                }
                None => return (StringContent::Owned(owned), false, false),
            }
        }
    }
}
