//! Helper utilities for lexing

/// Check if a character is considered a letter for identifier purposes
/// (ASCII alphabetic or underscore)
pub(super) fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Byte-level variant for ASCII fast paths.
pub(super) fn is_letter_byte(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}
