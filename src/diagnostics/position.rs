use serde::{Deserialize, Serialize};
use std::fmt;

/// Position in source code for error reporting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    /// Construct a source position from a 1-based line and 0-based column.
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

/// A source span with explicit start and end positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    /// Construct a source span from explicit start and end positions.
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}
