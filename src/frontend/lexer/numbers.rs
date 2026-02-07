//! Number literal parsing (integers and floats)
//!
//! Supports:
//! - Decimal integers: 42, 1_000_000
//! - Decimal floats: 3.14, 2.5e10, 1.5e-3
//! - Hexadecimal: 0xFF, 0x1A_BC
//! - Binary: 0b1010, 0b1111_0000
//! - Underscores for readability in all formats

use super::Lexer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumberKind {
    Int,
    Float,
}

impl Lexer {
    pub(super) fn read_number_span(&mut self) -> ((usize, usize), NumberKind) {
        if self.current_byte() == Some(b'0') && matches!(self.peek_byte(), Some(b'x' | b'X')) {
            return (self.read_hex_span(), NumberKind::Int);
        }

        if self.current_byte() == Some(b'0') && matches!(self.peek_byte(), Some(b'b' | b'B')) {
            return (self.read_binary_span(), NumberKind::Int);
        }

        self.read_decimal_span()
    }

    fn read_hex_span(&mut self) -> (usize, usize) {
        let start = self.current_index();

        self.read_char(); // '0'
        self.read_char(); // 'x'/'X'
        self.reader.consume_hex_run();

        (start, self.current_index())
    }

    fn read_binary_span(&mut self) -> (usize, usize) {
        let start = self.current_index();

        self.read_char(); // '0'
        self.read_char(); // 'b'/'B'
        self.reader.consume_binary_run();

        (start, self.current_index())
    }

    fn read_decimal_span(&mut self) -> ((usize, usize), NumberKind) {
        let start = self.current_index();

        self.reader.consume_decimal_run();

        let mut kind = NumberKind::Int;

        if self.current_byte() == Some(b'.') && self.peek_byte().is_some_and(|b| b.is_ascii_digit())
        {
            kind = NumberKind::Float;
            self.read_char();
            self.reader.consume_decimal_run();
        }

        if self.current_byte().is_some_and(|b| b == b'e' || b == b'E') {
            kind = NumberKind::Float;
            self.read_char();

            if self.current_byte().is_some_and(|b| b == b'+' || b == b'-') {
                self.read_char();
            }

            self.reader.consume_decimal_run();
        }

        ((start, self.current_index()), kind)
    }
}
