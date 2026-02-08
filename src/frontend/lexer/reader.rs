//! Byte-indexed source reader for the lexer.
//!
//! Invariants:
//! - `position` and `read_position` are byte offsets into `source`.
//! - `current()` returns the decoded scalar at `position`.
//! - Line/column are updated per decoded char (column is char-count, 0-based).

use std::rc::Rc;

use crate::frontend::position::Position;

#[derive(Debug, Clone)]
pub(super) struct CharReader {
    source: Rc<str>,
    position: usize,
    read_position: usize,
    current_char: Option<char>,
    line: usize,
    column: usize,
}

impl CharReader {
    pub(super) fn new(input: String) -> Self {
        let source: Rc<str> = Rc::from(input);

        let mut reader = Self {
            source,
            position: 0,
            read_position: 0,
            current_char: None,
            line: 1,
            column: 0,
        };
        reader.advance();
        reader
    }

    pub(super) fn source_arc(&self) -> Rc<str> {
        Rc::clone(&self.source)
    }

    #[inline(always)]
    pub(super) fn source_len(&self) -> usize {
        self.source.len()
    }

    #[inline(always)]
    pub(super) fn byte_at(&self, idx: usize) -> Option<u8> {
        self.bytes().get(idx).copied()
    }

    pub(super) fn current(&self) -> Option<char> {
        self.current_char
    }

    pub(super) fn current_byte(&self) -> Option<u8> {
        self.bytes().get(self.position).copied()
    }

    #[inline(always)]
    pub(super) fn seek_to(&mut self, new_position: usize) {
        if new_position == self.position {
            return;
        }

        self.set_current_from(new_position.min(self.bytes().len()));
    }

    #[inline(always)]
    pub(super) fn seek_to_ascii_no_newline(&mut self, new_position: usize) {
        let target = new_position.min(self.bytes().len());

        if target <= self.position {
            return;
        }

        debug_assert!(
            self.bytes()[self.position..target]
                .iter()
                .all(|b| b.is_ascii() && *b != b'\n')
        );

        self.column += target - self.position;

        if let Some((ch, len)) = self.decode_at(target) {
            self.position = target;
            self.read_position = target + len;
            self.current_char = Some(ch);
        } else {
            self.position = self.bytes().len();
            self.read_position = self.bytes().len() + 1;
            self.current_char = None;
        }
    }

    pub(super) fn advance(&mut self) -> Option<char> {
        if self.current_char.is_none() && self.read_position > self.bytes().len() {
            return None;
        }

        if self.current_char == Some('\n') {
            self.line += 1;
            self.column = 0;
        } else if self.current_char.is_some() {
            self.column += 1;
        }

        let idx = self.read_position;

        if let Some((ch, len)) = self.decode_at(idx) {
            self.position = idx;
            self.read_position = idx + len;
            self.current_char = Some(ch);
            self.current_char
        } else {
            self.position = self.bytes().len();
            self.read_position = self.bytes().len() + 1;
            self.current_char = None;
            None
        }
    }

    pub(super) fn advance_ascii_bytes(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        let target = self.position.saturating_add(n).min(self.bytes().len());
        self.set_current_from(target);
    }

    pub(super) fn peek(&self) -> Option<char> {
        self.decode_at(self.read_position).map(|(ch, _)| ch)
    }

    pub(super) fn peek_byte(&self) -> Option<u8> {
        self.bytes().get(self.read_position).copied()
    }

    pub(super) fn peek2_byte(&self) -> Option<u8> {
        self.bytes().get(self.read_position + 1).copied()
    }

    pub(super) fn peek_n(&self, n: usize) -> Option<char> {
        if n == 0 {
            return None;
        }

        let mut idx = self.read_position;

        for _ in 1..n {
            let (_, len) = self.decode_at(idx)?;
            idx += len;
        }

        self.decode_at(idx).map(|(ch, _)| ch)
    }

    pub(super) fn consume_hex_run(&mut self) {
        self.consume_ascii_while(|b| b == b'_' || b.is_ascii_hexdigit());
    }

    pub(super) fn consume_binary_run(&mut self) {
        self.consume_ascii_while(|b| matches!(b, b'0' | b'1' | b'_'));
    }

    pub(super) fn consume_decimal_run(&mut self) {
        self.consume_ascii_while(|b| b == b'_' || b.is_ascii_digit());
    }

    pub(super) fn consume_identifier_continue_run(&mut self) {
        self.consume_ascii_while(|b| b == b'_' || b.is_ascii_alphanumeric());
    }

    fn consume_ascii_while<F>(&mut self, mut predicate: F)
    where
        F: FnMut(u8) -> bool,
    {
        let bytes = self.bytes();
        let mut idx = self.position;

        while idx < bytes.len() {
            let b = bytes[idx];

            if !predicate(b) {
                break;
            }

            idx += 1;
        }

        if idx != self.position {
            self.set_current_from(idx);
        }
    }

    pub(super) fn index(&self) -> usize {
        self.position
    }

    pub(super) fn position(&self) -> Position {
        Position::new(self.line, self.column)
    }

    pub(super) fn slice(&self, start: usize, end: usize) -> String {
        self.slice_str(start, end).to_string()
    }

    pub(super) fn slice_str(&self, start: usize, end: usize) -> &str {
        self.source.get(start..end).unwrap_or_else(|| {
            panic!(
                "invalid source slice {}..{} for source len {}",
                start,
                end,
                self.source.len()
            )
        })
    }

    pub(super) fn skip_ascii_whitespace(&mut self) {
        self.consume_ascii_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'));
    }

    pub(super) fn advance_until_special_in_string(&mut self) {
        let bytes = self.bytes();
        let mut idx = self.position;
        while idx < bytes.len() {
            match bytes[idx] {
                b'"' | b'\\' | b'\n' | b'\r' => break,
                b'#' if bytes.get(idx + 1) == Some(&b'{') => break,
                _ => idx += 1,
            }
        }

        if idx != self.position {
            self.set_current_from(idx);
        }
    }

    pub(super) fn advance_until_slash_or_star(&mut self) {
        let bytes = self.bytes();
        let mut idx = self.position;
        while idx < bytes.len() {
            match bytes[idx] {
                b'/' | b'*' => break,
                _ => idx += 1,
            }
        }

        if idx != self.position {
            self.set_current_from(idx);
        }
    }

    pub(super) fn skip_line_comment_body(&mut self) {
        let bytes = self.bytes();
        let mut idx = self.position;
        while idx < bytes.len() && bytes[idx] != b'\n' {
            idx += 1;
        }

        self.set_current_from(idx);
    }

    fn set_current_from(&mut self, new_position: usize) {
        if new_position < self.position {
            return;
        }

        self.advance_logical_position(self.position, new_position);

        if let Some((ch, len)) = self.decode_at(new_position) {
            self.position = new_position;
            self.read_position = new_position + len;
            self.current_char = Some(ch);
        } else {
            self.position = self.bytes().len();
            self.read_position = self.bytes().len() + 1;
            self.current_char = None;
        }
    }

    fn advance_logical_position(&mut self, start: usize, end: usize) {
        let Some(slice) = self.source.get(start..end) else {
            return;
        };

        if slice.is_ascii() {
            for &b in slice.as_bytes() {
                if b == b'\n' {
                    self.line += 1;
                    self.column = 0;
                } else {
                    self.column += 1;
                }
            }
        } else {
            for ch in slice.chars() {
                if ch == '\n' {
                    self.line += 1;
                    self.column = 0;
                } else {
                    self.column += 1;
                }
            }
        }
    }

    fn decode_at(&self, idx: usize) -> Option<(char, usize)> {
        let first = *self.bytes().get(idx)?;
        if first.is_ascii() {
            return Some((first as char, 1));
        }

        self.source[idx..]
            .chars()
            .next()
            .map(|ch| (ch, ch.len_utf8()))
    }

    #[inline]
    fn bytes(&self) -> &[u8] {
        self.source.as_bytes()
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

    #[test]
    fn peek2_byte_supports_hot_lookahead() {
        let reader = CharReader::new("///".to_string());
        assert_eq!(reader.current_byte(), Some(b'/'));
        assert_eq!(reader.peek_byte(), Some(b'/'));
        assert_eq!(reader.peek2_byte(), Some(b'/'));
    }

    #[test]
    #[should_panic(expected = "invalid source slice")]
    fn slice_str_panics_on_invalid_utf8_boundary() {
        let reader = CharReader::new("é".to_string());
        let _ = reader.slice_str(1, 2);
    }
}
