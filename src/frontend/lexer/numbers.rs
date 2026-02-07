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

    pub(super) fn read_number(&mut self) -> (String, bool) {
        let ((start, end), kind) = self.read_number_span();
        (self.slice_chars(start, end), kind == NumberKind::Float)
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

    /// Read a hexadecimal literal (0x1F, 0xFF, etc.)
    fn read_hex_literal(&mut self) -> (String, bool) {
        let start = self.current_index();

        // Consume '0x' or '0X'
        self.read_char(); // '0'
        self.read_char(); // 'x' or 'X'

        // Read hex digits (0-9, a-f, A-F) and underscores
        while self
            .current_char()
            .is_some_and(|c| c.is_ascii_hexdigit() || c == '_')
        {
            self.read_char();
        }

        let literal = self.slice_chars(start, self.current_index());
        (literal, false) // Hex literals are always integers
    }

    /// Read a binary literal (0b1010, 0b1111_0000, etc.)
    fn read_binary_literal(&mut self) -> (String, bool) {
        let start = self.current_index();

        // Consume '0b' or '0B'
        self.read_char(); // '0'
        self.read_char(); // 'b' or 'B'

        // Read binary digits (0-1) and underscores
        while self
            .current_char()
            .is_some_and(|c| c == '0' || c == '1' || c == '_')
        {
            self.read_char();
        }

        let literal = self.slice_chars(start, self.current_index());
        (literal, false) // Binary literals are always integers
    }

    /// Read a decimal number (integer or float, with optional scientific notation)
    fn read_decimal_number(&mut self) -> (String, bool) {
        let start = self.current_index();

        // Read integer part (including underscores)
        while self
            .current_char()
            .is_some_and(|c| c.is_ascii_digit() || c == '_')
        {
            self.read_char();
        }

        let mut is_float = false;

        // Check for decimal point
        if self.current_char() == Some('.') && self.peek_char().is_some_and(|c| c.is_ascii_digit())
        {
            is_float = true;
            self.read_char(); // consume '.'

            // Read fractional part (including underscores)
            while self
                .current_char()
                .is_some_and(|c| c.is_ascii_digit() || c == '_')
            {
                self.read_char();
            }
        }

        // Check for scientific notation (e or E)
        if self.current_char().is_some_and(|c| c == 'e' || c == 'E') {
            is_float = true;

            self.read_char(); // consume 'e' or 'E'

            // Optional sign (+ or -)
            if self.current_char().is_some_and(|c| c == '+' || c == '-') {
                self.read_char();
            }

            // Read exponent digits (including underscores)
            while self
                .current_char()
                .is_some_and(|c| c.is_ascii_digit() || c == '_')
            {
                self.read_char();
            }
        }

        let literal = self.slice_chars(start, self.current_index());
        (literal, is_float)
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
