//! Text helpers shared by the sweep and verify: ANSI stripping and the
//! diagnostic codes a compiler run reported.

use std::io::Write;

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
    let params = bytes[2..].iter().take_while(|b| b.is_ascii_digit() || **b == b';').count();
    (bytes.get(2 + params) == Some(&b'm')).then_some(3 + params)
}

/// Console colour: on when stdout is a terminal and `NO_COLOR` is unset.
/// Only for what is printed; logs stay plain.
#[derive(Clone, Copy)]
pub struct Paint {
    on: bool,
}

impl Paint {
    pub fn detect() -> Self {
        use std::io::IsTerminal;
        Self { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    /// No colour, for text written to a file.
    pub fn plain() -> Self {
        Self { on: false }
    }

    #[cfg(test)]
    pub fn always() -> Self {
        Self { on: true }
    }

    fn wrap(self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }

    pub fn green(self, text: &str) -> String {
        self.wrap("32", text)
    }

    pub fn red(self, text: &str) -> String {
        self.wrap("1;31", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.wrap("33", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.wrap("2", text)
    }

    pub fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }

    /// A step heading: bold cyan.
    pub fn heading(self, text: &str) -> String {
        self.wrap("1;36", text)
    }

    /// The closing summary's banner: bold magenta, unlike any step heading.
    pub fn banner(self, text: &str) -> String {
        self.wrap("1;35", text)
    }

    /// Whether colour, and so an interactive terminal, is in use.
    pub fn is_on(self) -> bool {
        self.on
    }
}

/// A single console line redrawn in place, for progress that would otherwise
/// scroll a line per item. Off, it writes nothing, so piped output and logs
/// keep every line instead.
pub struct StatusLine {
    on: bool,
    shown: bool,
}

impl StatusLine {
    pub fn new(on: bool) -> Self {
        Self { on, shown: false }
    }

    /// Replaces the line with `text`, without a newline.
    pub fn update(&mut self, out: &mut impl Write, text: &str) {
        if self.on {
            let _ = write!(out, "\r\x1b[2K{text}");
            let _ = out.flush();
            self.shown = true;
        }
    }

    /// Erases the line, so that ordinary output starts at the left margin.
    pub fn clear(&mut self, out: &mut impl Write) {
        if self.shown {
            let _ = write!(out, "\r\x1b[2K");
            let _ = out.flush();
            self.shown = false;
        }
    }
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
    fn colours_only_when_on() {
        assert_eq!(Paint::always().dim("x"), "\x1b[2mx\x1b[0m");
        assert_eq!(Paint { on: false }.dim("[parity]"), "[parity]");
    }

    #[test]
    fn a_status_line_redraws_in_place_and_clears_only_what_it_drew() {
        let mut out = Vec::new();
        let mut status = StatusLine::new(true);
        status.clear(&mut out);
        assert!(out.is_empty());
        status.update(&mut out, "fixture 1/3");
        status.update(&mut out, "fixture 2/3");
        status.clear(&mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\r\x1b[2Kfixture 1/3\r\x1b[2Kfixture 2/3\r\x1b[2K"
        );

        let mut out = Vec::new();
        let mut off = StatusLine::new(false);
        off.update(&mut out, "fixture 1/3");
        off.clear(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn finds_codes_sorted_and_unique() {
        let text = "error[E300]: a\n[E1234] b\n[E300] again\n[E12] short [E12345] long [E300x]";
        assert_eq!(diagnostic_codes(text), ["E1234", "E300"]);
        assert!(diagnostic_codes("no codes").is_empty());
    }
}
