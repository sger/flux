//! Character-level source reader for the lexer.
//!
//! Invariants:
//! - `current()` returns the character at the current cursor, or `None` at EOF.
//! - `advance()` moves the cursor forward by one character and updates line/column
//!   based on the previous current character.
//! - Columns are 0-based and match existing lexer diagnostics behavior.

use crate::frontend::position::Position;

#[derive(Debug, Clone)]
pub(super) struct CharReader {
    input: Vec<char>,
    position: usize,
    read_position: usize,
    current_char: Option<char>,
    line: usize,
    column: usize,
}

impl CharReader {
    pub(super) fn new(input: String) -> Self {
        let mut reader = Self {
            input: input.chars().collect(),
            position: 0,
            read_position: 0,
            current_char: None,
            line: 1,
            column: 0,
        };
        reader.advance();
        reader
    }

    pub(super) fn current(&self) -> Option<char> {
        self.current_char
    }

    pub(super) fn advance(&mut self) -> Option<char> {
        if self.current_char.is_none() && self.read_position > self.input.len() {
            return None;
        }

        // Update logical source position based on the previous current char.
        if self.current_char == Some('\n') {
            self.line += 1;
            self.column = 0;
        } else if self.current_char.is_some() {
            self.column += 1;
        }

        self.current_char = self.input.get(self.read_position).copied();
        self.position = self.read_position;
        self.read_position += 1;
        self.current_char
    }

    pub(super) fn peek(&self) -> Option<char> {
        self.input.get(self.read_position).copied()
    }

    pub(super) fn peek_n(&self, n: usize) -> Option<char> {
        if n == 0 {
            return None;
        }

        self.read_position
            .checked_add(n - 1)
            .and_then(|idx| self.input.get(idx).copied())
    }

    pub(super) fn index(&self) -> usize {
        self.position
    }

    pub(super) fn position(&self) -> Position {
        Position::new(self.line, self.column)
    }

    pub(super) fn slice(&self, start: usize, end: usize) -> String {
        self.input[start..end].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::CharReader;
    use crate::frontend::position::Position;

    #[test]
    fn tracks_line_and_column_across_newlines() {
        let mut reader = CharReader::new("a\nb".to_string());
        assert_eq!(reader.current(), Some('a'));
        assert_eq!(reader.position(), Position::new(1, 0));

        reader.advance(); // '\n'
        assert_eq!(reader.current(), Some('\n'));
        assert_eq!(reader.position(), Position::new(1, 1));

        reader.advance(); // 'b'
        assert_eq!(reader.current(), Some('b'));
        assert_eq!(reader.position(), Position::new(2, 0));
    }

    #[test]
    fn peek_n_reads_future_chars_without_advancing() {
        let reader = CharReader::new("abc".to_string());
        assert_eq!(reader.current(), Some('a'));
        assert_eq!(reader.peek(), Some('b'));
        assert_eq!(reader.peek_n(1), Some('b'));
        assert_eq!(reader.peek_n(2), Some('c'));
        assert_eq!(reader.peek_n(3), None);
    }

    #[test]
    fn peek_n_zero_returns_none() {
        let reader = CharReader::new("abc".to_string());
        assert_eq!(reader.peek_n(0), None);
    }

    #[test]
    fn eof_advance_is_stable() {
        let mut reader = CharReader::new("a".to_string());
        assert_eq!(reader.current(), Some('a'));

        reader.advance();
        assert_eq!(reader.current(), None);
        let eof_pos = reader.position();

        reader.advance();
        reader.advance();
        assert_eq!(reader.current(), None);
        assert_eq!(reader.position(), eof_pos);
    }
}
