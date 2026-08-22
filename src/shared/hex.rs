//! Lowercase hex encoding.
//!
//! Hashes reach users as text in three places that must agree byte for byte:
//! `.flxi` interface fingerprints, the bytecode and native module caches, and
//! `Flow.Crypto.sha256`. A cache keyed on one spelling and validated against
//! another silently misses, so the encoding lives in one place rather than
//! being re-derived per call site.

/// Encode bytes as lowercase hex, two characters per byte.
///
/// The output is what `sha256sum` and `git hash-object` print, so a digest
/// produced here can be compared against other tools' output directly.
pub fn encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a String cannot fail; the Result is discarded rather
        // than unwrapped to keep this allocation-free after the reserve above.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nothing_as_the_empty_string() {
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn pads_each_byte_to_two_characters() {
        // The leading zero is what distinguishes hex from a bare integer
        // formatting, and dropping it would silently shorten digests.
        assert_eq!(encode(&[0x00]), "00");
        assert_eq!(encode(&[0x0f]), "0f");
        assert_eq!(encode(&[0x01, 0x02]), "0102");
    }

    #[test]
    fn uses_lowercase_for_digits_above_nine() {
        assert_eq!(encode(&[0xab, 0xcd, 0xef]), "abcdef");
        assert_eq!(encode(&[0xff]), "ff");
    }

    #[test]
    fn encodes_a_full_digest_to_sixty_four_characters() {
        let digest = [0u8; 32];
        assert_eq!(encode(&digest).len(), 64);
    }
}
