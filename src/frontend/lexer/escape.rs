//! Escape sequence handling for string literals

use super::{Lexer, LexerWarning};

impl Lexer {
    /// Process an escape sequence after seeing backslash
    pub(super) fn read_escape_sequence(&mut self) -> Option<char> {
        let warning_position = self.cursor_position();

        let (result, advance_char) = match self.current_byte() {
            Some(b'n') => (Some('\n'), true),
            Some(b't') => (Some('\t'), true),
            Some(b'r') => (Some('\r'), true),
            Some(b'\\') => (Some('\\'), true),
            Some(b'"') => (Some('"'), true),
            Some(b'#') => (Some('#'), true), // \# for literal #
            Some(byte) if byte.is_ascii() => {
                let c = byte as char;
                // Unknown escape - emit warning and return the character as-is
                self.warnings.push(LexerWarning {
                    message: format!(
                        "Unknown escape sequence '\\{}'. Valid escapes are: \\n \\t \\r \\\\ \\\" \\#",
                        c
                    ),
                    position: warning_position,
                });
                (Some(c), true)
            }
            Some(_) => {
                let char_start = self.current_index();
                self.read_char();
                let char_end = self.current_index();
                let c = self
                    .slice_str(char_start, char_end)
                    .chars()
                    .next()
                    .unwrap_or('\u{FFFD}');

                self.warnings.push(LexerWarning {
                    message: format!(
                        "Unknown escape sequence '\\{}'. Valid escapes are: \\n \\t \\r \\\\ \\\" \\#",
                        c
                    ),
                    position: warning_position,
                });

                return Some(c);
            }
            None => return None,
        };

        if advance_char {
            self.read_char();
        }
        result
    }
}
