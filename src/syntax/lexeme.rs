use std::{fmt, ops::Deref, rc::Rc};

use crate::syntax::byte_span::ByteSpan;

#[derive(Clone)]
pub enum Lexeme {
    Static(&'static str),
    Owned(String),
    Span { source: Rc<str>, span: ByteSpan },
}

impl Lexeme {
    pub fn as_str(&self) -> &str {
        match self {
            Lexeme::Static(s) => s,
            Lexeme::Owned(s) => s,
            Lexeme::Span { source, span } => {
                source.get(span.start..span.end).unwrap_or_else(|| {
                    panic!(
                        "invalid lexeme span {}..{} for source len {}",
                        span.start,
                        span.end,
                        source.len()
                    )
                })
            }
        }
    }

    pub fn len_chars(&self) -> usize {
        let s = self.as_str();

        if s.is_ascii() {
            s.len()
        } else {
            s.chars().count()
        }
    }

    pub fn from_span(source: Rc<str>, start: usize, end: usize) -> Self {
        Lexeme::Span {
            source,
            span: ByteSpan::new(start, end),
        }
    }
}

impl Deref for Lexeme {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for Lexeme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Lexeme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl PartialEq for Lexeme {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Lexeme {}

impl PartialEq<&str> for Lexeme {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<Lexeme> for &str {
    fn eq(&self, other: &Lexeme) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<String> for Lexeme {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Lexeme> for String {
    fn eq(&self, other: &Lexeme) -> bool {
        self == other.as_str()
    }
}

impl From<String> for Lexeme {
    fn from(value: String) -> Self {
        Lexeme::Owned(value)
    }
}

impl From<&str> for Lexeme {
    fn from(value: &str) -> Self {
        Lexeme::Owned(value.to_string())
    }
}

impl From<&String> for Lexeme {
    fn from(value: &String) -> Self {
        Lexeme::Owned(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_lexeme_returns_exact_slice() {
        let source: Rc<str> = Rc::from("foobar");
        let lexeme = Lexeme::Span {
            source,
            span: ByteSpan::new(3, 6),
        };

        assert_eq!(lexeme.as_str(), "bar");
    }

    #[test]
    #[should_panic(expected = "invalid lexeme span")]
    fn span_lexeme_panics_on_invalid_utf8_boundary() {
        let source: Rc<str> = Rc::from("é");
        let lexeme = Lexeme::Span {
            source,
            span: ByteSpan::new(1, 2),
        };

        let _ = lexeme.as_str();
    }
}
