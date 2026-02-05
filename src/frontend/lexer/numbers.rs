//! Number literal parsing (integers and floats)

use super::Lexer;

impl Lexer {
    pub(super) fn read_number(&mut self) -> (String, bool) {
        let start = self.position;
        while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
            self.read_char();
        }
        let mut is_float = false;
        if self.current_char == Some('.') && self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.read_char(); // consume '.'
            while self.current_char.is_some_and(|c| c.is_ascii_digit()) {
                self.read_char();
            }
        }
        let literal: String = self.input[start..self.position].iter().collect();
        (literal, is_float)
    }
}
