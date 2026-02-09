//! Escape sequence handling for string literals

use super::{Lexer, LexerWarning};

impl Lexer {
    /// Validate and consume an escape sequence starting at backslash.
    /// Leaves the lexer at the next unread byte after the escape.
    pub(super) fn consume_escape_sequence(&mut self) {
        if self.current_byte() != Some(b'\\') {
            return;
        }

        match self.peek_byte() {
            // Fast path: valid ASCII escape consumes two bytes in one step.
            Some(b'n' | b't' | b'r' | b'\\' | b'"' | b'#') => {
                self.reader.advance_ascii_bytes(2);
            }
            Some(byte) if byte.is_ascii() => {
                self.reader.advance_ascii_bytes(1); // consume '\'
                let warning_position = self.cursor_position();
                let c = byte as char;

                self.warnings.push(LexerWarning {
                    message: format!("Unknown escape sequence '\\{}. Valid escapes are: \\n \\t \\t \\\\ \\\" \\#",
                        c),
                    position: warning_position,
                });
                self.reader.advance_ascii_bytes(1); // consume escaped byte
            }
            Some(_) => {
                self.read_char(); // consume '\'
                let warning_position = self.cursor_position();

                let char_start = self.current_index();
                self.read_char(); // consume escaped scalar
                let char_end = self.current_index();

                let c = self
                    .slice_str(char_start, char_end)
                    .chars()
                    .next()
                    .unwrap_or('\u{FFFD}');

                self.warnings.push(LexerWarning {
                    message: format!("Unknown escape sequence '\\{}. Valid escapes are: \\n \\t \\t \\\\ \\\" \\#",
                        c),
                    position: warning_position,
                });
            }
            None => {
                // Trailing backslash at EOF/newline context: consume '\' and let caller
                // decide whether the string is unterminated.
                self.reader.advance_ascii_bytes(1);
            }
        }
    }
}
