//! Escape sequence handling for string literals

use super::{Lexer, LexerWarning};

impl Lexer {
    /// Process an escape sequence after seeing backslash
    pub(super) fn read_escape_sequence(&mut self) -> Option<char> {
        let result = match self.current_char() {
            Some('n') => Some('\n'),
            Some('t') => Some('\t'),
            Some('r') => Some('\r'),
            Some('\\') => Some('\\'),
            Some('"') => Some('"'),
            Some('#') => Some('#'), // \# for literal #
            Some(c) => {
                // Unknown escape - emit warning and return the character as-is
                self.warnings.push(LexerWarning {
                    message: format!(
                        "Unknown escape sequence '\\{}'. Valid escapes are: \\n \\t \\r \\\\ \\\" \\#",
                        c
                    ),
                    position: self.cursor_position(),
                });
                Some(c)
            }
            None => None,
        };
        if self.current_char().is_some() {
            self.read_char();
        }
        result
    }
}
