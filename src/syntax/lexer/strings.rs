//! String literal parsing and interpolation handling

use crate::syntax::token::Token;
use crate::syntax::token_type::TokenType;

use super::Lexer;

impl Lexer {
    /// Helper to create a string-family token using the lexer's cursor end position
    fn string_token_with_cursor_end(
        &self,
        token_type: TokenType,
        content_start: usize,
        content_end: usize,
        line: usize,
        column: usize,
    ) -> Token {
        // String-family tokens use source cursor end (raw span), not cooked literal length.
        Token::new_span_with_end(
            token_type,
            self.source_arc(),
            content_start,
            content_end,
            line,
            column,
            self.cursor_position(),
        )
    }

    /// Read the start of a string (called when we see opening ")
    pub(super) fn read_string_start(&mut self) -> Token {
        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;
        self.read_char(); // skip opening quote

        let (content_start, content_end, ended, has_interpolation) = self.read_string_content();

        if has_interpolation {
            // String has interpolation - mark that we're in a string
            // Invariant: depth = 1 because we consumed the '{' of '#{' internally.
            self.enter_interpolated_string();
            // Return InterpolationStart instead of String to signal interpolation
            self.string_token_with_cursor_end(
                TokenType::InterpolationStart,
                content_start,
                content_end,
                line,
                column,
            )
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.string_token_with_cursor_end(
                TokenType::UnterminatedString,
                content_start,
                content_end,
                line,
                column,
            )
        } else {
            // Simple string with no interpolation
            self.string_token_with_cursor_end(
                TokenType::String,
                content_start,
                content_end,
                line,
                column,
            )
        }
    }

    /// Read the start of a triple-quoted multi-line string (called when we see `"""`).
    pub(super) fn read_multiline_string_start(&mut self) -> Token {
        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;

        // Consume the three opening quotes.
        self.read_char();
        self.read_char();
        self.read_char();

        // Skip a newline immediately following `"""` (standard behaviour).
        match self.current_byte() {
            Some(b'\n') => {
                self.read_char();
            }
            Some(b'\r') => {
                self.read_char();
                if self.current_byte() == Some(b'\n') {
                    self.read_char();
                }
            }
            _ => {}
        }

        let (content_start, content_end, ended, has_interpolation) =
            self.read_multiline_string_content();

        if has_interpolation {
            self.enter_multiline_interpolated_string();
            self.string_token_with_cursor_end(
                TokenType::InterpolationStart,
                content_start,
                content_end,
                line,
                column,
            )
        } else if !ended {
            self.string_token_with_cursor_end(
                TokenType::UnterminatedString,
                content_start,
                content_end,
                line,
                column,
            )
        } else {
            self.string_token_with_cursor_end(
                TokenType::String,
                content_start,
                content_end,
                line,
                column,
            )
        }
    }

    /// Scan multi-line string content until `"""`, interpolation `#{`, or EOF.
    /// Unlike `read_string_content`, newlines are allowed and do not terminate the scan.
    /// Returns `(content_start, content_end, ended_with_triple_quote, has_interpolation)`.
    fn read_multiline_string_content(&mut self) -> (usize, usize, bool, bool) {
        let span_start = self.current_index();
        let src_len = self.reader.source_len();
        let mut i = span_start;

        loop {
            // Fast scan: consume plain bytes. Newlines are allowed here.
            while i < src_len {
                let b = self.reader.byte_at(i).unwrap_or_default();
                let b1 = self.reader.byte_at(i + 1);
                if b == b'"' || b == b'\\' || b >= 0x80 || (b == b'#' && b1 == Some(b'{')) {
                    break;
                }
                i += 1;
            }

            if i >= src_len {
                // EOF without closing `"""`.
                self.reader.seek_to(i);
                return (span_start, i, false, false);
            }

            match self.reader.byte_at(i).unwrap_or_default() {
                b'"' => {
                    // Check for closing `"""`.
                    if self.reader.byte_at(i + 1) == Some(b'"')
                        && self.reader.byte_at(i + 2) == Some(b'"')
                    {
                        let end = i;
                        self.reader.seek_to(i + 3); // consume `"""`
                        return (span_start, end, true, false);
                    }
                    // Single or double quote inside the string — keep scanning.
                    i += 1;
                }
                b'#' => {
                    // Interpolation start `#{`.
                    let end = i;
                    self.reader.seek_to(i + 2); // consume `#{`
                    return (span_start, end, false, true);
                }
                b'\\' => {
                    // Escape sequence — use the shared slow path.
                    self.reader.seek_to(i);
                    self.consume_escape_sequence();
                    i = self.current_index();
                }
                _ => {
                    // Non-ASCII Unicode scalar.
                    self.reader.seek_to(i);
                    self.read_char();
                    i = self.current_index();
                }
            }
        }
    }

    /// Continue reading a string after an interpolation expression
    pub(super) fn continue_string(&mut self) -> Token {
        debug_assert!(self.in_interpolated_string_context());
        debug_assert!(!self.is_in_interpolation());

        let cursor = self.cursor_position();
        let line = cursor.line;
        let column = cursor.column;

        let (content_start, content_end, ended, has_interpolation) =
            if self.is_in_multiline_string() {
                self.read_multiline_string_content()
            } else {
                self.read_string_content()
            };

        if has_interpolation {
            // More interpolations to come - reset depth since we consumed the '{' of '#{'
            // Invariant: reset to 1 because '#{' consumed the '{' already.
            self.reset_current_interpolation_depth();
            // Return InterpolationStart to signal another interpolation
            self.string_token_with_cursor_end(
                TokenType::InterpolationStart,
                content_start,
                content_end,
                line,
                column,
            )
        } else if !ended {
            // Hit newline or EOF without closing quote
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(
                TokenType::UnterminatedString,
                content_start,
                content_end,
                line,
                column,
            )
        } else {
            // End of interpolated string
            self.exit_interpolated_string();
            self.string_token_with_cursor_end(
                TokenType::StringEnd,
                content_start,
                content_end,
                line,
                column,
            )
        }
    }

    /// Read string content until we hit closing quote or interpolation start
    /// Returns (content, ended_with_quote, has_interpolation)
    fn read_string_content(&mut self) -> (usize, usize, bool, bool) {
        let span_start = self.current_index();
        let len = self.reader.source_len();
        let mut i = span_start;

        loop {
            // ASCII scan: skip plain content until a special delimiter.
            while i < len {
                let b = self.reader.byte_at(i).unwrap_or_default();
                let b1 = self.reader.byte_at(i + 1);
                if b == b'\\'
                    || b == b'"'
                    || b == b'\n'
                    || b == b'\r'
                    || b >= 0x80
                    || (b == b'#' && b1 == Some(b'{'))
                {
                    break;
                }
                i += 1;
            }

            if i >= len {
                // EOF before closing quote.
                self.reader.seek_to_ascii_no_newline(i);
                return (span_start, i, false, false);
            }

            match self.reader.byte_at(i).unwrap_or_default() {
                b'\n' | b'\r' => {
                    // Strings cannot span lines.
                    self.reader.seek_to_ascii_no_newline(i);
                    return (span_start, i, false, false);
                }
                b'"' => {
                    // End of string.
                    let end = i;
                    self.reader.seek_to_ascii_no_newline(i + 1);
                    return (span_start, end, true, false);
                }
                b'#' => {
                    // Interpolation start '#{'.
                    let end = i;
                    self.reader.seek_to_ascii_no_newline(i + 2); // consume '#{'
                    return (span_start, end, false, true);
                }
                b'\\' => match self.reader.byte_at(i + 1) {
                    // Path for valid ASCII escapes.
                    Some(b'n' | b't' | b'r' | b'\\' | b'"' | b'#') => {
                        i += 2;
                    }
                    // Slow path preserves warning behavior for invalid escapes.
                    Some(_) => {
                        self.reader.seek_to(i);
                        self.consume_escape_sequence();
                        i = self.current_index();
                    }
                    // Trailing backslash at EOF.
                    None => {
                        self.reader.seek_to_ascii_no_newline(i + 1);
                        return (span_start, i + 1, false, false);
                    }
                },
                _ => {
                    // Non-ASCII fallback keeps UTF-8 behavior correct.
                    self.reader.seek_to(i);
                    self.read_char();
                    i = self.current_index();
                }
            }
        }
    }
}
