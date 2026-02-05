//! Helper utilities for lexing

/// Check if a character is considered a letter for identifier purposes
/// (ASCII alphabetic or underscore)
pub(super) fn is_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}
