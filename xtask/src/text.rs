//! Text helpers shared by the sweep and verify: ANSI stripping and the
//! diagnostic codes a compiler run reported.

/// Removes ANSI colour sequences (`ESC [ <digits and ;> m`).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\x1b') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match ansi_len(tail) {
            Some(len) => rest = &tail[len..],
            None => {
                out.push('\x1b');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The length of the colour sequence `text` starts with, if it starts with one.
fn ansi_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(1) != Some(&b'[') {
        return None;
    }
    let params = bytes[2..]
        .iter()
        .take_while(|b| b.is_ascii_digit() || **b == b';')
        .count();
    (bytes.get(2 + params) == Some(&b'm')).then_some(3 + params)
}

/// Every diagnostic code `[E123]` or `[E1234]` in `text`, sorted and unique.
pub fn diagnostic_codes(text: &str) -> Vec<String> {
    let mut codes = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[E") {
        let tail = &rest[start + 2..];
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if (3..=4).contains(&digits) && tail.as_bytes().get(digits) == Some(&b']') {
            codes.push(format!("E{}", &tail[..digits]));
        }
        rest = tail;
    }
    codes.sort();
    codes.dedup();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colour_sequences_only() {
        assert_eq!(strip_ansi("\x1b[1;31merror\x1b[0m: x"), "error: x");
        assert_eq!(strip_ansi("\x1b[mplain"), "plain");
        assert_eq!(strip_ansi("lone \x1b escape"), "lone \x1b escape");
        assert_eq!(strip_ansi("no colour"), "no colour");
    }

    #[test]
    fn finds_codes_sorted_and_unique() {
        let text = "error[E300]: a\n[E1234] b\n[E300] again\n[E12] short [E12345] long [E300x]";
        assert_eq!(diagnostic_codes(text), ["E1234", "E300"]);
        assert!(diagnostic_codes("no codes").is_empty());
    }
}
