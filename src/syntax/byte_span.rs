#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

impl ByteSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

pub fn slice(src: &str, span: ByteSpan) -> &str {
    src.get(span.start..span.end).unwrap_or("")
}
