//! Number literal parsing (integers and floats)
//!
//! Supports:
//! - Decimal integers: 42, 1_000_000
//! - Decimal floats: 3.14, 2.5e10, 1.5e-3
//! - Hexadecimal: 0xFF, 0x1A_BC
//! - Binary: 0b1010, 0b1111_0000
//! - Underscores for readability in all formats

use super::Lexer;

impl Lexer {
    pub(super) fn read_number(&mut self) -> (String, bool) {
        // Check for hex literal (0x or 0X)
        if self.current_char() == Some('0') && matches!(self.peek_char(), Some('x' | 'X')) {
            return self.read_hex_literal();
        }

        // Check for binary literal (0b or 0B)
        if self.current_char() == Some('0') && matches!(self.peek_char(), Some('b' | 'B')) {
            return self.read_binary_literal();
        }

        // Read decimal number (integer or float)
        self.read_decimal_number()
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
}
