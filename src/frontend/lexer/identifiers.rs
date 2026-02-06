//! Identifier parsing

use super::{Lexer, helpers::is_letter};

impl Lexer {
    pub(super) fn read_identifier(&mut self) -> String {
        let start = self.current_index();
        while self
            .current_char()
            .is_some_and(|c| is_letter(c) || c.is_ascii_digit())
        {
            self.read_char();
        }
        self.slice_chars(start, self.current_index())
    }
}
