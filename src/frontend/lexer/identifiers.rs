//! Identifier parsing

use super::Lexer;

impl Lexer {
    pub(super) fn read_identifier_span(&mut self) -> (usize, usize) {
        let start = self.current_index();
        self.reader.consume_identifier_continue_run();
        (start, self.current_index())
    }

    pub(super) fn read_identifier(&mut self) -> String {
        let (start, end) = self.read_identifier_span();
        self.slice_chars(start, end)
    }
}
